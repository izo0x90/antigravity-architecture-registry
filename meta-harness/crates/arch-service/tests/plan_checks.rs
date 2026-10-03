//! Plan checks on a temp copy of the Rust fixture: type rules, loops,
//! schema fit, planned calls against real signatures, spec rules,
//! completeness and lifecycle gates.

use std::path::Path;

use arch_graph::{
    Component, ComponentId, FindingCode, ImplementationSpec, Invariant, InvariantKind, LogicStep, Plan, PlanId, ProposedLocation, Provenance,
    Relation, RelationKind, RepoPath, Report, Schemas, SideEffect, Signature, Stage, Status, ToolRef, TypeSystem, ViewCatalog,
};
use arch_index::PackSet;
use arch_service::{Actor, ArchitectureService, FsIndex, FsPlans, PlanOp, PlanRepository};
use serde_json::{Value, json};

struct Repo {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    service: ArchitectureService,
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

fn repo_with_types(types: TypeSystem) -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../arch-index/tests/fixtures/rust/sample"), &root);
    let service = ArchitectureService::open(
        &root,
        PackSet::built_in().unwrap(),
        types,
        ViewCatalog::defaults().unwrap(),
        Box::new(FsPlans::new(&root).unwrap()),
        Box::new(FsIndex::new(&root).unwrap()),
    )
    .unwrap();
    service.ingest(&root).unwrap();
    Repo { _dir: dir, root, service }
}

fn repo() -> Repo {
    repo_with_types(TypeSystem::defaults().unwrap())
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

/// A complete planned component (description and signature set).
fn comp(symbol: &str, type_name: &str) -> Component {
    Component {
        id: id(symbol),
        name: symbol.rsplit("::").next().unwrap().into(),
        type_name: type_name.into(),
        description: format!("The {symbol}."),
        status: Status::New,
        stage: Stage::Declared,
        anchor: None,
        proposed: Some(ProposedLocation { language: "rust".into(), file: RepoPath::new("src/lib.rs").unwrap(), parent: Some(id("sample")), symbol: symbol.into() }),
        signature: if matches!(type_name, "function" | "operation") { Some(Signature::default()) } else { None },
        schemas: Schemas::default(),
        spec: None,
        placeholders: vec![],
    }
}

fn rel(kind: RelationKind, a: &str, b: &str) -> Relation {
    Relation::new(kind, id(a), id(b), Provenance::PLANNED)
}

fn schema(v: Value) -> Option<Value> {
    Some(arch_graph::Dsl::defaults().unwrap().compile(&v).unwrap())
}

/// Saves `plan` (greenfield unless it says otherwise) and validates it.
fn check(r: &Repo, mut plan: Plan) -> Report {
    plan.greenfield = true;
    FsPlans::new(&r.root).unwrap().save(&plan).unwrap();
    r.service.validate(&plan.id).unwrap()
}

fn messages(report: &Report, code: FindingCode) -> Vec<String> {
    report.findings.iter().filter(|f| f.code == code).map(|f| f.message.clone()).collect()
}

// req: P5.2
#[test]
fn type_rules_are_findings() {
    let r = repo();
    let mut plan = Plan::new("types", "Types");
    let mut record = comp("sample::Record", "class");
    record.signature = Some(Signature::default());
    record.spec = Some(ImplementationSpec::default());
    let mut port = comp("sample::Port", "interface");
    port.schemas.properties = schema(json!({"x": "int"}));
    let widget = comp("sample::Widget", "widget");
    let inner = comp("sample::Step::apply::Inner", "class");
    plan.components.extend([record, port, widget, inner]);
    plan.relations.push(rel(RelationKind::Contains, "sample", "sample::Record"));
    plan.relations.push(rel(RelationKind::Contains, "sample", "sample::Port"));
    plan.relations.push(rel(RelationKind::Contains, "sample::Step::apply", "sample::Step::apply::Inner"));
    plan.relations.push(rel(RelationKind::Calls, "sample::store::Store", "sample::log"));
    let report = check(&r, plan);
    assert_eq!(messages(&report, FindingCode::UnknownType), vec!["unknown type 'widget'"]);
    let fields = messages(&report, FindingCode::DisallowedField);
    assert!(fields.contains(&"a class can't have signature".to_owned()), "{fields:?}");
    assert!(fields.contains(&"a class can't have implementation spec".to_owned()), "{fields:?}");
    assert!(fields.contains(&"an interface can't have properties".to_owned()), "{fields:?}");
    let parents = messages(&report, FindingCode::DisallowedParent);
    assert_eq!(parents.len(), 1, "{parents:?}");
    assert!(parents[0].contains("a class can't sit in rust:sample::Step::apply (an operation); it may sit in: module, class, function"), "{parents:?}");
    let callers = messages(&report, FindingCode::NotExecutable);
    assert_eq!(callers, vec!["rust:sample::store::Store is a class, which can't call rust:sample::log"]);
    // A specialized type inherits its base's rules (is-a for parents too).
    let mut types = TypeSystem::defaults().unwrap();
    types.extend_from_toml("[types.service]\nextends = \"class\"\n[types.handler]\nextends = \"function\"\nallowed_parents = [\"class\"]").unwrap();
    let r = repo_with_types(types);
    let mut plan = Plan::new("special", "Specialized");
    let svc = comp("sample::Svc", "service");
    let mut handler = comp("sample::Svc::on", "handler");
    handler.proposed.as_mut().unwrap().parent = Some(id("sample::Svc"));
    let stray = comp("sample::stray", "handler");
    plan.components.extend([svc, handler, stray]);
    plan.relations.push(rel(RelationKind::Contains, "sample", "sample::Svc"));
    plan.relations.push(rel(RelationKind::Contains, "sample::Svc", "sample::Svc::on"));
    plan.relations.push(rel(RelationKind::Contains, "sample", "sample::stray"));
    let report = check(&r, plan);
    let parents = messages(&report, FindingCode::DisallowedParent);
    assert_eq!(parents.len(), 1, "a handler in a service (a class) is fine; at module level it isn't: {parents:?}");
    assert!(parents[0].starts_with("a handler can't sit in rust:sample (a module)"));
}

// req: P5.4
#[test]
fn containment_implements_and_schema_ref_cycles_are_reported_with_their_loop() {
    let r = repo();
    let mut plan = Plan::new("loops", "Loops");
    plan.components.extend([comp("sample::a", "module"), comp("sample::b", "module"), comp("sample::I", "interface"), comp("sample::J", "interface")]);
    let mut node = comp("sample::Node", "class");
    node.schemas.properties = schema(json!({"next": "Edge?"}));
    let mut edge = comp("sample::Edge", "class");
    edge.schemas.properties = schema(json!({"to": "Node"}));
    let mut orphan = comp("sample::Orphan", "class");
    orphan.schemas.properties = schema(json!({"ghost": "Ghost"}));
    plan.components.extend([node, edge, orphan]);
    for (kind, a, b) in [
        (RelationKind::Contains, "sample::a", "sample::b"),
        (RelationKind::Contains, "sample::b", "sample::a"),
        (RelationKind::Implements, "sample::I", "sample::J"),
        (RelationKind::Implements, "sample::J", "sample::I"),
    ] {
        plan.relations.push(rel(kind, a, b));
    }
    let report = check(&r, plan);
    let loops = messages(&report, FindingCode::Cycle);
    assert!(loops.contains(&"contains loop: rust:sample::a -> rust:sample::b -> rust:sample::a".to_owned()), "{loops:?}");
    assert!(loops.contains(&"implements loop: rust:sample::I -> rust:sample::J -> rust:sample::I".to_owned()), "{loops:?}");
    assert!(loops.contains(&"schema reference loop: Edge -> Node -> Edge".to_owned()), "{loops:?}");
    assert_eq!(loops.len(), 3, "each loop once: {loops:?}");
    assert_eq!(messages(&report, FindingCode::MissingSchemaRef), vec!["its properties reference Ghost, which isn't a data type here"]);
}

// req: P5.5
#[test]
fn schema_mismatches_are_reported_for_implements_calls_and_data_flow() {
    let r = repo();
    let mut plan = Plan::new("fit", "Fit");
    // Implements: the interface promises callers may pass a string id and a
    // list of tags; the implementation wants an integer id and integer tags.
    let mut put = comp("sample::Sink::put", "operation");
    put.schemas.inputs = schema(json!({"id": "str", "tags": "str[]"}));
    put.schemas.outputs = schema(json!("bool"));
    let mut file_put = comp("sample::FileSink::put", "function");
    file_put.schemas.inputs = schema(json!({"id": "int", "tags": "int[]"}));
    file_put.schemas.outputs = schema(json!("bool?"));
    // Calls: the caller passes no `id` and a meta map with the wrong nesting.
    let mut store = comp("sample::store_it", "function");
    store.schemas.inputs = schema(json!({"id": "int", "meta": {"size": "int"}}));
    let caller = comp("sample::caller", "function");
    let mut call = rel(RelationKind::Calls, "sample::caller", "sample::store_it");
    call.expects = Some(Schemas { inputs: schema(json!({"meta": {"size": "str"}})), ..Schemas::default() });
    // Data flow: the producer's `count` may be absent; the consumer needs it.
    let mut produce = comp("sample::produce", "function");
    produce.schemas.outputs = schema(json!({"count": "int?"}));
    let mut consume = comp("sample::consume", "function");
    consume.schemas.inputs = schema(json!({"n": "int"}));
    let mut flow = rel(RelationKind::DataFlow, "sample::produce", "sample::consume");
    flow.ports = Some(("count".into(), "n".into()));
    plan.components.extend([put, file_put, store, caller, produce, consume]);
    plan.relations.extend([rel(RelationKind::Implements, "sample::FileSink::put", "sample::Sink::put"), call, flow]);
    let report = check(&r, plan);
    let found = messages(&report, FindingCode::SchemaMismatch);
    let has = |needle: &str| found.iter().any(|m| m.contains(needle));
    assert!(has("doesn't fit rust:sample::Sink::put it implements: inputs: id: type string where integer is expected"), "{found:?}");
    assert!(has("inputs: tags[]: type string where integer is expected"), "array items: {found:?}");
    assert!(has("outputs: value: option 1 (null) is not accepted"), "anyOf: {found:?}");
    assert!(has("rust:sample::caller -> rust:sample::store_it: inputs: id: required field missing"), "{found:?}");
    assert!(has("inputs: meta.size: type string where integer is expected"), "nested: {found:?}");
    assert!(has("rust:sample::produce.count -> rust:sample::consume.n: value: option 1 (null) is not accepted"), "{found:?}");
    assert_eq!(found.len(), 6, "{found:?}");
}

// req: P5.6
#[test]
fn planned_calls_are_checked_against_the_callees_real_signature() {
    let r = repo();
    let mut plan = Plan::new("sig", "Signatures");
    // util::shout(s: &str) -> String exists in the code.
    let cases: [(&str, Value, Value); 5] = [
        ("ok", json!({"s": "str"}), json!("string")),
        ("renamed", json!({"text": "str"}), json!("string")),
        ("extra", json!({"s": "str", "loud": "bool"}), json!("string")),
        ("typed", json!({"s": "int"}), json!("string")),
        ("returns", json!({"s": "str"}), json!("int")),
    ];
    for (name, inputs, outputs) in &cases {
        plan.components.push(comp(&format!("sample::{name}"), "function"));
        plan.relations.push(rel(RelationKind::Contains, "sample", &format!("sample::{name}")));
        let mut call = rel(RelationKind::Calls, &format!("sample::{name}"), "sample::util::shout");
        call.expects = Some(Schemas { inputs: schema(inputs.clone()), outputs: schema(outputs.clone()), ..Schemas::default() });
        plan.relations.push(call);
    }
    let report = check(&r, plan);
    let about = |name: &str| -> Vec<String> {
        let prefix = format!("rust:sample::{name} -> rust:sample::util::shout: ");
        messages(&report, FindingCode::CallMismatch).into_iter().filter_map(|m| m.strip_prefix(&prefix).map(str::to_owned)).collect()
    };
    assert!(about("ok").is_empty(), "{:?}", about("ok"));
    assert_eq!(about("renamed"), vec!["parameter 's' isn't among the planned inputs", "planned input 'text' isn't a parameter of shout"]);
    assert_eq!(about("extra"), vec!["shout takes 1 argument(s); the call passes 2 (2 required)", "planned input 'loud' isn't a parameter of shout"]);
    assert_eq!(about("typed"), vec!["parameter 's': &str doesn't carry integer"]);
    assert_eq!(about("returns"), vec!["returns String: String doesn't carry integer"]);
}

// req: P5.8
#[test]
fn spec_rules_cover_step_order_invariant_inheritance_tools_and_side_effects() {
    let r = repo();
    let mut plan = Plan::new("spec", "Spec");
    let inv = |name: &str, kind| Invariant { name: name.into(), kind, description: String::new() };
    let step = |sequence, name: &str| LogicStep { sequence, name: name.into(), description: String::new(), algorithm: None, complexity: None, check: None };
    let mut port = comp("sample::Port::write", "operation");
    port.schemas.side_effects = vec![SideEffect { target: "db".into(), description: String::new() }];
    port.spec = None;
    // Operations don't take specs; the interface's invariants live on a function it extends.
    let mut base = comp("sample::base_write", "function");
    base.spec = Some(ImplementationSpec { invariants: vec![inv("durable", InvariantKind::PostCondition)], ..ImplementationSpec::default() });
    let mut write = comp("sample::write", "function");
    write.schemas.side_effects = vec![SideEffect { target: "db".into(), description: String::new() }, SideEffect { target: "fs".into(), description: String::new() }];
    write.spec = Some(ImplementationSpec {
        logic_steps: vec![step(1, "open"), step(1, "lock"), step(3, "flush")],
        invariants: vec![inv("durable", InvariantKind::StateInvariant), inv("fast", InvariantKind::Complexity)],
        validation: vec![ToolRef { tool: "nope".into(), targets: vec![], args: None }],
        ..ImplementationSpec::default()
    });
    let mut caller = comp("sample::save_all", "function");
    caller.spec = Some(ImplementationSpec { logic_steps: vec![step(1, "save"), step(2, "report")], ..ImplementationSpec::default() });
    let mut call = rel(RelationKind::Calls, "sample::save_all", "sample::write");
    call.expects = Some(Schemas { side_effects: vec![SideEffect { target: "db".into(), description: String::new() }], ..Schemas::default() });
    plan.components.extend([port, base, write, caller]);
    plan.relations.extend([
        rel(RelationKind::Implements, "sample::write", "sample::base_write"),
        rel(RelationKind::Implements, "sample::base_write", "sample::Port::write"),
        call,
    ]);
    let report = check(&r, plan);
    let order = messages(&report, FindingCode::LogicStepOrder);
    assert_eq!(order, vec!["logic step 1 appears 2 times", "logic steps must be numbered 1..2 without gaps; found [1, 3]"]);
    assert_eq!(messages(&report, FindingCode::InvariantMismatch), vec!["invariant 'durable' is a state_invariant here but a post_condition in rust:sample::base_write"]);
    assert_eq!(messages(&report, FindingCode::UnknownTool), vec!["validation tool 'nope' isn't defined"]);
    let effects = messages(&report, FindingCode::UndeclaredSideEffect);
    assert!(effects.contains(&"changes 'fs', which the interface it implements doesn't declare".to_owned()), "{effects:?}");
    assert!(effects.contains(&"rust:sample::write also changes 'fs', which rust:sample::save_all doesn't expect".to_owned()), "{effects:?}");
    assert_eq!(effects.len(), 2, "db is declared everywhere: {effects:?}");
    assert!(report.findings.iter().all(|f| f.component() != Some(&id("sample::save_all")) || f.code == FindingCode::MissingTouchPoint), "steps 1..2 are fine");
}

fn add(r: &Repo, plan: &str, name: &str, type_name: &str, description: &str, signature: Option<Signature>) {
    r.service
        .apply(
            &pid(plan),
            PlanOp::AddComponent {
                name: name.into(),
                type_name: type_name.into(),
                description: description.into(),
                parent: Some(id("sample")),
                file: None,
                signature,
                schemas: None,
                spec: None,
            },
        )
        .unwrap();
}

fn relate(r: &Repo, plan: &str, a: &str, b: &str) {
    r.service.apply(&pid(plan), PlanOp::Relate { kind: RelationKind::Calls, source: id(a), target: id(b), order: None, ports: None, note: None, expects: None }).unwrap();
}

// req: P5.15
#[test]
fn missing_required_details_block_approval_with_a_precise_list() {
    let mut types = TypeSystem::defaults().unwrap();
    types.extend_from_toml("[types.handler]\nextends = \"function\"\nrequired_fields = [\"logic_steps\", \"invariants\"]").unwrap();
    let r = repo_with_types(types);
    r.service.create_plan("gate", "Gate", "").unwrap();
    add(&r, "gate", "bare", "function", "", None);
    add(&r, "gate", "on_event", "handler", "Handles events.", Some(Signature::default()));
    relate(&r, "gate", "sample::run", "sample::bare");
    relate(&r, "gate", "sample::run", "sample::on_event");
    let report = r.service.validate(&pid("gate")).unwrap();
    let incomplete = messages(&report, FindingCode::SpecIncomplete);
    assert!(incomplete.contains(&"a function needs description, signature before approval".to_owned()), "{incomplete:?}");
    assert!(incomplete.contains(&"a handler needs logic_steps, invariants before approval".to_owned()), "inherited and own: {incomplete:?}");
    assert!(report.findings.iter().filter(|f| f.code == FindingCode::SpecIncomplete).all(|f| f.severity == arch_graph::Severity::Warning), "a warning while declared");
    let err = r.service.approve(&pid("gate"), &id("sample::bare"), Stage::ArchApproved, &user()).unwrap_err().to_string();
    assert!(err.contains("a function needs description, signature before approval"), "{err}");
    // Filling in exactly what was listed unblocks it.
    r.service
        .apply(&pid("gate"), PlanOp::Remove { id: id("sample::bare") })
        .unwrap();
    add(&r, "gate", "bare", "function", "Now described.", Some(Signature::default()));
    relate(&r, "gate", "sample::run", "sample::bare");
    r.service.approve(&pid("gate"), &id("sample::bare"), Stage::ArchApproved, &user()).unwrap();
}

// req: P5.9
#[test]
fn stage_transitions_enforce_validation_location_and_dependencies() {
    let r = repo();
    r.service.create_plan("life", "Lifecycle", "").unwrap();
    let sig = || Some(Signature { params: vec![], returns: Some("bool".into()) });
    add(&r, "life", "outer", "function", "Calls inner.", sig());
    add(&r, "life", "inner", "function", "Does the work.", sig());
    relate(&r, "life", "sample::run", "sample::outer");
    relate(&r, "life", "sample::outer", "sample::inner");
    let approve = |symbol: &str, to: Stage| r.service.approve(&pid("life"), &id(symbol), to, &user());
    // Dependencies: designing outer in detail needs inner's architecture approved.
    approve("sample::outer", Stage::ArchApproved).unwrap();
    let err = approve("sample::outer", Stage::PlanApproved).unwrap_err().to_string();
    assert!(err.contains("PlanApproved needs rust:sample::inner at ArchApproved or later; it is Declared"), "{err}");
    approve("sample::inner", Stage::ArchApproved).unwrap();
    approve("sample::outer", Stage::PlanApproved).unwrap();
    approve("sample::inner", Stage::PlanApproved).unwrap();
    // Implementing outer needs inner's structure in the code first.
    std::fs::write(r.root.join("src/lib.rs"), std::fs::read_to_string(r.root.join("src/lib.rs")).unwrap() + "\npub fn outer() -> bool {\n    inner()\n}\n\npub fn inner() -> bool {\n    todo!()\n}\n").unwrap();
    let v = r.service.verify(&pid("life"), &id("sample::outer"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, None);
    assert_eq!(messages(&v.report, FindingCode::DependencyNotReady), vec!["Implemented needs rust:sample::inner at Skeleton or later; it is PlanApproved"]);
    assert_eq!(r.service.verify(&pid("life"), &id("sample::inner"), Stage::Skeleton).unwrap().advanced_to, Some(Stage::Skeleton));
    assert_eq!(r.service.verify(&pid("life"), &id("sample::outer"), Stage::Implemented).unwrap().advanced_to, Some(Stage::Implemented));
    // Validation: a component with errors can't be approved.
    let mut plan = r.service.plan(&pid("life")).unwrap();
    let mut bad = comp("sample::Bad", "class");
    bad.signature = Some(Signature::default());
    plan.components.push(bad);
    // Location: past the design, a component needs a place in the code.
    let mut nowhere = comp("sample::nowhere", "function");
    nowhere.proposed = None;
    plan.components.push(nowhere);
    plan.relations.push(rel(RelationKind::Contains, "sample", "sample::Bad"));
    FsPlans::new(&r.root).unwrap().save(&plan).unwrap();
    let err = approve("sample::Bad", Stage::ArchApproved).unwrap_err().to_string();
    assert!(err.contains("a class can't have signature"), "{err}");
    approve("sample::nowhere", Stage::ArchApproved).unwrap();
    let err = approve("sample::nowhere", Stage::PlanApproved).unwrap_err().to_string();
    assert!(err.contains("needs a proposed location"), "{err}");
}

fn stage(r: &Repo, plan: &str, symbol: &str) -> Stage {
    r.service.plan(&pid(plan)).unwrap().component(&id(symbol)).unwrap().stage
}

// req: P5.10
#[test]
fn actionable_work_comes_in_dependency_order_and_changes_cascade_in_waves() {
    let r = repo();
    r.service.create_plan("q", "Queue", "").unwrap();
    let sig = || Some(Signature { params: vec![], returns: Some("bool".into()) });
    // Added in reverse: c calls b, b calls a; d builds on existing util::shout.
    for name in ["c", "b", "a", "d"] {
        add(&r, "q", name, "function", "Part of the queue.", sig());
    }
    relate(&r, "q", "sample::run", "sample::c");
    relate(&r, "q", "sample::c", "sample::b");
    relate(&r, "q", "sample::b", "sample::a");
    relate(&r, "q", "sample::d", "sample::util::shout");
    let next = |action| {
        let plan = r.service.plan(&pid("q")).unwrap();
        let graph = r.service.graph(Some(&pid("q"))).unwrap();
        arch_service::next_actionable(&plan, &graph, r.service.types(), action).into_iter().map(|i| i.0).collect::<Vec<_>>()
    };
    assert_eq!(
        arch_service::topological(&r.service.plan(&pid("q")).unwrap(), &r.service.graph(Some(&pid("q"))).unwrap(), r.service.types()),
        vec![id("sample::a"), id("sample::d"), id("sample::b"), id("sample::c")],
        "dependencies first, ties by id"
    );
    for name in ["a", "b", "c", "d"] {
        r.service.approve(&pid("q"), &id(&format!("sample::{name}")), Stage::ArchApproved, &user()).unwrap();
    }
    assert_eq!(next(arch_service::Action::Plan), vec!["rust:sample::a", "rust:sample::d", "rust:sample::b", "rust:sample::c"]);
    for name in ["a", "b", "c", "d"] {
        r.service.approve(&pid("q"), &id(&format!("sample::{name}")), Stage::PlanApproved, &user()).unwrap();
    }
    assert_eq!(next(arch_service::Action::Build), vec!["rust:sample::a", "rust:sample::d"], "b and c wait for the structure they call");
    // Changing a's design sends it back for approval and its dependents back a step, in waves.
    let plan = r.service.plan(&pid("q")).unwrap();
    let graph = r.service.graph(Some(&pid("q"))).unwrap();
    let waves: Vec<(String, Stage, Stage, u32)> =
        arch_service::cascade(&mut plan.clone(), &graph, r.service.types(), &id("sample::a")).into_iter().map(|d| (d.id.0, d.from, d.to, d.wave)).collect();
    assert_eq!(
        waves,
        vec![
            ("rust:sample::a".into(), Stage::PlanApproved, Stage::Declared, 0),
            ("rust:sample::b".into(), Stage::PlanApproved, Stage::ArchApproved, 1),
            ("rust:sample::c".into(), Stage::PlanApproved, Stage::ArchApproved, 2),
        ]
    );
    // The same happens through a plan edit.
    r.service.apply(&pid("q"), PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::a"), target: id("sample::log"), order: None, ports: None, note: None, expects: None }).unwrap();
    assert_eq!((stage(&r, "q", "sample::a"), stage(&r, "q", "sample::b"), stage(&r, "q", "sample::c")), (Stage::Declared, Stage::ArchApproved, Stage::ArchApproved));
    assert_eq!(stage(&r, "q", "sample::d"), Stage::PlanApproved, "unrelated work keeps its approval");
    // Existing code changing shape cascades too: shout gains a parameter.
    let util = r.root.join("src/util.rs");
    std::fs::write(&util, std::fs::read_to_string(&util).unwrap().replace("pub fn shout(s: &str) -> String", "pub fn shout(s: &str, loud: bool) -> String")).unwrap();
    let refresh = r.service.refresh(std::slice::from_ref(&util)).unwrap();
    let sent_back: Vec<(String, Stage)> = refresh.downgraded.iter().map(|(_, d)| (d.id.0.clone(), d.to)).collect();
    assert_eq!(sent_back, vec![("rust:sample::d".to_owned(), Stage::ArchApproved)]);
    assert_eq!(stage(&r, "q", "sample::d"), Stage::ArchApproved);
}

// req: P5.11
#[test]
fn contracts_carry_location_signature_interfaces_invariants_steps_and_real_callees() {
    let r = repo();
    let mut plan = Plan::new("contract", "Contract");
    let inv = |name: &str, kind| Invariant { name: name.into(), kind, description: format!("{name} holds") };
    let mut port = comp("sample::Port", "interface");
    port.spec = None;
    let mut base = comp("sample::base_write", "function");
    base.spec = Some(ImplementationSpec { invariants: vec![inv("durable", InvariantKind::PostCondition)], ..ImplementationSpec::default() });
    let mut write = comp("sample::write", "function");
    write.signature = Some(Signature { params: vec![arch_graph::Param { name: "item".into(), ty: Some("&str".into()) }], returns: Some("bool".into()) });
    write.schemas.inputs = schema(json!({"item": "str"}));
    write.schemas.side_effects = vec![SideEffect { target: "fs".into(), description: "writes the file".into() }];
    write.spec = Some(ImplementationSpec {
        pattern: Some("write-then-rename".into()),
        logic_steps: vec![LogicStep { sequence: 1, name: "shout".into(), description: "Uppercase the item.".into(), algorithm: None, complexity: None, check: None }],
        invariants: vec![inv("atomic", InvariantKind::PostCondition)],
        tasks: vec![arch_graph::Task { text: "fsync".into(), done: false }],
        ..ImplementationSpec::default()
    });
    let mut call = rel(RelationKind::Calls, "sample::write", "sample::util::shout");
    call.expects = Some(Schemas { inputs: schema(json!({"s": "str"})), ..Schemas::default() });
    plan.components.extend([port, base, write]);
    plan.relations.extend([rel(RelationKind::Implements, "sample::write", "sample::base_write"), rel(RelationKind::Implements, "sample::base_write", "sample::Port"), call]);
    FsPlans::new(&r.root).unwrap().save(&plan).unwrap();
    let slice = r.service.contract(&pid("contract"), &id("sample::write")).unwrap();
    assert_eq!(slice.interface_chain.iter().map(|n| n.id.0.as_str()).collect::<Vec<_>>(), vec!["rust:sample::base_write", "rust:sample::Port"]);
    assert_eq!(slice.invariants.iter().map(|h| (h.from.0.as_str(), h.invariant.name.as_str())).collect::<Vec<_>>(), vec![("rust:sample::write", "atomic"), ("rust:sample::base_write", "durable")]);
    let shout = &slice.calls[0];
    assert!(shout.anchor.is_some() && shout.signature.is_some(), "real callee from the code");
    let md = slice.to_markdown();
    // JSON key order depends on serde_json features; render the same way.
    let inputs = slice.schemas.inputs.as_ref().unwrap().to_string();
    let passes = shout.expects.as_ref().unwrap().inputs.as_ref().unwrap().to_string();
    let data = format!("## Data\n- Inputs: `{inputs}`\n- Changes `fs`: writes the file");
    let passed = format!("  - passes `{passes}`");
    for needle in [
        "## Location\nPut it in src/lib.rs as `sample::write`, inside rust:sample.",
        "## Signature\n`(item: &str) -> bool`",
        data.as_str(),
        "## Pattern\nwrite-then-rename",
        "## Logic steps\n1. **shout**: Uppercase the item.",
        "## Tasks\n- [ ] fsync",
        "## Invariants\n- **atomic** (post_condition): atomic holds\n- **durable** (post_condition) (from rust:sample::base_write): durable holds",
        "## Must call (in order)\n- `shout` function (s: &str) -> String — src/util.rs:1",
        passed.as_str(),
        "## Implements\n- `base_write` function",
        "## Through those, also implements\n- `Port` interface",
    ] {
        assert!(md.contains(needle), "missing {needle:?} in:\n{md}");
    }
}

// req: P5.18
#[test]
fn logic_step_checks_run_during_validation_and_record_pass_or_fail() {
    let r = repo();
    let mut plan = Plan::new("logic", "Logic");
    let step = |sequence, name: &str, check: Option<&str>| LogicStep {
        sequence,
        name: name.into(),
        description: String::new(),
        algorithm: None,
        complexity: None,
        check: check.map(str::to_owned),
    };
    let mut parse = comp("sample::parse", "function");
    parse.spec = Some(ImplementationSpec {
        logic_steps: vec![step(1, "read", Some("test -f src/lib.rs")), step(2, "split", Some("echo 'split is broken'; exit 3")), step(3, "free", None)],
        ..ImplementationSpec::default()
    });
    plan.components.push(parse);
    let report = check(&r, plan);
    assert_eq!(messages(&report, FindingCode::LogicCheckPassed), vec!["logic step 1 'read' check passed"]);
    let failed = messages(&report, FindingCode::LogicCheckFailed);
    assert_eq!(failed, vec!["logic step 2 'split' check failed: split is broken"]);
    let severity = |code| report.findings.iter().find(|f| f.code == code).unwrap().severity;
    assert_eq!((severity(FindingCode::LogicCheckPassed), severity(FindingCode::LogicCheckFailed)), (arch_graph::Severity::Info, arch_graph::Severity::Error));
    // The shell is configurable (tools.toml `[logic_checks]`).
    let catalog = arch_service::ToolCatalog::from_toml("[logic_checks]\nshell = [\"bash\", \"-c\"]\ntimeout_secs = 5").unwrap();
    assert_eq!(catalog.logic_checks.shell, vec!["bash", "-c"]);
    assert!(arch_service::ToolCatalog::from_toml("[logic_checks]\nshell = []\ntimeout_secs = 5").is_err());
}
