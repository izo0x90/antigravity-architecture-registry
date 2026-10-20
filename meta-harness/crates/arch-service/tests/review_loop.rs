//! The review on a temp copy of the Rust fixture: the user marks the plan
//! reviewed and later changes are diffed; approval asks the discussions;
//! comments a plan's file still holds (from before discussions) go to them
//! once, and the file is written without them.

use std::path::Path;

use std::sync::{Arc, Mutex};

use arch_graph::{Change, ComponentId, Param, PlanId, RelationKind, Signature, Stage, TypeSystem};
use arch_service::{Actor, ArchitectureService, FsPlans, PlanOp, PlanRepository, ServiceError};
use serde_json::{Value, json};

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

fn open(root: &Path) -> ArchitectureService {
    ArchitectureService::open(
        root,
        common::shipped_packs(),
        TypeSystem::defaults().unwrap(),
        Box::new(FsPlans::new(root).unwrap()),
        arch_service::open_facts(root, &arch_service::Limits::defaults().unwrap()).unwrap(),
    )
    .unwrap()
}

fn id(s: &str) -> ComponentId {
    ComponentId::for_symbol("rust", s)
}

fn pid() -> PlanId {
    PlanId::new("r").unwrap()
}

fn user() -> Actor {
    Actor::User { name: "izo".into() }
}

fn agent() -> Actor {
    Actor::Agent { session: "s1".into() }
}

/// A plan adding `validate` (called by `run`), on the fixture.
fn setup() -> (tempfile::TempDir, std::path::PathBuf, ArchitectureService) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample"), &root);
    let service = open(&root);
    service.ingest(&root).unwrap();
    service.create_plan("r", "Review", "", arch_graph::PlanKind::Change).unwrap();
    service
        .apply(
            &pid(),
            PlanOp::AddComponent {
                name: "validate".into(),
                type_name: "function".into(),
                description: "Checks input.".into(),
                parent: Some(id("sample")),
                file: None,
                signature: Some(Signature { params: vec![Param { name: "input".into(), ty: Some("&str".into()), optional: false, variadic: false }], returns: Some("bool".into()), qualifiers: vec![] }),
                schemas: None,
                spec: None,
            },
        )
        .unwrap();
    service.apply(&pid(), PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::run"), target: id("sample::validate"), order: None, ports: None, note: None, expects: None }).unwrap();
    (dir, root, service)
}

/// What a mover was handed: each plan's comments' ids, in turn.
type Moved = Arc<Mutex<Vec<(String, Vec<String>)>>>;

/// A mover that takes them (or refuses while `refuse` says so).
fn mover(moved: &Moved, refuse: &Arc<Mutex<bool>>) -> arch_service::Mover {
    let (moved, refuse) = (moved.clone(), refuse.clone());
    Box::new(move |plan, comments| {
        if *refuse.lock().unwrap() {
            return Err("not now".into());
        }
        moved.lock().unwrap().push((plan.to_string(), comments.iter().map(|c| c["id"].as_str().unwrap_or_default().to_owned()).collect()));
        Ok(())
    })
}

fn file(root: &Path) -> std::path::PathBuf {
    root.join(".nah/plans/r.json")
}

/// The plan's file with `comments` in it, as files held them before.
fn with_comments(root: &Path, comments: Value) {
    let mut plan: Value = serde_json::from_str(&std::fs::read_to_string(file(root)).unwrap()).unwrap();
    plan["comments"] = comments;
    std::fs::write(file(root), serde_json::to_string_pretty(&plan).unwrap()).unwrap();
}

fn held(root: &Path) -> Option<Value> {
    let plan: Value = serde_json::from_str(&std::fs::read_to_string(file(root)).unwrap()).unwrap();
    plan.get("comments").cloned()
}

#[test]
fn comments_a_plans_file_still_holds_go_to_the_discussions_once_and_its_file_is_written_without_them() {
    let (_dir, root, s) = setup();
    let old = json!([
        { "id": "c1", "anchor": { "on": "component", "id": "rust:sample::validate" }, "author": { "kind": "user", "name": "izo" }, "text": "Pure?", "at": "2026-10-07T00:51:37Z" },
        { "id": "c2", "anchor": { "on": "plan" }, "kind": "change_request", "author": { "kind": "user", "name": "izo" }, "text": "Shorter.", "at": "2026-10-07T00:52:00Z", "status": "answered" }
    ]);
    with_comments(&root, old.clone());
    drop(s);
    // Read before anything takes them: they wait in the file, a save keeps them.
    let s = open(&root);
    s.apply(&pid(), PlanOp::Modify { id: id("sample::run"), description: Some("Validates first.".into()), signature: None, schemas: None, spec: None }).unwrap();
    assert_eq!(held(&root), Some(old.clone()), "saved with them still in it");
    // A mover that fails leaves them in the file.
    let (moved, refuse) = (Moved::default(), Arc::new(Mutex::new(true)));
    s.move_comments_to(mover(&moved, &refuse)).unwrap();
    assert_eq!(held(&root), Some(old.clone()));
    assert!(s.move_comments_to(mover(&moved, &refuse)).is_err(), "they go one way");
    // Read again (the file changed, say) once it takes them: moved once, the file without them.
    *refuse.lock().unwrap() = false;
    s.refresh(&[file(&root)]).unwrap();
    assert_eq!(*moved.lock().unwrap(), [("r".to_owned(), vec!["c1".to_owned(), "c2".to_owned()])]);
    assert_eq!(held(&root), None, "the file is written without them");
    assert_eq!(s.plan(&pid()).unwrap().components.iter().filter(|c| c.name == "validate").count(), 1, "the rest of the plan as it was");
    s.refresh(&[file(&root)]).unwrap();
    assert_eq!(moved.lock().unwrap().len(), 1, "once");
    // A file that comes with comments later (checked out, say): moved as it's read.
    with_comments(&root, json!([{ "id": "c9", "anchor": { "on": "plan" }, "author": { "kind": "user", "name": "izo" }, "text": "Later.", "at": "2026-10-08T00:00:00Z" }]));
    s.refresh(&[file(&root)]).unwrap();
    assert_eq!(moved.lock().unwrap().last().map(|(_, ids)| ids.clone()), Some(vec!["c9".to_owned()]));
    assert_eq!(held(&root), None);
    // A fresh read has nothing to move.
    assert!(FsPlans::new(&root).unwrap().get(&pid()).unwrap().is_some());
}

#[test]
fn approval_asks_the_discussions_and_waits_while_they_say_why_not() {
    let (_dir, _root, s) = setup();
    let open = Arc::new(Mutex::new(true));
    let held = open.clone();
    s.approvals_ask(Box::new(move |plan, id| (*held.lock().unwrap() && id.as_str() == "rust:sample::validate").then(|| format!("{id} in {plan} has discussions not settled: d1"))));
    let err = s.approve(&pid(), &id("sample::validate"), Stage::ArchApproved, &user()).unwrap_err().to_string();
    assert!(err.contains("rust:sample::validate in r has discussions not settled: d1"), "{err}");
    assert!(matches!(s.approve(&pid(), &id("sample::validate"), Stage::ArchApproved, &agent()), Err(ServiceError::Forbidden(_))));
    *open.lock().unwrap() = false;
    assert_eq!(s.approve(&pid(), &id("sample::validate"), Stage::ArchApproved, &user()).unwrap().to, Stage::ArchApproved);
}

#[test]
fn reviewing_snapshots_the_plan_and_later_changes_are_diffed() {
    let (_dir, _root, s) = setup();
    let before = s.review_diff(&pid()).unwrap();
    assert_eq!(before.since, None, "never reviewed: everything is new");
    assert!(before.changes.contains(&Change::ComponentAdded { id: id("sample::validate") }));
    assert!(matches!(s.mark_reviewed(&pid(), &agent()), Err(ServiceError::Forbidden(_))));
    let snapshot = s.mark_reviewed(&pid(), &user()).unwrap();
    assert_eq!(snapshot.by, "izo");
    assert!(s.review_diff(&pid()).unwrap().is_empty());
    s.apply(&pid(), PlanOp::Modify { id: id("sample::run"), description: Some("Validates first.".into()), signature: None, schemas: None, spec: None }).unwrap();
    let diff = s.review_diff(&pid()).unwrap();
    assert_eq!(diff.since.as_deref(), Some(snapshot.at.as_str()));
    assert!(diff.changes.iter().any(|c| matches!(c, Change::ComponentAdded { id: x } if x == &id("sample::run"))), "run joined the plan as modified: {diff:?}");
    assert_eq!(diff.fields(&id("sample::run")), ["added".to_owned()].into_iter().collect());
}
