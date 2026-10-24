//! The developer sandbox end to end.
//! Machine-dependent: it needs this machine's `antigravity-acp` pack
//! installed and passing its check. Without the pack installed it says so
//! and skips.

use std::path::Path;
use std::time::Duration;

use harness_contracts::{AuthStatus, HarnessLaunch, ProviderStatus, RuntimePolicy, SessionConfig, ThreadId};
use harness_core::ProviderDriver;

/// The harness pack providing the antigravity instance, when it's
/// installed on this machine; else why not.
fn installed(catalog: &nah_packs::Catalog) -> Result<(), String> {
    let pack = catalog.of_kind(nah_packs::Kind::Harness).find(|p| p.manifest.provides.as_ref().is_some_and(|p| p.instance.as_deref() == Some("antigravity"))).ok_or("no harness pack provides antigravity")?;
    let lock = nah_packs::Lock::read(&catalog.lock_path).map_err(|e| e.to_string())?;
    if lock.packs.contains_key(pack.name()) { Ok(()) } else { Err(format!("{} isn't installed on this machine", pack.name())) }
}

#[tokio::test]
async fn the_driver_starts_the_installed_pack_and_opens_a_session_needs_the_pack() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog = meta_harness::packs::catalog(root).unwrap();
    if let Err(why) = installed(&catalog) {
        return eprintln!("skipped: {why}");
    }
    // The launch comes from the pack (its check passed), not an old install.
    let entry = meta_harness::packs::harness_launches(&catalog).into_iter().find(|e| e.profile.instance.as_str() == "antigravity").expect("the antigravity harness");
    let launch = match &entry.launch {
        HarnessLaunch::Ready { launch } => launch.clone(),
        other => panic!("no launch from the pack: {other:?}"),
    };
    assert!(launch.program.starts_with(&catalog.root.display().to_string()), "from the pack's own folder: {launch:?}");
    println!("launch: {} {}", launch.program, launch.args.join(" "));
    // The driver's own code: probe (ACP initialize), then a new session.
    let driver = harness_acp::AcpDriver::new(root.to_path_buf());
    let instance = driver.create(entry.profile.instance.clone(), driver.default_config(&entry)).await.unwrap();
    let started = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(600), instance.refresh()).await.expect("initialize answered in time").unwrap();
    let status = instance.snapshot().borrow().status.clone();
    println!("probe: {status:?} in {:?}", started.elapsed());
    assert_eq!(status, ProviderStatus::Ready, "ACP initialize through the driver");
    let config = SessionConfig { thread_id: ThreadId::new("e2e"), policy: RuntimePolicy::default(), model: None, resume: None, mcp_servers: Vec::new() };
    let started = std::time::Instant::now();
    let opened = tokio::time::timeout(Duration::from_secs(600), instance.adapter().open_session(config)).await.expect("a session in time");
    let signed_in = instance.snapshot().borrow().auth.status;
    println!("signed in: {signed_in:?}");
    if signed_in == AuthStatus::SignedIn {
        let session = opened.expect("a new session");
        println!("session {} in {:?}", session.thread_id(), started.elapsed());
        return session.close().await.unwrap();
    }
    // Signed out: the session is refused as such (not left waiting on a
    // login nobody sees), and its login shows its link, to be finished
    // from any device, refusing a paste that isn't its callback.
    let refused = opened.err().expect("no session signed out");
    println!("refused in {:?}: {refused}", started.elapsed());
    assert!(matches!(refused, harness_core::HarnessError::AuthRequired { .. }), "{refused}");
    let auth = instance.auth().expect("it signs in");
    let started = std::time::Instant::now();
    let waiting = auth.begin("oauth-personal").await.unwrap().expect("a link to open");
    let link = waiting.url.clone().expect("its link");
    println!("link in {:?}: {link}", started.elapsed());
    assert!(link.starts_with("https://accounts.google.com/") && link.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A"), "{link}");
    assert_eq!(instance.snapshot().borrow().auth.waiting.as_deref(), Some(&waiting), "every device sees it");
    let wrong = auth.complete("http://127.0.0.1:1/?code=x").await.unwrap_err();
    assert!(matches!(wrong, harness_core::HarnessError::Refused { .. }), "{wrong}");
    auth.cancel().await.unwrap();
    assert_eq!(instance.snapshot().borrow().auth.waiting, None);
}

/// The body of `GET path` from the server at `port`.
/// The cockpit token the test's server is started with: reads need the cockpit.
const TOKEN: &str = "sandbox-e2e-token";

fn get(port: u16, path: &str) -> Option<String> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\nCookie: nah_cockpit={TOKEN}\r\nConnection: close\r\n\r\n").ok()?;
    let mut text = String::new();
    stream.read_to_string(&mut text).ok()?;
    text.split_once("\r\n\r\n").map(|(_, body)| body.to_owned())
}

/// Processes still running whose environment holds `marker` (what a
/// process started, and everything it started, inherits).
fn running_with(marker: &str) -> Vec<String> {
    let needle = format!("NAH_E2E_RUN={marker}");
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()))
        .filter(|e| std::fs::read(e.path().join("environ")).is_ok_and(|env| env.split(|b| *b == 0).any(|v| v == needle.as_bytes())))
        .map(|e| std::fs::read_to_string(e.path().join("cmdline")).unwrap_or_default().replace('\0', " "))
        .collect()
}

#[test]
fn the_server_starts_with_only_the_pack_and_its_harness_comes_up_ready_needs_the_pack() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Err(why) = installed(&meta_harness::packs::catalog(manifest).unwrap()) {
        return eprintln!("skipped: {why}");
    }
    let build = manifest.join(meta_harness::server::static_dir()).join(meta_harness::server::COCKPIT_BUILD);
    if !build.exists() {
        return eprintln!("skipped: the cockpit isn't built ({} is missing; run `make ui`)", build.display());
    }
    // Only the pack: none of the old install's folders.
    let home = std::env::var("HOME").unwrap();
    for old in [format!("{home}/.local/share/antigravity-acp"), "/opt/qemu-user".into(), "/opt/x86_64-sysroot".into()] {
        assert!(!Path::new(&old).exists(), "{old} is still there");
    }
    // A project of its own, so its store and watcher are its own.
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("src")).unwrap();
    std::fs::write(project.path().join("Cargo.toml"), "[package]\nname = \"e2e\"\n").unwrap();
    std::fs::write(project.path().join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    let marker = format!("{}-{}", std::process::id(), project.path().display());
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut server = std::process::Command::new(env!("CARGO_BIN_EXE_meta-harness"))
        .current_dir(manifest)
        .env("NAH_REPO_ROOT", project.path())
        .env_remove("PORT")
        .env("METAHARNESS_PORT", port.to_string())
        .env("HOST", "127.0.0.1")
        .env("NAH_E2E_RUN", &marker)
        .env("NAH_COCKPIT_TOKEN", TOKEN)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    let mut status = String::new();
    while started.elapsed() < Duration::from_secs(300) {
        if let Some(body) = get(port, "/api/instances") {
            status = body;
            if status.contains(r#""type":"ready""#) {
                break;
            }
        }
        if server.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    println!("antigravity ready after {:?}: {status}", started.elapsed());
    // Stopped the way a user stops it: SIGTERM, a clean shutdown that
    // leaves nothing it started running.
    let _ = std::process::Command::new("kill").args(["-TERM", &server.id().to_string()]).status();
    let stopping = std::time::Instant::now();
    while server.try_wait().unwrap().is_none() && stopping.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(100));
    }
    let exited = server.try_wait().unwrap();
    if exited.is_none() {
        let _ = server.kill();
        let _ = server.wait();
    }
    assert!(status.contains(r#""instanceId":"antigravity""#) && status.contains(r#""type":"ready""#), "{status}");
    assert!(exited.is_some_and(|s| s.success()), "SIGTERM stops it cleanly within 30 s: {exited:?}");
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(running_with(&marker), Vec::<String>::new(), "nothing it started is left running");
}
