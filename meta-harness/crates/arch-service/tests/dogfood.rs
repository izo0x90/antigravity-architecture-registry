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
    assert!(calls("arch_service::service::ArchitectureService::ingest", "arch_index::extract::extract_file"));
    assert!(calls("arch_index::ingest", "arch_index::resolve::resolve"));
    // The Python scripts come in through the Python pack.
    assert!(graph.component(&ComponentId::for_symbol("python", "scripts.req_status")).is_some());
    // Nothing changed: the second run parses nothing.
    let second = service.ingest(&root).unwrap();
    assert_eq!((second.files, second.parsed, second.cached, second.removed), (first.files, 0, first.files, 0), "{second:?}");
    assert_eq!((second.components, second.relations), (first.components, first.relations));
}
