//! The packs this build ships, the `pack` commands, and harnesses getting
//! their launch from their installed pack (or an instance's own).

use std::path::Path;

use harness_contracts::{HarnessEntry, HarnessLaunch};
use harness_core::ProviderDriver;
use meta_harness::packs::{Command, distribution, harness_launches, run};
use nah_packs::{Catalog, Installed, Lock, Places};

fn places(dir: &Path) -> Places {
    Places { project: dir.join("project"), home: dir.join("home"), config_home: dir.join("config"), data_home: dir.join("data") }
}

/// A harness that answers ACP `initialize` as version `version`.
fn fake_server(path: &Path, version: &str) {
    std::fs::write(path, format!("#!/bin/sh\nread line\necho '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":1,\"agentInfo\":{{\"name\":\"fake\",\"version\":\"agy_acp_server_{version}\"}}}}}}'\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Records `antigravity-acp` as installed for `platform`, in `dest`.
fn installed(catalog: &Catalog, platform: &str) -> std::path::PathBuf {
    let version = catalog.get("antigravity-acp").unwrap().manifest.pack.version.clone();
    let dest = catalog.root.join("antigravity-acp").join(&version);
    std::fs::create_dir_all(&dest).unwrap();
    let mut lock = Lock::default();
    lock.packs.insert("antigravity-acp".into(), Installed { pack_version: version, tool_version: Some("1.1.1".into()), platform: platform.into(), dest: dest.clone(), sha256: vec![], installed_at: 0 });
    lock.write(&catalog.lock_path).unwrap();
    dest
}

/// The launch of instance `instance`.
fn launch_of(entries: &[HarnessEntry], instance: &str) -> Option<HarnessLaunch> {
    entries.iter().find(|e| e.profile.instance.as_str() == instance).map(|e| e.launch.clone())
}

fn unavailable(launch: Option<HarnessLaunch>) -> (String, Option<String>) {
    match launch {
        Some(HarnessLaunch::Unavailable { reason, fix }) => (reason, fix),
        other => panic!("not unavailable: {other:?}"),
    }
}

#[test]
fn the_shipped_harness_pack_gives_its_driver_a_launch_once_it_passes_its_check() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::load(&distribution(), &places(dir.path())).unwrap();
    let pack = catalog.get("antigravity-acp").expect("shipped");
    let provides = pack.manifest.provides.as_ref().unwrap();
    assert_eq!((provides.driver.as_str(), provides.instance.as_deref()), ("acp", Some("antigravity")));
    let listed = run(&Command::List, &catalog).unwrap();
    assert!(listed.contains("antigravity-acp") && listed.contains("not installed"), "{listed}");
    // Not installed: no launch, and the command that fixes it.
    let (reason, fix) = unavailable(launch_of(&harness_launches(&catalog), "antigravity"));
    assert!(reason.contains("isn't installed") && fix.as_deref().is_some_and(|f| f.starts_with("meta-harness pack install antigravity-acp")), "{reason} {fix:?}");
    assert!(run(&Command::Which("antigravity-acp".into()), &catalog).unwrap_err().to_string().contains("meta-harness pack install antigravity-acp"));

    // Installed, and its check (ACP initialize) answers a supported version:
    // the driver gets the launch.
    let dest = installed(&catalog, "linux-x86_64");
    fake_server(&dest.join("agy_acp_server.par"), "1.1.1");
    // The harness a session starts (the check doesn't reach it) is needed too.
    let (reason, _) = unavailable(launch_of(&harness_launches(&catalog), "antigravity"));
    assert!(reason.contains("localharness_external is missing"), "{reason}");
    std::fs::write(dest.join("localharness_external"), "").unwrap();
    let launch = match launch_of(&harness_launches(&catalog), "antigravity") {
        Some(HarnessLaunch::Ready { launch }) => launch,
        other => panic!("{other:?}"),
    };
    assert_eq!(launch.program, dest.join("agy_acp_server.par").display().to_string());
    // The server finds its harness next to itself; its login opens no browser here.
    assert_eq!(launch.env, [("BROWSER".to_owned(), "true".to_owned())]);
    assert!(run(&Command::Check(Some("antigravity-acp".into())), &catalog).unwrap().contains("ready (1.1.1)"));
    let which = run(&Command::Which("antigravity-acp".into()), &catalog).unwrap();
    assert!(which.contains(&launch.program) && which.contains("the pack, installed for linux-x86_64"), "{which}");
    let entry = harness_launches(&catalog).into_iter().find(|e| e.profile.instance.as_str() == "antigravity").unwrap();
    assert_eq!((entry.profile.driver.as_str(), entry.profile.display_name.as_str()), ("acp", "Antigravity"));
    harness_acp::AcpSettings::parse(&entry.profile.settings).expect("its settings are the ACP driver's");
    let config = harness_acp::AcpDriver::new(dir.path().into()).default_config(&entry);
    assert_eq!(config["launch"]["launch"]["program"], launch.program);

    // A version outside the pack's range: no launch; update fixes it.
    fake_server(&dest.join("agy_acp_server.par"), "1.3.0");
    let (reason, fix) = unavailable(launch_of(&harness_launches(&catalog), "antigravity"));
    assert!(reason.contains("1.3.0 isn't in") && fix.as_deref().is_some_and(|f| f.starts_with("meta-harness pack update antigravity-acp")), "{reason} {fix:?}");
    // The server gone: the check fails, with why.
    std::fs::remove_file(dest.join("agy_acp_server.par")).unwrap();
    let (reason, _) = unavailable(launch_of(&harness_launches(&catalog), "antigravity"));
    assert!(reason.contains("is missing"), "{reason}");
}

#[test]
fn an_instances_own_launch_from_harnesses_toml_wins_and_which_shows_it() {
    let dir = tempfile::tempdir().unwrap();
    let places = places(dir.path());
    let catalog = Catalog::load(&distribution(), &places).unwrap();
    installed(&catalog, "linux-x86_64");
    std::fs::create_dir_all(places.config_home.join("nah")).unwrap();
    std::fs::create_dir_all(places.project.join(".nah")).unwrap();
    std::fs::write(places.config_home.join("nah/harnesses.toml"), "[antigravity]\nprogram = \"/users/own\"\n").unwrap();
    let launch = |all: &[HarnessEntry]| match launch_of(all, "antigravity") {
        Some(HarnessLaunch::Ready { launch }) => launch.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(launch(&harness_launches(&catalog)).program, "/users/own", "the user's own, without checking the pack");
    // The project's file comes later: it wins.
    std::fs::write(places.project.join(".nah/harnesses.toml"), "[antigravity]\nprogram = \"/project/own\"\nargs = [\"--x\"]\nenv = { A = \"b\" }\n").unwrap();
    let own = launch(&harness_launches(&catalog));
    assert_eq!((own.program.as_str(), own.args.clone(), own.env.clone()), ("/project/own", vec!["--x".to_owned()], vec![("A".to_owned(), "b".to_owned())]));
    let which = run(&Command::Which("antigravity-acp".into()), &catalog).unwrap();
    assert!(which.contains("/project/own --x") && which.contains("A=b") && which.contains(".nah/harnesses.toml for antigravity"), "{which}");
    // A bad file says where.
    std::fs::write(places.project.join(".nah/harnesses.toml"), "[antigravity]\nprogramme = \"x\"\n").unwrap();
    let (reason, _) = unavailable(launch_of(&harness_launches(&catalog), "antigravity"));
    assert!(reason.contains("harnesses.toml"), "a bad file is the reason, naming itself: {reason}");
}

#[test]
fn on_the_developer_sandbox_the_launch_is_the_emulated_server() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::load(&distribution(), &places(dir.path())).unwrap();
    let dest = installed(&catalog, "linux-arm64-16k");
    let launch = nah_packs::harness_launch(&catalog, &Lock::read(&catalog.lock_path).unwrap(), "antigravity-acp").unwrap();
    let d = dest.display().to_string();
    assert_eq!(launch.program, format!("{d}/qemu/usr/bin/qemu-x86_64"));
    assert_eq!(&launch.args[..5], ["-L".to_owned(), format!("{d}/sysroot"), "-E".to_owned(), format!("LD_LIBRARY_PATH={d}/sysroot/lib/x86_64-linux-gnu"), format!("{d}/libexec/agy_acp_server.par")]);
    assert!(launch.args[5].starts_with("--uid=") && launch.args[5].len() > "--uid=".len() && !launch.args[5].contains('{'), "the user filled in: {}", launch.args[5]);
    // The server finds the harness next to itself in libexec; its login opens no browser here.
    assert_eq!(launch.env, [("BROWSER".to_owned(), "true".to_owned())]);
    assert!(run(&Command::List, &catalog).unwrap().contains("linux-arm64-16k (developer)"), "pack list names the developer platform");
}

#[tokio::test]
async fn a_harness_without_a_launch_says_why_and_how_to_fix_it() {
    let driver = harness_acp::AcpDriver::new(std::env::temp_dir());
    let none = HarnessLaunch::Unavailable { reason: "antigravity-acp isn't installed".into(), fix: Some("meta-harness pack install antigravity-acp".into()) };
    let profile = harness_contracts::HarnessProfile { instance: harness_contracts::InstanceId::new("antigravity"), driver: harness_contracts::DriverKind::new("acp"), display_name: "Antigravity".into(), settings: serde_json::Value::Null };
    let instance = driver.create(profile.instance.clone(), driver.default_config(&HarnessEntry { profile, launch: none })).await.unwrap();
    instance.refresh().await.unwrap();
    let status = instance.snapshot().borrow().status.clone();
    assert!(matches!(&status, harness_contracts::ProviderStatus::Unavailable { reason } if reason.contains("isn't installed") && reason.contains("meta-harness pack install antigravity-acp")), "{status:?}");
    // A session asked for anyway says the same, not a missing file.
    let config = harness_contracts::SessionConfig { thread_id: harness_contracts::ThreadId::new("t"), policy: Default::default(), model: None, resume: None, mcp_servers: Vec::new() };
    let err = instance.adapter().open_session(config).await.err().expect("no session without a launch").to_string();
    assert!(err.contains("isn't installed") && !err.contains("os error"), "{err}");
}

#[test]
fn no_install_scripts_or_hard_coded_harness_paths_remain() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for gone in ["scripts/install-antigravity-acp.sh", "scripts/install-antigravity-acp-linux-arm-sandbox.sh", "scripts/patch-par-ca.py"] {
        assert!(!root.join(gone).exists(), "{gone}");
    }
    let driver = std::fs::read_to_string(root.join("crates/harness-acp/src/driver.rs")).unwrap();
    assert!(!driver.contains(".local/share") && !driver.contains("AGY_EXECUTABLE"));
}

#[test]
fn languages_and_their_servers_are_shipped_packs() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::load(&distribution(), &places(dir.path())).unwrap();
    let languages = code_analyzer::PackSet::from_catalog(&catalog).unwrap();
    for (language, server) in [("rust", "rust-analyzer"), ("python", "basedpyright"), ("typescript", "typescript-server")] {
        let pack = languages.by_name(language).unwrap_or_else(|| panic!("{language} is a shipped language pack"));
        assert_eq!(pack.server.as_deref(), Some(server));
        assert!(pack.highlights.as_deref().is_some_and(|h| h.contains("@")), "{language} has highlight queries");
        assert_eq!(catalog.get(server).unwrap().kind(), nah_packs::Kind::Server);
        // Its launch is filled in for the project (and, for a server
        // installed in its own folder, that folder), its caches outside it.
        let mut lock = Lock::default();
        lock.packs.insert(server.into(), Installed { pack_version: "1.0.0".into(), tool_version: None, platform: "x".into(), dest: "/packs/x".into(), sha256: vec![], installed_at: 0 });
        let launch = nah_packs::server_launch(&catalog, &lock, server, Path::new("/repo")).unwrap();
        assert!(!launch.cache.starts_with("/repo"), "{launch:?}");
        assert!(launch.env.iter().all(|(_, v)| !v.starts_with("/repo")), "{launch:?}");
        // Checking it never fails: it's ready, out of range, or missing.
        assert!(nah_packs::state(&catalog, &Lock::default(), server).is_ok());
    }
}

#[test]
fn meta_harness_analyze_writes_what_the_server_keeps() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/code-analyzer/tests/fixtures/rust/sample");
    for file in ["Cargo.toml", "src/lib.rs", "src/store.rs", "src/util.rs"] {
        std::fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
        std::fs::copy(fixture.join(file), root.join(file)).unwrap();
    }
    let out = dir.path().join("analysis.json");
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_meta-harness")).args(["analyze", "--root"]).arg(&root).arg("--out").arg(&out).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(String::from_utf8_lossy(&run.stdout).contains("3 files read"), "{}", String::from_utf8_lossy(&run.stdout));
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();

    // The server, over the same project with the same languages' servers,
    // keeps the same result.
    let setup = meta_harness::project::analysis_setup(&root).unwrap();
    let facts = arch_service::open_facts(&root, &setup.limits).unwrap();
    let ready = std::time::Duration::from_secs(setup.limits.servers.ready_s);
    let service = arch_service::ArchitectureService::open(&root, setup.packs, setup.types, Box::new(arch_service::MemoryPlans::default()), facts).unwrap().with_limits(setup.limits).with_servers(&setup.catalog);
    service.ingest(&root).unwrap();
    service.wait_for_servers(ready).unwrap();
    let stored: Vec<serde_json::Value> = written["files"].as_array().unwrap().iter().flat_map(|f| f["relations"].as_array().unwrap().iter().cloned()).collect();
    // Each relation, with how it's known (the server that proved it, if one did).
    let rows: std::collections::BTreeSet<String> = stored.iter().map(|r| format!("{} {} {} {} {}", r["kind"], r["source"], r["target"], r["evidence"], r["prover"])).collect();
    let kept: std::collections::BTreeSet<String> = service
        .file_rows()
        .iter()
        .flat_map(|f| f.relations.iter().map(|r| serde_json::to_value(r).unwrap()).map(|r| format!("{} {} {} {} {}", r["kind"], r["source"], r["target"], r["evidence"], r["prover"])))
        .collect();
    assert!(!rows.is_empty());
    assert_eq!(rows, kept, "the same rows");
    assert!(written["files"].as_array().unwrap().iter().any(|f| f["refs"].as_array().is_some_and(|r| !r.is_empty())), "refs too");
    // The servers' caches for this throwaway project go with it.
    drop(service);
    let lock = Lock::read(&setup.catalog.lock_path).unwrap_or_default();
    for server in setup.catalog.of_kind(nah_packs::Kind::Server) {
        if let Ok(launch) = nah_packs::server_launch(&setup.catalog, &lock, server.name(), &root) {
            let _ = std::fs::remove_dir_all(launch.cache);
        }
    }
}

#[test]
fn a_stray_file_among_packs_is_an_error_and_update_covers_tools_found_on_path() {
    let dir = tempfile::tempdir().unwrap();
    let places = places(dir.path());
    let packs = places.project.join(".nah/packs");
    std::fs::create_dir_all(&packs).unwrap();
    // A pack written as one file: an error, not skipped.
    std::fs::write(packs.join("mine.toml"), "[pack]\nname = \"mine\"\n").unwrap();
    let err = Catalog::load(&distribution(), &places).unwrap_err().to_string();
    assert!(err.contains("mine.toml isn't in a pack"), "{err}");
    std::fs::remove_file(packs.join("mine.toml")).unwrap();
    // A tool found on its own at a version the pack doesn't support.
    std::fs::create_dir_all(&places.home).unwrap();
    let tool = places.home.join("fake-tool");
    std::fs::write(&tool, "#!/bin/sh\necho 'fake-tool 1.4.2'\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let newer = "#!/bin/sh\necho 'fake-tool 2.1.0'\n";
    let pack = format!(
        "[pack]\nname = \"fake-server\"\nkind = \"server\"\nversion = \"1.0.0\"\ndescription = \"x\"\n[tool]\nname = \"fake\"\nsupports = \">=2.0.0\"\n[tool.check]\ncommand = \"{tool}\"\nargs = [\"--version\"]\nversion = '^fake-tool (\\d+\\.\\d+\\.\\d+)'\n[[install]]\nplatforms = [\"linux-x86_64\", \"linux-arm64\", \"linux-arm64-16k\", \"macos-arm64\", \"macos-x86_64\"]\n[[install.steps]]\nrun = {{ command = \"sh\", args = [\"-c\", \"printf \\\"{newer}\\\" > {tool}\"] }}\n[server]\ncommand = \"{tool}\"\nrequires = [\"textDocument/definition\"]\nready_timeout_s = 5\nrequest_timeout_s = 5\n",
        tool = tool.display(),
        newer = newer.replace('\n', "\\\\n"),
    );
    std::fs::create_dir_all(packs.join("fake")).unwrap();
    std::fs::write(packs.join("fake/pack.toml"), pack).unwrap();
    // Only the fake pack: updating every pack must never install a real
    // one on the machine running the test.
    let bare = dir.path().join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    for file in ["index.toml", "platforms.toml"] {
        std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("packs").join(file), bare.join(file)).unwrap();
    }
    let catalog = Catalog::load(&nah_packs::Distribution::from_dir(&bare).unwrap(), &places).unwrap();
    assert_eq!(catalog.packs().map(|p| p.name()).collect::<Vec<_>>(), ["fake-server"], "nothing else to update");
    let out = run(&Command::Update(None), &catalog).unwrap();
    assert!(out.contains("fake-server (found on PATH): 1.4.2 isn't in >=2.0.0; pack 1.0.0 installed"), "{out}");
    assert!(std::fs::read_to_string(&tool).unwrap().contains("2.1.0"), "the update ran");
    let out = run(&Command::Update(None), &catalog).unwrap();
    assert!(out.contains("fake-server is up to date"), "installed now: {out}");
}
