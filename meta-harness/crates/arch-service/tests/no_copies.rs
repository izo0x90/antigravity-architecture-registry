//! No whole-graph copies: requests, plan edits,
//! approvals, verification and refresh read the shared graph and a plan's
//! own layer. A graph has no `Clone` (arch-graph won't compile with one), so
//! the copy left to catch is a rebuild that puts every component into a graph
//! again: inserts are counted. Alone in its binary: the counter is
//! process-wide.

use std::path::Path;

use arch_graph::tree::{TreeRequest, ViewKind};
use arch_graph::{ComponentId, Param, PlanId, RelationKind, Signature, Source, Stage, TypeSystem};
use arch_service::{Actor, ArchitectureService, FsPlans, PlanOp};

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

fn id(s: &str) -> ComponentId {
    ComponentId::for_symbol("rust", s)
}

#[test]
fn requests_plan_edits_approvals_verification_and_refresh_copy_no_graph() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample"), &root);
    let packs = common::shipped_packs();
    let service = ArchitectureService::open(
        &root,
        packs,
        TypeSystem::defaults().unwrap(),
        Box::new(FsPlans::new(&root).unwrap()),
        arch_service::open_facts(&root, &arch_service::Limits::defaults().unwrap()).unwrap(),
    )
    .unwrap();
    service.ingest(&root).unwrap();
    let plan = PlanId::new("p").unwrap();
    service.create_plan("p", "P", "", arch_graph::PlanKind::Change).unwrap();
    let user = Actor::User { name: "izo".into() };
    let source = Source::Plan(plan.clone());

    // A step may put into a graph at most the plan's own items and the
    // changed file's components; a rebuild puts in all of them.
    let mut did: Vec<(String, usize)> = Vec::new();
    let mut step = |name: &str, f: &mut dyn FnMut()| {
        let inserts = arch_graph::graph_inserts();
        f();
        did.push((name.to_owned(), arch_graph::graph_inserts() - inserts));
    };
    step("add a component", &mut || {
        service
            .apply(&plan, PlanOp::AddComponent { name: "validate".into(), type_name: "function".into(), description: "Checks input.".into(), parent: Some(id("sample")), file: None, signature: Some(Signature { params: vec![Param { name: "input".into(), ty: Some("&str".into()), optional: false, variadic: false }], returns: Some("bool".into()), qualifiers: vec![] }), schemas: None, spec: None })
            .unwrap();
    });
    step("relate", &mut || {
        service.apply(&plan, PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::run"), target: id("sample::validate"), order: None, ports: None, note: None, expects: None }).unwrap();
        service.apply(&plan, PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::validate"), target: id("sample::store::Store::new"), order: None, ports: None, note: None, expects: None }).unwrap();
    });
    step("read the plan's graph", &mut || {
        service.graph(&source).unwrap();
        service.graph(&source).unwrap();
    });
    step("trees", &mut || {
        for view in ViewKind::ALL {
            service.tree(&TreeRequest { view, source: source.clone(), ..TreeRequest::default() }).unwrap();
            service.tree(&TreeRequest { view, ..TreeRequest::default() }).unwrap();
        }
    });
    step("details", &mut || {
        service.component(&id("sample::validate"), &source).unwrap();
    });
    step("search", &mut || {
        service.search("run", arch_service::SearchKind::Symbols, &source, Some(5)).unwrap();
        service.search("run", arch_service::SearchKind::Symbols, &Source::Code, Some(5)).unwrap();
    });
    step("expand and context", &mut || {
        let request = TreeRequest { view: ViewKind::Calls, source: source.clone(), ..TreeRequest::default() };
        let tree = service.tree(&request).unwrap();
        if let Some(card) = tree.cards.first() {
            service.expand(&request, &card.key, 1).unwrap();
            service.context(&request, &card.key).unwrap();
        }
    });
    step("code view", &mut || {
        service.code_analysis("src/lib.rs").unwrap();
    });
    step("validate", &mut || {
        service.validate(&plan).unwrap();
    });
    step("approve", &mut || {
        service.approve(&plan, &id("sample::validate"), Stage::ArchApproved, &user).unwrap();
        service.approve(&plan, &id("sample::validate"), Stage::PlanApproved, &user).unwrap();
    });
    step("verify", &mut || {
        service.verify(&plan, &id("sample::validate"), Stage::Skeleton).unwrap();
    });
    step("refresh after a code change", &mut || {
        let util = root.join("src/util.rs");
        let text = std::fs::read_to_string(&util).unwrap();
        std::fs::write(&util, format!("{text}\npub fn added_later() {{}}\n")).unwrap();
        service.refresh(&[util]).unwrap();
        service.graph(&source).unwrap();
    });
    // A step may insert only the plan's own items and, for the refresh, the
    // changed file's components; more is a rebuild.
    let planned = service.plan(&plan).unwrap().components.len();
    let in_util = service.observed().unwrap().components().filter(|c| c.anchor.as_ref().is_some_and(|a| a.file.as_str() == "src/util.rs")).count();
    let rebuilt: Vec<_> = did.iter().filter(|(name, n)| *n > planned + if name.starts_with("refresh") { in_util } else { 0 }).collect();
    assert!(rebuilt.is_empty(), "whole rebuilds (step, components inserted; the plan has {planned} items, src/util.rs {in_util} components): {rebuilt:?}");
}
