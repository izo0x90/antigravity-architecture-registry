//! A temporary store (`meta-harness analyze`, tests) is gone once the
//! service that used it is dropped, even if the process exits right after:
//! dropping the service stops the pipeline thread that holds the store.

use arch_graph::TypeSystem;
use arch_service::{ArchitectureService, MemoryPlans};

mod common;

#[test]
fn a_temporary_store_is_removed_when_the_service_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    // The only test in this binary: the temp folder is its own.
    unsafe { std::env::set_var("TMPDIR", tmp.path()) };
    let root = tmp.path().join("app");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn run() { go(); }\nfn go() {}\n").unwrap();
    let stores = || {
        std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("nah-store-"))
            .count()
    };
    let limits = arch_service::Limits::defaults().unwrap();
    let service = ArchitectureService::open(
        &root,
        common::shipped_packs(),
        TypeSystem::defaults().unwrap(),
        Box::new(MemoryPlans::default()),
        arch_service::temporary_facts(&limits).unwrap(),
    )
    .unwrap();
    service.ingest(&root).unwrap();
    assert!(stores() > 0, "the store is in the temp folder while in use");
    drop(service);
    assert_eq!(stores(), 0, "gone as soon as the service is");
}
