//! The installer: runs a pack's steps for the current platform. It knows
//! kinds of steps (download, verify, unpack, place, run a command), never a
//! particular tool. Steps run in a staging folder; only when every one
//! succeeded does it replace the install and update the lock, so a failure
//! leaves what was there.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::catalog::{Catalog, Pack};
use crate::lock::{Installed, Lock};
use crate::manifest::{Install, Step, Unpack};
use crate::platform::PlatformDef;
use crate::{PackError, template};

/// What a pack's state is on this machine.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Not installed.
    Missing,
    /// Installed and its tool is in the supported range.
    Ready { tool_version: Option<String> },
    /// Its tool is there but at a version the pack doesn't support.
    OutOfRange { found: String, supports: String },
    /// Installed with an older (or other) version of the pack.
    Outdated { installed: String, available: String },
    /// Installed, but something's wrong (a missing file, a check that fails).
    Broken(String),
}

impl State {
    pub fn is_ready(&self) -> bool {
        matches!(self, State::Ready { .. })
    }
}

/// Packs with nothing to install (languages) are always ready.
pub fn state(catalog: &Catalog, lock: &Lock, name: &str) -> Result<State, PackError> {
    let pack = catalog.get(name)?;
    let Some(tool) = &pack.manifest.tool else {
        return Ok(State::Ready { tool_version: None });
    };
    let installed = lock.packs.get(name);
    if let Some(installed) = installed
        && installed.pack_version != pack.manifest.pack.version
    {
        return Ok(State::Outdated { installed: installed.pack_version.clone(), available: pack.manifest.pack.version.clone() });
    }
    let dest = installed.map(|i| i.dest.clone());
    if let (Some(dest), Some(provides)) = (&dest, &pack.manifest.provides)
        && !dest.join(&provides.executable).is_file()
    {
        return Ok(State::Broken(format!("{} is missing", dest.join(&provides.executable).display())));
    }
    match &tool.check {
        Some(_) => match tool_version(pack, dest.as_deref())? {
            None if installed.is_none() => Ok(State::Missing),
            None => Ok(State::Broken(format!("{} doesn't answer its version check", tool.name))),
            Some(found) => {
                let range = semver::VersionReq::parse(&tool.supports).map_err(|e| PackError::Manifest { pack: name.to_owned(), reason: e.to_string() })?;
                match semver::Version::parse(&found) {
                    Ok(v) if range.matches(&v) => Ok(State::Ready { tool_version: Some(found) }),
                    _ => Ok(State::OutOfRange { found, supports: tool.supports.clone() }),
                }
            }
        },
        // No way to ask the tool: what was installed is what's there.
        None => Ok(match installed {
            Some(i) => State::Ready { tool_version: i.tool_version.clone() },
            None => State::Missing,
        }),
    }
}

/// The tool's version, by its check (None when it can't be run or says
/// nothing the pattern finds).
pub fn tool_version(pack: &Pack, dest: Option<&Path>) -> Result<Option<String>, PackError> {
    let Some(check) = pack.manifest.tool.as_ref().and_then(|t| t.check.as_ref()) else { return Ok(None) };
    // A check that runs a program inside the install folder: nothing to run before it's installed.
    if dest.is_none() && std::iter::once(&check.command).chain(&check.args).any(|t| template::names(t).contains(&"dest")) {
        return Ok(None);
    }
    let values = base_values(pack, dest);
    let fill = |t: &str| template::expand(t, &values).map_err(|reason| PackError::Manifest { pack: pack.name().to_owned(), reason });
    let command = fill(&check.command)?;
    let args = check.args.iter().map(|a| fill(a)).collect::<Result<Vec<_>, _>>()?;
    let Ok(output) = std::process::Command::new(&command).args(&args).output() else { return Ok(None) };
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let re = regex::Regex::new(&check.version).map_err(|e| PackError::Manifest { pack: pack.name().to_owned(), reason: e.to_string() })?;
    Ok(re.captures(&text).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned()))
}

/// The values every template of a pack can use.
fn base_values<'a>(pack: &'a Pack, dest: Option<&'a Path>) -> BTreeMap<&'a str, &'a str> {
    let mut values = BTreeMap::new();
    if let Some(dest) = dest.and_then(Path::to_str) {
        values.insert("dest", dest);
    }
    if let Some(v) = pack.manifest.tool.as_ref().and_then(|t| t.version.as_deref()) {
        values.insert("tool_version", v);
    }
    values
}

/// Installs `name` for `platform`: runs its steps in a staging folder, then
/// puts the result in place and records it in the lock. Returns what was
/// recorded.
pub fn install(catalog: &Catalog, name: &str, platform: &PlatformDef) -> Result<Installed, PackError> {
    let pack = catalog.get(name)?;
    let fail = |reason: String| PackError::Install { pack: name.to_owned(), reason };
    let entry: &Install = pack
        .manifest
        .install
        .iter()
        .find(|i| i.platforms.contains(&platform.name))
        .ok_or_else(|| fail(format!("no install steps for {}", platform.name)))?;
    let version = &pack.manifest.pack.version;
    let dest = catalog.root.join(name).join(version);
    let staging = catalog.root.join(format!(".staging-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| fail(format!("{}: {e}", staging.display())))?;
    let result = run_steps(pack, entry, platform, &staging, &dest);
    let sums = match result {
        Ok(sums) => sums,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    // In place: the old install moves aside until the lock says the new one.
    let aside = catalog.root.join(format!(".old-{name}-{}", std::process::id()));
    if dest.exists() {
        std::fs::rename(&dest, &aside).map_err(|e| fail(format!("{}: {e}", dest.display())))?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| fail(format!("{}: {e}", parent.display())))?;
    }
    let placed = std::fs::rename(&staging, &dest).map_err(|e| fail(format!("{}: {e}", dest.display())));
    let recorded = placed.and_then(|()| {
        let tool_version = tool_version(pack, Some(&dest))?.or_else(|| pack.manifest.tool.as_ref().and_then(|t| t.version.clone()));
        let installed = Installed {
            pack_version: version.clone(),
            tool_version,
            platform: platform.name.clone(),
            dest: dest.clone(),
            sha256: sums,
            installed_at: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        };
        let mut lock = Lock::read(&catalog.lock_path)?;
        let previous = lock.packs.insert(name.to_owned(), installed.clone());
        lock.write(&catalog.lock_path)?;
        Ok((installed, previous))
    });
    match recorded {
        Ok((installed, previous)) => {
            let _ = std::fs::remove_dir_all(&aside);
            // An older version's folder isn't needed any more.
            if let Some(previous) = previous
                && previous.dest != dest
            {
                let _ = std::fs::remove_dir_all(&previous.dest);
            }
            Ok(installed)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dest);
            let _ = std::fs::remove_dir_all(&staging);
            if aside.exists() {
                let _ = std::fs::rename(&aside, &dest);
            }
            Err(e)
        }
    }
}

/// Runs the steps into `staging`; returns the checksums verified.
fn run_steps(pack: &Pack, entry: &Install, platform: &PlatformDef, staging: &Path, dest: &Path) -> Result<Vec<String>, PackError> {
    let name = pack.name();
    let fail = |reason: String| PackError::Install { pack: name.to_owned(), reason };
    let mut values = base_values(pack, Some(dest));
    values.insert("platform", &platform.name);
    values.extend(platform.vars.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    if let Some(vars) = entry.vars.get(&platform.name) {
        values.extend(vars.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    let fill = |t: &str| template::expand(t, &values).map_err(|e| fail(e));
    let mut sums = Vec::new();
    for (i, step) in entry.steps.iter().enumerate() {
        match step {
            Step::Download(d) => {
                let url = fill(&d.url)?;
                let expected = fill(&d.sha256)?;
                tracing::info!(pack = name, step = i, %url, "download");
                let file = staging.join(format!(".download-{i}"));
                let got = download(&url, &file).map_err(|e| fail(format!("{url}: {e}")))?;
                if got != expected {
                    return Err(fail(format!("{url}: checksum {got} isn't the pack's {expected}")));
                }
                sums.push(got);
                let into = staging.join(fill(&d.into)?);
                std::fs::create_dir_all(&into).map_err(|e| fail(format!("{}: {e}", into.display())))?;
                match d.unpack {
                    Unpack::None => {
                        let target = into.join(fill(d.save_as.as_deref().unwrap_or_default())?);
                        std::fs::rename(&file, &target).map_err(|e| fail(format!("{}: {e}", target.display())))?;
                    }
                    Unpack::Zip => {
                        let reader = std::fs::File::open(&file).map_err(|e| fail(e.to_string()))?;
                        zip::ZipArchive::new(reader).and_then(|mut a| a.extract(&into)).map_err(|e| fail(format!("{url}: {e}")))?;
                        std::fs::remove_file(&file).map_err(|e| fail(e.to_string()))?;
                    }
                    Unpack::TarGz => {
                        let reader = std::fs::File::open(&file).map_err(|e| fail(e.to_string()))?;
                        tar::Archive::new(flate2::read::GzDecoder::new(reader)).unpack(&into).map_err(|e| fail(format!("{url}: {e}")))?;
                        std::fs::remove_file(&file).map_err(|e| fail(e.to_string()))?;
                    }
                }
            }
            Step::Run(c) => {
                let command = fill(&c.command)?;
                let args = c.args.iter().map(|a| fill(a)).collect::<Result<Vec<_>, _>>()?;
                tracing::info!(pack = name, step = i, %command, ?args, "run");
                let output = std::process::Command::new(&command).args(&args).current_dir(staging).output().map_err(|e| fail(format!("{command}: {e}")))?;
                if !output.status.success() {
                    let err = String::from_utf8_lossy(&output.stderr);
                    let tail: Vec<&str> = err.lines().rev().take(5).collect();
                    return Err(fail(format!("{command} {} failed ({}): {}", args.join(" "), output.status, tail.into_iter().rev().collect::<Vec<_>>().join(" | "))));
                }
            }
        }
    }
    if let Some(provides) = &pack.manifest.provides {
        let program = staging.join(&provides.executable);
        if !program.is_file() {
            return Err(fail(format!("the steps didn't produce {}", provides.executable)));
        }
        mark_executable(&program).map_err(|e| fail(format!("{}: {e}", program.display())))?;
    }
    Ok(sums)
}

/// Fetches `url` (`http(s)://` or `file://`) into `file`; returns its sha256.
fn download(url: &str, file: &Path) -> Result<String, String> {
    let mut reader: Box<dyn Read> = match url.strip_prefix("file://") {
        Some(path) => Box::new(std::fs::File::open(path).map_err(|e| e.to_string())?),
        None => Box::new(ureq::get(url).call().map_err(|e| e.to_string())?.into_body().into_reader()),
    };
    let mut out = std::fs::File::create(file).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    out.sync_all().map_err(|e| e.to_string())?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(unix)]
fn mark_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn mark_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// The installed program a harness pack provides.
pub fn executable(catalog: &Catalog, lock: &Lock, name: &str) -> Result<PathBuf, PackError> {
    let pack = catalog.get(name)?;
    let provides = pack.manifest.provides.as_ref().ok_or_else(|| PackError::Manifest { pack: name.to_owned(), reason: "it provides no program".into() })?;
    let installed = lock.packs.get(name).ok_or_else(|| PackError::NotInstalled(name.to_owned()))?;
    Ok(installed.dest.join(&provides.executable))
}

/// An indexer's run contract, filled in: the program, its arguments, and
/// how long it may take.
#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub timeout: std::time::Duration,
}

/// How to run indexer `name` over `root`, writing to `out` (`dest` is its
/// install folder, when it has one).
pub fn invocation(catalog: &Catalog, lock: &Lock, name: &str, root: &Path, out: &Path) -> Result<Invocation, PackError> {
    let pack = catalog.get(name)?;
    let run = pack.manifest.run.as_ref().ok_or_else(|| PackError::Manifest { pack: name.to_owned(), reason: "it has no [run]".into() })?;
    let dest = lock.packs.get(name).map(|i| i.dest.clone());
    let mut values = base_values(pack, dest.as_deref());
    let (root, out) = (root.to_string_lossy(), out.to_string_lossy());
    values.insert("root", &root);
    values.insert("out", &out);
    let fill = |t: &str| template::expand(t, &values).map_err(|reason| PackError::Manifest { pack: name.to_owned(), reason });
    Ok(Invocation { program: fill(&run.command)?, args: run.args.iter().map(|a| fill(a)).collect::<Result<_, _>>()?, timeout: std::time::Duration::from_secs(run.timeout_s) })
}

