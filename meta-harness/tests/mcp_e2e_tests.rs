//! The agent's architecture tools through the real MCP endpoint (SSE stream +
//! message posts), on a copy of the Rust fixture repo.

mod common;

use common::{Mcp, spawn_app};
use serde_json::json;

#[tokio::test]
async fn every_tool_works_through_the_mcp_endpoint() {
    let (addr, state) = spawn_app().await;
    let mut mcp = Mcp::lead(addr).await;
    let init = mcp.rpc("initialize", json!({})).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "nah-architecture");
    let tools = mcp.rpc("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in ["ingest", "list_types", "search", "inspect", "view", "changes", "navigate", "plan_list", "plan_create", "plan_import", "plan_edit", "plan_validate", "contract", "verify", "run_tools", "review"] {
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
    assert!(!err && text.contains("rust:sample::store::Store::save [function · src/store.rs:10 Existing/Implemented] save"), "{text}");
    let (err, text) = mcp.tool("search", json!({ "query": "to_uppercase", "kind": "text" })).await;
    assert!(!err && text.contains("src/util.rs:2 · in shout: s.to_uppercase()  (card rust:sample::util::shout Existing/Implemented)"), "{text}");
    let (err, text) = mcp.tool("search", json!({ "query": "util", "kind": "files" })).await;
    assert!(!err && text.contains("src/util.rs"), "{text}");
    let (err, text) = mcp.tool("search", json!({ "query": "zzz" })).await;
    assert!(!err && text.contains("No matches"));

    let (err, text) = mcp.tool("inspect", json!({ "id": "rust:sample::run" })).await;
    assert!(!err && text.contains("Store::new()"), "includes source: {text}");
    let (err, _) = mcp.tool("inspect", json!({ "id": "rust:nope" })).await;
    assert!(err);

    let (err, text) = mcp.tool("view", json!({ "view": "contains", "root": "rust:sample::store" })).await;
    assert!(!err && text.contains("contains (code)") && text.contains("Store [class"), "{text}");
    let (err, _) = mcp.tool("view", json!({ "view": "teleport" })).await;
    assert!(err);

    let (mut events, _) = (state.harness.hub.subscribe().1, ());
    let (err, _) = mcp.tool("navigate", json!({ "view": "calls", "select": "rust:sample::run" })).await;
    assert!(!err);
    let event = events.recv().await.unwrap();
    assert!(matches!(event, harness_contracts::ClientEvent::Architecture { event: arch_graph::ArchEvent::ViewRequested { by: arch_graph::Requester::Agent, .. } }));
    let (err, _) = mcp.tool("navigate", json!({ "select": "rust:ghost" })).await;
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
    state.project.arch.approve(&arch_graph::PlanId::new("val").unwrap(), &arch_graph::ComponentId::new("rust:sample::validate"), arch_graph::Stage::ArchApproved, &arch_service::Actor::User { name: "izo".into() }).unwrap();
    let (err, text) = mcp.tool("verify", json!({ "plan": "val", "id": "rust:sample::validate", "goal": "skeleton" })).await;
    assert!(!err && text.contains("Not yet") && text.contains("isn't in the code"), "{text}");
    let lib = state.project.arch.root().join("src/lib.rs");
    let mut code = std::fs::read_to_string(&lib).unwrap();
    code.push_str("\npub fn validate(input: &str) -> bool {\n    todo!()\n}\n");
    std::fs::write(&lib, &code).unwrap();
    let (err, text) = mcp.tool("verify", json!({ "plan": "val", "id": "rust:sample::validate", "goal": "skeleton" })).await;
    assert!(!err && text.contains("is now Skeleton") && text.contains("replace the placeholder bodies"), "{text}");
    std::fs::write(&lib, code.replace("    todo!()\n", "    !input.is_empty()\n")).unwrap();
    // Implementing needs the user's plan approval (after the skeleton is fine).
    let (err, text) = mcp.tool("verify", json!({ "plan": "val", "id": "rust:sample::validate", "goal": "implemented" })).await;
    assert!(!err && text.contains("approves its plan"), "{text}");
    state.project.arch.approve(&arch_graph::PlanId::new("val").unwrap(), &arch_graph::ComponentId::new("rust:sample::validate"), arch_graph::Stage::PlanApproved, &arch_service::Actor::User { name: "izo".into() }).unwrap();
    let (err, text) = mcp.tool("verify", json!({ "plan": "val", "id": "rust:sample::validate", "goal": "implemented" })).await;
    assert!(!err && text.contains("is now Implemented") && text.contains("nothing pending"), "{text}");
    let (err, _) = mcp.tool("verify", json!({ "plan": "val", "id": "rust:sample::validate", "goal": "declared" })).await;
    assert!(err);
    let (err, _) = mcp.tool("teleport", json!({})).await;
    assert!(err);
}

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
    let steps = |plan: &Plan, only: Option<&ComponentId>| meta_harness::mcp::text::next_steps(plan, &arch_graph::Graph::merge(arch_graph::Graph::default(), std::slice::from_ref(plan)), &types, only);
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

#[tokio::test]
async fn tools_report_failures_as_errors_the_agent_can_act_on() {
    let (addr, _) = spawn_app().await;
    let mut mcp = Mcp::lead(addr).await;
    for (tool, args, expected) in [
        ("run_tools", json!({ "plan": "nope" }), "not found"),
        ("review", json!({ "plan": "nope", "id": "rust:sample::run" }), "not found"),
        ("plan_edit", json!({ "plan": "../x", "ops": [] }), "invalid"),
        ("inspect", json!({ "id": "rust:nothing::here" }), "not found"),
    ] {
        let (err, text) = mcp.tool(tool, args).await;
        assert!(err && text.to_lowercase().contains(expected), "{tool}: {text}");
    }
    let (err, _) = mcp.tool("plan_create", json!({ "id": "p", "title": "P" })).await;
    assert!(!err);
    // Ready work over REST, in dependency order.
    let (status, body) = common::http(addr, "GET", "/api/arch/plans/p/next?action=build", None).await;
    assert_eq!((status, body.as_str()), (200, "[]"));
    let (status, _) = common::http(addr, "GET", "/api/arch/plans/p/next?action=dance", None).await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn notifications_are_taken_but_never_answered() {
    let (addr, _) = spawn_app().await;
    let mut mcp = Mcp::lead(addr).await;
    assert_eq!(mcp.notify("notifications/initialized").await, 202);
    assert_eq!(mcp.notify("notifications/unknown").await, 202, "unknown notifications aren't errors either");
    // Nothing was put on the stream for them: the next message answers the next request.
    let next = mcp.rpc_next("ping").await;
    assert!(next["id"].is_number() && next.get("error").is_none(), "{next}");
}

#[tokio::test]
async fn inspect_opens_any_card_a_view_shows_and_says_how_links_are_known() {
    let (addr, _state) = spawn_app().await;
    let mut mcp = Mcp::lead(addr).await;
    mcp.rpc("initialize", json!({})).await;
    let (err, text) = mcp.tool("ingest", json!({})).await;
    assert!(!err, "{text}");
    // A data type's card, and a flow's start and end.
    let (err, text) = mcp.tool("inspect", json!({ "id": "type:Store" })).await;
    assert!(!err && text.contains("\"rust:sample::store::Store\""), "{text}");
    for end in ["start", "end"] {
        let (err, text) = mcp.tool("inspect", json!({ "id": format!("{end}:rust:sample::run") })).await;
        assert!(!err && text.contains("\"rust:sample::run\""), "{end}: {text}");
    }
    // No language server here: the calls it found were matched by name.
    let (err, text) = mcp.tool("inspect", json!({ "id": "rust:sample::run" })).await;
    assert!(!err && text.contains("\"known\": \"matched by name, unconfirmed\""), "{text}");
    let (err, text) = mcp.tool("inspect", json!({ "id": "type:Nothing" })).await;
    assert!(err && text.contains("no data type"), "{text}");
}

/// The next view request the server sends the user.
async fn next_view(events: &mut tokio::sync::broadcast::Receiver<harness_contracts::ClientEvent>) -> arch_graph::ArchEvent {
    loop {
        if let harness_contracts::ClientEvent::Architecture { event: event @ arch_graph::ArchEvent::ViewRequested { .. } } = events.recv().await.unwrap() {
            return event;
        }
    }
}

#[tokio::test]
async fn the_agent_moves_the_users_view_to_a_plan_and_back_to_code() {
    let (addr, state) = spawn_app().await;
    let mut mcp = Mcp::lead(addr).await;
    mcp.rpc("initialize", json!({})).await;
    assert!(!mcp.tool("ingest", json!({})).await.0);
    assert!(!mcp.tool("plan_create", json!({ "id": "val", "title": "Validate input" })).await.0);
    // The user's graph, as the cockpit holds it.
    use cockpit_ui::buffer::Buffer;
    let mut graph = cockpit_ui::graph::GraphState::default();
    let mut events = state.harness.hub.subscribe().1;
    let (err, text) = mcp.tool("navigate", json!({ "view": "calls", "source": "plan:val" })).await;
    assert!(!err, "{text}");
    graph.on_event(&next_view(&mut events).await);
    assert_eq!(graph.nav.request().source, arch_graph::Source::Plan(arch_graph::PlanId::new("val").unwrap()), "the user sees the plan");
    let (err, text) = mcp.tool("navigate", json!({ "source": "code" })).await;
    assert!(!err, "{text}");
    graph.on_event(&next_view(&mut events).await);
    let request = graph.nav.request();
    assert_eq!((request.source, request.view), (arch_graph::Source::Code, arch_graph::tree::ViewKind::Calls), "back to the code, in the same view");
}

#[tokio::test]
async fn reads_take_a_source_over_rest_and_mcp() {
    let (addr, _state) = spawn_app().await;
    let mut mcp = Mcp::lead(addr).await;
    mcp.rpc("initialize", json!({})).await;
    assert!(!mcp.tool("ingest", json!({})).await.0);
    assert!(!mcp.tool("plan_create", json!({ "id": "val", "title": "Validate input" })).await.0);
    let ops = json!([{ "op": "add_component", "name": "validate", "type": "function", "parent": "rust:sample", "description": "Rejects empty input." }]);
    assert!(!mcp.tool("plan_edit", json!({ "plan": "val", "ops": ops })).await.0);
    // MCP: the plan's item is there with the plan as the source, not in the code.
    let (err, text) = mcp.tool("inspect", json!({ "id": "rust:sample::validate", "source": "plan:val" })).await;
    assert!(!err && text.contains("Rejects empty input."), "{text}");
    let (err, _) = mcp.tool("inspect", json!({ "id": "rust:sample::validate", "source": "code" })).await;
    assert!(err, "not in the code");
    let (err, text) = mcp.tool("view", json!({ "view": "contains", "root": "rust:sample", "source": "plan:val" })).await;
    assert!(!err && text.contains("validate"), "{text}");
    // The old argument is refused, not ignored.
    let (err, text) = mcp.tool("inspect", json!({ "id": "rust:sample::validate", "plan": "val" })).await;
    assert!(err && text.contains("unknown field `plan`"), "{text}");
    let (err, text) = mcp.tool("search", json!({ "query": "validate", "source": "val" })).await;
    assert!(err && text.contains("use \"code\" or \"plan:<id>\""), "{text}");
    // REST: the same, as a query parameter.
    let (status, body) = common::http(addr, "GET", "/api/graph/details/rust:sample::validate?source=plan:val", None).await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = common::http(addr, "GET", "/api/graph/details/rust:sample::validate?source=code", None).await;
    assert_eq!(status, 404);
    let (status, body) = common::http(addr, "GET", "/api/arch/components/rust:sample::validate?source=plan:val", None).await;
    assert!(status == 200 && body.contains("Rejects empty input."), "{status} {body}");
    let (status, body) = common::http(addr, "GET", "/api/search?q=validate&source=plan:val", None).await;
    assert!(status == 200 && body.contains("rust:sample::validate"), "{status} {body}");
    // The old argument is refused over REST too, never quietly ignored.
    for old in ["/api/arch/components/rust:sample::validate?plan=val", "/api/graph/details/rust:sample::validate?plan=val", "/api/search?q=validate&plan=val", "/api/arch/file?path=src/lib.rs&plan=val"] {
        let (status, body) = common::http(addr, "GET", old, None).await;
        assert!(status == 400 && body.contains("plan"), "{old}: {status} {body}");
    }
}

/// Each tool names the capability it needs: a role sees only the tools it
/// may use, and a call it may not make is refused with what it lacks.
#[tokio::test]
async fn each_role_sees_and_may_use_only_its_tools() {
    use meta_harness::mcp::McpCaller;
    use nah_access::Principal;
    let (_, state) = spawn_app().await;
    let service = &state.project.mcp_state.service;
    let as_role = |role: &str| McpCaller { principal: Principal::agent(role), slot: None, label: role.into() };
    let names = |who: &Principal| service.tools_for(who).iter().map(|t| t.name.clone()).collect::<Vec<_>>();
    let reads = ["ingest", "list_types", "search", "inspect", "view", "changes", "plan_list", "contract", "cockpit_read", "cockpit_actions", "terminals", "terminal_read", "tags", "tag_read", "tag_find"];
    assert!(names(&Principal::agent("external")).is_empty(), "an agent with no key: nothing, unless a project grants it");
    // The slots' tools come before the terminals', the tags' after them,
    // then the discussions'.
    let mut sub = reads.map(String::from).to_vec();
    sub.splice(10..10, ["tell_lead".into(), "tell_user".into()]);
    sub.extend(["tag", "untag", "tag_edit", "thing_read", "discussions", "discussion_read", "discuss", "discussion_reply", "discussion_answer"].map(String::from));
    assert_eq!(names(&Principal::agent("sub")), sub, "a sub-slot reads, reports to the lead and the user, tags and discusses");
    let lead = names(&Principal::agent("lead"));
    assert!(lead.len() == 42 && !lead.contains(&"tell_lead".to_owned()), "the lead has every tool but reporting to itself and typing into terminals: {lead:?}");
    assert!(!lead.contains(&"terminal_type".to_owned()), "typing into the user's terminals is no role's unless granted");
    assert!(names(&Principal::Anonymous).is_empty());

    let err = service.call("navigate", json!({ "view": "calls" }), &as_role("sub")).await.unwrap_err().to_string();
    assert_eq!(err, "not allowed: this session's role (sub) can't use navigate (it needs view.drive)");
    let err = service.call("plan_create", json!({ "id": "x", "title": "X" }), &as_role("external")).await.unwrap_err().to_string();
    assert!(err.contains("(it needs plan.edit)"), "{err}");
    let err = service.call("terminal_type", json!({ "terminal": "t1", "text": "ls" }), &as_role("lead")).await.unwrap_err().to_string();
    assert!(err.contains("(it needs shell.drive)"), "{err}");
    assert!(service.call("list_types", json!({}), &as_role("sub")).await.is_ok(), "what it may use works");
    assert!(service.call("no_such_tool", json!({}), &as_role("lead")).await.unwrap_err().to_string().contains("tool 'no_such_tool'"));
}

/// The lead runs its sub-slots through its tools; a sub-slot reports back
/// and can't reach other slots.
#[tokio::test]
async fn the_lead_runs_sub_slots_and_a_sub_slot_reports_back() {
    let (addr, state) = spawn_app().await;
    let mut lead = Mcp::lead(addr).await;
    let (err, text) = lead.tool("slot_list", json!({})).await;
    assert!(!err && text.starts_with("No sub-slots."), "{text}");
    let (err, text) = lead.tool("slot_start", json!({ "harness": "fake", "name": "User" })).await;
    assert!(err && text.contains("that's the user"), "no agent passes for the user: {text}");
    let (err, text) = lead.tool("slot_start", json!({ "harness": "fake", "name": "helper", "mode": "full-access" })).await;
    assert!(!err && text.starts_with("Started helper (id sub-"), "{text}");
    let sub = state.harness.slots.list().into_iter().find(|s| s.name == "helper").unwrap();
    assert_eq!(sub.policy.runtime_mode, harness_contracts::RuntimeMode::FullAccess);
    let (err, text) = lead.tool("slot_send", json!({ "slot": "helper", "text": "count the files", "wait_secs": 5 })).await;
    assert!(!err && text.contains("helper answered:") && text.contains("From the lead (lead):"), "{text}");
    let (err, text) = lead.tool("slot_send", json!({ "slot": "lead", "text": "hi" })).await;
    assert!(err && text.contains("that's you"), "{text}");

    let thread = sub.current_thread().cloned().or_else(|| state.harness.slots.get(&sub.id).and_then(|s| s.current_thread().cloned())).expect("its session runs");
    let mut helper = Mcp::as_session(addr, &thread).await;
    let (err, text) = helper.tool("tell_lead", json!({ "text": "done: 3 files" })).await;
    assert!(!err && text.starts_with("Delivered"), "{text}");
    let (refused, text) = helper.tool("slot_send", json!({ "slot": "helper", "text": "hi" })).await;
    assert!(refused && text.contains("role (sub) can't use slot_send"), "{text}");
    let (err, _) = lead.tool("slot_close", json!({ "slot": "helper" })).await;
    assert!(!err && state.harness.slots.get(&sub.id).is_none());
}
