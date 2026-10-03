//! The service end to end on a temp copy of the Rust fixture: ingest, plan,
//! check, approve, contract, write code, verify.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arch_graph::{
    ArchEvent, ChangeReason, ComponentId, FindingCode, Graph, Param, Plan, PlanId, ProposedLocation, Provenance, Relation, RelationKind, RepoPath,
    Requester, Severity, Signature, Stage, Status, TypeSystem, ViewCatalog, ViewRequest,
};
use arch_index::PackSet;
use arch_service::{Actor, ArchitectureService, FsIndex, FsPlans, PlanOp, PlanRepository, ServiceError};

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

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    service: ArchitectureService,
    events: Arc<Mutex<Vec<ArchEvent>>>,
}

fn repo() -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../arch-index/tests/fixtures/rust/sample"), &root);
    let service = ArchitectureService::open(
        &root,
        PackSet::built_in().unwrap(),
        TypeSystem::defaults().unwrap(),
        ViewCatalog::defaults().unwrap(),
        Box::new(FsPlans::new(&root).unwrap()),
        Box::new(FsIndex::new(&root).unwrap()),
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    service.subscribe(move |e| sink.lock().unwrap().push(e.clone()));
    service.ingest(&root).unwrap();
    Repo { _dir: dir, root, service, events }
}

fn id(s: &str) -> ComponentId {
    ComponentId::for_symbol("rust", s)
}

fn pid(s: &str) -> PlanId {
    PlanId::new(s).unwrap()
}

fn user() -> Actor {
    Actor::User { name: "izo".into() }
}

fn add_validate(r: &Repo, plan: &str) {
    r.service
        .apply(
            &pid(plan),
            PlanOp::AddComponent {
                name: "validate".into(),
                type_name: "function".into(),
                description: "Checks input before running.".into(),
                parent: Some(id("sample")),
                file: None,
                signature: Some(Signature { params: vec![Param { name: "input".into(), ty: Some("&str".into()) }], returns: Some("bool".into()) }),
                schemas: None,
                spec: None,
            },
        )
        .unwrap();
}

fn relate(r: &Repo, plan: &str, kind: RelationKind, a: &str, b: &str) {
    r.service.apply(&pid(plan), PlanOp::Relate { kind, source: id(a), target: id(b), order: None, ports: None, note: None }).unwrap();
}

fn write(r: &Repo, file: &str, text: &str) {
    std::fs::write(r.root.join(file), text).unwrap();
}

fn append(r: &Repo, file: &str, text: &str) {
    let mut current = std::fs::read_to_string(r.root.join(file)).unwrap();
    current.push_str(text);
    write(r, file, &current);
}

// req: P3.5
#[test]
fn service_merges_code_and_plans_and_announces_changes() {
    let r = repo();
    assert!(matches!(r.events.lock().unwrap().last(), Some(ArchEvent::Changed { reason: ChangeReason::Ingested, .. })));
    r.service.create_plan("v", "Validation", "").unwrap();
    add_validate(&r, "v");
    let observed = r.service.observed().unwrap();
    assert!(observed.component(&id("sample::validate")).is_none(), "plans never touch the observed graph");
    let merged = r.service.graph(Some(&pid("v"))).unwrap();
    assert_eq!(merged.provenance(&id("sample::validate")), Provenance::PLANNED);
    assert_eq!(merged.provenance(&id("sample::run")), Provenance::OBSERVED);
    let events = r.events.lock().unwrap();
    assert!(events.iter().any(|e| matches!(e, ArchEvent::Changed { reason: ChangeReason::PlanEdited, components, .. } if components.contains(&id("sample::validate")))));
}

// req: P4.7
#[test]
fn ingestion_is_incremental_and_follows_deletes_and_gitignore() {
    let r = repo();
    let again = r.service.ingest(&r.root).unwrap();
    assert_eq!((again.parsed, again.cached), (0, 3), "nothing changed");
    append(&r, "src/util.rs", "\npub fn quiet(s: &str) -> String { s.to_lowercase() }\n");
    let changed = r.service.ingest(&r.root).unwrap();
    assert_eq!((changed.parsed, changed.cached), (1, 2));
    assert!(r.service.observed().unwrap().component(&id("sample::util::quiet")).is_some());
    std::fs::remove_file(r.root.join("src/store.rs")).unwrap();
    let removed = r.service.ingest(&r.root).unwrap();
    assert_eq!(removed.removed, 1);
    assert!(r.service.observed().unwrap().component(&id("sample::store::Store")).is_none());
    write(&r, ".gitignore", "src/generated.rs\n");
    write(&r, "src/generated.rs", "pub fn gen() {}\n");
    r.service.ingest(&r.root).unwrap();
    assert!(r.service.observed().unwrap().component(&id("sample::generated::gen")).is_none(), "ignored files stay out");
    // Scoped ingest of one file leaves the rest alone.
    let scoped = r.service.ingest(&r.root.join("src/util.rs")).unwrap();
    assert_eq!((scoped.files, scoped.removed), (1, 0));
    assert!(r.service.observed().unwrap().component(&id("sample::run")).is_some());
}

// req: P4.11
#[test]
fn referenced_components_and_subtrees_stay_existing_with_real_anchors() {
    let r = repo();
    r.service.create_plan("p", "Extend run", "").unwrap();
    r.service.apply(&pid("p"), PlanOp::Reference { id: id("sample::run"), kinds: vec![RelationKind::Calls], depth: 2 }).unwrap();
    let scope = r.service.scope(&pid("p")).unwrap();
    for expected in ["sample::run", "sample::store::Store::new", "sample::Upper::apply", "sample::store::Store::save", "sample::log", "sample::util::shout", "sample::store::Store::check"] {
        assert!(scope.contains(&id(expected)), "{expected} in scope: {scope:?}");
    }
    assert!(!scope.contains(&id("sample::later")));
    let graph = r.service.graph(Some(&pid("p"))).unwrap();
    for c in scope.iter().map(|i| graph.component(i).unwrap()) {
        assert_eq!((c.status, c.stage), (Status::Existing, Stage::Implemented), "{}", c.id);
        assert!(c.anchor.is_some());
    }
    let err = r.service.apply(&pid("p"), PlanOp::Reference { id: id("sample::nope"), kinds: vec![], depth: 0 }).unwrap_err();
    assert!(matches!(err, ServiceError::InvalidOp(_)));
}

// req: P5.1
#[test]
fn plan_operations_validate_inputs_and_reject_bad_ones() {
    let r = repo();
    r.service.create_plan("p", "Ops", "").unwrap();
    add_validate(&r, "p");
    let plan = r.service.plan(&pid("p")).unwrap();
    let v = plan.component(&id("sample::validate")).unwrap();
    assert_eq!(v.proposed.as_ref().unwrap(), &ProposedLocation { language: "rust".into(), file: RepoPath::new("src/lib.rs").unwrap(), parent: Some(id("sample")), symbol: "sample::validate".into() });
    assert!(plan.relations.iter().any(|rel| rel.kind == RelationKind::Contains && rel.target == id("sample::validate")));

    let reject = |op: PlanOp, needle: &str| {
        let err = r.service.apply(&pid("p"), op).unwrap_err().to_string();
        assert!(err.contains(needle), "'{err}' should mention '{needle}'");
    };
    let add = |name: &str, ty: &str, parent: Option<&str>, file: Option<&str>| PlanOp::AddComponent {
        name: name.into(),
        type_name: ty.into(),
        description: String::new(),
        parent: parent.map(id),
        file: file.map(str::to_owned),
        signature: None,
        schemas: None,
        spec: None,
    };
    reject(add("x", "gizmo", Some("sample"), None), "unknown type");
    reject(add("x", "function", Some("sample::nope"), None), "doesn't exist");
    reject(add("run", "function", Some("sample"), None), "already exists");
    reject(add("two words", "function", Some("sample"), None), "single identifier");
    reject(add("m", "module", Some("sample::run"), None), "can't contain");
    reject(add("x", "function", None, None), "parent or a file");
    reject(add("x", "function", None, Some("notes/readme.md")), "no language pack");
    reject(PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::Upper"), target: id("sample::log"), order: None, ports: None, note: None }, "can't call");
    reject(PlanOp::Relate { kind: RelationKind::DataFlow, source: id("sample::run"), target: id("sample::log"), order: None, ports: None, note: None }, "ports");
    reject(PlanOp::Unrelate { kind: RelationKind::Calls, source: id("sample::run"), target: id("sample::later") }, "no calls relation");
    reject(PlanOp::Remove { id: id("sample::run") }, "isn't part of this plan");

    r.service.apply(&pid("p"), PlanOp::Modify { id: id("sample::run"), description: Some("now validates".into()), signature: None, schemas: None, spec: None }).unwrap();
    let plan = r.service.plan(&pid("p")).unwrap();
    let run = plan.component(&id("sample::run")).unwrap();
    assert_eq!((run.status, run.stage, run.description.as_str()), (Status::Modifying, Stage::Declared, "now validates"));
    r.service.apply(&pid("p"), PlanOp::Remove { id: id("sample::validate") }).unwrap();
    let plan = r.service.plan(&pid("p")).unwrap();
    assert!(plan.component(&id("sample::validate")).is_none());
    assert!(!plan.relations.iter().any(|rel| rel.target == id("sample::validate")));
}

/// Saves a plan as-is (bypassing operations) to test checks on bad input.
fn save_raw(r: &Repo, plan: &Plan) {
    FsPlans::new(&r.root).unwrap().save(plan).unwrap();
}

fn planned(symbol: &str, file: &str, parent: Option<&str>) -> arch_graph::Component {
    arch_graph::Component {
        id: id(symbol),
        name: symbol.rsplit("::").next().unwrap().into(),
        type_name: "function".into(),
        description: String::new(),
        status: Status::New,
        stage: Stage::Declared,
        anchor: None,
        proposed: Some(ProposedLocation { language: "rust".into(), file: RepoPath::new(file).unwrap(), parent: parent.map(id), symbol: symbol.into() }),
        signature: None,
        schemas: Default::default(),
        spec: None,
        placeholders: vec![],
    }
}

// req: P5.3
#[test]
fn dangling_references_are_reported_with_ids_and_referrers() {
    let r = repo();
    let mut plan = Plan::new("raw", "Raw");
    plan.greenfield = true;
    plan.components.push(planned("sample::orphan", "src/lib.rs", Some("sample::gone")));
    plan.relations.push(Relation::new(RelationKind::Calls, id("sample::run"), id("sample::ghost"), Provenance::PLANNED));
    plan.references.push(arch_graph::Reference { id: id("sample::removed"), kinds: vec![], depth: 0 });
    save_raw(&r, &plan);
    let report = r.service.validate(&pid("raw")).unwrap();
    let dangling: Vec<&str> = report.findings.iter().filter(|f| f.code == FindingCode::DanglingReference).map(|f| f.message.as_str()).collect();
    assert_eq!(dangling.len(), 3, "{dangling:?}");
    assert!(dangling.iter().any(|m| m.contains("rust:sample::gone")));
    assert!(dangling.iter().any(|m| m.contains("rust:sample::ghost") && m.contains("rust:sample::run")));
    assert!(dangling.iter().any(|m| m.contains("rust:sample::removed")));
}

// req: P5.7
#[test]
fn proposed_locations_are_checked() {
    let r = repo();
    let mut plan = Plan::new("loc", "Locations");
    plan.greenfield = true;
    plan.components.push(planned("sample::nowhere::f", "src/nowhere/deep.rs", None));
    let mut wrong_lang = planned("sample::py", "src/script.py", None);
    wrong_lang.proposed.as_mut().unwrap().language = "rust".into();
    plan.components.push(wrong_lang);
    plan.components.push(planned("sample::store::Store::extra", "src/lib.rs", Some("sample::store::Store")));
    plan.components.push(planned("sample::log", "src/lib.rs", Some("sample")));
    plan.components.push(planned("sample::fine", "src/lib.rs", Some("sample")));
    save_raw(&r, &plan);
    let report = r.service.validate(&pid("loc")).unwrap();
    let about = |sym: &str, code: FindingCode| report.findings.iter().any(|f| f.code == code && f.component() == Some(&id(sym)));
    assert!(about("sample::nowhere::f", FindingCode::InvalidLocation), "new file in a missing directory");
    assert!(about("sample::py", FindingCode::InvalidLocation), "no pack for .py");
    assert!(about("sample::store::Store::extra", FindingCode::InvalidLocation), "parent lives in another file");
    assert!(about("sample::log", FindingCode::NameCollision), "symbol already in the code");
    assert!(!report.findings.iter().any(|f| f.component() == Some(&id("sample::fine"))), "{report:?}");
}

// req: P5.12
#[test]
fn findings_have_stable_codes_and_order() {
    let r = repo();
    let mut plan = Plan::new("rep", "Report");
    plan.components.push(planned("sample::a", "src/lib.rs", Some("sample")));
    plan.components.push(planned("sample::log", "src/lib.rs", Some("sample")));
    save_raw(&r, &plan);
    let report = r.service.validate(&pid("rep")).unwrap();
    let severities: Vec<Severity> = report.findings.iter().map(|f| f.severity).collect();
    let mut sorted = severities.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(severities, sorted, "errors first");
    assert_eq!(report.codes(), vec![FindingCode::MissingTouchPoint, FindingCode::NameCollision]);
    let json = serde_json::to_value(&report.findings[0]).unwrap();
    assert_eq!(json["code"], "missing_touch_point");
    assert_eq!(json["severity"], "error");
    assert_eq!(json["target"]["kind"], "plan");
    assert_eq!(report, r.service.validate(&pid("rep")).unwrap(), "deterministic");
}

// req: P5.14
#[test]
fn only_users_approve_and_only_legal_checked_moves() {
    let r = repo();
    r.service.create_plan("a", "Approvals", "").unwrap();
    add_validate(&r, "a");
    relate(&r, "a", RelationKind::Calls, "sample::run", "sample::validate");
    let agent = Actor::Agent { session: "s1".into() };
    let err = r.service.approve(&pid("a"), &id("sample::validate"), Stage::ArchApproved, &agent).unwrap_err();
    assert!(matches!(err, ServiceError::Forbidden(_)));
    assert!(r.service.approve(&pid("a"), &id("sample::validate"), Stage::PlanApproved, &user()).is_err(), "can't skip arch approval");
    assert!(r.service.approve(&pid("a"), &id("sample::validate"), Stage::Implemented, &user()).is_err(), "implemented comes from verification");
    let approval = r.service.approve(&pid("a"), &id("sample::validate"), Stage::ArchApproved, &user()).unwrap();
    assert_eq!((approval.by.as_str(), approval.from, approval.to), ("izo", Stage::Declared, Stage::ArchApproved));
    assert!(approval.at.len() == 20 && approval.at.ends_with('Z'), "{}", approval.at);
    let plan = r.service.plan(&pid("a")).unwrap();
    assert_eq!(plan.approvals, vec![approval]);
    // A component with blocking findings can't be approved.
    let mut bad = Plan::new("b", "Bad");
    bad.components.push(planned("sample::store::Store::misplaced", "src/lib.rs", Some("sample::store::Store")));
    save_raw(&r, &bad);
    let err = r.service.approve(&pid("b"), &id("sample::store::Store::misplaced"), Stage::ArchApproved, &user()).unwrap_err();
    assert!(err.to_string().contains("can't be approved yet"), "{err}");
}

// req: P5.16
#[test]
fn new_work_must_touch_existing_code_unless_greenfield() {
    let r = repo();
    r.service.create_plan("t", "Touch", "").unwrap();
    add_validate(&r, "t");
    let codes = |r: &Repo| r.service.validate(&pid("t")).unwrap().codes();
    assert!(codes(&r).contains(&FindingCode::MissingTouchPoint), "containment alone isn't integration");
    relate(&r, "t", RelationKind::Calls, "sample::run", "sample::validate");
    assert!(!codes(&r).contains(&FindingCode::MissingTouchPoint));
    r.service.apply(&pid("t"), PlanOp::Unrelate { kind: RelationKind::Calls, source: id("sample::run"), target: id("sample::validate") }).unwrap();
    r.service.apply(&pid("t"), PlanOp::SetGreenfield { greenfield: true }).unwrap();
    assert!(!codes(&r).contains(&FindingCode::MissingTouchPoint));
}

// req: P5.17
#[test]
fn contract_slice_carries_real_signatures_and_neighbors() {
    let r = repo();
    r.service.create_plan("c", "Contract", "").unwrap();
    add_validate(&r, "c");
    relate(&r, "c", RelationKind::Calls, "sample::run", "sample::validate");
    relate(&r, "c", RelationKind::Calls, "sample::validate", "sample::store::Store::new");
    let slice = r.service.contract(&pid("c"), &id("sample::validate")).unwrap();
    assert_eq!(slice.proposed.as_ref().unwrap().file.as_str(), "src/lib.rs");
    let new = slice.calls.iter().find(|n| n.id == id("sample::store::Store::new")).unwrap();
    assert_eq!(new.anchor.as_ref().unwrap().file.as_str(), "src/store.rs", "real location of the callee");
    assert_eq!(new.signature.as_ref().unwrap().returns.as_deref(), Some("Self"));
    assert_eq!(slice.called_by.iter().map(|n| n.id.clone()).collect::<Vec<_>>(), vec![id("sample::run")]);
    assert_eq!(slice.container.as_ref().unwrap().id, id("sample"));
    let md = slice.to_markdown();
    assert!(md.contains("Put it in src/lib.rs as `sample::validate`"), "{md}");
    assert!(md.contains("## Must call (in order)") && md.contains("src/store.rs:6"), "{md}");
    assert!(md.contains("`(input: &str) -> bool`"));
}

fn approved_validate(r: &Repo, plan: &str) {
    r.service.create_plan(plan, "Verify", "").unwrap();
    add_validate(r, plan);
    relate(r, plan, RelationKind::Calls, "sample::run", "sample::validate");
    relate(r, plan, RelationKind::Calls, "sample::validate", "sample::store::Store::new");
    r.service.approve(&pid(plan), &id("sample::validate"), Stage::ArchApproved, &user()).unwrap();
    r.service.approve(&pid(plan), &id("sample::validate"), Stage::PlanApproved, &user()).unwrap();
}

// req: P6.1
#[test]
fn verification_finds_the_component_or_reports_moved_and_missing() {
    let r = repo();
    approved_validate(&r, "v");
    let v = r.service.verify(&pid("v"), &id("sample::validate"), Stage::Skeleton).unwrap();
    assert_eq!(v.report.codes(), vec![FindingCode::MissingSymbol]);
    assert_eq!(v.advanced_to, None);
    append(&r, "src/util.rs", "\npub fn validate(input: &str) -> bool { crate::store::Store::new(); !input.is_empty() }\n");
    r.service.ingest(&r.root).unwrap();
    let v = r.service.verify(&pid("v"), &id("sample::validate"), Stage::Skeleton).unwrap();
    assert_eq!(v.report.codes(), vec![FindingCode::Moved]);
    let moved = &v.report.findings[0];
    assert!(moved.message.contains("sample::util::validate"), "{}", moved.message);
    assert_eq!(moved.at.as_ref().unwrap().file.as_str(), "src/util.rs");
}

// req: P6.2
#[test]
fn planned_calls_must_happen_inside_the_body() {
    let r = repo();
    approved_validate(&r, "c");
    append(&r, "src/lib.rs", "\npub fn validate(input: &str) -> bool { log(input); !input.is_empty() }\n");
    let v = r.service.verify(&pid("c"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.report.codes(), vec![FindingCode::MissingCallSite]);
    assert!(v.report.findings[0].message.contains("doesn't call rust:sample::store::Store::new"));
    assert!(v.report.findings[0].at.is_some(), "points at the code");
    write(&r, "src/lib.rs", &std::fs::read_to_string(r.root.join("src/lib.rs")).unwrap().replace("log(input); !input", "crate::store::Store::new(); !input"));
    let v = r.service.verify(&pid("c"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert!(!v.report.has_errors(), "{:?}", v.report);
    assert_eq!(v.advanced_to, Some(Stage::Implemented));
    let graph: Graph = r.service.graph(Some(&pid("c"))).unwrap();
    let calls: Vec<_> = graph.outgoing(&id("sample::validate"), Some(RelationKind::Calls)).collect();
    assert!(calls.iter().any(|c| c.target == id("sample::store::Store::new") && c.provenance.is_both() && !c.sites.is_empty()), "call site recorded");
}

// req: P6.3
#[test]
fn signatures_are_compared_with_the_plan() {
    let r = repo();
    approved_validate(&r, "s");
    append(&r, "src/lib.rs", "\npub fn validate(text: String) -> u8 { crate::store::Store::new(); 0 }\n");
    let v = r.service.verify(&pid("s"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.report.codes(), vec![FindingCode::SignatureMismatch]);
    let msg = &v.report.findings[0].message;
    assert!(msg.contains("named 'text'") && msg.contains("planned type &str, code has String") && msg.contains("returns: planned bool, code has u8"), "{msg}");
}

// req: P6.10
#[test]
fn skeleton_verifies_with_placeholders_and_blocks_implemented_until_they_are_gone() {
    let r = repo();
    approved_validate(&r, "k");
    append(&r, "src/lib.rs", "\npub fn validate(input: &str) -> bool {\n    crate::store::Store::new();\n    todo!()\n}\n");
    let v = r.service.verify(&pid("k"), &id("sample::validate"), Stage::Skeleton).unwrap();
    assert_eq!(v.advanced_to, Some(Stage::Skeleton), "{:?}", v.report);
    assert_eq!(v.report.findings[0].severity, Severity::Info);
    let v = r.service.verify(&pid("k"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.report.codes(), vec![FindingCode::PlaceholderRemains]);
    assert_eq!(v.advanced_to, None);
    assert_eq!(r.service.plan(&pid("k")).unwrap().component(&id("sample::validate")).unwrap().stage, Stage::Skeleton);
    write(&r, "src/lib.rs", &std::fs::read_to_string(r.root.join("src/lib.rs")).unwrap().replace("    todo!()\n", "    !input.is_empty()\n"));
    let v = r.service.verify(&pid("k"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, Some(Stage::Implemented), "{:?}", v.report);
    let merged = r.service.graph(Some(&pid("k"))).unwrap();
    merged.component(&id("sample::validate")).unwrap().check_location().unwrap();
}

#[test]
fn view_requests_are_checked_and_announced() {
    let r = repo();
    assert!(r.service.request_view(ViewRequest { view: Some("nonsense".into()), ..Default::default() }, Requester::Agent).is_err());
    r.service.request_view(ViewRequest { view: Some("calls".into()), focus: Some(id("sample::run")), ..Default::default() }, Requester::Agent).unwrap();
    assert!(matches!(r.events.lock().unwrap().last(), Some(ArchEvent::ViewRequested { by: Requester::Agent, .. })));
    let view = r.service.named_view("calls", None, vec![id("sample::run")], Some(1)).unwrap();
    assert!(view.text.contains("run [function"));
    assert!(view.layout.rects.contains_key(&id("sample::run")));
    let found = r.service.search("store", None, 10).unwrap();
    let top: Vec<_> = found.iter().take(2).map(|c| c.id.clone()).collect();
    assert_eq!(top, vec![id("sample::store"), id("sample::store::Store")], "exact name matches first, ties by id");
    let detail = r.service.component(&id("sample::log"), None).unwrap();
    assert!(detail.code.unwrap().contains("println!"));
    assert!(view.components.contains_key(&id("sample::run")), "views carry their components");
    assert_eq!(r.service.component_at("src/store.rs", 11, None).unwrap().id, id("sample::store::Store::save"));
    assert_eq!(r.service.component_at("src/store.rs", 2, None).unwrap().id, id("sample::store::Store"));
    assert!(r.service.component_at("src/none.rs", 1, None).is_err());
    let file = r.service.file_view("src/store.rs", None).unwrap();
    assert_eq!(file.components.len(), 6, "module, Store, new, save, check, helper");
    assert!(file.relations.iter().any(|(rel, _)| rel.kind == RelationKind::Calls && rel.target == id("sample::store::Store::check")));
}
