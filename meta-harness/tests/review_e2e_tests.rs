//! The review loop through the server: the user comments and marks the plan
//! reviewed over REST (the cockpit's side), the agent reads the points and
//! answers them over MCP, and approval waits for the user to accept.

mod common;

use arch_graph::{ComponentId, PlanId, RelationKind};
use arch_service::PlanOp;
use common::{Mcp, fixture_architecture, http, http_cockpit, spawn_app_with};
use serde_json::{Value, json};

fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

fn id(s: &str) -> ComponentId {
    ComponentId::new(format!("rust:{s}"))
}

#[tokio::test]
async fn comments_reach_the_agent_and_answers_come_back_for_the_user() {
    let arch = fixture_architecture();
    let plan = PlanId::new("v").unwrap();
    arch.create_plan("v", "Validate", "").unwrap();
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
    arch.apply(&plan, PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::run"), target: id("sample::validate"), order: None, ports: None, note: None, expects: None }).unwrap();
    let (addr, _state) = spawn_app_with(arch.clone()).await;

    // The user reviews, then raises a change request on a spec section.
    let (status, body) = http_cockpit(addr, "POST", "/api/arch/plans/v/reviewed", Some(r#"{"by":"izo"}"#)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(parse(&body)["by"], "izo");
    let (status, body) = http_cockpit(
        addr,
        "POST",
        "/api/arch/plans/v/comments",
        Some(r#"{"anchor":{"on":"section","id":"rust:sample::validate","section":"overview"},"kind":"change_request","text":"Say what it rejects.","by":"izo"}"#),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!((parse(&body)["id"].as_str(), parse(&body)["status"].as_str()), (Some("c1"), Some("open")));
    let (status, _) = http_cockpit(addr, "POST", "/api/arch/plans/v/comments", Some(r#"{"anchor":{"on":"plan"},"text":"x","by":" "}"#)).await;
    assert_eq!(status, 400, "who's commenting is required");
    let (status, body) = http(addr, "GET", "/api/arch/plans/v/comments", None).await;
    assert_eq!((status, parse(&body).as_array().map(Vec::len)), (200, Some(1)));

    // Approval waits on it.
    let (status, body) = http_cockpit(addr, "POST", "/api/arch/plans/v/components/rust:sample::validate/approve", Some(r#"{"to":"arch_approved","by":"izo"}"#)).await;
    assert_eq!(status, 400);
    assert!(body.contains("unaddressed review comments") && body.contains("c1"), "{body}");

    // The agent sees the point as structured feedback, and next steps point at it.
    let mut mcp = Mcp::connect(addr).await;
    let (err, text) = mcp.tool("plan_validate", json!({ "plan": "v" })).await;
    assert!(!err && text.contains("1 open point(s) from the user (c1)"), "{text}");
    let (err, text) = mcp.tool("review_comments", json!({ "plan": "v" })).await;
    assert!(!err, "{text}");
    assert!(text.contains("[c1] CHANGE REQUEST on rust:sample::validate § overview (open) by izo: Say what it rejects."), "{text}");
    assert!(text.contains("No plan changes since the user's review"), "{text}");

    // It changes the plan, sees the diff, and answers linking the change.
    let (err, text) = mcp.tool("answer_comment", json!({ "plan": "v", "comment": "c1", "answer": "Done.", "changes": ["rust:sample::validate"] })).await;
    assert!(err && text.contains("hasn't changed"), "answers need real changes: {text}");
    arch.apply(&plan, PlanOp::Remove { id: id("sample::validate") }).unwrap();
    arch.apply(
        &plan,
        PlanOp::AddComponent {
            name: "validate".into(),
            type_name: "function".into(),
            description: "Rejects empty input.".into(),
            parent: Some(id("sample")),
            file: None,
            signature: Some(arch_graph::Signature { params: vec![arch_graph::Param { name: "input".into(), ty: Some("&str".into()), optional: false, variadic: false }], returns: Some("bool".into()), qualifiers: vec![] }),
            schemas: None,
            spec: None,
        },
    )
    .unwrap();
    arch.apply(&plan, PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::run"), target: id("sample::validate"), order: None, ports: None, note: None, expects: None }).unwrap();
    let (status, body) = http(addr, "GET", "/api/arch/plans/v/diff", None).await;
    assert_eq!(status, 200);
    assert_eq!(parse(&body)["changes"], json!([{ "change": "component_changed", "id": "rust:sample::validate", "fields": ["description"] }]));
    let (err, text) = mcp.tool("review_comments", json!({ "plan": "v" })).await;
    assert!(!err && text.contains("- changed rust:sample::validate: description"), "{text}");
    let (err, text) = mcp.tool("reply_comment", json!({ "plan": "v", "comment": "c1", "text": "Empty only, or whitespace too?" })).await;
    assert!(!err && text.contains("1 replies"), "{text}");
    let (err, text) = mcp.tool("answer_comment", json!({ "plan": "v", "comment": "c1", "answer": "It now says it rejects empty input.", "changes": ["rust:sample::validate"] })).await;
    assert!(!err && text.contains("Answered c1") && text.contains("linked: changed rust:sample::validate: description"), "{text}");

    // The user sees the answer, reopens once, then accepts; approval goes through.
    let (_, body) = http(addr, "GET", "/api/arch/plans/v/comments", None).await;
    let c1 = &parse(&body)[0];
    assert_eq!((c1["status"].as_str(), c1["answer"]["changes"][0]["fields"][0].as_str()), (Some("answered"), Some("description")));
    let (status, body) = http_cockpit(addr, "POST", "/api/arch/plans/v/comments/c1/reopen", Some(r#"{"text":"And whitespace?","by":"izo"}"#)).await;
    assert_eq!((status, parse(&body)["status"].as_str()), (200, Some("open")));
    let (err, _) = mcp.tool("answer_comment", json!({ "plan": "v", "comment": "c1", "answer": "Whitespace counts as empty.", "changes": ["rust:sample::validate"] })).await;
    assert!(!err);
    let (status, body) = http_cockpit(addr, "POST", "/api/arch/plans/v/comments/c1/accept", Some(r#"{"by":"izo"}"#)).await;
    assert_eq!((status, parse(&body)["status"].as_str()), (200, Some("resolved")), "{body}");
    let (status, body) = http_cockpit(addr, "POST", "/api/arch/plans/v/comments/c1/replies", Some(r#"{"text":"Thanks.","by":"izo"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = http_cockpit(addr, "POST", "/api/arch/plans/v/components/rust:sample::validate/approve", Some(r#"{"to":"arch_approved","by":"izo"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let (_, body) = http(addr, "GET", "/api/arch/plans/v/comments", None).await;
    assert_eq!(parse(&body), json!([]), "resolved points drop out of the open list");
    let (_, body) = http(addr, "GET", "/api/arch/plans/v/comments?all=true", None).await;
    assert_eq!(parse(&body).as_array().map(Vec::len), Some(1));
    // There is no agent tool to accept or approve.
    let (err, _) = mcp.tool("accept_comment", json!({ "plan": "v", "comment": "c1" })).await;
    assert!(err);
}
