//! Importing the old registry document through REST and the MCP endpoint.

mod common;

use common::{Mcp, fixture_architecture, http, spawn_app_with};
use serde_json::{Value, json};

// req: P3.6
#[tokio::test]
async fn the_old_registry_imports_through_rest_and_mcp() {
    let arch = fixture_architecture();
    let old = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../system_architecture.json")).unwrap();
    std::fs::write(arch.root().join("system_architecture.json"), &old).unwrap();
    let (addr, _) = spawn_app_with(arch).await;

    let (status, body) = http(addr, "POST", "/api/arch/import", Some(&json!({ "plan": "calc", "language": "rust" }).to_string())).await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["plan"]["id"], "calc");
    assert_eq!(v["report"]["components"].as_array().unwrap().len(), 11);
    assert!(v["report"]["unmapped"].as_array().unwrap().iter().any(|u| u["item"] == "type enum"), "{body}");
    let (status, _) = http(addr, "GET", "/api/arch/plans/calc", None).await;
    assert_eq!(status, 200, "the imported plan was saved");
    let (status, body) = http(addr, "POST", "/api/arch/import", Some(&json!({ "plan": "nope", "file": "missing.json" }).to_string())).await;
    assert_eq!(status, 404, "{body}");
    let (status, _) = http(addr, "POST", "/api/arch/import", Some(&json!({ "plan": "esc", "file": "../x.json" }).to_string())).await;
    assert_eq!(status, 400, "paths stay inside the repo");

    let mut mcp = Mcp::connect(addr).await;
    let (err, text) = mcp.tool("plan_import", json!({ "id": "calc2", "language": "rust" })).await;
    assert!(!err, "{text}");
    assert!(text.starts_with("Imported plan calc2: 11 components (0 matched to code)"), "{text}");
    assert!(text.contains("Not carried over as is:") && text.contains("- usage node user_calls_calculate expected_inputs"), "{text}");
    let (err, text) = mcp.tool("plan_import", json!({ "id": "calc2" })).await;
    assert!(err && text.contains("already exists"), "{text}");
}
