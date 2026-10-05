//! The agent's architecture tools through the real MCP endpoint (SSE stream +
//! message posts), on a copy of the Rust fixture repo.

mod common;

use common::{Mcp, spawn_app};
use serde_json::json;

// req: P8.3, P8.4
#[tokio::test]
async fn every_tool_works_through_the_mcp_endpoint() {
    let (addr, state) = spawn_app().await;
    let mut mcp = Mcp::connect(addr).await;
    let init = mcp.rpc("initialize", json!({})).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "nah-architecture");
    let tools = mcp.rpc("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in ["ingest", "list_types", "search", "inspect", "view", "navigate", "plan_list", "plan_create", "plan_import", "plan_edit", "plan_validate", "contract", "verify", "run_tools", "review", "review_comments", "answer_comment", "reply_comment"] {
        assert!(names.contains(&expected), "{expected} in {names:?}");
    }
    assert!(!names.iter().any(|n| n.contains("approve")), "agents can't approve");

    let (err, text) = mcp.tool("ingest", json!({})).await;
    assert!(!err && text.contains("3 files"), "{text}");
    let (err, _) = mcp.tool("ingest", json!({ "scope": "/etc" })).await;
    assert!(err, "absolute scopes are rejected");

    let (err, text) = mcp.tool("list_types", json!({})).await;
    assert!(!err && text.contains("\"function\""));

    let (err, text) = mcp.tool("search", json!({ "query": "save" })).await;
    assert!(!err && text.contains("rust:sample::store::Store::save [function Existing/Implemented] src/store.rs:10"), "{text}");
    let (err, text) = mcp.tool("search", json!({ "query": "zzz" })).await;
    assert!(!err && text.contains("No matches"));

    let (err, text) = mcp.tool("inspect", json!({ "id": "rust:sample::run" })).await;
    assert!(!err && text.contains("Store::new()"), "includes source: {text}");
    let (err, _) = mcp.tool("inspect", json!({ "id": "rust:nope" })).await;
    assert!(err);

    let (err, text) = mcp.tool("view", json!({ "name": "structure", "roots": ["rust:sample::store"] })).await;
    assert!(!err && text.contains("View 'structure'") && text.contains("Store [class"), "{text}");
    let (err, _) = mcp.tool("view", json!({ "name": "teleport" })).await;
    assert!(err);

    let (mut events, _) = (state.hub.subscribe().1, ());
    let (err, _) = mcp.tool("navigate", json!({ "view": "calls", "focus": "rust:sample::run" })).await;
    assert!(!err);
    let event = events.recv().await.unwrap();
    assert!(matches!(event, harness_contracts::ClientEvent::Architecture { event: arch_graph::ArchEvent::ViewRequested { by: arch_graph::Requester::Agent, .. } }));
    let (err, _) = mcp.tool("navigate", json!({ "focus": "rust:ghost" })).await;
    assert!(err);

    let (err, text) = mcp.tool("plan_list", json!({})).await;
    assert!(!err && text.contains("No plans yet"));
    let (err, text) = mcp.tool("plan_create", json!({ "id": "val", "title": "Validate input" })).await;
    assert!(!err && text.contains("Next:"), "{text}");
    let (err, _) = mcp.tool("plan_create", json!({ "id": "val", "title": "again" })).await;
    assert!(err, "duplicate plan");

    let ops = json!([
        { "op": "add_component", "name": "validate", "type": "function", "parent": "rust:sample", "description": "Rejects empty input.",
          "signature": { "params": [{ "name": "input", "ty": "&str" }], "returns": "bool" } }
    ]);
    let (err, text) = mcp.tool("plan_edit", json!({ "plan": "val", "ops": ops })).await;
    assert!(!err && text.contains("rust:sample::validate (Declared): write its spec"), "next steps: {text}");
    let (err, text) = mcp.tool("plan_validate", json!({ "plan": "val" })).await;
    assert!(!err && text.contains("ERROR [missing_touch_point]"), "{text}");
    let (err, text) = mcp.tool("plan_edit", json!({ "plan": "val", "ops": [{ "op": "relate", "kind": "calls", "source": "rust:sample::run", "target": "rust:sample::validate" }, { "op": "remove", "id": "rust:sample::log" }] })).await;
    assert!(err && text.contains("op 2 failed"), "{text}");
    let (err, text) = mcp.tool("plan_validate", json!({ "plan": "val" })).await;
    assert!(!err && text.contains("The plan checks out"), "op 1 stuck: {text}");

    let (err, text) = mcp.tool("contract", json!({ "plan": "val", "id": "rust:sample::validate" })).await;
    assert!(!err && text.contains("## Called by") && text.contains("`run`"), "{text}");
    let (err, _) = mcp.tool("contract", json!({ "plan": "val", "id": "rust:sample::run" })).await;
    assert!(err, "not a plan component");

    // The user approves in the cockpit; then the agent lays a skeleton and verifies.
    state.arch.approve(&arch_graph::PlanId::new("val").unwrap(), &arch_graph::ComponentId::new("rust:sample::validate"), arch_graph::Stage::ArchApproved, &arch_service::Actor::User { name: "izo".into() }).unwrap();
    let (err, text) = mcp.tool("verify", json!({ "plan": "val", "id": "rust:sample::validate", "goal": "skeleton" })).await;
    assert!(!err && text.contains("Not yet") && text.contains("isn't in the code"), "{text}");
    let lib = state.arch.root().join("src/lib.rs");
    let mut code = std::fs::read_to_string(&lib).unwrap();
    code.push_str("\npub fn validate(input: &str) -> bool {\n    todo!()\n}\n");
    std::fs::write(&lib, &code).unwrap();
    let (err, text) = mcp.tool("verify", json!({ "plan": "val", "id": "rust:sample::validate", "goal": "skeleton" })).await;
    assert!(!err && text.contains("is now Skeleton") && text.contains("replace the placeholder bodies"), "{text}");
    std::fs::write(&lib, code.replace("    todo!()\n", "    !input.is_empty()\n")).unwrap();
    // Implementing needs the user's plan approval (after the skeleton is fine).
    let (err, text) = mcp.tool("verify", json!({ "plan": "val", "id": "rust:sample::validate", "goal": "implemented" })).await;
    assert!(!err && text.contains("approves its plan"), "{text}");
    state.arch.approve(&arch_graph::PlanId::new("val").unwrap(), &arch_graph::ComponentId::new("rust:sample::validate"), arch_graph::Stage::PlanApproved, &arch_service::Actor::User { name: "izo".into() }).unwrap();
    let (err, text) = mcp.tool("verify", json!({ "plan": "val", "id": "rust:sample::validate", "goal": "implemented" })).await;
    assert!(!err && text.contains("is now Implemented") && text.contains("nothing pending"), "{text}");
    let (err, _) = mcp.tool("verify", json!({ "plan": "val", "id": "rust:sample::validate", "goal": "declared" })).await;
    assert!(err);
    let (err, _) = mcp.tool("teleport", json!({})).await;
    assert!(err);
}

// req: P8.7
#[tokio::test]
async fn tool_results_end_with_stage_specific_next_steps() {
    use arch_graph::{Component, ComponentId, Plan, ProposedLocation, RepoPath, Stage, Status};
    let mut plan = Plan::new("p", "P");
    for (name, stage) in [("a", Stage::Declared), ("b", Stage::ArchApproved), ("c", Stage::PlanApproved), ("d", Stage::Skeleton), ("e", Stage::Implemented)] {
        plan.components.push(Component {
            id: ComponentId::new(format!("rust:x::{name}")),
            name: name.into(),
            type_name: "function".into(),
            description: String::new(),
            status: Status::New,
            stage,
            anchor: None,
            proposed: Some(ProposedLocation { language: "rust".into(), file: RepoPath::new("src/x.rs").unwrap(), parent: None, symbol: format!("x::{name}") }),
            signature: None,
            schemas: Default::default(),
            spec: None,
            placeholders: vec![],
        });
    }
    let types = arch_graph::TypeSystem::defaults().unwrap();
    let steps = |plan: &Plan, only: Option<&ComponentId>| meta_harness::mcp::service::next_steps(plan, &arch_graph::Graph::merge(arch_graph::Graph::new(), std::slice::from_ref(plan)), &types, only);
    let text = steps(&plan, None);
    assert!(text.contains("rust:x::a (Declared): write its spec") && text.contains("let the user approve the architecture in the cockpit"));
    assert!(text.contains("rust:x::b (ArchApproved): either lay the skeleton in src/x.rs"));
    assert!(text.contains("rust:x::c (PlanApproved): implement it in src/x.rs"));
    assert!(text.contains("rust:x::d (Skeleton): replace the placeholder bodies in src/x.rs"));
    assert!(!text.contains("rust:x::e"), "nothing to do for implemented");
    let only = steps(&plan, Some(&ComponentId::new("rust:x::e")));
    assert!(only.contains("nothing pending"));
    // Dependencies first; work that depends on unfinished parts waits.
    plan.relations.push(arch_graph::Relation::new(arch_graph::RelationKind::Calls, ComponentId::new("rust:x::c"), ComponentId::new("rust:x::a"), arch_graph::Provenance::PLANNED));
    let text = steps(&plan, None);
    assert!(text.contains("rust:x::c (PlanApproved): waits: Implemented needs rust:x::a at Skeleton or later; it is Declared"), "{text}");
    assert!(text.find("rust:x::a").unwrap() < text.find("rust:x::c").unwrap(), "what c calls comes first");
}

// req: P8.4
#[tokio::test]
async fn tools_report_failures_as_errors_the_agent_can_act_on() {
    let (addr, _) = spawn_app().await;
    let mut mcp = Mcp::connect(addr).await;
    for (tool, args, expected) in [
        ("run_tools", json!({ "plan": "nope" }), "not found"),
        ("review", json!({ "plan": "nope", "id": "rust:sample::run" }), "not found"),
        ("review_comments", json!({ "plan": "nope" }), "not found"),
        ("answer_comment", json!({ "plan": "nope", "comment": "c1", "answer": "x" }), "not found"),
        ("reply_comment", json!({ "plan": "nope", "comment": "c1", "text": "x" }), "not found"),
        ("plan_edit", json!({ "plan": "../x", "ops": [] }), "invalid"),
        ("inspect", json!({ "id": "rust:nothing::here" }), "not found"),
    ] {
        let (err, text) = mcp.tool(tool, args).await;
        assert!(err && text.to_lowercase().contains(expected), "{tool}: {text}");
    }
    // A real plan, but a comment that doesn't exist.
    let (err, _) = mcp.tool("plan_create", json!({ "id": "p", "title": "P" })).await;
    assert!(!err);
    let (err, text) = mcp.tool("answer_comment", json!({ "plan": "p", "comment": "c9", "answer": "x" })).await;
    assert!(err && text.contains("c9"), "{text}");
    let (err, text) = mcp.tool("reply_comment", json!({ "plan": "p", "comment": "c9", "text": "x" })).await;
    assert!(err && text.contains("c9"), "{text}");
    // Ready work over REST, in dependency order.
    let (status, body) = common::http(addr, "GET", "/api/arch/plans/p/next?action=build", None).await;
    assert_eq!((status, body.as_str()), (200, "[]"));
    let (status, _) = common::http(addr, "GET", "/api/arch/plans/p/next?action=dance", None).await;
    assert_eq!(status, 400);
}
