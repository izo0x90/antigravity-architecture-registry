//! Ingests this repository (meta-harness) with the built-in packs: the
//! engine must handle real code, not just fixtures.

use std::path::Path;

use arch_graph::{ComponentId, RelationKind, TypeSystem, ViewCatalog};
use arch_index::PackSet;
use arch_service::{ArchitectureService, MemoryIndex, MemoryPlans};

// req: P4.10
#[test]
fn ingesting_this_repository_finds_known_code_and_reruns_from_cache() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let service = ArchitectureService::open(
        &root,
        PackSet::built_in().unwrap(),
        TypeSystem::defaults().unwrap(),
        ViewCatalog::defaults().unwrap(),
        Box::new(MemoryPlans::default()),
        Box::new(MemoryIndex::default()),
    )
    .unwrap();
    let first = service.ingest(&root).unwrap();
    assert!(first.files > 50 && first.parsed == first.files && first.cached == 0, "{first:?}");
    assert!(first.components > 500, "{first:?}");
    let graph = service.observed().unwrap();
    let id = |s: &str| ComponentId::for_symbol("rust", s);
    for known in [
        "arch_service::service::ArchitectureService",
        "arch_service::service::ArchitectureService::validate",
        "arch_service::checks::check_plan",
        "arch_index::resolve::resolve",
        "arch_graph::graph::Graph",
        "arch_graph::view::query",
    ] {
        assert!(graph.component(&id(known)).is_some(), "{known} was ingested");
    }
    let calls = |from: &str, to: &str| graph.outgoing(&id(from), Some(RelationKind::Calls)).any(|r| r.target == id(to));
    assert!(calls("arch_service::service::ArchitectureService::validate", "arch_service::checks::check_plan"));
    assert!(calls("arch_service::service::ArchitectureService::index_scope", "arch_index::extract::extract_file"));
    assert!(calls("arch_index::ingest", "arch_index::resolve::resolve"));
    // The Python scripts come in through the Python pack.
    assert!(graph.component(&ComponentId::for_symbol("python", "scripts.req_status")).is_some());
    // Nothing changed: the second run parses nothing.
    let second = service.ingest(&root).unwrap();
    assert_eq!((second.files, second.parsed, second.cached, second.removed), (first.files, 0, first.files, 0), "{second:?}");
    assert_eq!((second.components, second.relations), (first.components, first.relations));
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

// req: P4.7
#[test]
fn cached_extracts_from_another_pack_version_are_parsed_again() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../arch-index/tests/fixtures/rust/sample"), &root);
    let open = |packs: PackSet| {
        ArchitectureService::open(
            &root,
            packs,
            TypeSystem::defaults().unwrap(),
            ViewCatalog::defaults().unwrap(),
            Box::new(MemoryPlans::default()),
            Box::new(arch_service::FsIndex::new(&root).unwrap()),
        )
        .unwrap()
    };
    let first = open(PackSet::built_in().unwrap()).ingest(&root).unwrap();
    assert_eq!((first.parsed, first.cached), (3, 0));
    let same = open(PackSet::built_in().unwrap()).ingest(&root).unwrap();
    assert_eq!((same.parsed, same.cached), (0, 3), "same pack: served from the cache");
    // The same language pack, edited: everything it extracted is stale.
    let text = include_str!("../../arch-index/packs/rust.toml").to_owned() + "\n# edited\n";
    let mut edited = PackSet::built_in().unwrap();
    edited.add(arch_index::Pack::from_toml(&text).unwrap());
    let again = open(edited).ingest(&root).unwrap();
    assert_eq!((again.parsed, again.cached), (3, 0), "another pack version: parsed again");
}
