//! The packs this build ships (`packs/`, compiled in) and the `pack`
//! commands: list, check, install, update and which. What each pack is and
//! how it installs is in its files; this only asks the engine.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use harness_contracts::{DriverKind, HarnessEntry, HarnessLaunch, HarnessProfile, InstanceId, Launch};

use include_dir::{Dir, include_dir};
use nah_packs::{Catalog, Distribution, Lock, PackError, Places, State};

static SHIPPED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/packs");

/// The packs shipped with this build.
pub fn distribution() -> Distribution {
    let mut files = Vec::new();
    let mut dirs = vec![&SHIPPED];
    while let Some(dir) = dirs.pop() {
        files.extend(dir.files().map(|f| (f.path().to_string_lossy().replace('\\', "/"), f.contents().to_vec())));
        dirs.extend(dir.dirs());
    }
    Distribution::new(files)
}

/// Every pack available for `project`: shipped, then the project's and the user's.
pub fn catalog(project: &Path) -> Result<Catalog, PackError> {
    Catalog::load(&distribution(), &Places::of_user(project)?)
}

/// An instance's own launch, from the harness overrides (`harnesses.toml`).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnLaunch {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    env_remove: Vec<String>,
}

/// The instances' own launches, by instance, and the file each came from:
/// the override files in order, a later one's entry replacing an earlier one's.
fn own_launches(catalog: &Catalog) -> Result<BTreeMap<String, (Launch, PathBuf)>, PackError> {
    let mut out = BTreeMap::new();
    for file in &catalog.harness_overrides {
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(PackError::Config(format!("{}: {e}", file.display()))),
        };
        let entries: BTreeMap<String, OwnLaunch> = toml::from_str(&text).map_err(|e| PackError::Config(format!("{}: {e}", file.display())))?;
        for (instance, own) in entries {
            out.insert(instance, (Launch { program: own.program, args: own.args, env: own.env.into_iter().collect(), env_remove: own.env_remove }, file.clone()));
        }
    }
    Ok(out)
}

/// The instance a harness pack is (its own name for it, else the pack's).
fn instance_of(pack: &nah_packs::Pack, provides: &nah_packs::Provides) -> String {
    provides.instance.clone().unwrap_or_else(|| pack.name().to_owned())
}

/// Every harness pack as an instance the server runs: its profile (the
/// instance, its driver, its name, the driver's settings) and how it's
/// started: the instance's own launch (`harnesses.toml`), else its
/// installed pack's when the pack passes its check; else why not and the
/// command that fixes it. Something that can't be read (the lock, an
/// override file) is each one's reason. Two packs naming one instance: the
/// later one's left out, and logged.
pub fn harness_launches(catalog: &Catalog) -> Vec<HarnessEntry> {
    let lock = Lock::read(&catalog.lock_path);
    let own = own_launches(catalog);
    let mut out: Vec<HarnessEntry> = Vec::new();
    for pack in catalog.of_kind(nah_packs::Kind::Harness) {
        let Some(provides) = &pack.manifest.provides else { continue };
        let instance = instance_of(pack, provides);
        if out.iter().any(|e| e.profile.instance.as_str() == instance) {
            tracing::warn!("harness packs name instance {instance} twice: {} is left out", pack.name());
            continue;
        }
        let launch = match (&own, &lock) {
            (Err(e), _) => HarnessLaunch::Unavailable { reason: e.to_string(), fix: Some("fix the file it names".into()) },
            (Ok(own), _) if own.contains_key(&instance) => HarnessLaunch::Ready { launch: own[&instance].0.clone() },
            (Ok(_), Err(e)) => HarnessLaunch::Unavailable { reason: format!("can't read the packs' lock: {e}"), fix: Some(format!("meta-harness pack install {}", pack.name())) },
            (Ok(_), Ok(lock)) => pack_launch(catalog, lock, pack.name()).unwrap_or_else(|e| HarnessLaunch::Unavailable { reason: e.to_string(), fix: Some(format!("meta-harness pack install {}", pack.name())) }),
        };
        let profile = HarnessProfile {
            instance: InstanceId::new(&instance),
            driver: DriverKind::new(&provides.driver),
            display_name: provides.display_name.clone().unwrap_or_else(|| pack.name().to_owned()),
            settings: serde_json::to_value(&provides.settings).unwrap_or_default(),
        };
        out.push(HarnessEntry { profile, launch });
    }
    out.sort_by(|a, b| a.profile.instance.cmp(&b.profile.instance));
    out
}

/// An installed harness pack's launch, if it passes its check.
fn pack_launch(catalog: &Catalog, lock: &Lock, name: &str) -> Result<HarnessLaunch, PackError> {
    // The command, then a re-probe of the harness (it looks the launch up again).
    let install = Some(format!("meta-harness pack install {name}, then re-probe the harness"));
    let update = Some(format!("meta-harness pack update {name}, then re-probe the harness"));
    Ok(match nah_packs::state(catalog, lock, name)? {
        State::Ready { .. } => {
            let l = nah_packs::harness_launch(catalog, lock, name)?;
            HarnessLaunch::Ready { launch: Launch { program: l.program, args: l.args, env: l.env, env_remove: l.env_remove } }
        }
        State::Missing => HarnessLaunch::Unavailable { reason: format!("{name} isn't installed"), fix: install },
        State::Broken(why) => HarnessLaunch::Unavailable { reason: format!("{name} fails its check: {why}"), fix: install },
        state @ (State::OutOfRange { .. } | State::Outdated { .. }) => HarnessLaunch::Unavailable { reason: format!("{name}: {}", describe(&state)), fix: update },
    })
}

/// A `pack` command.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Every pack: kind, version, the tool's supported versions, its state.
    List,
    /// One pack's state (or every pack's).
    Check(Option<String>),
    Install(String),
    /// Reinstall what's outdated or out of range (one, or every installed one).
    Update(Option<String>),
    /// How an installed pack's program is started (with any instance's
    /// own launch from `harnesses.toml` in its place).
    Which(String),
}

fn describe(state: &State) -> String {
    match state {
        State::Missing => "not installed".into(),
        State::Ready { tool_version: Some(v) } => format!("ready ({v})"),
        State::Ready { tool_version: None } => "ready".into(),
        State::OutOfRange { found, supports } => format!("{found} isn't in {supports}"),
        State::Outdated { installed, available } => format!("pack {installed} installed, {available} available"),
        State::Broken(why) => format!("broken: {why}"),
    }
}

/// What the lock records of `pack`, without running anything.
fn recorded(lock: &Lock, pack: &nah_packs::Pack) -> String {
    match (&pack.manifest.tool, lock.packs.get(pack.name())) {
        (None, _) => "nothing to install".into(),
        (Some(_), None) => "not installed (or found on its own: pack check)".into(),
        (Some(_), Some(i)) if i.pack_version != pack.manifest.pack.version => format!("pack {} installed, {} available", i.pack_version, pack.manifest.pack.version),
        (Some(_), Some(i)) => format!("installed {} for {} (pack check to check it)", i.tool_version.as_deref().unwrap_or("?"), i.platform),
    }
}

/// Runs `command`; returns what to print, or why it failed.
pub fn run(command: &Command, catalog: &Catalog) -> Result<String, PackError> {
    let lock = Lock::read(&catalog.lock_path)?;
    let platform = catalog.platforms.current()?.clone();
    let mut out = String::new();
    match command {
        Command::List | Command::Check(None) => {
            for pack in catalog.packs() {
                let m = &pack.manifest;
                let tool = m.tool.as_ref().map(|t| format!("{} {}", t.name, t.supports)).unwrap_or_default();
                // Listing says what the lock records (quick: no tool is run);
                // checking runs each tool's check.
                let state = match command {
                    Command::List => recorded(&lock, pack),
                    _ => describe(&nah_packs::state(catalog, &lock, pack.name())?),
                };
                let _ = writeln!(out, "{:<20} {:<9} {:<8} {:<34} {}", pack.name(), pack.kind().name(), m.pack.version, tool, state);
            }
            if matches!(command, Command::List) {
                let names: Vec<String> = catalog
                    .platforms
                    .list
                    .iter()
                    .map(|p| {
                        let mut name = p.name.clone();
                        if p.developer {
                            name.push_str(" (developer)");
                        }
                        if p.name == platform.name {
                            name.push_str(" [this machine]");
                        }
                        name
                    })
                    .collect();
                let _ = writeln!(out, "platforms: {}", names.join(", "));
            }
        }
        Command::Check(Some(name)) => {
            let state = nah_packs::state(catalog, &lock, name)?;
            let _ = writeln!(out, "{name}: {}", describe(&state));
            if !state.is_ready() {
                return Err(PackError::Install { pack: name.clone(), reason: describe(&state) });
            }
        }
        Command::Install(name) => {
            let installed = nah_packs::install(catalog, name, &platform)?;
            let _ = writeln!(out, "{name} {} installed in {}", installed.pack_version, installed.dest.display());
        }
        Command::Update(which) => {
            // Every installed pack, and every pack whose tool is found on
            // its own (on PATH): an old one there is updated too.
            let names: Vec<String> = match which {
                Some(name) => vec![name.clone()],
                None => {
                    let mut names: std::collections::BTreeSet<String> = lock.packs.keys().cloned().collect();
                    for pack in catalog.packs().filter(|p| p.manifest.tool.is_some() && !lock.packs.contains_key(p.name())) {
                        if nah_packs::state(catalog, &lock, pack.name())? != State::Missing {
                            names.insert(pack.name().to_owned());
                        }
                    }
                    names.into_iter().collect()
                }
            };
            for name in names {
                // Installed once, but no pack of that name any more: said, left alone.
                if catalog.get(&name).is_err() {
                    let _ = writeln!(out, "{name}: no such pack any more (its install is left as it is)");
                    continue;
                }
                let found = if lock.packs.contains_key(&name) { "" } else { " (found on PATH)" };
                match nah_packs::state(catalog, &lock, &name)? {
                    state @ (State::Outdated { .. } | State::OutOfRange { .. } | State::Broken(_) | State::Missing) => {
                        let installed = nah_packs::install(catalog, &name, &platform)?;
                        let _ = writeln!(out, "{name}{found}: {}; pack {} installed", describe(&state), installed.pack_version);
                    }
                    State::Ready { .. } => {
                        let _ = writeln!(out, "{name}{found} is up to date");
                    }
                }
            }
        }
        Command::Which(name) => {
            let pack = catalog.get(name)?;
            let (launch, from) = match (&pack.manifest.provides, &pack.manifest.server) {
                (Some(provides), _) => match own_launches(catalog)?.remove(&instance_of(pack, provides)) {
                    Some((launch, file)) => (launch, format!("{} for {}", file.display(), instance_of(pack, provides))),
                    None => {
                        let l = nah_packs::harness_launch(catalog, &lock, name)?;
                        (Launch { program: l.program, args: l.args, env: l.env, env_remove: l.env_remove }, format!("the pack, installed for {}", lock.packs[name].platform))
                    }
                },
                (None, Some(_)) => {
                    let l = nah_packs::server_launch(catalog, &lock, name, &catalog.project)?;
                    (Launch { program: l.program, args: l.args, env: l.env, env_remove: Vec::new() }, "the pack".to_owned())
                }
                (None, None) => return Err(PackError::Manifest { pack: name.clone(), reason: "it starts no program".into() }),
            };
            let _ = writeln!(out, "{} {}", launch.program, launch.args.join(" "));
            for (k, v) in &launch.env {
                let _ = writeln!(out, "  {k}={v}");
            }
            for k in &launch.env_remove {
                let _ = writeln!(out, "  (without {k})");
            }
            let _ = writeln!(out, "  (from {from})");
        }
    }
    Ok(out)
}
