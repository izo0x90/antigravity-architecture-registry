//! Validation tools, reviewers and re-verification on change, through REST,
//! the MCP endpoint and the file watcher, on a copy of the Rust fixture repo.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use arch_graph::{ArchEvent, ComponentId, PlanId, Stage};
use arch_service::{Actor, ArchitectureService, PlanOp, ProcessRunner, ReviewerCatalog, ToolCatalog};
use common::{Mcp, fixture_architecture_with, http, spawn_app_with};
use serde_json::{Value, json};

fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

fn tools() -> ToolCatalog {
    ToolCatalog::from_toml(
        r#"
[tools.mentions]
description = "The component's file defines it."
executable = "sh"
default_args = ["-c", "grep -q \"fn ${0##*::}\" \"$1\"", "{symbols}", "{files}"]
applies_to = ["function"]
"#,
    )
    .unwrap()
}

fn reviewers() -> ReviewerCatalog {
    let mut catalog = ReviewerCatalog::defaults().unwrap();
    catalog.extend_with(ReviewerCatalog::from_toml("[reviewers.tiny]\ncategory = \"anti_patterns\"\nkind = \"rule\"\nblocking = true\napplies_to = [\"function\"]\nrules = [{ rule = \"max_lines\", limit = 2 }]\n").unwrap());
    catalog
}

fn id(s: &str) -> ComponentId {
    ComponentId::new(format!("rust:{s}"))
}

/// Plan "v": validate (calls Store::new), approved, implemented in src/lib.rs.
fn implemented_validate(arch: &ArchitectureService, body: &str) {
    let plan = PlanId::new("v").unwrap();
    arch.create_plan("v", "Validate", "", arch_graph::PlanKind::Change).unwrap();
    arch.apply(
        &plan,
        PlanOp::AddComponent {
            name: "validate".into(),
            type_name: "function".into(),
            description: "Checks input.".into(),
            parent: Some(id("sample")),
            file: None,
            signature: Some(arch_graph::Signature { params: vec![arch_graph::Param { name: "input".into(), ty: Some("&str".into()), optional: false, variadic: false }], returns: Some("bool".into()), qualifiers: vec![] }),
            schemas: None,
            spec: None,
        },
    )
    .unwrap();
    arch.apply(&plan, PlanOp::Relate { kind: arch_graph::RelationKind::Calls, source: id("sample::validate"), target: id("sample::store::Store::new"), order: None, ports: None, note: None, expects: None }).unwrap();
    for to in [Stage::ArchApproved, Stage::PlanApproved] {
        arch.approve(&plan, &id("sample::validate"), to, &Actor::User { name: "izo".into() }).unwrap();
    }
    let lib = arch.root().join("src/lib.rs");
    let mut code = std::fs::read_to_string(&lib).unwrap();
    code.push_str(body);
    std::fs::write(&lib, code).unwrap();
}

#[tokio::test]
async fn tools_reviews_and_refresh_over_rest_and_mcp() {
    let arch = fixture_architecture_with(|a| a.with_tools(tools(), Arc::new(ProcessRunner)).with_reviewers(reviewers(), None));
    implemented_validate(&arch, "\npub fn validate(input: &str) -> bool {\n    crate::store::Store::new();\n    !input.is_empty()\n}\n");
    let (addr, _state) = spawn_app_with(arch.clone()).await;

    let (status, body) = http(addr, "GET", "/api/arch/tools", None).await;
    assert_eq!(status, 200);
    assert_eq!(parse(&body)["tools"]["mentions"]["executable"], "sh");
    let (status, body) = http(addr, "GET", "/api/arch/reviewers", None).await;
    assert_eq!(status, 200);
    assert_eq!(parse(&body)["reviewers"]["tiny"]["kind"], "rule");

    let (status, body) = http(addr, "POST", "/api/arch/ingest", Some("{}")).await;
    assert_eq!((status, parse(&body)["changed"].clone()), (200, json!(["src/lib.rs"])));
    let (status, body) = http(addr, "POST", "/api/arch/plans/v/tools", Some(r#"{"component":"rust:sample::validate"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let report = parse(&body);
    assert_eq!(report["runs"][0]["tool"], "mentions");
    assert_eq!(report["runs"][0]["args"], json!(["-c", "grep -q \"fn ${0##*::}\" \"$1\"", "sample::validate", "src/lib.rs"]));
    assert_eq!(report["runs"][0]["passed"], true);
    let (status, _) = http(addr, "POST", "/api/arch/plans/nope/tools", Some("{}")).await;
    assert_eq!(status, 404);

    let (status, body) = http(addr, "POST", "/api/arch/plans/v/components/rust:sample::validate/review", None).await;
    assert_eq!(status, 200, "{body}");
    let reviews = parse(&body);
    assert_eq!(reviews["report"]["findings"][0]["code"], "review_finding");
    assert_eq!(reviews["report"]["findings"][0]["severity"], "error");

    // Through the agent's tools.
    let mut mcp = Mcp::connect(addr).await;
    let (err, text) = mcp.tool("run_tools", json!({ "plan": "v" })).await;
    assert!(!err && text.contains("PASS mentions on rust:sample::validate") && text.contains("Next:") && text.contains("rust:sample::validate ("), "{text}");
    let (err, text) = mcp.tool("review", json!({ "plan": "v", "id": "rust:sample::validate" })).await;
    assert!(!err && text.contains("tiny (anti_patterns): 1 points") && text.contains("ERROR (src/lib.rs:") && text.contains("Address the blocking points"), "{text}");
    let (err, text) = mcp.tool("verify", json!({ "plan": "v", "id": "rust:sample::validate", "goal": "implemented" })).await;
    assert!(!err && text.contains("Not yet") && text.contains("[anti_patterns/tiny]"), "the blocking review blocks: {text}");
    let (err, _) = mcp.tool("review", json!({ "plan": "v", "id": "rust:ghost" })).await;
    assert!(err);

    // Shrink it below the limit and verify; then a change elsewhere breaks
    // the call and a refresh reports it.
    let lib = arch.root().join("src/lib.rs");
    let code = std::fs::read_to_string(&lib).unwrap().replace("    crate::store::Store::new();\n    !input.is_empty()\n}", "    crate::store::Store::new(); !input.is_empty() }");
    std::fs::write(&lib, code).unwrap();
    let (err, text) = mcp.tool("verify", json!({ "plan": "v", "id": "rust:sample::validate", "goal": "implemented" })).await;
    assert!(!err && text.contains("is now Implemented"), "{text}");
    let store = arch.root().join("src/store.rs");
    std::fs::write(&store, std::fs::read_to_string(&store).unwrap().replace("pub fn new()", "pub fn make()")).unwrap();
    let (status, body) = http(addr, "POST", "/api/arch/refresh", Some(r#"{"paths":["src/store.rs","README.md"]}"#)).await;
    assert_eq!(status, 200, "{body}");
    let refresh = parse(&body);
    assert_eq!(refresh["reverified"].as_array().unwrap().len(), 1);
    assert_eq!(refresh["reverified"][0]["component"], "rust:sample::validate");
    assert_eq!(refresh["reverified"][0]["report"]["findings"][0]["code"], "missing_call_site");
    let (status, _) = http(addr, "POST", "/api/arch/refresh", Some(r#"{"paths":["../etc/passwd"]}"#)).await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn the_watcher_reverifies_when_a_file_changes() {
    let arch = fixture_architecture_with(|a| a);
    implemented_validate(&arch, "\npub fn validate(input: &str) -> bool {\n    crate::store::Store::new();\n    !input.is_empty()\n}\n");
    let v = arch.verify(&PlanId::new("v").unwrap(), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, Some(Stage::Implemented), "{:?}", v.report);
    let seen: Arc<Mutex<Vec<ArchEvent>>> = Arc::default();
    let sink = seen.clone();
    arch.subscribe(move |e| sink.lock().unwrap().push(e.clone()));
    meta_harness::watch::watch(arch.clone()).unwrap();
    // Give the watcher a moment to register, then break the planned call.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let lib = arch.root().join("src/lib.rs");
    std::fs::write(&lib, std::fs::read_to_string(&lib).unwrap().replace("    crate::store::Store::new();\n    !input", "    !input")).unwrap();
    let validated = |events: &[ArchEvent]| events.iter().any(|e| matches!(e, ArchEvent::Validated { component: Some(c), errors: 1, .. } if c == &id("sample::validate")));
    for _ in 0..100 {
        if validated(&seen.lock().unwrap()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no re-verification after the change: {:?}", seen.lock().unwrap());
}
