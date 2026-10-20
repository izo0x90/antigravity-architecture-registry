//! Slots on the server: the lead and sub-slots, their sessions and keys,
//! messages between them, and what survives a restart, against the fake
//! harness.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{FAKE, Mcp, given_tools, spawn_app};
use harness_contracts::{InstanceId, McpTransport, NativeId, SessionKey, Slot, SlotId, SlotSession, SlotStatus, Starter, ThreadId};
use harness_core::slots::{HeldReport, SlotStore};
use harness_core::{Delivery, Open, Origin, Reported, Sent, SlotManager, SlotSettings};
use serde_json::json;

fn fake() -> SlotSettings {
    SlotSettings { instance: Some(InstanceId::new(FAKE)), ..SlotSettings::default() }
}

/// The key a session's agent was given.
fn key_of(thread: &ThreadId) -> String {
    let ours = given_tools(thread).into_iter().find(|s| s.name == "nah-architecture").unwrap();
    let McpTransport::Sse { url, .. } = ours.transport else { panic!() };
    url.split_once("?key=").unwrap().1.to_owned()
}

#[tokio::test]
async fn the_lead_is_always_there_and_sub_slots_come_and_go_without_touching_it() {
    let (_, state) = spawn_app().await;
    let slots = &state.harness.slots;
    let lead = slots.lead();
    assert_eq!(slots.list().len(), 1, "the lead, from the start");
    assert!(slots.get(&lead).unwrap().instance_id.is_none(), "its harness is chosen before its first session");
    assert!(slots.open_session(&lead, Open::New { resume: None }).await.unwrap_err().to_string().contains("choose one first"));

    slots.set_harness(&lead, InstanceId::new(FAKE)).await.unwrap();
    let lead_thread = slots.open_session(&lead, Open::New { resume: None }).await.unwrap().thread;
    let sub = slots.create("sub", None, fake(), Starter::Lead).await.unwrap();
    assert_eq!(slots.get(&sub).unwrap().name, "fake 1", "named after its harness");
    let first = slots.open_session(&sub, Open::New { resume: None }).await.unwrap();
    let second = slots.open_session(&sub, Open::New { resume: None }).await.unwrap();
    assert_eq!(slots.get(&sub).unwrap().current, Some(second.session.clone()), "the newest is current");
    slots.switch(&sub, &first.session).await.unwrap();
    assert_eq!(slots.get(&sub).unwrap().current, Some(first.session.clone()));
    slots.close_session(&sub, &first.session).await.unwrap();
    assert!(state.harness.sessions.get(&first.thread).is_err(), "its session ended");
    assert_eq!(slots.get(&sub).unwrap().current, Some(second.session.clone()), "another becomes current");
    slots.close(&sub).await.unwrap();
    assert!(slots.get(&sub).is_none() && state.harness.sessions.get(&second.thread).is_err(), "the slot and its sessions are gone");
    assert!(state.harness.sessions.get(&lead_thread).is_ok(), "the lead's session never moved");
    assert!(slots.close(&lead).await.unwrap_err().to_string().contains("the lead slot stays"));
    assert!(slots.set_harness(&lead, InstanceId::new("other")).await.unwrap_err().to_string().contains("close its sessions first"));
}

#[tokio::test]
async fn each_session_has_its_own_key_which_says_whose_calls_they_are() {
    let (addr, state) = spawn_app().await;
    let slots = &state.harness.slots;
    let sub = slots.create("sub", Some("helper".into()), fake(), Starter::User).await.unwrap();
    let one = slots.open_session(&sub, Open::New { resume: None }).await.unwrap();
    let two = slots.open_session(&sub, Open::New { resume: None }).await.unwrap();
    let (k1, k2) = (key_of(&one.thread), key_of(&two.thread));
    assert_ne!(k1, k2, "a key per session");
    let holder = slots.holder(&k1).unwrap();
    assert_eq!((holder.slot, holder.role.as_str(), holder.name.as_str()), (sub.clone(), "sub", "helper"));

    // The agent is told only what a sub-slot may use.
    let mut mcp = Mcp::as_session(addr, &one.thread).await;
    let tools = mcp.rpc("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"view") && !names.contains(&"navigate") && !names.contains(&"plan_edit"), "{names:?}");
    let (refused, text) = mcp.tool("navigate", json!({ "view": "calls" })).await;
    assert!(refused && text.contains("role (sub) can't use navigate"), "{text}");

    // A session's key ends with it: whoever still has it is no one known.
    slots.close_session(&sub, &one.session).await.unwrap();
    assert!(slots.holder(&k1).is_none());
    assert!(slots.holder(&k2).is_some());
}

#[tokio::test]
async fn the_lead_sends_to_a_sub_slot_and_gets_its_answer() {
    let (_, state) = spawn_app().await;
    let slots = &state.harness.slots;
    let sub = slots.create("sub", Some("helper".into()), fake(), Starter::Lead).await.unwrap();
    // Not running yet: sending opens its session first.
    let sent = slots.send(&sub, "count the files".into(), Delivery::Queue, Origin::Lead, Some(Duration::from_secs(5))).await.unwrap();
    let Sent::Replied { text, .. } = sent else { panic!("{sent:?}") };
    assert!(text.starts_with("echo: From the lead (lead):\n\ncount the files"), "marked as the lead's: {text}");
    let sent = slots.send(&sub, "[hang] take your time".into(), Delivery::Queue, Origin::User, Some(Duration::from_millis(100))).await.unwrap();
    assert!(matches!(sent, Sent::StillWorking { .. }), "{sent:?}");
    let mut report = slots.read(&sub, 10).unwrap();
    for _ in 0..50 {
        if report.messages.iter().any(|(_, m)| m.contains("count the files")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        report = slots.read(&sub, 10).unwrap();
    }
    assert!(report.messages.iter().any(|(_, m)| m.contains("count the files")), "{:?}", report.messages);
}

#[tokio::test]
async fn a_report_reaches_a_running_lead_or_waits_for_one() {
    let (_, state) = spawn_app().await;
    let slots = &state.harness.slots;
    let lead = slots.lead();
    slots.set_harness(&lead, InstanceId::new(FAKE)).await.unwrap();
    let sub = slots.create("sub", Some("helper".into()), fake(), Starter::Lead).await.unwrap();
    assert_eq!(slots.report(&sub, "found three bugs".into()).await.unwrap(), Reported::Held, "no lead session: held");
    let opened = slots.open_session(&lead, Open::New { resume: None }).await.unwrap();
    assert_eq!(opened.delivered_reports, 1, "delivered as the lead opened");
    // The hub hears of it from the session's own stream, a moment later.
    let mut heard = false;
    for _ in 0..50 {
        heard = slots.read(&lead, 10).unwrap().messages.iter().any(|(_, m)| m.contains("From helper (sub-slot):\n\nfound three bugs"));
        if heard {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(heard, "{:?}", slots.read(&lead, 10).unwrap().messages);
    assert_eq!(slots.report(&sub, "and a fourth".into()).await.unwrap(), Reported::Delivered);
}

/// A store that can be made to fail, for what happens then.
#[derive(Default)]
struct Store {
    slots: Mutex<Vec<Slot>>,
    reports: Mutex<Vec<HeldReport>>,
    failing: std::sync::atomic::AtomicBool,
}

impl SlotStore for Store {
    fn load(&self) -> Result<Vec<Slot>, String> {
        Ok(self.slots.lock().unwrap().clone())
    }
    fn save(&self, slot: &Slot) -> Result<(), String> {
        if self.failing.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("the disk is full".into());
        }
        let mut slots = self.slots.lock().unwrap();
        slots.retain(|s| s.id != slot.id);
        slots.push(slot.clone());
        Ok(())
    }
    fn remove(&self, id: &SlotId) -> Result<(), String> {
        self.slots.lock().unwrap().retain(|s| &s.id != id);
        Ok(())
    }
    fn hold_report(&self, report: &HeldReport) -> Result<(), String> {
        self.reports.lock().unwrap().push(report.clone());
        Ok(())
    }
    fn take_reports(&self) -> Result<Vec<HeldReport>, String> {
        Ok(std::mem::take(&mut *self.reports.lock().unwrap()))
    }
    fn note(&self, _: &str, _: &str, _: &str, _: Option<&str>) {}
}

fn manager(state: &meta_harness::modules::AppState, store: Arc<Store>, open_within: Duration) -> Arc<SlotManager> {
    let ours = meta_harness::startup::our_tools("http://127.0.0.1:9");
    SlotManager::load(state.harness.sessions.clone(), state.harness.registry.clone(), state.harness.hub.clone(), store, ours, Arc::new(|| Ok(Vec::new())), open_within).unwrap()
}

#[tokio::test]
async fn after_a_restart_slots_come_back_and_resume_on_first_use() {
    let (_, state) = spawn_app().await;
    let store = Arc::new(Store::default());
    let before = manager(&state, store.clone(), Duration::from_secs(5));
    let sub = before.create("sub", Some("helper".into()), fake(), Starter::User).await.unwrap();
    let opened = before.open_session(&sub, Open::New { resume: None }).await.unwrap();
    let worker = before.create("worker", None, fake(), Starter::Queue).await.unwrap();
    drop(before);

    let after = manager(&state, store.clone(), Duration::from_secs(5));
    let slot = after.get(&sub).expect("it came back");
    assert_eq!(slot.current, Some(opened.session.clone()));
    assert!(slot.current_thread().is_none(), "nothing runs until it's used");
    assert!(after.get(&worker).is_none(), "a queue's worker from the last run is gone");
    let Sent::Replied { .. } = after.send(&sub, "hello again".into(), Delivery::Queue, Origin::User, Some(Duration::from_secs(5))).await.unwrap() else { panic!() };
    let resumed = after.get(&sub).unwrap();
    assert_eq!(resumed.current, Some(opened.session), "the same session, resumed");
    assert!(resumed.current_thread().is_some());
}

#[tokio::test]
async fn a_session_that_cant_be_resumed_opens_fresh_and_says_so() {
    let (_, state) = spawn_app().await;
    let store = Arc::new(Store::default());
    let key = SessionKey::new("old");
    store
        .save(&Slot {
            id: SlotId::new("sub-1"),
            role: "sub".into(),
            name: "helper".into(),
            instance_id: Some(InstanceId::new(FAKE)),
            policy: Default::default(),
            model: None,
            sessions: vec![SlotSession { key: key.clone(), native_id: Some(NativeId::new("gone")), title: None, thread: None }],
            current: Some(key.clone()),
            started_by: Starter::User,
            status: SlotStatus::Ready,
        })
        .unwrap();
    let slots = manager(&state, store, Duration::from_secs(5));
    let opened = slots.open_session(&SlotId::new("sub-1"), Open::Again(key)).await.unwrap();
    assert!(opened.note.as_deref().is_some_and(|n| n.contains("couldn't be resumed") && n.contains("a new one was opened")), "{:?}", opened.note);
}

#[tokio::test]
async fn a_slot_whose_harness_is_missing_says_so_until_it_comes() {
    let (_, state) = spawn_app().await;
    let slots = &state.harness.slots;
    let sub = slots.create("sub", None, SlotSettings { instance: Some(InstanceId::new("later")), ..SlotSettings::default() }, Starter::User).await.unwrap();
    let SlotStatus::Unavailable { reason, fix } = slots.get(&sub).unwrap().status else { panic!("unavailable") };
    assert!(reason.contains("isn't set up") && fix.unwrap().contains("pack install later"));
    assert!(slots.open_session(&sub, Open::New { resume: None }).await.unwrap_err().to_string().contains("isn't set up"));
    // The harness appears: the slot follows.
    let mut entry = common::fake_harness();
    entry.profile.instance = InstanceId::new("later");
    state.harness.registry.configure(state.harness.registry.default_configs(&[common::fake_harness(), entry])).await.unwrap();
    for _ in 0..50 {
        if slots.get(&sub).unwrap().status == SlotStatus::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(slots.get(&sub).unwrap().status, SlotStatus::Ready);
}

#[tokio::test]
async fn a_session_that_cant_be_kept_doesnt_stay_running() {
    let (_, state) = spawn_app().await;
    let store = Arc::new(Store::default());
    let slots = manager(&state, store.clone(), Duration::from_secs(5));
    let sub = slots.create("sub", None, fake(), Starter::User).await.unwrap();
    store.failing.store(true, std::sync::atomic::Ordering::Relaxed);
    let err = slots.open_session(&sub, Open::New { resume: None }).await.unwrap_err().to_string();
    assert!(err.contains("the disk is full"), "{err}");
    assert!(slots.get(&sub).unwrap().sessions.is_empty(), "nothing half-kept");
    assert!(state.harness.slots.list().len() == 1 && state.harness.sessions.get(&ThreadId::new("x")).is_err());
    let running = slots.list().iter().flat_map(|s| s.sessions.iter()).filter(|s| s.thread.is_some()).count();
    assert_eq!(running, 0, "the session it opened was closed again");
}

#[tokio::test]
async fn a_harness_that_never_opens_doesnt_hold_its_slot_forever() {
    let (_, state) = spawn_app().await;
    let slots = manager(&state, Arc::new(Store::default()), Duration::from_millis(200));
    let settings = SlotSettings { policy: harness_contracts::RuntimePolicy { cwd: Some("[hang-open]".into()), ..Default::default() }, ..fake() };
    let sub = slots.create("sub", None, settings, Starter::User).await.unwrap();
    let (opening, closing) = tokio::join!(slots.open_session(&sub, Open::New { resume: None }), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        slots.close(&sub).await
    });
    assert!(opening.unwrap_err().to_string().contains("didn't open a session within"));
    assert!(closing.is_ok() && slots.get(&sub).is_none(), "closed once the open gave up");
}

/// The cockpit's routes for slots: start one, open, prompt (which resumes
/// first), switch and close sessions, close it.
#[tokio::test]
async fn the_cockpit_runs_slots_over_http() {
    let (addr, state) = spawn_app().await;
    let (status, body) = common::http(addr, "POST", "/api/slots", Some(r#"{"instanceId":"fake","name":"helper"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let slot = serde_json::from_str::<serde_json::Value>(&body).unwrap()["slotId"].as_str().unwrap().to_owned();
    let (status, body) = common::http(addr, "POST", &format!("/api/slots/{slot}/prompt"), Some(r#"{"text":"hello"}"#)).await;
    assert_eq!(status, 200, "a prompt to a slot that isn't running opens its session: {body}");
    let first = state.harness.slots.get(&SlotId::new(&slot)).unwrap().current.unwrap();
    let (status, body) = common::http(addr, "POST", &format!("/api/slots/{slot}/sessions"), Some("{}")).await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = common::http(addr, "POST", &format!("/api/slots/{slot}/current"), Some(&format!(r#"{{"session":"{first}"}}"#))).await;
    assert_eq!(status, 204);
    let (status, _) = common::http(addr, "DELETE", &format!("/api/slots/{slot}/sessions/{first}"), None).await;
    assert_eq!(status, 204);
    assert_eq!(state.harness.slots.get(&SlotId::new(&slot)).unwrap().sessions.len(), 1);
    let (status, body) = common::http(addr, "POST", "/api/slots", Some(r#"{"instanceId":"nope"}"#)).await;
    assert_eq!(status, 404, "{body}");
    let (status, body) = common::http(addr, "POST", "/api/slots", Some(r#"{"role":"lead","instanceId":"fake"}"#)).await;
    assert_eq!(status, 400, "the lead isn't made here: {body}");
    let (status, _) = common::http(addr, "DELETE", &format!("/api/slots/{slot}"), None).await;
    assert_eq!(status, 204);
    let (status, _) = common::http(addr, "DELETE", "/api/slots/lead", None).await;
    assert_eq!(status, 400, "the lead stays");
}

/// A chat's message and turn tagged: named by the slot, read from the
/// session, kept in the tags' logs (what a session is is the harness's);
/// the turn's tool calls one line each unless asked in full.
#[tokio::test]
async fn a_chats_message_and_turn_are_tagged_named_by_the_slot_and_kept() {
    let (addr, state) = spawn_app().await;
    let (status, body) = common::http(addr, "POST", "/api/slots", Some(r#"{"instanceId":"fake","name":"helper"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let slot = serde_json::from_str::<serde_json::Value>(&body).unwrap()["slotId"].as_str().unwrap().to_owned();
    let (status, body) = common::http(addr, "POST", &format!("/api/slots/{slot}/prompt"), Some(r#"{"text":"hello there"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let mut lead = Mcp::lead(addr).await;
    // Its first turn's first message: the answer the fake harness gives.
    let mut said = String::new();
    for _ in 0..50 {
        let (error, out) = lead.tool("tag", json!({ "tag": "greeting", "subject": "chat helper turn 1 message 1" })).await;
        said = out;
        if !error {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(said.starts_with("greeting (on 1 thing)"), "{said}");
    let (error, said) = lead.tool("tag", json!({ "tag": "greeting", "subject": "chat helper turn 1" })).await;
    assert!(!error, "{said}");
    let (_, read) = lead.tool("tag_read", json!({ "tag": "greeting" })).await;
    // Named with its session (the start of its id: a slot has several).
    assert!(read.contains("1. chat helper@native-") && read.contains(" turn 1 message 1 (tagged by lead; as it is now):\necho: hello there"), "{read}");
    assert!(read.contains(" turn 1 (tagged by lead; as it is now):\necho: hello there"), "{read}");
    // Kept in the logs: the tag holds where, never the text.
    let tag = state.tags.tags.find("greeting").unwrap();
    assert!(tag.on.iter().all(|t| t.kept.is_some()) && !serde_json::to_string(&tag).unwrap().contains("hello there"));
    // Another sub-slot can't read the helper's chat (no role but the lead's
    // observes slots), nor tag it.
    let (status, body) = common::http(addr, "POST", "/api/slots", Some(r#"{"instanceId":"fake","name":"other"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let other = serde_json::from_str::<serde_json::Value>(&body).unwrap()["slotId"].as_str().unwrap().to_owned();
    let (status, body) = common::http(addr, "POST", &format!("/api/slots/{other}/prompt"), Some(r#"{"text":"hi"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let thread = state.harness.slots.get(&SlotId::new(&other)).unwrap().current_thread().cloned().unwrap();
    let mut peer = Mcp::as_session(addr, &thread).await;
    let (error, said) = peer.tool("tag_read", json!({ "tag": "greeting", "thing": 1 })).await;
    assert!(error && said.contains("can't read"), "{said}");
    let (_, listed) = peer.tool("tag_read", json!({ "tag": "greeting" })).await;
    assert!(!listed.contains("hello there"), "{listed}");
    let (error, said) = peer.tool("tag", json!({ "tag": "peek", "subject": "chat helper turn 1" })).await;
    assert!(error && said.contains("can't read"), "{said}");
    // Its slot closed (the session gone from here): as it was, from the logs.
    state.harness.slots.close(&SlotId::new(&slot)).await.unwrap();
    let (_, read) = lead.tool("tag_read", json!({ "tag": "greeting", "thing": 1 })).await;
    assert!(read.contains("(as it was when tagged: changed or gone since):\necho: hello there"), "{read}");
}

/// What a turn's tools gave is left out of a read, one line said in its
/// place, unless it's read in full; a prompt naming the tag leaves it out
/// the same way (one way of folding, for every read).
#[tokio::test]
async fn a_turns_tool_output_is_folded_the_same_in_every_read_unless_read_in_full() {
    let (addr, _state) = spawn_app().await;
    let (status, body) = common::http(addr, "POST", "/api/slots", Some(r#"{"instanceId":"fake","name":"helper"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let slot = serde_json::from_str::<serde_json::Value>(&body).unwrap()["slotId"].as_str().unwrap().to_owned();
    let (status, body) = common::http(addr, "POST", &format!("/api/slots/{slot}/prompt"), Some(r#"{"text":"[tool] look"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let mut lead = Mcp::lead(addr).await;
    let mut said = String::new();
    for _ in 0..50 {
        let (error, out) = lead.tool("tag", json!({ "tag": "turns", "subject": "chat helper turn 1" })).await;
        said = out;
        if !error {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(said.starts_with("turns (on 1 thing)"), "{said}");
    let folded = "(2 lines left out: tag_read's `full` gives them)";
    let (_, read) = lead.tool("tag_read", json!({ "tag": "turns" })).await;
    assert!(read.contains("• Bash") && read.contains(folded) && !read.contains("tool line one"), "{read}");
    let (_, read) = lead.tool("tag_read", json!({ "tag": "turns", "thing": 1 })).await;
    assert!(read.contains(folded) && !read.contains("tool line one"), "one thing: {read}");
    let (_, read) = lead.tool("tag_read", json!({ "tag": "turns", "full": true })).await;
    assert!(read.contains("tool line one") && read.contains("tool line two") && !read.contains(folded), "in full: {read}");
    // A prompt naming the tag: what it hands the agent is folded the same
    // (the fake agent says back what it was given).
    let (status, body) = common::http(addr, "POST", &format!("/api/slots/{slot}/prompt"), Some(r#"{"text":"and #turns"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let mut given = String::new();
    for _ in 0..50 {
        let (_, out) = lead.tool("slot_read", json!({ "slot": "helper" })).await;
        given = out;
        if given.contains("echo: and #turns") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(given.contains(folded) && !given.contains("tool line one"), "{given}");
}

/// A turn tagged while it runs is kept whole: what it said then, and the
/// rest once it ends.
#[tokio::test]
async fn a_chat_turn_tagged_while_it_runs_is_kept_whole_once_it_ends() {
    let (addr, state) = spawn_app().await;
    let (status, body) = common::http(addr, "POST", "/api/slots", Some(r#"{"instanceId":"fake","name":"asker"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let slot = serde_json::from_str::<serde_json::Value>(&body).unwrap()["slotId"].as_str().unwrap().to_owned();
    // A turn that waits on a question (the fake harness answers once it's answered).
    let (status, body) = common::http(addr, "POST", &format!("/api/slots/{slot}/prompt"), Some(r#"{"text":"[ask] go on"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let mut lead = Mcp::lead(addr).await;
    let (error, said) = lead.tool("tag", json!({ "tag": "pending", "subject": "chat asker turn 1" })).await;
    assert!(!error, "{said}");
    // Answered: the turn ends with its answer.
    let thread = state.harness.slots.get(&SlotId::new(&slot)).unwrap().current_thread().cloned().unwrap();
    let request = state.harness.hub.thread_state(&thread).into_iter().find_map(|e| match e {
        harness_contracts::ClientEvent::Session { event: harness_contracts::AdapterEvent::RequestUpdated { request } } => Some(request.id),
        _ => None,
    }).expect("its question");
    let session = state.harness.sessions.get(&thread).unwrap();
    session.respond(&request, harness_contracts::RequestResponse { option_id: Some("allow".into()), answers: Default::default() }).await.unwrap();
    let mut read = String::new();
    for _ in 0..40 {
        read = lead.tool("tag_read", json!({ "tag": "pending", "thing": 1 })).await.1;
        if read.contains("done, as allowed") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(read.contains("done, as allowed"), "the rest, once it ended: {read}");
}
