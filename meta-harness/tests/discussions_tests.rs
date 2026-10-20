//! Discussions with the agents: what's waiting on one first, one read
//! whole, starting one on a thing by its name (asking the lead, another
//! slot, or the user), replying and answering; the slot asked told of
//! what's new; settling and reopening the user's alone.

mod common;

use std::time::{Duration, Instant};

use arch_graph::PlanId;
use common::{Mcp, fixture_architecture, http, spawn_app_with};
use harness_contracts::SlotId;
use serde_json::{Value, json};

fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

/// Waits (up to ten seconds) until `done` says so.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "{what} didn't happen");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn agents_read_start_reply_and_answer_discussions_and_the_slot_asked_is_told() {
    let arch = fixture_architecture();
    arch.create_plan("p", "Plan", "", arch_graph::PlanKind::Review).unwrap();
    arch.apply(&PlanId::new("p").unwrap(), arch_service::PlanOp::AddItem { title: "Flaky test".into(), text: "It fails one run in ten.".into(), under: None }).unwrap();
    let (addr, state) = spawn_app_with(arch.clone()).await;
    let mut lead = Mcp::lead(addr).await;
    let slots = state.harness.slots.clone();
    // What the lead's session was sent (the fake harness says it back).
    let told = || slots.read(&SlotId::new("lead"), 50).unwrap().messages.into_iter().filter_map(|(_, text)| text.strip_prefix("echo: ").map(str::to_owned)).collect::<Vec<_>>();

    // The user asks the lead about an item: the lead's session is told.
    let item = harness_contracts::Subject::PlanItem { plan: PlanId::new("p").unwrap(), item: "i1".into() };
    let (status, body) = http(addr, "POST", "/api/discussions", Some(&json!({ "subject": item, "text": "Why does it fail?" }).to_string())).await;
    assert_eq!(status, 200, "{body}");
    until("the lead told", || told().iter().any(|t| t.contains("From the user (discussion d1):") && t.contains("Discussion d1 on plan p item i1: the user asks you:\n\nWhy does it fail?"))).await;
    let (status, body) = http(addr, "POST", "/api/discussions", Some(&json!({ "subject": item, "text": "x", "asked": "nobody" }).to_string())).await;
    assert!(status == 400 && body.contains(r#"no agent slot \"nobody\""#), "{body}");

    // What waits on the lead comes first; read whole.
    let (error, listed) = lead.tool("discuss", json!({ "on": "plan p", "text": "Is this list complete?", "ask": "user" })).await;
    assert!(!error && listed.contains("Started d2 · question on plan p (open, asks the user) by lead"), "{listed}");
    let (_, listed) = lead.tool("discussions", json!({ "plan": "p" })).await;
    assert_eq!(listed, "Waiting on you:\n- d1 · question on plan p item i1 (open, asks lead) by the user: Why does it fail?\nThe rest:\n- d2 · question on plan p (open, asks the user) by lead: Is this list complete?", "{listed}");
    let (_, read) = lead.tool("discussion_read", json!({ "id": "d1" })).await;
    assert!(read.contains("Whose move: yours (lead is asked).") && read.contains("plan p item i1 now (") && read.contains("Flaky test (open)\n  It fails one run in ten.") && read.contains("- the user said ("), "{read}");
    let (status, body) = http(addr, "GET", "/api/discussions/d2", None).await;
    assert_eq!((status, parse(&body)["asked"].as_str()), (200, Some("user")), "an agent asks the user");

    // It replies (nothing told back to itself), then answers; the user settles.
    let before = told().len();
    let (error, said) = lead.tool("discussion_reply", json!({ "id": "d1", "text": "Looking." })).await;
    assert!(!error && said.contains("Said in d1 (it's open still)"), "{said}");
    let (error, said) = lead.tool("discussion_answer", json!({ "id": "d1", "text": "A race on the port." })).await;
    assert!(!error && said.contains("Answered d1"), "{said}");
    let (status, _) = http(addr, "POST", "/api/discussions/d1/reopen", Some(r#"{"text":"Which port?"}"#)).await;
    assert_eq!(status, 200);
    until("the lead told it was reopened", || told().iter().any(|t| t.contains("the user reopened it:\n\nWhich port?"))).await;
    assert_eq!(told().len(), before + 1, "only what others said is told");
    let (_, listed) = lead.tool("discussions", json!({ "on": "plan p item i1" })).await;
    assert!(listed.starts_with("Waiting on you:\n- d1"), "{listed}");

    // A change request on the item, answered naming it; only the user settles.
    let (error, _) = lead.tool("discuss", json!({ "on": "plan p item i1", "kind": "change_request", "text": "Mark it decided?" })).await;
    assert!(!error);
    let (error, said) = lead.tool("discussion_answer", json!({ "id": "d3", "text": "No change." })).await;
    assert!(error && said.contains("name what you changed"), "{said}");
    let (error, said) = lead.tool("discussion_answer", json!({ "id": "d3", "text": "Done.", "changed": ["plan p item i1"] })).await;
    assert!(!error, "{said}");
    let (error, _) = lead.tool("discussion_settle", json!({ "id": "d3" })).await;
    assert!(error, "no tool settles: the user does");
    let (error, said) = lead.tool("discuss", json!({ "on": "plan p item i9", "text": "?" })).await;
    assert!(error && said.contains("no item i9"), "{said}");
}
