//! What's built once per snapshot, not per request,
//! and expand and context building only their branch.
//! Each answer's timing says which stages ran.

use std::path::Path;

use arch_graph::tree::{TreeRequest, ViewKind};
use arch_graph::{PlanId, Source, TypeSystem};
use arch_service::{ArchitectureService, MemoryPlans, PlanOp, SearchKind};

mod common;

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

fn sample() -> (tempfile::TempDir, std::path::PathBuf, ArchitectureService) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample"), &root);
    let facts = arch_service::open_facts(&root, &arch_service::Limits::defaults().unwrap()).unwrap();
    let service = ArchitectureService::open(&root, common::shipped_packs(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), facts).unwrap();
    service.ingest(&root).unwrap();
    (dir, root, service)
}

/// The stages the last answer of kind `what` ran.
fn stages(service: &ArchitectureService, what: &str) -> Vec<String> {
    service.perf().unwrap().recent.iter().rev().find(|t| t.what == what).map(|t| t.stages.iter().map(|s| s.stage.clone()).collect()).unwrap_or_default()
}

#[test]
fn the_data_flow_index_the_symbol_list_and_the_badges_are_built_once_per_snapshot() {
    let (_dir, root, service) = sample();
    let flows = TreeRequest { view: ViewKind::DataFlow, ..TreeRequest::default() };
    service.tree(&flows).unwrap();
    assert!(stages(&service, "tree").contains(&"flow index".to_owned()), "built for the first request");
    assert!(stages(&service, "tree").contains(&"badges".to_owned()));
    service.tree(&flows).unwrap();
    service.tree(&TreeRequest { view: ViewKind::DataFlow, depth: Some(3), ..TreeRequest::default() }).unwrap();
    assert!(!stages(&service, "tree").contains(&"flow index".to_owned()), "kept for the next ones: {:?}", stages(&service, "tree"));
    assert!(!stages(&service, "tree").contains(&"badges".to_owned()));
    service.search("run", SearchKind::Symbols, &Source::Code, None).unwrap();
    assert_eq!(service.symbol_lists_built(), 1, "built for the first search");
    service.search("save", SearchKind::Symbols, &Source::Code, None).unwrap();
    assert_eq!(service.symbol_lists_built(), 1, "kept for the next ones");
    // A new snapshot: built again, once.
    let util = root.join("src/util.rs");
    std::fs::write(&util, std::fs::read_to_string(&util).unwrap() + "\npub fn added() {}\n").unwrap();
    service.refresh(&[util]).unwrap();
    service.tree(&flows).unwrap();
    assert!(stages(&service, "tree").contains(&"flow index".to_owned()), "the code changed");
    service.search("added", SearchKind::Symbols, &Source::Code, None).unwrap();
    assert_eq!(service.symbol_lists_built(), 2, "the code changed");
    let found = service.search("added", SearchKind::Symbols, &Source::Code, None).unwrap();
    assert!(found.hits.iter().any(|h| h.label.contains("added")), "the new snapshot's list");
    // A plan edited: the badges again.
    service.create_plan("p", "P", "").unwrap();
    service.tree(&flows).unwrap();
    assert!(stages(&service, "tree").contains(&"badges".to_owned()), "the plans changed");
    service.tree(&flows).unwrap();
    assert!(!stages(&service, "tree").contains(&"badges".to_owned()));
    let plan = PlanId::new("p").unwrap();
    service.apply(&plan, PlanOp::AddComponent { name: "check".into(), type_name: "function".into(), description: "Checks.".into(), parent: Some(arch_graph::ComponentId::for_symbol("rust", "sample")), file: None, signature: None, schemas: None, spec: None }).unwrap();
    service.tree(&flows).unwrap();
    assert!(stages(&service, "tree").contains(&"badges".to_owned()));
}

#[test]
fn expand_walks_only_the_asked_branch_and_says_nothing_already_shown() {
    let (_dir, _root, service) = sample();
    let request = TreeRequest { view: ViewKind::Calls, root: Some("rust:sample::run".into()), depth: Some(1), ..TreeRequest::default() };
    let tree = service.tree(&request).unwrap();
    let shown: Vec<String> = tree.cards.iter().map(|c| c.key.clone()).collect();
    let card = tree.cards.iter().find(|c| c.key != "rust:sample::run" && c.more > 0).expect("a card with more under it").key.clone();
    let added = service.expand(&request, &card, 1).unwrap();
    // The tree sent is kept: only the branch is walked, then merged into it.
    let expand_stages = stages(&service, "expand");
    assert!(expand_stages.iter().any(|s| s == "branch") && !expand_stages.iter().any(|s| s == "held" || s == "walk"), "no whole tree is walked: {expand_stages:?}");
    assert!(added.cards.iter().all(|c| !shown.contains(&c.key)), "only new cards");
    // The same cards a deeper tree adds under it.
    let deeper = service.tree(&TreeRequest { opened: vec![arch_graph::tree::Opened { card: card.clone(), levels: 1 }], ..request.clone() }).unwrap();
    let mut new_in_deeper: Vec<String> = deeper.cards.iter().map(|c| c.key.clone()).filter(|k| !shown.contains(k)).collect();
    let mut expanded: Vec<String> = added.cards.iter().map(|c| c.key.clone()).collect();
    new_in_deeper.sort();
    expanded.sort();
    assert_eq!(expanded, new_in_deeper);
}

#[test]
fn the_layout_keeps_to_limits_toml_layout_ms_and_a_cut_answer_says_so() {
    let (_dir, _root, service) = sample();
    let request = TreeRequest { view: ViewKind::Calls, root: Some("rust:sample::run".into()), depth: Some(3), ..TreeRequest::default() };
    let whole = service.tree(&request).unwrap();
    assert!(whole.cut.is_none(), "in time: all of it");
    // No time at all for laying out: the root is placed, the rest is cut.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample"), &root);
    let mut limits = arch_service::Limits::defaults().unwrap();
    limits.layout_ms = 0;
    let facts = arch_service::open_facts(&root, &limits).unwrap();
    let hurried = ArchitectureService::open(&root, common::shipped_packs(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), facts).unwrap().with_limits(limits);
    hurried.ingest(&root).unwrap();
    let tree = hurried.tree(&request).unwrap();
    let cut = tree.cut.expect("cut by the layout's limit");
    assert_eq!(cut.reason, arch_graph::tree::CutReason::Time);
    assert!(tree.cards.len() < whole.cards.len() && cut.hidden > 0, "{} of {} cards", tree.cards.len(), whole.cards.len());
}
