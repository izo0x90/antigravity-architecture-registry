//! The review loop on a temp copy of the Rust fixture: the user comments and
//! marks the plan reviewed, the agent answers each point with the changes it
//! made, and approval waits until every point on a component is resolved.

use std::path::Path;

use arch_graph::{
    Change, CommentAnchor, CommentId, CommentKind, CommentStatus, ComponentId, Param, Plan, PlanId, RelationKind, Signature, Stage, TypeSystem,
};
use arch_index::PackSet;
use arch_service::{Actor, ArchitectureService, FsIndex, FsPlans, NewComment, PlanOp, PlanRepository, ServiceError};

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
        PackSet::built_in().unwrap(),
        TypeSystem::defaults().unwrap(),
        Box::new(FsPlans::new(root).unwrap()),
        Box::new(FsIndex::new(root).unwrap()),
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
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../arch-index/tests/fixtures/rust/sample"), &root);
    let service = open(&root);
    service.ingest(&root).unwrap();
    service.create_plan("r", "Review", "").unwrap();
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

fn comment(s: &ArchitectureService, anchor: CommentAnchor, kind: CommentKind, text: &str) -> CommentId {
    s.add_comment(&pid(), NewComment { anchor, kind, text: text.into() }, &user()).unwrap().id
}

// req: R.1
#[test]
fn comments_on_nodes_sections_and_relations_persist_with_the_plan() {
    let (_dir, root, s) = setup();
    let on_node = comment(&s, CommentAnchor::Component { id: id("sample::validate") }, CommentKind::Comment, "Why a bool and not a Result?");
    let on_section = comment(&s, CommentAnchor::Section { id: id("sample::validate"), section: "logic".into() }, CommentKind::ChangeRequest, "Write the logic steps.");
    let on_relation = comment(
        &s,
        CommentAnchor::Relation { kind: RelationKind::Calls, source: id("sample::run"), target: id("sample::validate") },
        CommentKind::Comment,
        "Call it first thing.",
    );
    assert_eq!((on_node.0.as_str(), on_section.0.as_str(), on_relation.0.as_str()), ("c1", "c2", "c3"));

    // Anchors must exist; text is required; agents don't raise comments.
    assert!(matches!(
        s.add_comment(&pid(), NewComment { anchor: CommentAnchor::Component { id: id("sample::nope") }, kind: CommentKind::Comment, text: "x".into() }, &user()),
        Err(ServiceError::NotFound(_))
    ));
    assert!(matches!(
        s.add_comment(
            &pid(),
            NewComment { anchor: CommentAnchor::Relation { kind: RelationKind::Calls, source: id("sample::validate"), target: id("sample::run") }, kind: CommentKind::Comment, text: "x".into() },
            &user()
        ),
        Err(ServiceError::NotFound(_))
    ));
    assert!(s.add_comment(&pid(), NewComment { anchor: CommentAnchor::Plan, kind: CommentKind::Comment, text: "  ".into() }, &user()).is_err());
    assert!(matches!(s.add_comment(&pid(), NewComment { anchor: CommentAnchor::Plan, kind: CommentKind::Comment, text: "x".into() }, &agent()), Err(ServiceError::Forbidden(_))));

    // Threads: either side replies; status doesn't move.
    let replied = s.reply(&pid(), &on_node, "Because callers only branch on it.", &agent()).unwrap();
    assert_eq!((replied.replies.len(), replied.status), (1, CommentStatus::Open));

    // Persisted in the plan file: a fresh service sees them.
    let stored = FsPlans::new(&root).unwrap().get(&pid()).unwrap().unwrap();
    assert_eq!(stored.comments.len(), 3);
    assert_eq!(stored.comments[1].anchor, CommentAnchor::Section { id: id("sample::validate"), section: "logic".into() });
    assert_eq!(stored.comments[0].replies[0].text, "Because callers only branch on it.");
    let again = open(&root);
    assert_eq!(again.comments(&pid(), false).unwrap().len(), 3);
    assert!(Plan::from_json(&stored.to_json().unwrap()).unwrap() == *stored);
}

// req: R.2
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

// req: R.3
#[test]
fn agents_answer_with_linked_changes_and_unaddressed_points_block_approval() {
    let (_dir, _root, s) = setup();
    s.mark_reviewed(&pid(), &user()).unwrap();
    let question = comment(&s, CommentAnchor::Component { id: id("sample::validate") }, CommentKind::Comment, "Is it pure?");
    let change = comment(&s, CommentAnchor::Section { id: id("sample::validate"), section: "overview".into() }, CommentKind::ChangeRequest, "Say what it rejects.");
    let elsewhere = comment(&s, CommentAnchor::Component { id: id("sample::run") }, CommentKind::Comment, "Unrelated to validate.");

    // Blocked while anything on it is unaddressed, with each point listed.
    let err = s.approve(&pid(), &id("sample::validate"), Stage::ArchApproved, &user()).unwrap_err().to_string();
    assert!(err.contains("c1 on rust:sample::validate (open") && err.contains("c2 on rust:sample::validate § overview (open"), "{err}");
    assert!(!err.contains("c3"), "comments elsewhere don't block it: {err}");

    // A change request needs a linked change that touches what it's about.
    let no_change = s.answer_comment(&pid(), &change, "Done.", &[], &agent()).unwrap_err().to_string();
    assert!(no_change.contains("link the change you made"), "{no_change}");
    let not_changed = s.answer_comment(&pid(), &change, "Done.", &[id("sample::validate")], &agent()).unwrap_err().to_string();
    assert!(not_changed.contains("hasn't changed since the review"), "{not_changed}");
    s.apply(&pid(), PlanOp::Modify { id: id("sample::run"), description: Some("Validates first.".into()), signature: None, schemas: None, spec: None }).unwrap();
    let wrong = s.answer_comment(&pid(), &change, "Done.", &[id("sample::run")], &agent()).unwrap_err().to_string();
    assert!(wrong.contains("link the change you made to it"), "a change elsewhere doesn't answer it: {wrong}");

    // The agent changes the plan (re-adding the planned component with a
    // better description) and links it.
    s.apply(&pid(), PlanOp::Remove { id: id("sample::validate") }).unwrap();
    s.apply(
        &pid(),
        PlanOp::AddComponent {
            name: "validate".into(),
            type_name: "function".into(),
            description: "Rejects empty and whitespace-only input.".into(),
            parent: Some(id("sample")),
            file: None,
            signature: Some(Signature { params: vec![Param { name: "input".into(), ty: Some("&str".into()), optional: false, variadic: false }], returns: Some("bool".into()), qualifiers: vec![] }),
            schemas: None,
            spec: None,
        },
    )
    .unwrap();
    s.apply(&pid(), PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::run"), target: id("sample::validate"), order: None, ports: None, note: None, expects: None }).unwrap();
    let answered = s.answer_comment(&pid(), &change, "Described what it rejects.", &[id("sample::validate")], &agent()).unwrap();
    assert_eq!(answered.status, CommentStatus::Answered);
    let answer = answered.answer.unwrap();
    assert_eq!(answer.changes, vec![Change::ComponentChanged { id: id("sample::validate"), fields: vec!["description".into()] }]);
    assert!(s.answer_comment(&pid(), &question, "Yes: no I/O.", &[], &agent()).is_ok(), "questions can be answered without changes");

    // Answered isn't addressed: the user has to accept.
    let err = s.approve(&pid(), &id("sample::validate"), Stage::ArchApproved, &user()).unwrap_err().to_string();
    assert!(err.contains("(answered; accept or reopen it)"), "{err}");
    assert!(matches!(s.accept_comment(&pid(), &change, &agent()), Err(ServiceError::Forbidden(_))));
    assert!(s.accept_comment(&pid(), &elsewhere, &user()).is_err(), "nothing to accept yet");
    s.accept_comment(&pid(), &change, &user()).unwrap();
    // Reopened: open again, with the reason in the thread.
    let reopened = s.reopen_comment(&pid(), &question, "What about logging?", &user()).unwrap();
    assert_eq!((reopened.status, reopened.replies.last().unwrap().text.as_str()), (CommentStatus::Open, "What about logging?"));
    assert!(s.approve(&pid(), &id("sample::validate"), Stage::ArchApproved, &user()).is_err());
    s.answer_comment(&pid(), &question, "It logs nothing; the caller does.", &[], &agent()).unwrap();
    s.accept_comment(&pid(), &question, &user()).unwrap();
    assert!(s.answer_comment(&pid(), &question, "again", &[], &agent()).is_err(), "resolved points are closed");

    let approval = s.approve(&pid(), &id("sample::validate"), Stage::ArchApproved, &user()).unwrap();
    assert_eq!(approval.to, Stage::ArchApproved);
    assert_eq!(s.comments(&pid(), false).unwrap().iter().map(|c| c.id.0.as_str()).collect::<Vec<_>>(), vec!["c3"], "only the unrelated point is left");
    // A plan-wide point blocks every approval in the plan.
    comment(&s, CommentAnchor::Plan, CommentKind::Comment, "Hold everything.");
    assert!(s.approve(&pid(), &id("sample::validate"), Stage::PlanApproved, &user()).unwrap_err().to_string().contains("on the plan"));
}
