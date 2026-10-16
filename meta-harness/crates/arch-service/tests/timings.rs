//! Slowness is visible: every answer is timed stage by stage, kept for
//! /api/perf, and one over the limit says so.

use std::path::{Path, PathBuf};

use arch_graph::TypeSystem;
use arch_graph::tree::{TreeRequest, ViewKind};
use arch_service::{ArchitectureService, Limits, MemoryPlans, SearchKind, open_facts};

mod common;

fn sample() -> PathBuf {
    let root = common::lasting_dir().join("sample");
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample");
    std::fs::create_dir_all(root.join("src")).unwrap();
    for f in ["Cargo.toml", "src/lib.rs", "src/store.rs", "src/util.rs"] {
        std::fs::copy(from.join(f), root.join(f)).unwrap();
    }
    root
}

fn open(root: &Path, limits: Limits) -> ArchitectureService {
    let facts = open_facts(root, &limits).unwrap();
    ArchitectureService::open(root, common::shipped_packs(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), facts).unwrap().with_limits(limits)
}

fn stages(timed: &arch_service::Timed) -> Vec<&str> {
    timed.stages.iter().map(|s| s.stage.as_str()).collect()
}

#[test]
fn every_answer_is_timed_by_stage_and_kept_for_the_perf_page() {
    let root = sample();
    let service = open(&root, Limits::defaults().unwrap());
    service.ingest(&root).unwrap();
    let request = TreeRequest { view: ViewKind::Calls, root: Some("rust:sample::run".into()), depth: Some(2), ..TreeRequest::default() };
    let json = service.tree_json(&request).unwrap();
    let tree: arch_graph::tree::Tree = serde_json::from_slice(&json).unwrap();
    assert!(tree.slow_ms.is_none(), "quick: nothing to say");
    service.search("save", SearchKind::Symbols, &arch_graph::Source::Code, None, None).unwrap();
    service.card("rust:sample::run", &arch_graph::Source::Code).unwrap();

    let perf = service.perf().unwrap();
    let whats: Vec<&str> = perf.recent.iter().map(|t| t.what.as_str()).collect();
    assert_eq!(whats, ["open", "ingest", "tree", "search", "card"]);
    let index = perf.last_index.as_ref().expect("the last index run");
    assert_eq!(stages(index), ["find files", "read", "link", "store", "graph"]);
    assert_eq!(index.size, Some((3, "files".into())));
    let tree_timing = &perf.recent[2];
    assert_eq!(stages(tree_timing), ["source", "badges", "check", "walk", "layout", "keep", "encode"]);
    assert_eq!(tree_timing.size, Some((json.len() as u64, "bytes".into())), "how big the answer was");
    assert!(tree_timing.asked.contains("calls") && tree_timing.asked.contains("rust:sample::run"), "what was asked: {}", tree_timing.asked);
    assert_eq!(stages(&perf.recent[3]), ["source", "symbols"]);
    assert!(perf.graph.components > 10 && perf.store.files == 3);
    assert!(perf.store.bytes > 0, "the SQLite file's size");
    assert!(perf.memory_bytes.is_some_and(|m| m > 0));
}

#[test]
fn an_answer_over_the_limit_says_so_and_only_the_last_few_are_kept() {
    let root = sample();
    let limits = Limits::with_overrides("[limits]\nslow_request_ms = 0\nperf_kept = 3\n").unwrap();
    let service = open(&root, limits);
    service.ingest(&root).unwrap();
    let tree = service.tree(&TreeRequest { view: ViewKind::Contains, depth: Some(3), ..TreeRequest::default() }).unwrap();
    assert!(tree.slow_ms.is_some(), "over a 0 ms limit: the tree says it was slow");
    for _ in 0..5 {
        service.search("run", SearchKind::Symbols, &arch_graph::Source::Code, None, None).unwrap();
    }
    let perf = service.perf().unwrap();
    assert_eq!(perf.recent.len(), 3, "only the last few");
    assert!(perf.recent.iter().all(|t| t.slow));
    assert_eq!(perf.last_index.as_ref().map(|t| t.what.as_str()), Some("ingest"), "the last index run stays");
}
