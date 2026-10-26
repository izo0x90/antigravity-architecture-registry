//! Importing the old registry document into a plan over the code.

use std::path::Path;

use arch_graph::{ComponentId, InvariantKind, RelationKind, Stage, Status, TypeSystem};
use arch_service::{ArchitectureService, MemoryPlans};

mod common;

fn service(root: &Path) -> ArchitectureService {
    let s = ArchitectureService::open(
        root,
        common::shipped_packs(),
        TypeSystem::defaults().unwrap(),
        Box::new(MemoryPlans::default()),
        arch_service::temporary_facts(&arch_service::Limits::defaults().unwrap()).unwrap(),
    )
    .unwrap();
    s.ingest(root).unwrap();
    s
}

fn write(root: &Path, file: &str, text: &str) {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn the_old_registry_imports_into_a_plan_matched_to_code() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Part of the old design already exists in code.
    write(root, "calculator/__init__.py", "");
    write(root, "calculator/validator.py", "class MathValidator:\n    def validate(self, request):\n        return request\n");
    let old = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../system_architecture.json")).unwrap();
    let s = service(root);
    let (plan, report) = s.import_registry("calc", "Calculator", &old, Some("python")).unwrap();
    let ids: Vec<String> = report.components.iter().map(|c| format!("{} -> {} {}", c.old_id, c.id, c.matched)).collect();
    assert_eq!(report.components.len(), 11, "every old component comes in: {ids:#?}");
    let id_of = |old: &str| report.components.iter().find(|c| c.old_id == old).unwrap().id.clone();
    let matched = |old: &str| report.components.iter().find(|c| c.old_id == old).unwrap().matched;
    assert!(matched("math_validator") && matched("math_validator_validate"), "{ids:#?}");
    assert!(!matched("calculator_service"));
    assert_eq!(id_of("calculator_service_calculate"), ComponentId::new("python:calculator_service.calculate"));
    let validate = plan.component(&id_of("math_validator_validate")).unwrap();
    assert_eq!(validate.status, Status::Modifying, "it had planned changes");
    let spec = validate.spec.as_ref().unwrap();
    assert_eq!(spec.invariants[0].kind, InvariantKind::PreCondition);
    assert_eq!(spec.logic_steps.len(), 2);
    assert!(!spec.tasks.is_empty());
    assert_eq!(
        validate.schemas.inputs.as_ref().unwrap()["properties"]["request"],
        serde_json::json!({"$ref": "calculation_request"}),
        "title references become $refs"
    );
    let logger = plan.component(&id_of("audit_logger_log")).unwrap();
    assert_eq!((logger.status, logger.stage), (Status::New, Stage::Declared), "no approvals are carried over");
    assert_eq!(logger.schemas.side_effects[0].target, "fs");
    // Structure, ordered calls and the workflow.
    let has = |kind: RelationKind, a: &str, b: &str| {
        plan.relations.iter().any(|r| r.kind == kind && r.source == id_of(a) && r.target == id_of(b))
    };
    assert!(has(RelationKind::Contains, "calculator_service", "calculator_service_calculate"));
    assert!(has(RelationKind::Calls, "calculator_client_run", "calculator_service_calculate"));
    let order = |b: &str| plan.relations.iter().find(|r| r.kind == RelationKind::Calls && r.target == id_of(b)).unwrap().order;
    assert_eq!((order("math_validator_validate"), order("audit_logger_log")), (Some(0), Some(1)));
    assert_eq!(plan.workflows.len(), 1);
    assert_eq!(plan.workflows[0].entry, id_of("calculator_client_run"));
    // What couldn't be carried over is reported.
    let reasons: Vec<String> = report.unmapped.iter().map(|u| format!("{}: {}", u.item, u.reason)).collect();
    assert!(reasons.iter().any(|r| r.starts_with("type enum: ") && r.contains("'class'")), "{reasons:#?}");
    assert!(reasons.iter().any(|r| r.starts_with("component audit_logger_log stage: was 'implemented'")), "{reasons:#?}");
    assert!(!reasons.iter().any(|r| r.contains("expected_")), "call-site expectations are carried, not dropped: {reasons:#?}");
    // Call-site expectations travel with the call, references converted.
    let call = |a: &str, b: &str| {
        plan.relations.iter().find(|r| r.kind == RelationKind::Calls && r.source == id_of(a) && r.target == id_of(b)).unwrap()
    };
    let expects = call("calculator_client_run", "calculator_service_calculate").expects.as_ref().unwrap();
    assert_eq!(expects.inputs.as_ref().unwrap()["properties"]["request"], serde_json::json!({"$ref": "calculation_request"}));
    assert_eq!(expects.outputs.as_ref().unwrap()["properties"]["result"], serde_json::json!({"$ref": "calculation_result"}));
    assert_eq!(expects.side_effects[0].target, "fs");
    let to_logger = call("calculator_service_calculate", "audit_logger_log").expects.as_ref().unwrap();
    assert!(to_logger.outputs.is_none() && to_logger.side_effects[0].description == "Appends to calc_history.log");
    // The plan was saved and checks run over it.
    assert_eq!(s.plan(&plan.id).unwrap(), plan);
    s.validate(&plan.id).unwrap();
    assert!(s.import_registry("calc", "again", &old, None).is_err(), "plan ids stay unique");
}

#[test]
fn unmappable_items_are_reported_not_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "src/lib.rs", "pub fn run() {}\n");
    let old = serde_json::json!({
        "components": {
            "lonely": {"id": "lonely", "name": "Lonely", "type": "function", "description": "No place.", "stage": "plan_approved"},
            "located": {"id": "located", "name": "located", "type": "function", "description": "In a file.", "status": "existing", "location": {"file_path": "src/new.rs"}},
            "orphan": {"id": "orphan", "name": "Orphan", "type": "function", "parent_id": "missing_parent", "description": ""},
            "widget": {"id": "widget", "name": "Widget", "type": "gizmo", "description": ""},
            "trigger": {"id": "trigger", "name": "Trigger", "type": "system_trigger", "description": ""},
            "run": {"id": "run", "name": "run", "type": "function", "description": "Old entry point.", "status": "deprecated", "location": {"file_path": "src/lib.rs"}},
            "dto": {"id": "dto", "name": "Dto", "type": "class", "description": "", "location": {"file_path": "src/lib.rs"}, "properties": {"type": "object", "properties": {"x": {"type": "object", "title": "nowhere"}}},
                    "implementation_spec": {"invariants": [{"name": "odd", "type": "side_effects", "description": ""}], "logic_steps": [], "validation": [{"tool_id": "ruff", "targets": ["src"]}]}}
        },
        "usage_trees": {"flow": {"node_id": "n1", "caller_id": "dto", "component_id": "located", "dependencies": [
            {"node_id": "n2", "caller_id": "located", "component_id": "ghost"}
        ]}},
        "component_types": {"system_trigger": {"allows_properties": false, "allows_signature": false, "is_executable_caller": true, "allowed_parent_types": []}},
        "validation_tools": {"ruff": {"id": "ruff", "executable": "ruff", "default_args": ["check", "{targets}"]}}
    })
    .to_string();
    let s = service(root);
    let (plan, report) = s.import_registry("odd", "Odd", &old, None).unwrap();
    let reasons: Vec<String> = report.unmapped.iter().map(|u| format!("{}: {}", u.item, u.reason)).collect();
    let says = |prefix: &str, part: &str| reasons.iter().any(|r| r.starts_with(prefix) && r.contains(part));
    assert!(says("component lonely:", "give a language"), "{reasons:#?}");
    assert!(says("component orphan:", "missing_parent doesn't exist"), "{reasons:#?}");
    assert!(says("component widget:", "unknown type 'gizmo'"), "{reasons:#?}");
    assert!(says("type system_trigger:", "aren't imported"), "{reasons:#?}");
    assert!(says("component located status:", "was 'existing'"), "{reasons:#?}");
    assert!(says("component dto properties:", "'nowhere'"), "{reasons:#?}");
    assert!(says("component dto invariant odd:", "unknown kind 'side_effects'"), "{reasons:#?}");
    assert!(says("usage node n1:", "can't call anything"), "{reasons:#?}");
    assert!(says("usage node n2:", "ghost wasn't imported"), "{reasons:#?}");
    // Located components get a proposed location from their file's pack.
    let located_id = report.components.iter().find(|c| c.old_id == "located").unwrap().id.clone();
    assert!(located_id.as_str().starts_with("rust:") && located_id.as_str().ends_with("::located"), "{located_id}");
    let located = plan.component(&located_id).unwrap();
    assert_eq!(located.proposed.as_ref().unwrap().file.as_str(), "src/new.rs");
    // Deprecated code stays in the code, marked deprecated.
    let run = report.components.iter().find(|c| c.old_id == "run").unwrap();
    assert!(run.matched);
    assert_eq!(plan.component(&run.id).unwrap().status, Status::Deprecated);
    assert_eq!(plan.components.len(), 3, "only what could be placed: {:?}", plan.components.iter().map(|c| &c.id).collect::<Vec<_>>());
    assert_eq!(report.tools.get("ruff").unwrap().default_args, vec!["check", "{targets}"]);
    assert!(s.import_registry("bad", "Bad", "{ not json", None).is_err());
}
