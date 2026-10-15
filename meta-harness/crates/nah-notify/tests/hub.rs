use std::sync::Arc;
use std::time::Duration;

use harness_contracts::{
    AdapterEvent, ClientEvent, InstanceId, Level, Notification, NotificationId, NotificationKind, NotificationState, RequestId, RequestKind, RequestStatus, RuntimeRequest, SessionKey, Slot,
    SlotId, SlotSession, SlotStatus, Starter, ThreadId, Turn, TurnId, TurnStatus,
};
use harness_core::EventHub;
use nah_notify::testing::{Ended, MemoryStore, Recorder};
use nah_notify::{NotificationStore, NotifyError, NotifyHub};

fn slot(id: &str, name: &str, thread: Option<&str>) -> ClientEvent {
    let key = SessionKey::new(format!("{id}-s"));
    let slot = Slot {
        id: SlotId::new(id),
        role: if id == "lead" { "lead".into() } else { "sub".into() },
        name: name.into(),
        instance_id: Some(InstanceId::new("claude-code")),
        policy: Default::default(),
        model: None,
        sessions: vec![SlotSession { key: key.clone(), native_id: None, title: None, thread: thread.map(ThreadId::new) }],
        current: Some(key),
        started_by: Starter::User,
        status: SlotStatus::Ready,
    };
    ClientEvent::SlotUpdated { slot }
}

fn request(id: &str, status: RequestStatus) -> ClientEvent {
    let request = RuntimeRequest {
        id: RequestId::new(id),
        thread_id: ThreadId::new("sub-t"),
        turn_id: None,
        item_id: None,
        kind: RequestKind::Permission,
        status,
        title: "Edit src/main.rs".into(),
        detail: None,
        options: Vec::new(),
        questions: Vec::new(),
        response: None,
    };
    ClientEvent::Session { event: AdapterEvent::RequestUpdated { request } }
}

/// A turn that runs, then ends as `status` (one first seen ended is history).
fn ran(id: &str, thread: &str, status: TurnStatus) -> [ClientEvent; 2] {
    [turn(id, thread, TurnStatus::Running), turn(id, thread, status)]
}

fn turn(id: &str, thread: &str, status: TurnStatus) -> ClientEvent {
    ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new(id), thread_id: ThreadId::new(thread), ordinal: 1, status, failure: None, usage: None } } }
}

struct Rig {
    events: Arc<EventHub>,
    store: Arc<MemoryStore>,
    recorder: Arc<Recorder>,
    hub: Arc<NotifyHub>,
}

fn rig_with(events: Arc<EventHub>, store: Arc<MemoryStore>) -> Rig {
    let recorder = Arc::new(Recorder::default());
    let hub = NotifyHub::start(events.clone(), store.clone(), Arc::new(Ended::default()), vec![recorder.clone()]).unwrap();
    Rig { events, store, recorder, hub }
}

fn rig() -> Rig {
    rig_with(Arc::new(EventHub::default()), Arc::new(MemoryStore::default()))
}

async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
}

fn ids(list: &[Notification]) -> Vec<&str> {
    list.iter().map(|n| n.id.as_str()).collect()
}

#[tokio::test]
async fn an_approval_waits_until_answered_and_every_step_is_kept_published_and_handed_on() {
    let r = rig();
    r.events.publish(slot("sub-1", "reviewer", Some("sub-t")));
    r.events.publish(request("r1", RequestStatus::Pending));
    settle().await;
    let open = r.hub.open();
    assert_eq!((ids(&open), open[0].kind, open[0].slot.as_ref().map(|s| s.as_str())), (vec!["req:r1"], NotificationKind::Approval, Some("sub-1")));
    let (replay, _) = r.events.subscribe();
    assert!(replay.iter().any(|e| matches!(e, ClientEvent::NotificationUpdated { notification } if notification.id.as_str() == "req:r1")), "a late client gets it");
    // The same event again changes nothing.
    r.events.publish(request("r1", RequestStatus::Pending));
    r.events.publish(request("r1", RequestStatus::Resolved));
    settle().await;
    assert!(r.hub.open().is_empty());
    assert_eq!(r.store.get(&NotificationId::new("req:r1")).unwrap().map(|n| n.state), Some(NotificationState::Done));
    let delivered: Vec<(String, NotificationState)> = r.recorder.0.lock().unwrap().iter().map(|n| (n.id.to_string(), n.state)).collect();
    assert_eq!(delivered, [("req:r1".into(), NotificationState::Open), ("req:r1".into(), NotificationState::Done)]);
    let (replay, _) = r.events.subscribe();
    assert!(!replay.iter().any(|e| matches!(e, ClientEvent::NotificationUpdated { .. })), "ended: no longer replayed");
}

#[tokio::test]
async fn falling_behind_rebuilds_from_the_current_state() {
    let r = rig_with(Arc::new(EventHub::with_capacity(4)), Arc::new(MemoryStore::default()));
    r.events.publish(slot("sub-1", "reviewer", Some("sub-t")));
    r.events.publish(request("r1", RequestStatus::Pending));
    r.events.publish(turn("u1", "sub-t", TurnStatus::Running));
    settle().await;
    assert_eq!(ids(&r.hub.open()), ["req:r1"]);
    // A burst it can't keep up with: r1 answered, r2 asked, a turn done.
    r.events.publish(request("r1", RequestStatus::Resolved));
    r.events.publish(request("r2", RequestStatus::Pending));
    r.events.publish(turn("u1", "sub-t", TurnStatus::Completed));
    for n in 0..20 {
        r.events.publish(turn(&format!("noise{n}"), "nobodys", TurnStatus::Running));
    }
    settle().await;
    assert_eq!(ids(&r.hub.open()), ["req:r2", "turn:u1"], "as if nothing was missed");
    assert_eq!(r.store.get(&NotificationId::new("req:r1")).unwrap().map(|n| n.state), Some(NotificationState::Done));
}

#[tokio::test]
async fn a_restart_expires_what_waited_and_keeps_the_updates() {
    let store = Arc::new(MemoryStore::default());
    {
        let r = rig_with(Arc::new(EventHub::default()), store.clone());
        r.events.publish(slot("sub-1", "reviewer", Some("sub-t")));
        r.events.publish(request("r1", RequestStatus::Pending));
        for e in ran("u1", "sub-t", TurnStatus::Completed) {
            r.events.publish(e);
        }
        settle().await;
        assert_eq!(r.hub.open().len(), 2);
    }
    assert_eq!(store.expire_waiting().unwrap(), 1);
    let r = rig_with(Arc::new(EventHub::default()), store.clone());
    assert_eq!(ids(&r.hub.open()), ["turn:u1"]);
    assert_eq!(store.get(&NotificationId::new("req:r1")).unwrap().map(|n| n.state), Some(NotificationState::Expired));
}

#[tokio::test]
async fn one_that_cant_be_kept_is_not_held_or_published() {
    let r = rig();
    r.events.publish(slot("sub-1", "reviewer", Some("sub-t")));
    r.store.refuse.store(true, std::sync::atomic::Ordering::Relaxed);
    r.events.publish(request("r1", RequestStatus::Pending));
    settle().await;
    assert!(r.hub.open().is_empty() && r.recorder.0.lock().unwrap().is_empty());
    assert!(matches!(r.hub.message(None, "hello"), Err(NotifyError::Store(_))));
}

#[tokio::test]
async fn seeing_a_slot_dismisses_its_updates_and_nothing_else() {
    let r = rig();
    r.events.publish(slot("lead", "lead", Some("lead-t")));
    r.events.publish(slot("sub-1", "reviewer", Some("sub-t")));
    r.events.publish(request("r1", RequestStatus::Pending));
    for e in ran("u1", "sub-t", TurnStatus::Completed) {
        r.events.publish(e);
    }
    for e in ran("u2", "lead-t", TurnStatus::Completed) {
        r.events.publish(e);
    }
    settle().await;
    let told = r.hub.message(Some(&SlotId::new("sub-1")), "Found two bugs.").unwrap();
    assert_eq!((told.title.as_str(), told.level), ("From reviewer", Level::Update));
    assert_eq!(r.hub.seen(&SlotId::new("sub-1")).unwrap(), 2, "its finished turn and its message");
    let open = r.hub.open();
    assert_eq!(ids(&open), ["req:r1", "turn:u2"], "waiting first; the lead's update stays");
    // Dismissing: once; an unknown one is said.
    let lead_turn = NotificationId::new("turn:u2");
    assert_eq!(r.hub.dismiss(&lead_turn).unwrap().state, NotificationState::Dismissed);
    assert_eq!(r.hub.dismiss(&lead_turn).unwrap().state, NotificationState::Dismissed, "again: no change");
    assert!(matches!(r.hub.dismiss(&NotificationId::new("nope")), Err(NotifyError::NotFound(_))));
    r.events.publish(request("r1", RequestStatus::Resolved));
    settle().await;
    let err = r.hub.dismiss(&NotificationId::new("req:r1")).unwrap_err();
    assert_eq!(err.to_string(), "notification req:r1 can't go from Done to Dismissed");
}

#[tokio::test]
async fn a_session_ended_unasked_is_a_crash() {
    let r = rig();
    r.events.publish(slot("sub-1", "reviewer", Some("sub-t")));
    r.events.publish(ClientEvent::ThreadClosed { thread_id: ThreadId::new("sub-t") });
    r.events.publish(slot("sub-1", "reviewer", None));
    settle().await;
    let open = r.hub.open();
    assert_eq!((ids(&open), open[0].kind), (vec!["crash:sub-t"], NotificationKind::SessionCrashed));
}

#[tokio::test]
async fn history_a_resumed_session_replays_is_not_news() {
    let r = rig();
    r.events.publish(slot("sub-1", "reviewer", Some("sub-t")));
    r.events.publish(turn("old", "sub-t", TurnStatus::Completed));
    r.events.publish(turn("old-failed", "sub-t", TurnStatus::Failed));
    settle().await;
    assert!(r.hub.open().is_empty(), "first seen ended: replayed, not news");
}
