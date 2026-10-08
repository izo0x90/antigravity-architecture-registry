//! Plan checks on a temp copy of the Rust fixture: type rules, loops,
//! schema fit, planned calls against real signatures, spec rules,
//! completeness and lifecycle gates.

use std::path::Path;

use arch_graph::{
    Component, ComponentId, FindingCode, ImplementationSpec, Invariant, InvariantKind, LogicStep, Plan, PlanId, ProposedLocation, Provenance,
    Relation, RelationKind, RepoPath, Report, Schemas, SideEffect, Signature, Stage, Status, ToolRef, TypeSystem,
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
    repo_with(types, "")
}

/// The fixture repo with project types and project code rules (TOML).
fn repo_with(types: TypeSystem, rules_toml: &str) -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../arch-index/tests/fixtures/rust/sample"), &root);
    let packs = PackSet::built_in().unwrap();
    let mut rules = arch_index::CodeRules::defaults(&packs, &types).unwrap();
    rules.add_toml(rules_toml, &packs, &types).unwrap();
    let service = ArchitectureService::open(
        &root,
        packs,
        types,
        Box::new(FsPlans::new(&root).unwrap()),
        Box::new(FsIndex::new(&root).unwrap()),
    )
    .unwrap()
    .with_rules(rules);
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
    assert!(
        report.findings.iter().filter(|f| f.code == FindingCode::UndeclaredSideEffect).all(|f| f.severity == arch_graph::Severity::Error),
        "undeclared side effects are errors, at the interface and at the call site"
    );
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
    let sig = || Some(Signature { params: vec![], returns: Some("bool".into()), qualifiers: vec![] });
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
    let sig = || Some(Signature { params: vec![], returns: Some("bool".into()), qualifiers: vec![] });
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
        let graph = r.service.graph(&arch_graph::Source::Plan(pid("q"))).unwrap();
        arch_service::next_actionable(&plan, &graph, r.service.types(), action).into_iter().map(|i| i.0).collect::<Vec<_>>()
    };
    assert_eq!(
        arch_service::topological(&r.service.plan(&pid("q")).unwrap(), &r.service.graph(&arch_graph::Source::Plan(pid("q"))).unwrap(), r.service.types()),
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
    let graph = r.service.graph(&arch_graph::Source::Plan(pid("q"))).unwrap();
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
    write.signature = Some(Signature { params: vec![arch_graph::Param { name: "item".into(), ty: Some("&str".into()), optional: false, variadic: false }], returns: Some("bool".into()), qualifiers: vec![] });
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

// req: P6.4
#[test]
fn planned_data_flows_are_confirmed_missing_or_explicitly_unverifiable() {
    let r = repo();
    let mut plan = Plan::new("flows", "Flows");
    for name in ["produce", "opaque", "consume", "sink", "pipeline"] {
        plan.components.push(comp(&format!("sample::{name}"), "function"));
        plan.relations.push(rel(RelationKind::Contains, "sample", &format!("sample::{name}")));
    }
    let flow = |a: &str, b: &str, to: &str| {
        let mut f = rel(RelationKind::DataFlow, a, b);
        f.ports = Some(("return".into(), to.into()));
        f
    };
    plan.relations.push(flow("sample::produce", "sample::consume", "text"));
    plan.relations.push(flow("sample::opaque", "sample::sink", "x"));
    plan.relations.push(flow("sample::produce", "sample::sink", "x"));
    FsPlans::new(&r.root).unwrap().save(&plan).unwrap();
    let lib = r.root.join("src/lib.rs");
    std::fs::write(
        &lib,
        std::fs::read_to_string(&lib).unwrap()
            + "\npub fn produce() -> String {\n    String::new()\n}\n\npub fn opaque() {\n}\n\npub fn consume(text: &str) {\n    let _ = text;\n}\n\npub fn sink(x: &str) {\n    let _ = x;\n}\n\npub fn pipeline() {\n    let s = produce();\n    consume(&s);\n    let v = opaque();\n    sink(v);\n}\n",
    )
    .unwrap();
    let flows = |symbol: &str| -> Vec<(FindingCode, String)> {
        let v = r.service.verify(&pid("flows"), &id(symbol), Stage::Implemented).unwrap();
        v.report.findings.into_iter().filter(|f| matches!(f.code, FindingCode::DataFlowMissing | FindingCode::DataFlowUnverifiable)).map(|f| (f.code, f.message)).collect()
    };
    assert!(flows("sample::consume").is_empty(), "produce -> consume is in the code: confirmed");
    let sink = flows("sample::sink");
    assert_eq!(
        sink,
        vec![
            (FindingCode::DataFlowMissing, "the code doesn't pass rust:sample::produce's output into rust:sample::sink (return -> x)".to_owned()),
            (FindingCode::DataFlowUnverifiable, "rust:sample::opaque -> rust:sample::sink (return -> x) is handed over but can't be confirmed: sample::opaque declares no return type".to_owned()),
        ]
    );
    // Seen from the source's side, the same three outcomes.
    assert_eq!(flows("sample::produce").len(), 1, "produce -> sink missing; produce -> consume confirmed");
}

// req: D.2, D.5, M.4, M.6
#[test]
fn the_data_flow_view_shows_data_with_its_steps_and_marks_planned_flows() {
    use arch_graph::tree::{LineKind, Mark, TreeRequest, ViewKind, Way};
    let r = repo();
    let tree = |request: TreeRequest| r.service.tree(&request).unwrap();
    // Data is the cards: where a Store comes from is Store::new, which takes nothing.
    let store = tree(TreeRequest { view: ViewKind::DataFlow, root: Some("type:Store".into()), way: Way::Up, depth: Some(1), ..TreeRequest::default() });
    assert_eq!(store.cards[0].kind, "data");
    assert!(store.card(&format!("start:{}", id("sample::store::Store::new"))).is_some(), "{:#?}", store.cards);
    // Planned flows beside the code's: one the code makes, one it doesn't.
    let mut plan = Plan::new("flows", "Flows");
    for name in ["produce", "consume", "sink"] {
        let mut c = comp(&format!("sample::{name}"), "function");
        if name == "produce" {
            // Its planned shape says what flows out of it.
            c.stage = Stage::Skeleton;
            c.signature = Some(Signature { params: vec![], returns: Some("String".into()), qualifiers: vec![] });
        }
        plan.components.push(c);
        plan.relations.push(rel(RelationKind::Contains, "sample", &format!("sample::{name}")));
    }
    let planned = |a: &str, b: &str, to: &str| {
        let mut f = rel(RelationKind::DataFlow, a, b);
        f.ports = Some(("return".into(), to.into()));
        f
    };
    plan.relations.push(planned("sample::produce", "sample::consume", "text"));
    plan.relations.push(planned("sample::produce", "sample::sink", "x"));
    FsPlans::new(&r.root).unwrap().save(&plan).unwrap();
    append_lib(&r, "\npub fn produce() -> String {\n    String::new()\n}\n\npub fn consume(text: &str) {\n    let _ = text;\n}\n\npub fn sink(x: &str) {\n    let _ = x;\n}\n\npub fn pipeline() {\n    let s = produce();\n    consume(&s);\n}\n");
    r.service.ingest(&r.root).unwrap();
    let flows = tree(TreeRequest { view: ViewKind::DataFlow, source: arch_graph::Source::Plan(pid("flows")), depth: Some(1), ..TreeRequest::default() });
    assert_eq!(flows.roots, vec!["type:String".to_owned()], "the plan's data: what produce returns");
    let to = |f: &str| flows.lines.iter().find(|l| l.kind == LineKind::Tree && l.to == format!("end:{}", id(f))).map(|l| l.mark);
    assert_eq!(to("sample::consume"), Some(Mark::Done), "planned and in the code");
    assert_eq!(to("sample::sink"), Some(Mark::Mismatch), "planned, not in the code");
}

fn lib_rs(r: &Repo) -> std::path::PathBuf {
    r.root.join("src/lib.rs")
}

fn append_lib(r: &Repo, code: &str) {
    let text = std::fs::read_to_string(lib_rs(r)).unwrap();
    std::fs::write(lib_rs(r), text + code).unwrap();
}

fn str_sig() -> Option<Signature> {
    Some(Signature { params: vec![arch_graph::Param { name: "input".into(), ty: Some("&str".into()), optional: false, variadic: false }], returns: Some("bool".into()), qualifiers: vec![] })
}

// req: P5.9
#[test]
fn approval_is_blocked_by_errors_on_its_relations_and_on_the_plan() {
    let r = repo();
    r.service.create_plan("rel", "Relations", "").unwrap();
    add(&r, "rel", "shouter", "function", "Calls shout.", Some(Signature::default()));
    // The planned call doesn't fit shout's real signature: an error on the relation, not the component.
    r.service
        .apply(
            &pid("rel"),
            PlanOp::Relate {
                kind: RelationKind::Calls,
                source: id("sample::shouter"),
                target: id("sample::util::shout"),
                order: None,
                ports: None,
                note: None,
                expects: Some(Schemas { inputs: Some(json!({"text": "str"})), ..Schemas::default() }),
            },
        )
        .unwrap();
    let err = r.service.approve(&pid("rel"), &id("sample::shouter"), Stage::ArchApproved, &user()).unwrap_err().to_string();
    assert!(err.contains("parameter 's' isn't among the planned inputs"), "{err}");
    // A plan-wide error (no touch point with existing code) blocks every approval in it.
    r.service.create_plan("alone", "Alone", "").unwrap();
    add(&r, "alone", "lonely", "function", "Talks to no one.", Some(Signature::default()));
    let err = r.service.approve(&pid("alone"), &id("sample::lonely"), Stage::ArchApproved, &user()).unwrap_err().to_string();
    assert!(err.contains("connect to existing code"), "{err}");
}

// req: P5.5
#[test]
fn callees_inherit_interface_schemas_calls_must_pass_required_inputs_and_ports_must_exist() {
    let r = repo();
    let mut plan = Plan::new("inherit", "Inherit");
    let mut put = comp("sample::Sink::put", "operation");
    put.schemas.inputs = schema(json!({"id": "int"}));
    put.schemas.outputs = schema(json!("bool"));
    // The implementation states no schemas of its own: the interface's apply.
    let file_put = comp("sample::file_put", "function");
    let caller = comp("sample::caller", "function");
    let mut wrong = rel(RelationKind::Calls, "sample::caller", "sample::file_put");
    wrong.expects = Some(Schemas { inputs: schema(json!({"id": "str"})), ..Schemas::default() });
    let silent = comp("sample::silent", "function");
    let mut nothing = rel(RelationKind::Calls, "sample::silent", "sample::file_put");
    nothing.expects = Some(Schemas { outputs: schema(json!("bool")), ..Schemas::default() });
    let mut produce = comp("sample::produce", "function");
    produce.schemas.outputs = schema(json!({"count": "int"}));
    let mut consume = comp("sample::consume", "function");
    consume.schemas.inputs = schema(json!({"n": "int"}));
    let mut flow = rel(RelationKind::DataFlow, "sample::produce", "sample::consume");
    flow.ports = Some(("total".into(), "n".into()));
    plan.components.extend([put, file_put, caller, silent, produce, consume]);
    plan.relations.extend([rel(RelationKind::Implements, "sample::file_put", "sample::Sink::put"), wrong, nothing, flow]);
    let report = check(&r, plan);
    let found = messages(&report, FindingCode::SchemaMismatch);
    let has = |needle: &str| found.iter().any(|m| m.contains(needle));
    assert!(has("rust:sample::caller -> rust:sample::file_put: inputs: id: type string where integer is expected"), "{found:?}");
    assert!(has("rust:sample::silent -> rust:sample::file_put: passes no inputs, but file_put requires some"), "{found:?}");
    assert!(has("port 'total' isn't a field of rust:sample::produce's outputs (fields: count)"), "{found:?}");
    assert_eq!(found.len(), 3, "{found:?}");
}

// req: P5.2
#[test]
fn side_effects_belong_to_types_that_run() {
    let r = repo();
    let mut plan = Plan::new("effects", "Effects");
    let mut record = comp("sample::Record", "class");
    record.schemas.side_effects = vec![SideEffect { target: "db".into(), description: String::new() }];
    plan.components.push(record);
    plan.relations.push(rel(RelationKind::Contains, "sample", "sample::Record"));
    let report = check(&r, plan);
    assert_eq!(messages(&report, FindingCode::DisallowedField), vec!["a class can't have side effects"]);
}

#[test]
fn references_to_a_name_two_data_types_share_are_ambiguous() {
    let r = repo();
    let mut plan = Plan::new("amb", "Ambiguous");
    let mut a = comp("sample::store::Config", "class");
    a.schemas.properties = schema(json!({"path": "str"}));
    let mut b = comp("sample::util::Config", "class");
    b.schemas.properties = schema(json!({"level": "int"}));
    let mut load = comp("sample::load", "function");
    load.schemas.outputs = schema(json!("Config"));
    plan.components.extend([a, b, load]);
    let report = check(&r, plan);
    assert_eq!(messages(&report, FindingCode::AmbiguousSchemaRef), vec!["Config could be any of rust:sample::store::Config, rust:sample::util::Config"]);
}

// req: P5.17
#[test]
fn contracts_list_the_open_placeholders_in_the_code() {
    let r = repo();
    r.service.create_plan("later", "Later", "").unwrap();
    // `later` exists in the code with a todo!() body.
    r.service.apply(&pid("later"), PlanOp::Modify { id: id("sample::later"), description: Some("Finish it.".into()), signature: None, schemas: None, spec: None }).unwrap();
    let slice = r.service.contract(&pid("later"), &id("sample::later")).unwrap();
    assert_eq!(slice.open_placeholders.len(), 1, "{slice:?}");
    let line = slice.open_placeholders[0].start_line;
    assert!(slice.to_markdown().contains(&format!("## Open placeholders\nLines {line} still have placeholder bodies.")));
}

// req: P5.7
#[test]
fn proposed_files_outside_the_repo_are_rejected() {
    let r = repo();
    r.service.create_plan("out", "Out", "").unwrap();
    for file in ["../outside.rs", "/etc/evil.rs", "src/../../x.rs"] {
        let err = r
            .service
            .apply(
                &pid("out"),
                PlanOp::AddComponent { name: "x".into(), type_name: "function".into(), description: String::new(), parent: Some(id("sample")), file: Some(file.into()), signature: None, schemas: None, spec: None },
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("inside the repo"), "{file}: {err}");
    }
    // A plan file edited by hand can't sneak one in either.
    let raw = r#"{"schema_version":1,"id":"hand","title":"Hand","components":[{"id":"rust:x","name":"x","type":"function","status":"new","stage":"declared","proposed":{"language":"rust","file":"../x.rs","symbol":"x"}}]}"#;
    std::fs::write(r.root.join(".nah/plans/hand.json"), raw).unwrap();
    assert!(matches!(r.service.plan(&pid("hand")), Err(arch_service::ServiceError::Corrupt(_))));
}

// req: P6.6
#[test]
fn implementing_needs_the_plan_approved_even_after_a_skeleton() {
    let r = repo();
    r.service.create_plan("skip", "Skip", "").unwrap();
    add(&r, "skip", "validate", "function", "Validates input.", str_sig());
    relate(&r, "skip", "sample::run", "sample::validate");
    r.service.approve(&pid("skip"), &id("sample::validate"), Stage::ArchApproved, &user()).unwrap();
    append_lib(&r, "\npub fn validate(input: &str) -> bool {\n    todo!()\n}\n");
    assert_eq!(r.service.verify(&pid("skip"), &id("sample::validate"), Stage::Skeleton).unwrap().advanced_to, Some(Stage::Skeleton));
    let text = std::fs::read_to_string(lib_rs(&r)).unwrap().replace("    todo!()\n}", "    !input.is_empty()\n}");
    std::fs::write(lib_rs(&r), text).unwrap();
    // Architecture approval plus a skeleton isn't enough: the user approves the plan first.
    let v = r.service.verify(&pid("skip"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, None);
    assert_eq!(messages(&v.report, FindingCode::ApprovalRequired), vec!["rust:sample::validate can only be implemented once the user approves its plan (PlanApproved)"]);
    r.service.approve(&pid("skip"), &id("sample::validate"), Stage::PlanApproved, &user()).unwrap();
    assert_eq!(r.service.verify(&pid("skip"), &id("sample::validate"), Stage::Implemented).unwrap().advanced_to, Some(Stage::Implemented));
}

// req: P6.6, P6.8
#[test]
fn verification_rereads_vanished_files_and_changed_callees() {
    let r = repo();
    // A component in lib.rs that calls Store::new in store.rs.
    let mut plan = Plan::new("fresh", "Fresh");
    let mut extra = comp("sample::validate", "function");
    extra.signature = str_sig();
    extra.stage = Stage::PlanApproved;
    plan.components.push(extra);
    plan.relations.push(rel(RelationKind::Calls, "sample::validate", "sample::store::Store::new"));
    plan.approvals.push(arch_graph::Approval { component: id("sample::validate"), from: Stage::ArchApproved, to: Stage::PlanApproved, by: "izo".into(), at: "2026-10-03T00:00:00Z".into(), kind: arch_graph::ApprovalKind::Approved });
    FsPlans::new(&r.root).unwrap().save(&plan).unwrap();
    append_lib(&r, "\npub fn validate(input: &str) -> bool {\n    crate::store::Store::new();\n    !input.is_empty()\n}\n");
    let verify = || r.service.verify(&pid("fresh"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(verify().advanced_to, Some(Stage::Implemented));
    // The callee changes shape without anyone re-ingesting: verification sees it.
    let store = r.root.join("src/store.rs");
    std::fs::write(&store, std::fs::read_to_string(&store).unwrap().replace("pub fn new()", "pub fn create()")).unwrap();
    let v = verify();
    assert_eq!(v.advanced_to, None);
    assert!(v.report.codes().contains(&FindingCode::MissingCallSite), "{:?}", v.report);
    std::fs::write(&store, std::fs::read_to_string(&store).unwrap().replace("pub fn create()", "pub fn new()")).unwrap();
    let first = verify();
    assert_eq!(first.advanced_to, Some(Stage::Implemented), "{:?}", first.report);
    // The file is deleted: verification notices instead of trusting the index.
    std::fs::remove_file(lib_rs(&r)).unwrap();
    let v = verify();
    assert_eq!(v.advanced_to, None);
    assert_eq!(v.report.codes(), vec![FindingCode::MissingSymbol]);
}

// req: P6.10, P6.11
#[test]
fn placeholders_hidden_in_unplanned_helpers_block_implemented() {
    let r = repo();
    r.service.create_plan("hide", "Hide", "").unwrap();
    add(&r, "hide", "validate", "function", "Validates input.", str_sig());
    relate(&r, "hide", "sample::run", "sample::validate");
    r.service.approve(&pid("hide"), &id("sample::validate"), Stage::ArchApproved, &user()).unwrap();
    r.service.approve(&pid("hide"), &id("sample::validate"), Stage::PlanApproved, &user()).unwrap();
    append_lib(&r, "\npub fn validate(input: &str) -> bool {\n    check_more(input)\n}\n\nfn check_more(input: &str) -> bool {\n    deeper(input)\n}\n\nfn deeper(input: &str) -> bool {\n    todo!()\n}\n");
    let v = r.service.verify(&pid("hide"), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, None);
    assert_eq!(
        messages(&v.report, FindingCode::PlaceholderRemains),
        vec!["relies on rust:sample::deeper, which still has 1 placeholder bodies (via rust:sample::check_more -> rust:sample::deeper)"]
    );
    // The scope reviewer's unplanned-call point blocks when the helper is a placeholder.
    let text = std::fs::read_to_string(lib_rs(&r)).unwrap().replace("    check_more(input)\n", "    deeper(input)\n");
    std::fs::write(lib_rs(&r), text).unwrap();
    r.service.ingest(&r.root).unwrap();
    let reviewed = r.service.review(&pid("hide"), &id("sample::validate")).unwrap();
    let blocking: Vec<&str> = reviewed.report.findings.iter().filter(|f| f.severity == arch_graph::Severity::Error).map(|f| f.message.as_str()).collect();
    assert!(blocking.iter().any(|m| m.contains("calls rust:sample::deeper, which the plan doesn't mention and which still has 1 placeholder bodies")), "{blocking:?}");
    // Skeletons may lean on placeholders.
    assert_eq!(r.service.verify(&pid("hide"), &id("sample::validate"), Stage::Skeleton).unwrap().advanced_to, Some(Stage::Skeleton));
}

// req: P3.5
#[test]
fn concurrent_plan_edits_are_never_lost() {
    let r = repo();
    r.service.create_plan("busy", "Busy", "").unwrap();
    std::thread::scope(|s| {
        for i in 0..8 {
            let service = &r.service;
            s.spawn(move || {
                service
                    .apply(
                        &pid("busy"),
                        PlanOp::AddComponent { name: format!("worker{i}"), type_name: "function".into(), description: format!("Worker {i}."), parent: Some(id("sample")), file: None, signature: Some(Signature::default()), schemas: None, spec: None },
                    )
                    .unwrap();
            });
        }
    });
    let plan = r.service.plan(&pid("busy")).unwrap();
    assert_eq!(plan.components.len(), 8, "every edit landed: {:?}", plan.components.iter().map(|c| &c.name).collect::<Vec<_>>());
}

// req: P6.3
#[test]
fn signatures_compare_qualifiers_and_ignore_spacing() {
    let sig = |ty: &str, returns: &str, qualifiers: &[&str]| Signature {
        params: vec![arch_graph::Param { name: "input".into(), ty: Some(ty.into()), optional: false, variadic: false }],
        returns: Some(returns.into()),
        qualifiers: qualifiers.iter().map(|q| q.to_string()).collect(),
    };
    assert!(arch_service::signature_diff(&sig("&str", "Vec<String>", &[]), &sig("& str", "Vec< String >", &[]), &[]).is_empty(), "spacing doesn't matter");
    assert!(!arch_service::signature_diff(&sig("&mut str", "u8", &[]), &sig("&mutstr", "u8", &[]), &[]).is_empty(), "word boundaries do");
    assert_eq!(arch_service::signature_diff(&sig("&str", "bool", &[]), &sig("&str", "bool", &["async"]), &[]), vec!["qualifiers: planned none, code has async"]);
    assert_eq!(arch_service::signature_diff(&sig("&str", "bool", &["unsafe"]), &sig("&str", "bool", &[]), &[]), vec!["qualifiers: planned unsafe, code has none"]);
}

// req: P5.5
#[test]
fn every_interface_in_the_chain_with_a_shape_must_fit() {
    let r = repo();
    let mut plan = Plan::new("multi", "Multi");
    let mut ia = comp("sample::IA::go", "operation");
    ia.schemas.inputs = schema(json!({"x": "int"}));
    let mut ib = comp("sample::IB::go", "operation");
    ib.schemas.inputs = schema(json!({"x": "str"}));
    let mut imp = comp("sample::imp", "function");
    imp.schemas.inputs = schema(json!({"x": "int"}));
    plan.components.extend([ia, ib, imp]);
    plan.relations.extend([rel(RelationKind::Implements, "sample::imp", "sample::IA::go"), rel(RelationKind::Implements, "sample::imp", "sample::IB::go")]);
    let found = messages(&check(&r, plan), FindingCode::SchemaMismatch);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("doesn't fit rust:sample::IB::go it implements: inputs: x: type string where integer is expected"), "{found:?}");
}

// req: P5.1, P5.2
#[test]
fn containment_follows_type_rules_and_each_component_has_one_container() {
    let r = repo();
    r.service.create_plan("c", "Containment", "").unwrap();
    add(&r, "c", "helper2", "function", "Helps.", str_sig());
    let relate_contains = |a: &str, b: &str| {
        r.service.apply(&pid("c"), PlanOp::Relate { kind: RelationKind::Contains, source: id(a), target: id(b), order: None, ports: None, note: None, expects: None })
    };
    let err = relate_contains("sample::helper2", "sample::store").unwrap_err().to_string();
    assert!(err.contains("is a function, which can't contain rust:sample::store (a module)"), "{err}");
    let err = relate_contains("sample::run", "sample::helper2").unwrap_err().to_string();
    assert!(err.contains("rust:sample::helper2 is already inside rust:sample; a component has one container"), "{err}");
    // A plan edited by hand with two containers is reported.
    let mut plan = Plan::new("two", "Two");
    plan.components.push(comp("sample::f2", "function"));
    plan.relations.extend([rel(RelationKind::Contains, "sample", "sample::f2"), rel(RelationKind::Contains, "sample::run", "sample::f2")]);
    let found = messages(&check(&r, plan), FindingCode::DisallowedParent);
    assert!(found.iter().any(|m| m == "sits in more than one container (rust:sample, rust:sample::run); a component has one"), "{found:?}");
}

// req: P5.6
#[test]
fn a_call_stating_expectations_without_inputs_passes_no_arguments() {
    let r = repo();
    let mut plan = Plan::new("noin", "No inputs");
    plan.components.push(comp("sample::caller2", "function"));
    plan.relations.push(rel(RelationKind::Contains, "sample", "sample::caller2"));
    let mut call = rel(RelationKind::Calls, "sample::caller2", "sample::util::shout");
    call.expects = Some(Schemas { outputs: schema(json!("string")), ..Schemas::default() });
    plan.relations.push(call);
    let found = messages(&check(&r, plan), FindingCode::CallMismatch);
    assert!(found.iter().any(|m| m.ends_with("parameter 's' isn't among the planned inputs")), "{found:?}");
    assert!(found.iter().any(|m| m.contains("shout takes 1 argument(s); the call passes 0")), "{found:?}");
}

// req: P5.9, P5.18
#[test]
fn approval_blocks_on_failing_logic_checks_and_code_rule_errors() {
    let r = repo_with(
        TypeSystem::defaults().unwrap(),
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
    // A failing logic-step check blocks the design's approval.
    r.service.create_plan("l", "Logic", "").unwrap();
    add(&r, "l", "parse2", "function", "Parses.", str_sig());
    relate(&r, "l", "sample::run", "sample::parse2");
    let step = LogicStep { sequence: 1, name: "split".into(), description: String::new(), algorithm: None, complexity: None, check: Some("echo nope; exit 1".into()) };
    r.service
        .apply(&pid("l"), PlanOp::Modify { id: id("sample::parse2"), description: None, signature: None, schemas: None, spec: Some(ImplementationSpec { logic_steps: vec![step], ..ImplementationSpec::default() }) })
        .unwrap();
    let err = r.service.approve(&pid("l"), &id("sample::parse2"), Stage::ArchApproved, &user()).unwrap_err().to_string();
    assert!(err.contains("logic step 1 'split' check failed: nope"), "{err}");
    // Changes to code that breaks a project rule can't be approved either.
    r.service.create_plan("m", "Modify log", "").unwrap();
    r.service.apply(&pid("m"), PlanOp::Modify { id: id("sample::log"), description: Some("Logs.".into()), signature: None, schemas: None, spec: None }).unwrap();
    let err = r.service.approve(&pid("m"), &id("sample::log"), Stage::ArchApproved, &user()).unwrap_err().to_string();
    assert!(err.contains("[no-println] log through the logger"), "{err}");
}

// req: P5.11, P5.17
#[test]
fn contracts_show_inherited_resolved_shapes_tools_effects_and_data_flows() {
    let r = repo();
    let mut plan = Plan::new("k", "Contract");
    let mut user_type = comp("sample::User", "class");
    user_type.signature = None;
    user_type.schemas.properties = schema(json!({"name": "str"}));
    let mut port = comp("sample::Saver::save", "operation");
    port.schemas.inputs = schema(json!({"who": "User"}));
    let mut save = comp("sample::save_user", "function");
    save.spec = Some(ImplementationSpec { validation: vec![ToolRef { tool: "clippy".into(), targets: vec!["src/lib.rs".into()], args: None }], ..ImplementationSpec::default() });
    let mut produce = comp("sample::load_user", "function");
    produce.schemas.outputs = schema(json!("User"));
    plan.components.extend([user_type, port, save, produce]);
    let mut call = rel(RelationKind::Calls, "sample::save_user", "sample::log");
    call.expects = Some(Schemas { side_effects: vec![SideEffect { target: "stdout".into(), description: "prints".into() }], ..Schemas::default() });
    let mut flow = rel(RelationKind::DataFlow, "sample::load_user", "sample::save_user");
    flow.ports = Some(("return".into(), "who".into()));
    plan.relations.extend([rel(RelationKind::Implements, "sample::save_user", "sample::Saver::save"), call, flow]);
    FsPlans::new(&r.root).unwrap().save(&plan).unwrap();
    let slice = r.service.contract(&pid("k"), &id("sample::save_user")).unwrap();
    assert_eq!(slice.data.inputs_from, Some(id("sample::Saver::save")), "inherited from the interface");
    assert_eq!(slice.data.inputs.as_ref().unwrap()["properties"]["who"]["properties"]["name"], json!({"type": "string"}), "the $ref is resolved");
    assert_eq!(slice.validation[0].tool, "clippy");
    assert_eq!(slice.data_in.len(), 1);
    assert_eq!(slice.data_in[0].ports, Some(("return".into(), "who".into())));
    let md = slice.to_markdown();
    for needle in [
        "- Inputs (from rust:sample::Saver::save): `",
        "\"name\"",
        "  - expects it to change `stdout`: prints",
        "## Data in\n- `load_user` function [return -> who] — planned",
        "## Validation tools\n- `clippy` on src/lib.rs",
    ] {
        assert!(md.contains(needle), "missing {needle:?} in:\n{md}");
    }
}

// req: P4.6
#[test]
fn calls_that_cant_be_proven_are_reported_not_linked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../arch-index/tests/fixtures/rust/sample"), &root);
    let service = ArchitectureService::open(
        &root,
        PackSet::built_in().unwrap(),
        TypeSystem::defaults().unwrap(),
        Box::new(arch_service::MemoryPlans::default()),
        Box::new(arch_service::MemoryIndex::default()),
    )
    .unwrap();
    let report = service.ingest(&root).unwrap();
    let unproven = service.unproven_calls().unwrap();
    assert_eq!(report.unproven, unproven.len());
    let call = unproven.iter().find(|c| c.caller == id("sample::store::store_each")).expect("s.save on an untyped receiver");
    assert_eq!(call.candidates, vec![id("sample::store::Store::save")]);
    let graph = service.observed().unwrap();
    assert!(!graph.outgoing(&id("sample::store::store_each"), Some(RelationKind::Calls)).any(|c| c.target == id("sample::store::Store::save")), "never guessed");
}
