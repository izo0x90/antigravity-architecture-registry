//! The packs this build ships, the `pack` commands, and harnesses getting
//! their program from their installed pack.

use std::path::Path;

use harness_core::ProviderDriver;
use meta_harness::packs::{Command, distribution, harness_programs, run};
use nah_packs::{Catalog, Installed, Lock, Places};

fn places(dir: &Path) -> Places {
    Places { project: dir.join("project"), home: dir.join("home"), config_home: dir.join("config"), data_home: dir.join("data") }
}

// req: AP.3, AP.4
#[test]
fn the_shipped_harness_pack_installs_for_its_driver_and_the_commands_report_it() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::load(&distribution(), &places(dir.path())).unwrap();
    let pack = catalog.get("antigravity-acp").expect("shipped");
    assert_eq!(pack.manifest.provides.as_ref().unwrap().driver, "antigravity");
    let listed = run(&Command::List, &catalog).unwrap();
    assert!(listed.contains("antigravity-acp") && listed.contains("not installed"), "{listed}");
    assert!(harness_programs(&catalog).unwrap().is_empty(), "nothing installed, nothing handed out");
    assert!(run(&Command::Which("antigravity-acp".into()), &catalog).unwrap_err().to_string().contains("meta-harness pack install antigravity-acp"));

    // Installed (as the installer records it): the driver gets its program.
    let dest = catalog.root.join("antigravity-acp/1.0.0");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("agy_acp_server.par"), "").unwrap();
    let mut lock = Lock::default();
    let platform = catalog.platforms.current().unwrap().name.clone();
    lock.packs.insert("antigravity-acp".into(), Installed { pack_version: "1.0.0".into(), tool_version: Some("1.1.1".into()), platform, dest: dest.clone(), sha256: vec![], installed_at: 0 });
    lock.write(&catalog.lock_path).unwrap();
    let programs = harness_programs(&catalog).unwrap();
    assert_eq!(programs.get("antigravity"), Some(&dest.join("agy_acp_server.par")));
    assert!(run(&Command::Check(Some("antigravity-acp".into())), &catalog).unwrap().contains("ready (1.1.1)"));
    assert_eq!(run(&Command::Which("antigravity-acp".into()), &catalog).unwrap().trim(), dest.join("agy_acp_server.par").display().to_string());
    let config = provider_antigravity::AntigravityDriver.default_config(programs.get("antigravity").map(|p| p.as_path()));
    assert_eq!(config["executable"], dest.join("agy_acp_server.par").display().to_string());
}

// req: AP.4
#[tokio::test]
async fn a_harness_without_its_pack_says_how_to_install_it() {
    let driver = provider_antigravity::AntigravityDriver;
    let instance = driver.create(harness_contracts::InstanceId::new("antigravity"), driver.default_config(None)).await.unwrap();
    instance.refresh().await.unwrap();
    let status = instance.snapshot().borrow().status.clone();
    assert!(matches!(&status, harness_contracts::ProviderStatus::Unavailable { reason } if reason.contains("harness pack")), "{status:?}");
}

// req: AP.4
#[test]
fn no_install_scripts_or_hard_coded_harness_paths_remain() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(!root.join("scripts/install-antigravity-acp.sh").exists());
    let driver = std::fs::read_to_string(root.join("crates/provider-antigravity/src/driver.rs")).unwrap();
    assert!(!driver.contains(".local/share") && !driver.contains("AGY_EXECUTABLE"));
}

// req: AP.5
#[test]
fn languages_and_indexers_are_shipped_packs() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::load(&distribution(), &places(dir.path())).unwrap();
    let languages = code_analyzer::PackSet::from_catalog(&catalog).unwrap();
    for (language, indexer) in [("rust", "rust-analyzer"), ("python", "scip-python"), ("typescript", "scip-typescript")] {
        let pack = languages.by_name(language).unwrap_or_else(|| panic!("{language} is a shipped language pack"));
        assert_eq!(pack.indexer.as_deref(), Some(indexer));
        assert!(pack.highlights.as_deref().is_some_and(|h| h.contains("@")), "{language} has highlight queries");
        let indexer = catalog.get(indexer).unwrap();
        assert_eq!(indexer.kind(), nah_packs::Kind::Indexer);
        // The run contract fills in the project, where the index goes, and
        // (for an indexer installed in its own folder) that folder.
        let run = |lock: &Lock| nah_packs::invocation(&catalog, lock, indexer.name(), Path::new("/repo"), Path::new("/tmp/out.scip"));
        let installed = run(&Lock::default()).is_ok();
        let mut lock = Lock::default();
        if !installed {
            assert!(run(&lock).unwrap_err().to_string().contains("dest"), "can't run before it's installed");
            lock.packs.insert(indexer.name().into(), Installed { pack_version: "1.0.0".into(), tool_version: None, platform: "x".into(), dest: "/packs/x".into(), sha256: vec![], installed_at: 0 });
        }
        let run = run(&lock).unwrap();
        assert!(run.args.iter().any(|a| a == "/repo") && run.args.iter().any(|a| a == "/tmp/out.scip"), "{run:?}");
        assert!(installed || run.program.starts_with("/packs/x/"), "{run:?}");
        // Checking it never fails: it's ready, out of range, or missing.
        assert!(nah_packs::state(&catalog, &Lock::default(), indexer.name()).is_ok());
    }
    let rust = nah_packs::invocation(&catalog, &Lock::default(), "rust-analyzer", Path::new("/repo"), Path::new("/tmp/out.scip")).unwrap();
    assert_eq!((rust.program.as_str(), rust.args.as_slice()), ("rust-analyzer", ["scip", "/repo", "--output", "/tmp/out.scip"].map(String::from).as_slice()));
    assert_eq!(rust.env, [("CARGO_TARGET_DIR".to_owned(), "/repo/.nah/cache/rust-analyzer/target".to_owned())], "its build goes under the project's cache, not target/");
}
