//! Notifications on the server, against the fake harness: what waits on the
//! user until answered, what happened, messages from agents, and the routes.

mod common;

use std::time::Duration;

use common::{FAKE, Mcp, http, spawn_app};
use harness_contracts::{InstanceId, Notification, NotificationKind, NotificationState, SlotId, Starter, ThreadId};
use harness_core::{Open, SlotSettings};
use meta_harness::server::AppState;
use serde_json::json;

fn fake() -> SlotSettings {
    SlotSettings { instance: Some(InstanceId::new(FAKE)), ..SlotSettings::default() }
}

/// Waits (a little) for the open notifications to settle as `done` says.
async fn open_when(state: &AppState, done: impl Fn(&[Notification]) -> bool) -> Vec<Notification> {
    for _ in 0..100 {
        let open = state.notify.open();
        if done(&open) {
            return open;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    state.notify.open()
}

async fn sub_with_session(state: &AppState) -> (SlotId, ThreadId) {
    let sub = state.slots.create("sub", Some("reviewer".into()), fake(), Starter::User).await.unwrap();
    let thread = state.slots.open_session(&sub, Open::New { resume: None }).await.unwrap().thread;
    (sub, thread)
}

#[tokio::test]
async fn an_approval_waits_on_the_user_until_answered_and_the_agent_carries_on() {
    let (addr, state) = spawn_app().await;
    let (sub, thread) = sub_with_session(&state).await;
    let (status, body) = http(addr, "POST", &format!("/api/slots/{sub}/prompt"), Some(r#"{"text":"tidy up [ask]"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let open = open_when(&state, |o| o.iter().any(|n| n.kind == NotificationKind::Approval)).await;
    let ask = open.iter().find(|n| n.kind == NotificationKind::Approval).expect("waiting on the user");
    assert_eq!((ask.slot.as_ref(), ask.title.as_str()), (Some(&sub), "Run cargo test"));
    let (status, body) = http(addr, "GET", "/api/notifications", None).await;
    assert_eq!(status, 200, "{body}");
    let listed: Vec<Notification> = serde_json::from_str(&body).unwrap();
    assert_eq!(listed[0].id, ask.id, "waiting comes first");
    // Answered the usual way: the notification is done, the turn finishes.
    let request = ask.request.clone().unwrap();
    let (status, body) = http(addr, "POST", &format!("/api/threads/{thread}/requests/{request}"), Some(r#"{"optionId":"allow"}"#)).await;
    assert_eq!(status, 204, "{body}");
    let open = open_when(&state, |o| !o.iter().any(|n| n.id == ask.id) && o.iter().any(|n| n.kind == NotificationKind::TurnFinished)).await;
    assert!(!open.iter().any(|n| n.id == ask.id), "answered: done");
    let finished = open.iter().find(|n| n.kind == NotificationKind::TurnFinished).expect("it carried on and finished");
    assert_eq!((finished.title.as_str(), finished.detail.as_deref()), ("reviewer finished a turn", Some("done, as allowed")));
    // An ended one can't be put away: said, not ignored.
    let (status, body) = http(addr, "POST", &format!("/api/notifications/{}/dismiss", ask.id), None).await;
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("can't go from Done to Dismissed"), "{body}");
}

#[tokio::test]
async fn a_question_waits_too_and_a_failed_turn_is_an_update() {
    let (addr, state) = spawn_app().await;
    let (sub, thread) = sub_with_session(&state).await;
    http(addr, "POST", &format!("/api/slots/{sub}/prompt"), Some(r#"{"text":"which one? [question]"}"#)).await;
    let open = open_when(&state, |o| o.iter().any(|n| n.kind == NotificationKind::Question)).await;
    let question = open.iter().find(|n| n.kind == NotificationKind::Question).unwrap().clone();
    let request = question.request.clone().unwrap();
    let answer = json!({ "answers": { "q": ["the first"] } }).to_string();
    assert_eq!(http(addr, "POST", &format!("/api/threads/{thread}/requests/{request}"), Some(&answer)).await.0, 204);
    open_when(&state, |o| !o.iter().any(|n| n.id == question.id)).await;
    http(addr, "POST", &format!("/api/slots/{sub}/prompt"), Some(r#"{"text":"go [fail]"}"#)).await;
    let open = open_when(&state, |o| o.iter().any(|n| n.kind == NotificationKind::TurnFailed)).await;
    let failed = open.iter().find(|n| n.kind == NotificationKind::TurnFailed).expect("failed: an update");
    assert_eq!(failed.detail.as_deref(), Some("the model refused"));
    // Seeing the slot's chat puts its updates away.
    let (status, body) = http(addr, "POST", "/api/notifications/seen", Some(&json!({ "slot": sub }).to_string())).await;
    assert_eq!((status, body.contains("\"dismissed\":")), (200, true), "{body}");
    assert!(state.notify.open().iter().all(|n| n.slot.as_ref() != Some(&sub)));
    let (_, recent) = http(addr, "GET", "/api/notifications?state=recent", None).await;
    let recent: Vec<Notification> = serde_json::from_str(&recent).unwrap();
    assert!(recent.iter().any(|n| n.id == failed.id && n.state == NotificationState::Dismissed), "kept, as dismissed");
}

#[tokio::test]
async fn a_turn_the_lead_started_in_a_sub_slot_is_an_update_and_a_report_reaches_both() {
    let (addr, state) = spawn_app().await;
    let mut lead = Mcp::lead(addr).await;
    let (sub, sub_thread) = sub_with_session(&state).await;
    let (failed, text) = lead.tool("slot_send", json!({ "slot": "reviewer", "text": "review it", "wait_secs": 5 })).await;
    assert!(!failed, "{text}");
    let open = open_when(&state, |o| o.iter().any(|n| n.kind == NotificationKind::TurnFinished && n.slot.as_ref() == Some(&sub))).await;
    assert!(open.iter().any(|n| n.kind == NotificationKind::TurnFinished && n.slot.as_ref() == Some(&sub)));
    // The sub-slot reports: the lead gets it, and so does the user.
    let mut reviewer = Mcp::as_session(addr, &sub_thread).await;
    let (failed, text) = reviewer.tool("tell_lead", json!({ "text": "Two bugs in the parser." })).await;
    assert!(!failed && text.starts_with("Delivered"), "{text}");
    let open = open_when(&state, |o| o.iter().any(|n| n.kind == NotificationKind::Message)).await;
    let message = open.iter().find(|n| n.kind == NotificationKind::Message).unwrap();
    assert_eq!((message.title.as_str(), message.detail.as_deref()), ("From reviewer", Some("Two bugs in the parser.")));
    let (failed, text) = reviewer.tool("tell_user", json!({ "text": "Done for today." })).await;
    assert!(!failed, "{text}");
    let open = open_when(&state, |o| o.iter().filter(|n| n.kind == NotificationKind::Message).count() == 2).await;
    assert_eq!(open.iter().filter(|n| n.kind == NotificationKind::Message).count(), 2);
}

#[tokio::test]
async fn a_session_that_dies_is_a_crash_and_one_closed_on_purpose_isnt() {
    let (addr, state) = spawn_app().await;
    let (sub, _) = sub_with_session(&state).await;
    http(addr, "POST", &format!("/api/slots/{sub}/prompt"), Some(r#"{"text":"bye [die]"}"#)).await;
    let open = open_when(&state, |o| o.iter().any(|n| n.kind == NotificationKind::SessionCrashed)).await;
    let crash = open.iter().find(|n| n.kind == NotificationKind::SessionCrashed).expect("a crash");
    assert_eq!(crash.title, "reviewer's session stopped");
    // Closed by the user: not a crash.
    let (other, _) = sub_with_session(&state).await;
    state.slots.close(&other).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!state.notify.open().iter().any(|n| n.kind == NotificationKind::SessionCrashed && n.slot.as_ref() == Some(&other)));
    assert_eq!(http(addr, "POST", "/api/notifications/nope/dismiss", None).await.0, 404);
}
