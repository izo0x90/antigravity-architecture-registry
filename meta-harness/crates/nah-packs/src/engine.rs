//! The installer: runs a pack's steps for the current platform. It knows
//! kinds of steps (download, verify, unpack, place, run a command), never a
//! particular tool. Steps run in a staging folder; only when every one
//! succeeded does it replace the install and update the lock, so a failure
//! leaves what was there.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::catalog::{Catalog, Pack};
use crate::lock::{Installed, Lock};
use crate::manifest::{Check, Install, Launch, Step, Unpack};
use crate::platform::PlatformDef;
use crate::{PackError, acp, steps, template};

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
    // What the install holds is there (what its check can't reach).
    if let Some(installed) = installed {
        let entry = pack.manifest.install.iter().find(|i| i.platforms.contains(&installed.platform));
        if let Some(missing) = entry.into_iter().flat_map(|e| &e.holds).find(|f| !installed.dest.join(f).exists()) {
            return Ok(State::Broken(format!("{} is missing (installed for {})", installed.dest.join(missing).display(), installed.platform)));
        }
    }
    // A harness's program is in place.
    if let Some(installed) = installed
        && pack.manifest.provides.is_some()
    {
        let launch = harness_launch(catalog, lock, name)?;
        if launch.program.contains(std::path::MAIN_SEPARATOR) && !Path::new(&launch.program).is_file() {
            return Ok(State::Broken(format!("{} is missing (installed for {})", launch.program, installed.platform)));
        }
    }
    let checked = match &tool.check {
        Some(Check::Acp { acp }) => match installed {
            None => None,
            Some(_) => Some(acp::version(&harness_launch(catalog, lock, name)?, Duration::from_secs(acp.timeout_s)).and_then(|v| acp::read_version(&v, acp.version.as_deref()))),
        },
        Some(Check::Command(_)) => Some(tool_version(pack, dest.as_deref(), catalog.timeouts.check)?.ok_or_else(|| format!("{} doesn't answer its version check", tool.name))),
        None => None,
    };
    match &tool.check {
        Some(_) => match checked {
            None => Ok(State::Missing),
            Some(Err(_)) if installed.is_none() => Ok(State::Missing),
            Some(Err(why)) => Ok(State::Broken(why)),
            Some(Ok(found)) => {
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

/// The tool's version, by its command check (None when it has none, it
/// can't be run, or it says nothing the pattern finds).
pub fn tool_version(pack: &Pack, dest: Option<&Path>, limit: Duration) -> Result<Option<String>, PackError> {
    let Some(Check::Command(check)) = pack.manifest.tool.as_ref().and_then(|t| t.check.as_ref()) else { return Ok(None) };
    // A check that runs a program inside the install folder: nothing to run before it's installed.
    if dest.is_none() && std::iter::once(&check.command).chain(&check.args).any(|t| template::names(t).contains(&"dest")) {
        return Ok(None);
    }
    let values = base_values(pack, dest);
    let fill = |t: &str| template::expand(t, &values).map_err(|reason| PackError::Manifest { pack: pack.name().to_owned(), reason });
    let command = fill(&check.command)?;
    let args = check.args.iter().map(|a| fill(a)).collect::<Result<Vec<_>, _>>()?;
    let Some(text) = output_within(&command, &args, limit) else { return Ok(None) };
    let re = regex::Regex::new(&check.version).map_err(|e| PackError::Manifest { pack: pack.name().to_owned(), reason: e.to_string() })?;
    Ok(re.captures(&text).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned()))
}

/// What `command` prints (stdout and stderr) within `limit`; None if it
/// can't be run or doesn't end in time (it's stopped then).
fn output_within(command: &str, args: &[String], limit: Duration) -> Option<String> {
    let mut child = std::process::Command::new(command).args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().ok()?;
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut text);
            }
            text
        })
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let started = Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Some(format!("{}{}", out.join().unwrap_or_default(), err.join().unwrap_or_default()))
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
    // This platform's steps, else those of the general one it narrows.
    let (entry, platform): (&Install, &PlatformDef) = catalog
        .platforms
        .and_general(platform)
        .find_map(|p| pack.manifest.install.iter().find(|i| i.platforms.contains(&p.name)).map(|i| (i, p)))
        .ok_or_else(|| fail(format!("no install steps for {}", platform.name)))?;
    let version = &pack.manifest.pack.version;
    let dest = catalog.root.join(name).join(version);
    let staging = catalog.root.join(format!(".staging-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| fail(format!("{}: {e}", staging.display())))?;
    let result = run_steps(catalog, pack, entry, platform, &staging, &dest);
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
        let found = match pack.manifest.tool.as_ref().and_then(|t| t.check.as_ref()) {
            // The harness answers over ACP, or the install failed.
            Some(Check::Acp { acp }) => {
                let launch = fill_launch(pack, launch_of(pack, entry), &dest)?;
                let found = acp::version(&launch, Duration::from_secs(acp.timeout_s)).and_then(|v| acp::read_version(&v, acp.version.as_deref()));
                Some(found.map_err(|why| fail(format!("the installed harness fails its check: {why}")))?)
            }
            _ => tool_version(pack, Some(&dest), catalog.timeouts.check)?,
        };
        let tool_version = found.or_else(|| pack.manifest.tool.as_ref().and_then(|t| t.version.clone()));
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
fn run_steps(catalog: &Catalog, pack: &Pack, entry: &Install, platform: &PlatformDef, staging: &Path, dest: &Path) -> Result<Vec<String>, PackError> {
    let name = pack.name();
    let fail = |reason: String| PackError::Install { pack: name.to_owned(), reason };
    let mut values = base_values(pack, Some(dest));
    values.insert("platform", &platform.name);
    values.extend(platform.vars.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    if let Some(vars) = entry.vars.get(&platform.name) {
        values.extend(vars.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    let fill = |t: &str| template::expand(t, &values).map_err(fail);
    let mut sums = Vec::new();
    let timeouts = catalog.timeouts;
    for (i, step) in entry.steps.iter().enumerate() {
        match step {
            Step::Download(d) => {
                let url = fill(&d.url)?;
                let expected = fill(&d.sha256)?;
                tracing::info!(pack = name, step = i, %url, "download");
                let file = staging.join(format!(".download-{i}"));
                let limit = d.timeout_s.map_or(timeouts.download, Duration::from_secs);
                let got = download(&url, &file, limit).map_err(|e| fail(format!("{url}: {e}")))?;
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
                    kind => {
                        let extract = d.extract.iter().map(|e| fill(e)).collect::<Result<Vec<_>, _>>()?;
                        steps::unpack(kind, &file, &into, &extract, staging).map_err(|e| fail(format!("{url}: {e}")))?;
                        std::fs::remove_file(&file).map_err(|e| fail(e.to_string()))?;
                    }
                }
            }
            Step::Run(c) => {
                let command = fill(&c.command)?;
                let args = c.args.iter().map(|a| fill(a)).collect::<Result<Vec<_>, _>>()?;
                tracing::info!(pack = name, step = i, %command, ?args, "run");
                let limit = c.timeout_s.map_or(timeouts.run, Duration::from_secs);
                run(&command, &args, staging, limit).map_err(fail)?;
            }
            Step::Link(l) => {
                let path = staging.join(fill(&l.path)?);
                steps::link(&path, &fill(&l.target)?).map_err(fail)?;
            }
            Step::EnsureGroup(g) => {
                let group = fill(&g.name)?;
                let tools = platform.groups.as_ref().ok_or_else(|| fail(format!("platforms.toml doesn't say how {} keeps its groups", platform.name)))?;
                let create: Vec<&str> = tools.create.iter().map(String::as_str).collect();
                if steps::ensure_group(&group, Path::new(&tools.file), &create, steps::is_root(), timeouts.run).map_err(fail)? {
                    tracing::info!(pack = name, step = i, %group, "created the system group");
                }
            }
            Step::SystemCa(c) => {
                let store = fill(&c.store)?;
                let subject = fill(&c.subject)?;
                match steps::system_ca(Path::new(&store), &subject).map_err(fail)? {
                    Some(pem) => {
                        let target = staging.join(fill(&c.save_as)?);
                        if let Some(parent) = target.parent() {
                            std::fs::create_dir_all(parent).map_err(|e| fail(format!("{}: {e}", parent.display())))?;
                        }
                        std::fs::write(&target, pem).map_err(|e| fail(format!("{}: {e}", target.display())))?;
                    }
                    None if c.optional => tracing::info!(pack = name, step = i, %subject, "no such certificate in {store}; skipped"),
                    None => return Err(fail(format!("no certificate whose subject has '{subject}' in {store}"))),
                }
            }
            Step::PatchArchive(p) => {
                let append = staging.join(fill(&p.append)?);
                let addition = match std::fs::read(&append) {
                    Ok(bytes) => bytes,
                    Err(_) if p.optional => {
                        tracing::info!(pack = name, step = i, "nothing to append ({}); skipped", append.display());
                        continue;
                    }
                    Err(e) => return Err(fail(format!("{}: {e}", append.display()))),
                };
                let members = p.members.iter().map(|m| glob::Pattern::new(m).map_err(|e| fail(format!("{m}: {e}")))).collect::<Result<Vec<_>, _>>()?;
                let file = staging.join(fill(&p.file)?);
                let changed = match &p.keep_size {
                    Some(k) => steps::patch_archive_in_place(&file, &members, &addition, k.block.as_bytes(), k.pad.as_bytes()[0]),
                    None => steps::patch_archive(&file, &members, &addition),
                }
                .map_err(fail)?;
                tracing::info!(pack = name, step = i, ?changed, "patched {}", file.display());
            }
        }
    }
    // A harness's program, if it's one the steps made, is there and runs.
    if pack.manifest.provides.is_some() {
        let launch = fill_launch(pack, launch_of(pack, entry), staging)?;
        let program = Path::new(&launch.program);
        if program.starts_with(staging) {
            if !program.is_file() {
                return Err(fail(format!("the steps didn't produce {}", program.strip_prefix(staging).unwrap_or(program).display())));
            }
            mark_executable(program).map_err(|e| fail(format!("{}: {e}", program.display())))?;
        }
    }
    Ok(sums)
}

/// Runs `command` in `dir`, without a shell, stopping it after `limit`.
pub(crate) fn run(command: &str, args: &[String], dir: &Path, limit: Duration) -> Result<(), String> {
    let mut child = std::process::Command::new(command)
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{command}: {e}"))?;
    // Its errors are read as it runs, so a full pipe never stops it.
    let mut stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(err) = stderr.as_mut() {
            let _ = err.read_to_string(&mut text);
        }
        text
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("{command}: {e}"))? {
            break status;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{command} {} took longer than {} s", args.join(" "), limit.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        let err = errors.join().unwrap_or_default();
        let tail: Vec<&str> = err.lines().rev().take(5).collect();
        return Err(format!("{command} {} failed ({status}): {}", args.join(" "), tail.into_iter().rev().collect::<Vec<_>>().join(" | ")));
    }
    Ok(())
}

/// Fetches `url` (`http(s)://` or `file://`) into `file` within `limit`;
/// returns its sha256.
fn download(url: &str, file: &Path, limit: Duration) -> Result<String, String> {
    let mut reader: Box<dyn Read> = match url.strip_prefix("file://") {
        Some(path) => Box::new(std::fs::File::open(path).map_err(|e| e.to_string())?),
        None => {
            let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(limit)).build().into();
            Box::new(agent.get(url).call().map_err(|e| e.to_string())?.into_body().into_reader())
        }
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

/// How an installed harness is started: its program, arguments and
/// environment, filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct HarnessLaunch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
}

/// The launch the install `entry` gives (its own, else the pack's).
fn launch_of<'a>(pack: &'a Pack, entry: &'a Install) -> Option<&'a Launch> {
    entry.launch.as_ref().or(pack.manifest.provides.as_ref().map(|p| &p.launch))
}

/// `launch` filled in for an install at `dest`.
fn fill_launch(pack: &Pack, launch: Option<&Launch>, dest: &Path) -> Result<HarnessLaunch, PackError> {
    let fail = |reason: String| PackError::Manifest { pack: pack.name().to_owned(), reason };
    let launch = launch.ok_or_else(|| fail("it provides no launch".into()))?;
    let user = current_user().map_err(fail)?;
    let mut values = base_values(pack, Some(dest));
    values.insert("user", &user);
    let fill = |t: &str| template::expand(t, &values).map_err(fail);
    Ok(HarnessLaunch {
        program: fill(&launch.program)?,
        args: launch.args.iter().map(|a| fill(a)).collect::<Result<_, _>>()?,
        env: launch.env.iter().map(|(k, v)| Ok((k.clone(), fill(v)?))).collect::<Result<_, PackError>>()?,
        env_remove: launch.env_remove.clone(),
    })
}

/// How installed harness `name` is started, for the platform it was
/// installed for.
pub fn harness_launch(catalog: &Catalog, lock: &Lock, name: &str) -> Result<HarnessLaunch, PackError> {
    let pack = catalog.get(name)?;
    let installed = lock.packs.get(name).ok_or_else(|| PackError::NotInstalled(name.to_owned()))?;
    let entry = pack.manifest.install.iter().find(|i| i.platforms.contains(&installed.platform));
    let launch = entry.and_then(|e| e.launch.as_ref()).or(pack.manifest.provides.as_ref().map(|p| &p.launch));
    fill_launch(pack, launch, &installed.dest)
}

/// The name of the user this runs as.
fn current_user() -> Result<String, String> {
    #[cfg(unix)]
    {
        let uid = rustix::process::geteuid().as_raw();
        let mut size = 4096;
        loop {
            let mut buf = vec![0u8; size];
            let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
            let mut found: *mut libc::passwd = std::ptr::null_mut();
            // SAFETY: every pointer is to memory this function owns, sized as given.
            let rc = unsafe { libc::getpwuid_r(uid, &mut entry, buf.as_mut_ptr().cast(), buf.len(), &mut found) };
            if rc == 0 && !found.is_null() {
                // SAFETY: getpwuid_r set pw_name to a NUL-terminated string in `buf`.
                return Ok(unsafe { std::ffi::CStr::from_ptr(entry.pw_name) }.to_string_lossy().into_owned());
            }
            // Too small a buffer: a bigger one (up to 1 MiB).
            if rc == libc::ERANGE && size < 1 << 20 {
                size *= 4;
                continue;
            }
            return Err(match rc {
                0 => format!("no user with uid {uid}"),
                rc => format!("can't read the user with uid {uid}: {}", std::io::Error::from_raw_os_error(rc)),
            });
        }
    }
    #[cfg(not(unix))]
    std::env::var("USERNAME").map_err(|_| "can't tell the user's name".to_owned())
}

/// A language server's start, filled in for one project: the program, its
/// arguments and environment, what to send it, and its time limits.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerLaunch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Its caches: under the user's data folder, one per project.
    pub cache: PathBuf,
    pub requires: Vec<String>,
    pub initialization_options: serde_json::Value,
    pub settings: serde_json::Value,
    pub ready_timeout: std::time::Duration,
    pub settle: std::time::Duration,
    pub request_timeout: std::time::Duration,
}

/// The file in a server's cache that names the project it's for.
const CACHE_PROJECT: &str = "project";

impl ServerLaunch {
    /// Makes its cache folder, recording which project it's for, and
    /// removes this server's caches for projects that no longer exist (a
    /// temporary copy analysed once leaves none behind for long).
    pub fn keep_cache(&self, project: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.cache)?;
        std::fs::write(self.cache.join(CACHE_PROJECT), project.to_string_lossy().as_bytes())?;
        let Some(all) = self.cache.parent() else { return Ok(()) };
        for entry in std::fs::read_dir(all)?.flatten() {
            let named = std::fs::read_to_string(entry.path().join(CACHE_PROJECT));
            if let Ok(named) = named
                && entry.path() != self.cache
                && !Path::new(&named).exists()
            {
                std::fs::remove_dir_all(entry.path())?;
            }
        }
        Ok(())
    }
}

/// How to start server `name` for `project` (`dest` is its install folder,
/// when it has one).
pub fn server_launch(catalog: &Catalog, lock: &Lock, name: &str, project: &Path) -> Result<ServerLaunch, PackError> {
    use sha2::Digest;
    let pack = catalog.get(name)?;
    let server = pack.manifest.server.as_ref().ok_or_else(|| PackError::Manifest { pack: name.to_owned(), reason: "it isn't a server pack".into() })?;
    let dest = lock.packs.get(name).map(|i| i.dest.clone());
    let project_key: String = sha2::Sha256::digest(project.to_string_lossy().as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    let cache = catalog.root.parent().unwrap_or(&catalog.root).join("cache").join(name).join(project_key);
    let mut values = base_values(pack, dest.as_deref());
    let (root, cache_text) = (project.to_string_lossy(), cache.to_string_lossy());
    values.insert("root", &root);
    values.insert("cache", &cache_text);
    let fill = |t: &str| template::expand(t, &values).map_err(|reason| PackError::Manifest { pack: name.to_owned(), reason });
    let json = |t: &toml::Table| serde_json::to_value(t).map_err(|e| PackError::Manifest { pack: name.to_owned(), reason: e.to_string() });
    Ok(ServerLaunch {
        program: fill(&server.command)?,
        args: server.args.iter().map(|a| fill(a)).collect::<Result<_, _>>()?,
        env: server.env.iter().map(|(k, v)| Ok((k.clone(), fill(v)?))).collect::<Result<_, PackError>>()?,
        requires: server.requires.clone(),
        initialization_options: json(&server.initialization_options)?,
        settings: json(&server.settings)?,
        ready_timeout: std::time::Duration::from_secs(server.ready_timeout_s),
        settle: std::time::Duration::from_millis(server.settle_ms),
        request_timeout: std::time::Duration::from_secs(server.request_timeout_s),
        cache,
    })
}

