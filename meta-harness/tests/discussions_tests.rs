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
    arch.apply(
        &PlanId::new("p").unwrap(),
        arch_service::PlanOp::AddItem { title: "Flaky test".into(), text: "It fails one run in ten.".into(), under: None },
    )
    .unwrap();
    let (addr, state) = spawn_app_with(arch.clone()).await;
    let mut lead = Mcp::lead(addr).await;
    let slots = state.harness.slots.clone();
    // What the lead's session was sent (the fake harness says it back).
    let told = || {
        slots
            .read(&SlotId::new("lead"), 50)
            .unwrap()
            .messages
            .into_iter()
            .filter_map(|(_, text)| text.strip_prefix("echo: ").map(str::to_owned))
            .collect::<Vec<_>>()
    };

    // The user asks the lead about an item: the lead's session is told.
    let item = harness_contracts::Subject::PlanItem { plan: PlanId::new("p").unwrap(), item: "i1".into() };
    let (status, body) =
        http(addr, "POST", "/api/discussions", Some(&json!({ "subject": item, "text": "Why does it fail?" }).to_string())).await;
    assert_eq!(status, 200, "{body}");
    until("the lead told", || {
        told().iter().any(|t| {
            t.contains("From the user (discussion d1):")
                && t.contains("Discussion d1 on plan p item i1: the user asks you:\n\nWhy does it fail?")
        })
    })
    .await;
    let (status, body) =
        http(addr, "POST", "/api/discussions", Some(&json!({ "subject": item, "text": "x", "asked": "nobody" }).to_string())).await;
    assert!(status == 400 && body.contains(r#"no agent slot \"nobody\""#), "{body}");

    // What waits on the lead comes first; read whole.
    let (error, listed) = lead.tool("discuss", json!({ "on": "plan p", "text": "Is this list complete?", "ask": "user" })).await;
    assert!(!error && listed.contains("Started d2 · question on plan p (open, asks the user) by lead"), "{listed}");
    let (_, listed) = lead.tool("discussions", json!({ "plan": "p" })).await;
    assert_eq!(
        listed,
        "Waiting on you:\n- d1 · question on plan p item i1 (open, asks lead) by the user: Why does it fail?\nThe rest:\n- d2 · question on plan p (open, asks the user) by lead: Is this list complete?",
        "{listed}"
    );
    let (_, read) = lead.tool("discussion_read", json!({ "id": "d1" })).await;
    assert!(
        read.contains("Whose move: yours (lead is asked).")
            && read.contains("plan p item i1 now (")
            && read.contains("Flaky test (open)\n  It fails one run in ten.")
            && read.contains("- the user said ("),
        "{read}"
    );
    let (status, body) = http(addr, "GET", "/api/discussions/d2", None).await;
    assert_eq!((status, parse(&body)["asked"].as_str()), (200, Some("user")), "an agent asks the user");
    // The user says their answer: the lead, who asked, is told; the user settles it.
    let (status, _) = http(addr, "POST", "/api/discussions/d2/reply", Some(r#"{"text":"Yes, complete."}"#)).await;
    assert_eq!(status, 200);
    until("the lead told the user's answer", || {
        told().iter().any(|t| t.contains("Discussion d2 on plan p: the user said:\n\nYes, complete."))
    })
    .await;
    let (status, body) = http(addr, "POST", "/api/discussions/d2/settle", Some("{}")).await;
    assert_eq!((status, parse(&body)["status"].as_str()), (200, Some("settled")), "{body}");
    until("the lead told it's settled", || told().iter().any(|t| t.contains("Discussion d2 on plan p: the user settled it"))).await;

    // It replies (nothing told back to itself), then answers; the user settles.
    let before = told().len();
    let (error, said) = lead.tool("discussion_reply", json!({ "id": "d1", "text": "Looking." })).await;
    assert!(!error && said.contains("Said in d1 (it's open still)"), "{said}");
    let (error, said) = lead.tool("discussion_answer", json!({ "id": "d1", "text": "A race on the port." })).await;
    assert!(!error && said.contains("Answered d1"), "{said}");
    // What's said is searched (FFF): each line, by discussion and message.
    let (_, found) = lead.tool("discussions", json!({ "find": "race port" })).await;
    assert!(found.contains("d1 message 3 line 1: A race on the port."), "{found}");
    let (status, _) = http(addr, "POST", "/api/discussions/d1/reopen", Some(r#"{"text":"Which port?"}"#)).await;
    assert_eq!(status, 200);
    until("the lead told it was reopened", || told().iter().any(|t| t.contains("the user reopened it:\n\nWhich port?"))).await;
    assert_eq!(told().len(), before + 1, "only what others said is told");
    let (_, listed) = lead.tool("discussions", json!({ "on": "plan p item i1" })).await;
    assert!(listed.starts_with("Waiting on you:\n- d1"), "{listed}");

    // A change request on the item, answered naming it; only the user settles.
    // (An agent can't ask itself.)
    let (error, said) = lead.tool("discuss", json!({ "on": "plan p item i1", "kind": "change_request", "text": "Mark it decided?" })).await;
    assert!(error && said.contains("can't ask yourself"), "{said}");
    let (status, body) = http(
        addr,
        "POST",
        "/api/discussions",
        Some(&json!({ "subject": item, "kind": "change_request", "text": "Mark it decided?" }).to_string()),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (error, said) = lead.tool("discussion_answer", json!({ "id": "d3", "text": "No change." })).await;
    assert!(error && said.contains("name what you changed"), "{said}");
    let (error, said) = lead.tool("discussion_answer", json!({ "id": "d3", "text": "Done.", "changed": ["plan p item i1"] })).await;
    assert!(!error, "{said}");
    let (error, _) = lead.tool("discussion_settle", json!({ "id": "d3" })).await;
    assert!(error, "no tool settles: the user does");
    let (error, said) = lead.tool("discuss", json!({ "on": "plan p item i9", "text": "?" })).await;
    assert!(error && said.contains("no item i9"), "{said}");
}

/// A sub-slot asks the lead (any agent may), never another slot (that's
/// messaging it); only the slot asked answers; what's said back reaches
/// the sub-slot that asked; a tag in it hands on only what the slot told
/// may read (another slot's chat: named, not handed).
#[tokio::test]
async fn a_sub_slot_asks_the_lead_only_the_lead_answers_and_the_asker_is_told() {
    let arch = fixture_architecture();
    arch.create_plan("p", "Plan", "", arch_graph::PlanKind::Review).unwrap();
    let (addr, state) = spawn_app_with(arch.clone()).await;
    let slots = state.harness.slots.clone();
    let mut sub_ids = Vec::new();
    for name in ["helper", "other"] {
        let (status, body) = http(addr, "POST", "/api/slots", Some(&json!({ "instanceId": "fake", "name": name }).to_string())).await;
        assert_eq!(status, 200, "{body}");
        let id = parse(&body)["slotId"].as_str().unwrap().to_owned();
        let (status, body) = http(addr, "POST", &format!("/api/slots/{id}/prompt"), Some(r#"{"text":"hello"}"#)).await;
        assert_eq!(status, 200, "{body}");
        sub_ids.push(SlotId::new(id));
    }
    // Two may share a name: asked by it, which is asked (by its id).
    let (status, body) = http(addr, "POST", "/api/slots", Some(r#"{"instanceId":"fake","name":"helper"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = http(
        addr,
        "POST",
        "/api/discussions",
        Some(&json!({ "subject": { "kind": "plan", "plan": "p" }, "text": "x", "asked": "helper" }).to_string()),
    )
    .await;
    assert!(status == 400 && body.contains("several are named so"), "{body}");
    // Nor is one named as the user is (only the user settles: `by` "user").
    for name in ["user", "You"] {
        let (status, body) = http(addr, "POST", "/api/slots", Some(&json!({ "instanceId": "fake", "name": name }).to_string())).await;
        assert!(status != 200 && body.contains("that's the user"), "{name}: {body}");
    }
    let thread = slots.get(&sub_ids[0]).unwrap().current_thread().cloned().unwrap();
    let mut helper = Mcp::as_session(addr, &thread).await;
    let (error, said) = helper.tool("discuss", json!({ "on": "plan p", "text": "Ask the other one", "ask": "other" })).await;
    assert!(error && said.contains("slots.message"), "asking another slot is messaging it: {said}");
    let (error, said) = helper.tool("discuss", json!({ "on": "plan p", "text": "Is p done?" })).await;
    assert!(!error && said.contains("asks lead"), "{said}");
    let (error, said) = helper.tool("discussion_answer", json!({ "id": "d1", "text": "Yes." })).await;
    assert!(error && said.contains("only it answers"), "only the slot asked answers: {said}");
    let mut lead = Mcp::lead(addr).await;
    // The lead's own chat, tagged (the helper may not read it): the lead
    // was told of d1 in it.
    let mut tagged = (true, String::new());
    for _ in 0..100 {
        tagged = lead.tool("tag", json!({ "tag": "notes", "subject": "chat lead turn 1" })).await;
        if !tagged.0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!tagged.0, "{}", tagged.1);
    // Another slot's chat: the helper can't start one on it, nor read one on it.
    let (error, said) = helper.tool("discuss", json!({ "on": "chat lead turn 1", "text": "?" })).await;
    assert!(error && said.contains("can't read"), "{said}");
    let (error, said) = lead.tool("discuss", json!({ "on": "chat lead turn 1", "text": "Mine?", "ask": "user" })).await;
    assert!(!error, "{said}");
    for tool in ["discussion_read", "discussion_reply"] {
        let (error, said) = helper.tool(tool, json!({ "id": "d2", "text": "x" })).await;
        assert!(error && said.contains("can't read"), "{tool}: {said}");
    }
    let (_, listed) = helper.tool("discussions", json!({})).await;
    assert!(!listed.contains("d2"), "not listed to it: {listed}");
    let (error, said) = lead.tool("discussion_answer", json!({ "id": "d1", "text": "Yes, done: see #notes" })).await;
    assert!(!error, "{said}");
    let told = || {
        slots
            .read(&sub_ids[0], 50)
            .unwrap()
            .messages
            .into_iter()
            .filter_map(|(_, text)| text.strip_prefix("echo: ").map(str::to_owned))
            .collect::<Vec<_>>()
    };
    until("the helper told the answer", || {
        told().iter().any(|t| t.contains("Discussion d1 on plan p: lead answered it:\n\nYes, done: see #notes"))
    })
    .await;
    // Asked by the user, the helper sees it waiting on it (by its slot, not its name).
    let (status, body) = http(
        addr,
        "POST",
        "/api/discussions",
        Some(&json!({ "subject": { "kind": "plan", "plan": "p" }, "text": "Your view?", "asked": sub_ids[0].as_str() }).to_string()),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let asked_helper = parse(&body)["id"].as_str().unwrap().to_owned();
    let (_, listed) = helper.tool("discussions", json!({})).await;
    assert!(listed.starts_with(&format!("Waiting on you:\n- {asked_helper}")) && listed.contains("asks helper"), "{listed}");
    // A slot not in it (and no slots.message) can't say something in it: that would prompt the helper.
    let other_thread = slots.get(&sub_ids[1]).unwrap().current_thread().cloned().unwrap();
    let mut other = Mcp::as_session(addr, &other_thread).await;
    let (error, said) = other.tool("discussion_reply", json!({ "id": asked_helper, "text": "hi" })).await;
    assert!(error && said.contains("slots.message"), "{said}");
    let answer = told().into_iter().find(|t| t.contains("lead answered it")).unwrap();
    assert!(!answer.contains("Is p done?\n"), "an agent's text is plain: its #notes is text, nothing handed on: {answer}");
    // Another slot's chat isn't the helper's to read, by name either.
    let (error, said) = helper.tool("thing_read", json!({ "thing": "chat lead" })).await;
    assert!(error && said.contains("can't read"), "{said}");
}
