//! The packs this build ships (`packs/`, compiled in) and the `pack`
//! commands: list, check, install, update and which. What each pack is and
//! how it installs is in its files; this only asks the engine.

use std::fmt::Write as _;
use std::path::Path;

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

/// The programs installed harness packs provide, by the driver that runs
/// them. A harness pack that isn't installed (or is broken) gives none.
pub fn harness_programs(catalog: &Catalog) -> Result<std::collections::BTreeMap<String, std::path::PathBuf>, PackError> {
    let lock = Lock::read(&catalog.lock_path)?;
    let mut out = std::collections::BTreeMap::new();
    for pack in catalog.of_kind(nah_packs::Kind::Harness) {
        let Some(provides) = &pack.manifest.provides else { continue };
        if nah_packs::state(catalog, &lock, pack.name())?.is_ready() {
            out.insert(provides.driver.clone(), nah_packs::executable(catalog, &lock, pack.name())?);
        }
    }
    Ok(out)
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
    /// Where an installed pack's program is.
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
                let state = nah_packs::state(catalog, &lock, pack.name())?;
                let _ = writeln!(out, "{:<20} {:<9} {:<8} {:<34} {}", pack.name(), pack.kind().name(), m.pack.version, tool, describe(&state));
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
            let names: Vec<String> = match which {
                Some(name) => vec![name.clone()],
                None => lock.packs.keys().cloned().collect(),
            };
            for name in names {
                match nah_packs::state(catalog, &lock, &name)? {
                    State::Outdated { .. } | State::OutOfRange { .. } | State::Broken(_) | State::Missing => {
                        let installed = nah_packs::install(catalog, &name, &platform)?;
                        let _ = writeln!(out, "{name} {} installed", installed.pack_version);
                    }
                    State::Ready { .. } => {
                        let _ = writeln!(out, "{name} is up to date");
                    }
                }
            }
        }
        Command::Which(name) => {
            let _ = writeln!(out, "{}", nah_packs::executable(catalog, &lock, name)?.display());
        }
    }
    Ok(out)
}
