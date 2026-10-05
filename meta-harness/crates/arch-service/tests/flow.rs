//! The service end to end on a temp copy of the Rust fixture: ingest, plan,
//! check, approve, contract, write code, verify.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arch_graph::{
    ArchEvent, ChangeReason, ComponentId, FindingCode, Graph, Param, Plan, PlanId, ProposedLocation, Provenance, Relation, RelationKind, RepoPath,
    Requester, Severity, Signature, Stage, Status, TypeSystem, ViewCatalog, ViewRequest,
};
use arch_index::{CodeRules, PackSet};
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
    repo_with("", |s| s)
}

/// The fixture repo with project code rules (TOML) on top of the defaults.
fn repo_with_rules(rules_toml: &str) -> Repo {
    repo_with(rules_toml, |s| s)
}

/// A repo with project code rules, set up further (tools, reviewers) before ingesting.
fn repo_with(rules_toml: &str, setup: impl FnOnce(ArchitectureService) -> ArchitectureService) -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../arch-index/tests/fixtures/rust/sample"), &root);
    let (packs, types) = (PackSet::built_in().unwrap(), TypeSystem::defaults().unwrap());
    let mut rules = CodeRules::defaults(&packs, &types).unwrap();
    rules.add_toml(rules_toml, &packs, &types).unwrap();
    let service = ArchitectureService::open(
        &root,
        packs,
        types,
        ViewCatalog::defaults().unwrap(),
        Box::new(FsPlans::new(&root).unwrap()),
        Box::new(FsIndex::new(&root).unwrap()),
    )
    .unwrap()
    .with_rules(rules);
    let service = setup(service);
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
                signature: Some(Signature { params: vec![Param { name: "input".into(), ty: Some("&str".into()), optional: false }], returns: Some("bool".into()), qualifiers: vec![] }),
                schemas: None,
                spec: None,
            },
        )
        .unwrap();
}

fn relate(r: &Repo, plan: &str, kind: RelationKind, a: &str, b: &str) {
    r.service.apply(&pid(plan), PlanOp::Relate { kind, source: id(a), target: id(b), order: None, ports: None, note: None, expects: None }).unwrap();
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
    reject(PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::Upper"), target: id("sample::log"), order: None, ports: None, note: None, expects: None }, "can't call");
    reject(PlanOp::Relate { kind: RelationKind::DataFlow, source: id("sample::run"), target: id("sample::log"), order: None, ports: None, note: None, expects: None }, "ports");
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
        description: format!("Does {symbol}."),
        status: Status::New,
        stage: Stage::Declared,
        anchor: None,
        proposed: Some(ProposedLocation { language: "rust".into(), file: RepoPath::new(file).unwrap(), parent: parent.map(id), symbol: symbol.into() }),
        signature: Some(Signature::default()),
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

// req: P6.12
#[test]
fn project_code_rules_report_in_checks_and_block_verification() {
    let r = repo_with_rules(
        r#"
[[rule]]
id = "no-println"
language = "rust"
types = ["function"]
kind = "forbidden"
rule = { kind = "macro_invocation", has = { field = "macro", regex = "^println$" } }
message = "log through the logger"
"#,
    );
    // Plan checks: a referenced component that breaks a rule is a finding at the match.
    r.service.create_plan("p", "Touch logging", "").unwrap();
    r.service.apply(&pid("p"), PlanOp::Reference { id: id("sample::log"), kinds: vec![], depth: 0 }).unwrap();
    let report = r.service.validate(&pid("p")).unwrap();
    let hit = report.findings.iter().find(|f| f.code == FindingCode::ForbiddenPattern).unwrap();
    assert_eq!((hit.severity, hit.component()), (Severity::Error, Some(&id("sample::log"))));
    assert!(hit.message.starts_with("[no-println] log through the logger"), "{}", hit.message);
    assert_eq!(hit.at.as_ref().map(|a| (a.file.as_str(), a.span.start_line)), Some(("src/lib.rs", 30)));
    // Verification: the rule blocks Implemented until the code complies.
    approved_validate(&r, "v");
    append(&r, "src/lib.rs", "\npub fn validate(input: &str) -> bool { crate::store::Store::new(); println!(\"{input}\"); !input.is_empty() }\n");
    let v = r.service.verify(&pid("v"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.report.codes(), vec![FindingCode::ForbiddenPattern]);
    assert_eq!(v.advanced_to, None);
    write(&r, "src/lib.rs", &std::fs::read_to_string(r.root.join("src/lib.rs")).unwrap().replace(" println!(\"{input}\");", ""));
    let v = r.service.verify(&pid("v"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, Some(Stage::Implemented), "{:?}", v.report);
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
    assert_eq!(file.components.len(), 7, "module, Store, new, save, check, helper, store_each");
    assert!(file.relations.iter().any(|(rel, _)| rel.kind == RelationKind::Calls && rel.target == id("sample::store::Store::check")));
}

fn str_fn(r: &Repo, plan: &str, name: &str, spec: Option<arch_graph::ImplementationSpec>) {
    r.service
        .apply(
            &pid(plan),
            PlanOp::AddComponent {
                name: name.into(),
                type_name: "function".into(),
                description: format!("{name} the input."),
                parent: Some(id("sample")),
                file: None,
                signature: Some(Signature { params: vec![Param { name: "input".into(), ty: Some("&str".into()), optional: false }], returns: Some("bool".into()), qualifiers: vec![] }),
                schemas: None,
                spec,
            },
        )
        .unwrap();
}

fn approve_both(r: &Repo, plan: &str, symbol: &str) {
    r.service.approve(&pid(plan), &id(symbol), Stage::ArchApproved, &user()).unwrap();
    r.service.approve(&pid(plan), &id(symbol), Stage::PlanApproved, &user()).unwrap();
}

fn stage_of(r: &Repo, plan: &str, symbol: &str) -> Stage {
    r.service.plan(&pid(plan)).unwrap().component(&id(symbol)).unwrap().stage
}

fn replace_in(r: &Repo, file: &str, from: &str, to: &str) {
    let text = std::fs::read_to_string(r.root.join(file)).unwrap();
    assert!(text.contains(from), "{from} not in {file}");
    write(r, file, &text.replace(from, to));
}

/// run -> validate -> check_all -> Store::new, with validate and check_all
/// planned and implemented.
fn implemented_chain(r: &Repo, plan: &str) {
    r.service.create_plan(plan, "Chain", "").unwrap();
    str_fn(r, plan, "validate", None);
    str_fn(r, plan, "check_all", None);
    relate(r, plan, RelationKind::Calls, "sample::run", "sample::validate");
    relate(r, plan, RelationKind::Calls, "sample::validate", "sample::check_all");
    relate(r, plan, RelationKind::Calls, "sample::check_all", "sample::store::Store::new");
    // Dependencies first: validate calls check_all.
    approve_both(r, plan, "sample::check_all");
    approve_both(r, plan, "sample::validate");
    append(r, "src/lib.rs", "\npub fn validate(input: &str) -> bool {\n    check_all(input)\n}\n\npub fn check_all(input: &str) -> bool {\n    crate::store::Store::new();\n    !input.is_empty()\n}\n");
    for symbol in ["sample::check_all", "sample::validate"] {
        let v = r.service.verify(&pid(plan), &id(symbol), Stage::Implemented).unwrap();
        assert_eq!(v.advanced_to, Some(Stage::Implemented), "{symbol}: {:?}", v.report);
    }
}

// req: P6.5
#[test]
fn breaking_one_hop_of_a_chain_reports_exactly_that_hop() {
    let r = repo();
    implemented_chain(&r, "ch");
    let plan = r.service.plan(&pid("ch")).unwrap();
    let hops: Vec<(String, String)> = arch_service::chain(&plan, &id("sample::validate")).iter().map(|h| (h.source.to_string(), h.target.to_string())).collect();
    assert_eq!(hops.len(), 3, "the whole chain, upstream and downstream: {hops:?}");
    // Break the far hop (check_all -> Store::new) in code, then verify the
    // component at the top: the chain is re-checked and only that hop fails.
    replace_in(&r, "src/lib.rs", "    crate::store::Store::new();\n    !input", "    !input");
    let v = r.service.verify(&pid("ch"), &id("sample::validate"), Stage::Implemented).unwrap();
    let errors: Vec<_> = v.report.findings.iter().filter(|f| f.severity == Severity::Error).collect();
    assert_eq!(errors.len(), 1, "{:?}", v.report);
    let broken = plan.relations.iter().find(|r| r.source == id("sample::check_all")).unwrap().id();
    assert_eq!(errors[0].code, FindingCode::MissingCallSite);
    assert_eq!(errors[0].target, arch_graph::Target::Relation { id: broken });
    assert_eq!(errors[0].at.as_ref().unwrap().file.as_str(), "src/lib.rs", "points at the caller's code");
    // The hop from existing code (run -> validate) isn't due yet: run isn't
    // being changed in this plan, so it's not reported.
    assert!(!v.report.findings.iter().any(|f| f.message.contains("sample::run")), "{:?}", v.report);
    // Break a second, different hop: each verification names the hops broken.
    replace_in(&r, "src/lib.rs", "    check_all(input)\n", "    !input.is_empty()\n");
    let v = r.service.verify(&pid("ch"), &id("sample::check_all"), Stage::Implemented).unwrap();
    let codes: Vec<_> = v.report.findings.iter().filter(|f| f.severity == Severity::Error).map(|f| (f.code, f.message.clone())).collect();
    assert_eq!(codes.len(), 2, "{codes:?}");
    assert!(codes.iter().any(|(_, m)| m.contains("sample::validate doesn't call rust:sample::check_all")));
    assert!(codes.iter().any(|(_, m)| m.contains("sample::check_all doesn't call rust:sample::store::Store::new")));
    // Restore both: clean again.
    replace_in(&r, "src/lib.rs", "pub fn validate(input: &str) -> bool {\n    !input.is_empty()\n}", "pub fn validate(input: &str) -> bool {\n    check_all(input)\n}");
    replace_in(&r, "src/lib.rs", "pub fn check_all(input: &str) -> bool {\n    !input", "pub fn check_all(input: &str) -> bool {\n    crate::store::Store::new();\n    !input");
    let v = r.service.verify(&pid("ch"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert!(!v.report.has_errors(), "{:?}", v.report);
}

/// Answers every tool run with a fixed outcome per tool, recording the calls.
#[derive(Default)]
struct FakeRunner {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    failing: Vec<String>,
}

impl arch_service::ToolRunner for FakeRunner {
    fn run(&self, _cwd: &Path, program: &str, args: &[String], _timeout: std::time::Duration) -> Result<arch_service::ToolOutput, String> {
        self.calls.lock().unwrap().push((program.to_owned(), args.to_vec()));
        let fails = self.failing.iter().any(|f| args.iter().any(|a| a == f));
        Ok(arch_service::ToolOutput { success: !fails, exit_code: Some(if fails { 1 } else { 0 }), output: if fails { "lint: bad\n".into() } else { String::new() } })
    }
}

/// Reviews by returning canned notes, recording what it was asked.
#[derive(Default)]
struct FakeReviewer {
    notes: Vec<arch_service::ReviewNote>,
    asked: Mutex<Vec<arch_service::ReviewRequest>>,
}

impl arch_service::ReviewProvider for FakeReviewer {
    fn review(&self, request: &arch_service::ReviewRequest) -> Result<Vec<arch_service::ReviewNote>, String> {
        self.asked.lock().unwrap().push(request.clone());
        Ok(self.notes.clone())
    }
}

fn reviewers(text: &str) -> arch_service::ReviewerCatalog {
    arch_service::ReviewerCatalog::from_toml(text).unwrap()
}

// req: P6.6
#[test]
fn only_a_clean_verification_moves_a_component_to_implemented() {
    let runner = Arc::new(FakeRunner { failing: vec!["--strict".into()], ..Default::default() });
    let tools = arch_service::ToolCatalog::from_toml("[tools.lint]\nexecutable = \"lint\"\ndefault_args = [\"{files}\"]\napplies_to = [\"function\"]\n").unwrap();
    let blocking = reviewers("[reviewers.tiny]\ncategory = \"anti_patterns\"\nkind = \"rule\"\nblocking = true\napplies_to = [\"function\"]\nrules = [{ rule = \"max_lines\", limit = 4 }]\n");
    let shared = runner.clone();
    let r = repo_with("", move |s| s.with_tools(tools, shared).with_reviewers(blocking, None));
    approved_validate(&r, "g");
    // Approval can't do it, for any actor.
    let err = r.service.approve(&pid("g"), &id("sample::validate"), Stage::Implemented, &user()).unwrap_err();
    assert!(matches!(&err, ServiceError::InvalidOp(m) if m.contains("verification")), "{err}");
    assert!(r.service.approve(&pid("g"), &id("sample::validate"), Stage::Skeleton, &user()).is_err());
    // Code that doesn't match: no.
    append(&r, "src/lib.rs", "\npub fn validate(input: &str) -> bool { !input.is_empty() }\n");
    let v = r.service.verify(&pid("g"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!((v.advanced_to, v.report.codes()), (None, vec![FindingCode::MissingCallSite]));
    assert!(runner.calls.lock().unwrap().is_empty(), "tools wait for the code checks to pass");
    // Matching code, but a blocking reviewer finds it too long: no.
    replace_in(&r, "src/lib.rs", "pub fn validate(input: &str) -> bool { !input.is_empty() }", "pub fn validate(input: &str) -> bool {\n    crate::store::Store::new();\n    let ok = !input.is_empty();\n    ok\n}");
    let v = r.service.verify(&pid("g"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!((v.advanced_to, v.report.codes()), (None, vec![FindingCode::ReviewFinding]), "{:?}", v.report);
    assert_eq!(stage_of(&r, "g", "sample::validate"), Stage::PlanApproved);
    // Short enough, but a tool fails: no.
    replace_in(&r, "src/lib.rs", "    let ok = !input.is_empty();\n    ok\n", "    !input.is_empty()\n");
    let failing = r.service.plan(&pid("g")).unwrap();
    let mut with_strict = failing.clone();
    with_strict.component_mut(&id("sample::validate")).unwrap().spec =
        Some(arch_graph::ImplementationSpec { validation: vec![arch_graph::ToolRef { tool: "lint".into(), targets: vec![], args: Some(vec!["--strict".into(), "{targets}".into()]) }], ..Default::default() });
    save_raw(&r, &with_strict);
    let v = r.service.verify(&pid("g"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!((v.advanced_to, v.report.codes()), (None, vec![FindingCode::ToolFailed]), "{:?}", v.report);
    assert_eq!(stage_of(&r, "g", "sample::validate"), Stage::PlanApproved);
    // Everything clean: now it moves, and only then.
    save_raw(&r, &failing);
    let v = r.service.verify(&pid("g"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, Some(Stage::Implemented), "{:?}", v.report);
    assert_eq!(stage_of(&r, "g", "sample::validate"), Stage::Implemented);
}

// req: P6.7
#[test]
fn validation_tools_expand_targets_apply_overrides_and_report() {
    let catalog = arch_service::ToolCatalog::from_toml(
        r#"
[tools.has]
description = "The target files mention the component."
executable = "sh"
default_args = ["-c", "grep -q \"fn ${0##*::}\" \"$@\"", "{symbols}", "{targets}"]

[tools.names]
executable = "sh"
default_args = ["-c", "test \"$1\" = 'sample::validate in src/lib.rs'", "names", "{symbols} in {files}"]
applies_to = ["function"]
"#,
    )
    .unwrap();
    assert!(arch_service::ToolCatalog::from_toml("[tools.x]\nexecutable = \"\"\n").is_err(), "a tool needs a program");
    let runner = Arc::new(Recording(Mutex::new(Vec::new())));
    let shared = runner.clone();
    let r = repo_with("", move |s| s.with_tools(catalog, shared));
    r.service.create_plan("t", "Tools", "").unwrap();
    let bind = |targets: Vec<&str>, args: Option<Vec<&str>>| arch_graph::ImplementationSpec {
        validation: vec![arch_graph::ToolRef { tool: "has".into(), targets: targets.into_iter().map(String::from).collect(), args: args.map(|a| a.into_iter().map(String::from).collect()) }],
        ..Default::default()
    };
    str_fn(&r, "t", "validate", Some(bind(vec!["src/lib.rs", "src/util.rs"], None)));
    append(&r, "src/lib.rs", "\npub fn validate(input: &str) -> bool { !input.is_empty() }\n");
    r.service.ingest(&r.root).unwrap();
    let report = r.service.run_tools(&pid("t"), Some(&id("sample::validate"))).unwrap();
    let ran: Vec<(String, Vec<String>)> = runner.0.lock().unwrap().clone();
    assert_eq!(ran[0], ("sh".to_owned(), ["-c", "grep -q \"fn ${0##*::}\" \"$@\"", "sample::validate", "src/lib.rs", "src/util.rs"].map(String::from).to_vec()), "{{targets}} expands to one arg per target");
    assert_eq!(ran[1].1[3], "sample::validate in src/lib.rs", "placeholders inside an arg join; type-scoped tools run unbound");
    assert_eq!(report.runs.len(), 2);
    assert!(report.runs.iter().all(|run| run.passed), "{report:?}");
    assert!(report.report.findings.is_empty());
    // Per-component override: the component's args replace the defaults.
    let mut plan = r.service.plan(&pid("t")).unwrap();
    plan.component_mut(&id("sample::validate")).unwrap().spec = Some(bind(vec![], Some(vec!["-c", "echo nope $0; exit 3", "{targets}"])));
    save_raw(&r, &plan);
    let report = r.service.run_tools(&pid("t"), None).unwrap();
    let failed: Vec<_> = report.runs.iter().filter(|run| !run.passed).collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].invocation.args, vec!["-c", "echo nope $0; exit 3", "src/lib.rs"], "no targets: the component's file");
    assert_eq!((failed[0].exit_code, failed[0].output.as_str()), (Some(3), "nope src/lib.rs"));
    assert_eq!(report.report.codes(), vec![FindingCode::ToolFailed]);
    assert!(report.report.findings[0].message.contains("has failed (exit 3)") && report.report.findings[0].message.contains("nope src/lib.rs"));
    // Binding a tool that doesn't exist is reported, not ignored.
    plan.component_mut(&id("sample::validate")).unwrap().spec =
        Some(arch_graph::ImplementationSpec { validation: vec![arch_graph::ToolRef { tool: "ghost".into(), targets: vec![], args: None }], ..Default::default() });
    save_raw(&r, &plan);
    let report = r.service.run_tools(&pid("t"), None).unwrap();
    assert_eq!(report.report.codes(), vec![FindingCode::UnknownTool]);
    assert!(report.report.findings[0].suggestion.as_ref().unwrap().contains("has, names"));
    // Components not in the code yet have nothing to run on.
    str_fn(&r, "t", "later_fn", None);
    assert!(r.service.run_tools(&pid("t"), Some(&id("sample::later_fn"))).is_err());
}

/// Runs tools for real, recording each call.
struct Recording(Mutex<Vec<(String, Vec<String>)>>);

impl arch_service::ToolRunner for Recording {
    fn run(&self, cwd: &Path, program: &str, args: &[String], timeout: std::time::Duration) -> Result<arch_service::ToolOutput, String> {
        self.0.lock().unwrap().push((program.to_owned(), args.to_vec()));
        arch_service::ProcessRunner.run(cwd, program, args, timeout)
    }
}

// req: P6.8
#[test]
fn changing_a_file_reverifies_only_the_components_it_affects() {
    let r = repo();
    r.service.create_plan("w", "Watch", "").unwrap();
    // validate (src/lib.rs) is planned to call Store::new (src/store.rs);
    // quiet (src/util.rs) calls nothing; pending isn't in the code yet.
    str_fn(&r, "w", "validate", None);
    relate(&r, "w", RelationKind::Calls, "sample::validate", "sample::store::Store::new");
    r.service
        .apply(
            &pid("w"),
            PlanOp::AddComponent {
                name: "quiet".into(),
                type_name: "function".into(),
                description: "Lowercases.".into(),
                parent: Some(id("sample::util")),
                file: None,
                signature: Some(Signature { params: vec![Param { name: "s".into(), ty: Some("&str".into()), optional: false }], returns: Some("String".into()), qualifiers: vec![] }),
                schemas: None,
                spec: None,
            },
        )
        .unwrap();
    str_fn(&r, "w", "pending", None);
    for symbol in ["sample::validate", "sample::util::quiet", "sample::pending"] {
        approve_both(&r, "w", symbol);
    }
    append(&r, "src/lib.rs", "\npub fn validate(input: &str) -> bool {\n    crate::store::Store::new();\n    !input.is_empty()\n}\n");
    append(&r, "src/util.rs", "\npub fn quiet(s: &str) -> String {\n    s.to_lowercase()\n}\n");
    for symbol in ["sample::validate", "sample::util::quiet"] {
        assert_eq!(r.service.verify(&pid("w"), &id(symbol), Stage::Implemented).unwrap().advanced_to, Some(Stage::Implemented));
    }
    let reverified = |paths: &[&str]| {
        let refresh = r.service.refresh(&paths.iter().map(|p| r.root.join(p)).collect::<Vec<_>>()).unwrap();
        let ids: Vec<String> = refresh.reverified.iter().map(|v| v.component.to_string()).collect();
        (ids, refresh)
    };
    // Its own file changes: only quiet.
    append(&r, "src/util.rs", "\n// touched\n");
    let (ids, refresh) = reverified(&["src/util.rs"]);
    assert_eq!(ids, vec!["rust:sample::util::quiet"]);
    assert_eq!(refresh.ingest.changed.iter().map(|f| f.as_str()).collect::<Vec<_>>(), vec!["src/util.rs"]);
    assert!(!refresh.reverified[0].report.has_errors());
    // A file it's planned to call into changes: only validate.
    append(&r, "src/store.rs", "\n// touched\n");
    assert_eq!(reverified(&["src/store.rs"]).0, vec!["rust:sample::validate"]);
    // Saved without changes, or not source the walk reads: nothing.
    assert!(reverified(&["src/lib.rs"]).0.is_empty());
    write(&r, "notes.md", "hi");
    write(&r, ".gitignore", "src/scratch.rs\n");
    write(&r, "src/scratch.rs", "pub fn validate(input: &str) -> bool { true }\n");
    let (ids, refresh) = reverified(&["notes.md", "src/scratch.rs", ".nah/plans/w.json"]);
    assert!(ids.is_empty() && refresh.ingest.files == 0, "{refresh:?}");
    // A change that breaks it is caught: Store::new renamed.
    replace_in(&r, "src/store.rs", "pub fn new()", "pub fn create()");
    let (ids, refresh) = reverified(&["src/store.rs"]);
    assert_eq!(ids, vec!["rust:sample::validate"]);
    assert_eq!(refresh.reverified[0].report.codes(), vec![FindingCode::MissingCallSite]);
    assert_eq!(refresh.reverified[0].stage, Stage::Implemented);
    assert!(r.events.lock().unwrap().iter().any(|e| matches!(e, ArchEvent::Validated { component: Some(c), errors: 1, .. } if c == &id("sample::validate"))));
    // A deleted file: what lived there is re-verified (and missing).
    std::fs::remove_file(r.root.join("src/util.rs")).unwrap();
    let (ids, refresh) = reverified(&["src/util.rs"]);
    assert_eq!(ids, vec!["rust:sample::util::quiet"]);
    assert_eq!(refresh.reverified[0].report.codes(), vec![FindingCode::MissingSymbol]);
}

// req: P6.11
#[test]
fn reviewers_run_by_category_and_blocking_findings_block() {
    let provider = Arc::new(FakeReviewer { notes: vec![arch_service::ReviewNote { message: "step 2 is skipped".into(), blocking: true, line: Some(9) }], ..Default::default() });
    let mut catalog = arch_service::ReviewerCatalog::defaults().unwrap();
    catalog.extend_with(reviewers(
        r#"
[reviewers.logic]
category = "logic"
kind = "session"
applies_to = ["function"]
blocking = true
brief = "Check the logic steps against the code."

[reviewers.shape]
category = "anti_patterns"
kind = "rule"
applies_to = ["class"]
blocking = true
rules = [{ rule = "max_lines", limit = 1 }]
"#,
    ));
    assert!(arch_service::ReviewerCatalog::from_toml("[reviewers.x]\ncategory = \"c\"\nkind = \"session\"\n").is_err(), "session reviewers need a brief");
    let shared = provider.clone();
    let r = repo_with("", move |s| s.with_reviewers(catalog, Some(shared)));
    approved_validate(&r, "rv");
    // Calls log as well, which the plan doesn't mention.
    append(&r, "src/lib.rs", "\npub fn validate(input: &str) -> bool {\n    crate::store::Store::new();\n    log(input);\n    !input.is_empty()\n}\n");
    r.service.ingest(&r.root).unwrap();
    let reviews = r.service.review(&pid("rv"), &id("sample::validate")).unwrap();
    let ran: Vec<(&str, &str, bool)> = reviews.runs.iter().map(|run| (run.reviewer.as_str(), run.category.as_str(), run.ran)).collect();
    assert_eq!(ran, vec![("logic", "logic", true), ("scope", "faithfulness", true), ("size", "anti_patterns", true)], "class-only reviewers skip functions");
    let rule = reviews.report.findings.iter().find(|f| f.message.starts_with("[faithfulness/scope]")).unwrap();
    assert_eq!(rule.severity, Severity::Warning, "non-blocking rule reviewer");
    assert!(rule.message.contains("calls rust:sample::log"), "{}", rule.message);
    let session = reviews.report.findings.iter().find(|f| f.message.starts_with("[logic/logic]")).unwrap();
    assert_eq!((session.severity, session.at.as_ref().unwrap().span.start_line), (Severity::Error, 9));
    let asked = provider.asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!((asked.category.as_str(), asked.brief.as_str()), ("logic", "Check the logic steps against the code."));
    assert!(asked.contract.contains("sample::validate") && asked.code.as_deref().unwrap().contains("log(input)"));
    // The session reviewer's blocking point blocks Implemented.
    let v = r.service.verify(&pid("rv"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, None);
    assert!(v.report.errors_for(&id("sample::validate")).any(|f| f.code == FindingCode::ReviewFinding));
    // No provider: a blocking session reviewer can't be skipped silently.
    let mut catalog = arch_service::ReviewerCatalog::default();
    catalog.extend_with(reviewers("[reviewers.logic]\ncategory = \"logic\"\nkind = \"session\"\nblocking = true\nbrief = \"Check it.\"\n"));
    let r2 = repo_with("", move |s| s.with_reviewers(catalog, None));
    approved_validate(&r2, "rv");
    append(&r2, "src/lib.rs", "\npub fn validate(input: &str) -> bool {\n    crate::store::Store::new();\n    !input.is_empty()\n}\n");
    let v = r2.service.verify(&pid("rv"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!((v.advanced_to, v.report.codes()), (None, vec![FindingCode::ReviewerUnavailable]));
    // Non-blocking points don't block.
    let lenient = Arc::new(FakeReviewer { notes: vec![arch_service::ReviewNote { message: "name could be clearer".into(), blocking: false, line: None }], ..Default::default() });
    let mut catalog = arch_service::ReviewerCatalog::default();
    catalog.extend_with(reviewers("[reviewers.logic]\ncategory = \"logic\"\nkind = \"session\"\nblocking = true\nbrief = \"Check it.\"\n"));
    let r3 = repo_with("", move |s| s.with_reviewers(catalog, Some(lenient)));
    approved_validate(&r3, "rv");
    append(&r3, "src/lib.rs", "\npub fn validate(input: &str) -> bool {\n    crate::store::Store::new();\n    !input.is_empty()\n}\n");
    let v = r3.service.verify(&pid("rv"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, Some(Stage::Implemented), "{:?}", v.report);
    assert_eq!(v.report.findings.iter().map(|f| f.severity).collect::<Vec<_>>(), vec![Severity::Warning]);
}
