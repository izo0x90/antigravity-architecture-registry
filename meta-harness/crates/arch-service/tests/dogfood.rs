//! Ingests this repository (meta-harness) with the built-in packs: the
//! engine must handle real code, not just fixtures.

use std::path::Path;

use arch_graph::{ComponentId, RelationKind, TypeSystem};
use code_analyzer::PackSet;
use arch_service::{ArchitectureService, MemoryPlans};

mod common;

#[test]
fn ingesting_this_repository_finds_known_code_and_reruns_from_cache() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let service = ArchitectureService::open(
        &root,
        common::shipped_packs(),
        TypeSystem::defaults().unwrap(),
        Box::new(MemoryPlans::default()),
        arch_service::temporary_facts(&arch_service::Limits::defaults().unwrap()).unwrap(),
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
        "code_analyzer::model::Model::apply",
        "arch_graph::graph::Graph",
        "arch_graph::tree::build",
    ] {
        assert!(graph.component(&id(known)).is_some(), "{known} was ingested");
    }
    let calls = |from: &str, to: &str| graph.outgoing(&id(from), Some(RelationKind::Calls)).any(|r| r.target == id(to));
    assert!(calls("arch_service::service::ArchitectureService::validate", "arch_service::checks::check_plan"));
    assert!(calls("arch_service::pipeline::read_file", "code_analyzer::extract::extract_file"));
    assert!(calls("code_analyzer::walk::walk", "code_analyzer::walk::walk_with"));
    // The Python scripts come in through the Python pack.
    assert!(graph.component(&ComponentId::for_symbol("python", "scripts.frame_check")).is_some());
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

#[test]
fn cached_extracts_from_another_pack_version_are_parsed_again() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample"), &root);
    let open = |packs: PackSet| {
        ArchitectureService::open(
            &root,
            packs,
            TypeSystem::defaults().unwrap(),
            Box::new(MemoryPlans::default()),
            arch_service::open_facts(&root, &arch_service::Limits::defaults().unwrap()).unwrap(),
        )
        .unwrap()
    };
    let first = open(common::shipped_packs()).ingest(&root).unwrap();
    assert_eq!((first.parsed, first.cached), (3, 0));
    let same = open(common::shipped_packs()).ingest(&root).unwrap();
    assert_eq!((same.parsed, same.cached), (0, 3), "same pack: served from the cache");
    // The project's own Rust pack, edited: everything the shipped one read is stale.
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
    let own = root.join(".nah/packs/rust");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::copy(shipped.join("language/rust/pack.toml"), own.join("pack.toml")).unwrap();
    std::fs::copy(shipped.join("language/rust/highlights.scm"), own.join("highlights.scm")).unwrap();
    std::fs::write(own.join("syntax.toml"), std::fs::read_to_string(shipped.join("language/rust/syntax.toml")).unwrap() + "\n# edited\n").unwrap();
    let places = nah_packs::Places { project: root.clone(), ..nah_packs::Places::isolated(&dir.path().join("elsewhere")) };
    let catalog = nah_packs::Catalog::load(&nah_packs::Distribution::from_dir(&shipped).unwrap(), &places).unwrap();
    let edited = code_analyzer::PackSet::from_catalog(&catalog).unwrap();
    let again = open(edited).ingest(&root).unwrap();
    assert_eq!((again.parsed, again.cached), (3, 0), "another pack version: parsed again");
}
