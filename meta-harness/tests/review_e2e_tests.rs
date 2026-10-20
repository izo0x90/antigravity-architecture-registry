//! The review through the server: a plan's parts as things, discussions on
//! them holding approvals, a plan's old comments moved into discussions,
//! review items.

mod common;

use arch_graph::{ComponentId, PlanId, RelationKind};
use arch_service::PlanOp;
use common::{Mcp, fixture_architecture, http, spawn_app_with};
use serde_json::{Value, json};

fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

fn id(s: &str) -> ComponentId {
    ComponentId::new(format!("rust:{s}"))
}

/// Comments a plan's file held before discussions did move in once, as
/// the plan is read: each anchor its part of the plan, each comment a
/// discussion as it stood (its replies, its answer and what that changed);
/// the file is written without them; approvals wait on them the same.
#[tokio::test]
async fn a_plans_comments_become_discussions_once_and_approvals_wait_on_them_the_same() {
    let arch = fixture_architecture();
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
    arch.apply(&plan, PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::run"), target: id("sample::validate"), order: None, ports: None, note: None, expects: None }).unwrap();
    // The file as an earlier build wrote it: a comment of every anchor and status.
    let file = arch.root().join(".nah/plans/v.json");
    let mut held: Value = parse(&std::fs::read_to_string(&file).unwrap());
    let user = json!({ "kind": "user", "name": "izo" });
    let agent = json!({ "kind": "agent", "session": "lead" });
    held["comments"] = json!([
        { "id": "c1", "anchor": { "on": "component", "id": "rust:sample::validate" }, "author": user, "text": "Is it pure?", "at": "2026-10-07T00:00:00Z", "status": "open",
          "replies": [{ "author": agent, "text": "Pure, yes.", "at": "2026-10-07T00:01:00Z" }] },
        { "id": "c2", "anchor": { "on": "section", "id": "rust:sample::validate", "section": "overview" }, "kind": "change_request", "author": user, "text": "Say what it rejects.", "at": "2026-10-07T00:02:00Z", "status": "answered",
          "answer": { "author": agent, "text": "It says so now.", "changes": [{ "change": "component_changed", "id": "rust:sample::validate", "fields": ["description"] }], "at": "2026-10-07T00:04:00Z" },
          "replies": [{ "author": user, "text": "Empty only?", "at": "2026-10-07T00:03:00Z" }] },
        { "id": "c3", "anchor": { "on": "relation", "kind": "calls", "source": "rust:sample::run", "target": "rust:sample::validate" }, "author": user, "text": "Call it first.", "at": "2026-10-07T00:05:00Z", "status": "resolved" },
        { "id": "c4", "anchor": { "on": "plan" }, "author": user, "text": "Hold everything.", "at": "2026-10-07T00:06:00Z" }
    ]);
    std::fs::write(&file, serde_json::to_string_pretty(&held).unwrap()).unwrap();
    arch.refresh(std::slice::from_ref(&file)).unwrap();
    let (addr, _state) = spawn_app_with(arch.clone()).await;

    let (_, body) = http(addr, "GET", "/api/discussions", None).await;
    let all = parse(&body);
    let brief: Vec<(String, String, String, String, usize)> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["id"].as_str().unwrap().to_owned(), d["on"]["what"].as_str().unwrap().to_owned(), d["kind"].as_str().unwrap().to_owned(), d["status"].as_str().unwrap().to_owned(), d["said"].as_array().unwrap().len()))
        .collect();
    let row = |id: &str, what: &str, kind: &str, status: &str, said| (id.to_owned(), what.to_owned(), kind.to_owned(), status.to_owned(), said);
    assert_eq!(
        brief,
        [
            row("d1", "plan v component rust:sample::validate", "question", "open", 2),
            row("d2", "plan v component rust:sample::validate section overview", "change_request", "answered", 3),
            row("d3", "plan v relation rust:sample::run calls rust:sample::validate", "question", "settled", 1),
            row("d4", "plan v", "question", "open", 1),
        ],
        "{body}"
    );
    let d2 = &all[1];
    let said: Vec<(&str, &str, &str)> = d2["said"].as_array().unwrap().iter().map(|s| (s["by"].as_str().unwrap(), s["kind"].as_str().unwrap(), s["text"].as_str().unwrap())).collect();
    assert_eq!(said, [("user", "message", "Say what it rejects."), ("user", "message", "Empty only?"), ("agent lead", "answer", "It says so now.")], "in the order said");
    assert_eq!(d2["said"][2]["changed"][0]["what"], "plan v component rust:sample::validate", "what the answer changed, named");
    assert_eq!(d2["said"][0]["atMs"], 1_791_331_320_000u64, "when it was said");
    assert!(parse(&std::fs::read_to_string(&file).unwrap()).get("comments").is_none(), "the file without them");

    // Approval waits on them as it did: everything not settled on it, and the plan's.
    let (status, body) = http(addr, "POST", "/api/arch/plans/v/components/rust:sample::validate/approve", Some(r#"{"to":"arch_approved","by":"izo"}"#)).await;
    assert_eq!(status, 400);
    assert!(body.contains("d1 on plan v component rust:sample::validate (open") && body.contains("d2 on plan v component rust:sample::validate section overview (answered") && body.contains("d4 on plan v (open"), "{body}");
    assert!(!body.contains("d3"), "a settled one doesn't: {body}");

    // Read again: nothing comes in twice.
    arch.refresh(std::slice::from_ref(&file)).unwrap();
    let (_, body) = http(addr, "GET", "/api/discussions", None).await;
    assert_eq!(parse(&body).as_array().map(Vec::len), Some(4));
}

/// A plan's parts are things notes are on: tagged by their names, read as
/// the plan has them now, nothing of them logged; a part taken out of the
/// plan is gone.
#[tokio::test]
async fn a_plans_parts_are_tagged_by_name_and_read_as_the_plan_has_them() {
    let arch = fixture_architecture();
    let plan = PlanId::new("v").unwrap();
    arch.create_plan("v", "Validate", "Checks what comes in.", arch_graph::PlanKind::Change).unwrap();
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
    let (addr, state) = spawn_app_with(arch.clone()).await;
    let mut lead = Mcp::lead(addr).await;
    for subject in ["plan v", "plan v component rust:sample::validate", "plan v component rust:sample::validate section signature", "plan v relation rust:sample::run calls rust:sample::validate"] {
        let (error, said) = lead.tool("tag", json!({ "tag": "review", "subject": subject })).await;
        assert!(!error, "{subject}: {said}");
    }
    let (_, read) = lead.tool("tag_read", json!({ "tag": "review" })).await;
    for (n, what, says) in [
        (1, "plan v", "Validate (1 components)\nChecks what comes in."),
        (2, "plan v component rust:sample::validate", "function validate (New, Declared"),
        (3, "plan v component rust:sample::validate section signature", "validate(input: &str) -> bool"),
        (4, "plan v relation rust:sample::run calls rust:sample::validate", "rust:sample::run calls rust:sample::validate"),
    ] {
        assert!(read.contains(&format!("{n}. {what} (tagged by lead; as it is now):\n{says}")), "{n}: {read}");
    }
    assert!(state.tags.tags.find("review").unwrap().on.iter().all(|t| t.kept.is_none()), "plans last: nothing logged");
    for wrong in ["plan nope", "plan v component rust:sample::nope", "plan v component rust:sample::validate section nope", "plan v relation rust:sample::run calls rust:sample::nope", "plan v item i9"] {
        let (error, said) = lead.tool("tag", json!({ "tag": "review", "subject": wrong })).await;
        assert!(error, "{wrong}: {said}");
    }
    // Nor sent as what it is, by the cockpit: a part that isn't there is refused.
    for wrong in [json!({ "kind": "plan", "plan": "nope" }), json!({ "kind": "plan_section", "plan": "v", "id": "rust:sample::validate", "section": "nope" }), json!({ "kind": "plan_item", "plan": "v", "item": "i9" })] {
        let (status, body) = http(addr, "POST", "/api/tags", Some(&json!({ "tag": "review", "subject": wrong }).to_string())).await;
        assert_eq!(status, 400, "{wrong}: {body}");
    }
    // Taken out of the plan: gone.
    arch.apply(&plan, PlanOp::Unrelate { kind: RelationKind::Calls, source: id("sample::run"), target: id("sample::validate") }).unwrap();
    let (_, read) = lead.tool("tag_read", json!({ "tag": "review" })).await;
    assert!(read.contains("4. plan v relation rust:sample::run calls rust:sample::validate (tagged by lead): gone, nothing kept of it"), "{read}");
}

/// A review, through the agents' tools: made, its items added, nested,
/// moved and set, listed nested with their states; kept in the plan's
/// file as they were made.
#[tokio::test]
async fn a_review_is_made_and_gone_through_with_the_plan_tools() {
    let arch = fixture_architecture();
    let (addr, _state) = spawn_app_with(arch.clone()).await;
    let mut lead = Mcp::lead(addr).await;
    let (error, said) = lead.tool("plan_create", json!({ "id": "findings", "title": "Findings", "kind": "review" })).await;
    assert!(!error && said.contains("Created review findings"), "{said}");
    let ops = json!([
        { "op": "add_item", "title": "Decide", "text": "What to decide." },
        { "op": "add_item", "title": "FFF skipped", "text": "tag_find reads each log.", "under": "i1" },
        { "op": "add_item", "title": "Open" },
        { "op": "add_item", "title": "Turns renumbered", "under": "i3" },
        { "op": "move_item", "id": "i4", "under": "i1" },
        { "op": "set_item", "id": "i2", "state": "decided" }
    ]);
    let (error, said) = lead.tool("plan_edit", json!({ "plan": "findings", "ops": ops })).await;
    assert!(!error && said.contains("4 items (i1 open, i2 decided, i3 open, i4 open)"), "{said}");
    let (error, said) = lead.tool("plan_edit", json!({ "plan": "findings", "ops": [{ "op": "add_component", "name": "x", "type": "function", "parent": "rust:sample" }] })).await;
    assert!(error && said.contains("is a review"), "{said}");
    let (_, listed) = lead.tool("plan_list", json!({})).await;
    assert!(listed.contains("findings — Findings (review)\n  i1 [open] Decide\n    i2 [decided] FFF skipped\n    i4 [open] Turns renumbered\n  i3 [open] Open\n"), "{listed}");
    let plan = arch.plan(&PlanId::new("findings").unwrap()).unwrap();
    assert_eq!(plan.items.iter().map(|i| (i.id.0.as_str(), i.title.as_str())).collect::<Vec<_>>(), [("i1", "Decide"), ("i2", "FFF skipped"), ("i3", "Open"), ("i4", "Turns renumbered")]);
}

/// A discussion on a plan's part holds its component's approval until the
/// user settles it; a change request's answer names a change made to it
/// since the user's review.
#[tokio::test]
async fn a_discussion_on_a_plans_part_holds_its_approval_until_the_user_settles_it() {
    let arch = fixture_architecture();
    let plan = PlanId::new("w").unwrap();
    arch.create_plan("w", "Walk", "", arch_graph::PlanKind::Change).unwrap();
    let add = |description: &str| PlanOp::AddComponent {
        name: "walk".into(),
        type_name: "function".into(),
        description: description.into(),
        parent: Some(id("sample")),
        file: None,
        signature: Some(arch_graph::Signature { params: vec![], returns: None, qualifiers: vec![] }),
        schemas: None,
        spec: None,
    };
    let calls = || PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::run"), target: id("sample::walk"), order: None, ports: None, note: None, expects: None };
    arch.apply(&plan, add("Walks.")).unwrap();
    arch.apply(&plan, calls()).unwrap();
    let (addr, _state) = spawn_app_with(arch.clone()).await;
    let (status, _) = http(addr, "POST", "/api/arch/plans/w/reviewed", Some(r#"{"by":"izo"}"#)).await;
    assert_eq!(status, 200);
    let section = harness_contracts::Subject::PlanSection { plan: plan.clone(), id: id("sample::walk"), section: "overview".into() };
    let (status, body) = http(addr, "POST", "/api/discussions", Some(&json!({ "subject": section, "kind": "change_request", "text": "Say where it walks." }).to_string())).await;
    assert_eq!(status, 200, "{body}");
    let started = parse(&body);
    assert_eq!((started["id"].as_str(), started["on"]["what"].as_str(), started["asked"].as_str()), (Some("d1"), Some("plan w component rust:sample::walk section overview"), Some("lead")));
    let approve = || http(addr, "POST", "/api/arch/plans/w/components/rust:sample::walk/approve", Some(r#"{"to":"arch_approved","by":"izo"}"#));
    let (status, body) = approve().await;
    assert!(status == 400 && body.contains("d1 on plan w component rust:sample::walk section overview (open"), "{status} {body}");

    let answer = json!({ "text": "Done.", "changed": ["plan w component rust:sample::walk"] }).to_string();
    let (status, body) = http(addr, "POST", "/api/discussions/d1/answer", Some(&answer)).await;
    assert!(status == 400 && body.contains("hasn't changed"), "a change since the review: {body}");
    arch.apply(&plan, PlanOp::Remove { id: id("sample::walk") }).unwrap();
    arch.apply(&plan, add("Walks the tree.")).unwrap();
    arch.apply(&plan, calls()).unwrap();
    let (status, body) = http(addr, "POST", "/api/discussions/d1/answer", Some(&answer)).await;
    assert_eq!((status, parse(&body)["status"].as_str()), (200, Some("answered")), "{body}");
    let (status, body) = approve().await;
    assert!(status == 400 && body.contains("answered: settle or reopen it"), "{body}");
    let (status, body) = http(addr, "POST", "/api/discussions/d1/settle", Some("{}")).await;
    assert_eq!((status, parse(&body)["status"].as_str()), (200, Some("settled")), "{body}");
    let (status, body) = approve().await;
    assert_eq!(status, 200, "{body}");

    let (_, body) = http(addr, "POST", "/api/discussions/on", Some(&json!({ "subject": harness_contracts::Subject::PlanComponent { plan: plan.clone(), id: id("sample::walk") } }).to_string())).await;
    assert_eq!(parse(&body).as_array().map(Vec::len), Some(1), "the discussions on a component: its sections' too: {body}");
}
