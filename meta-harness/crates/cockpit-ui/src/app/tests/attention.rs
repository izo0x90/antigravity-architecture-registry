//! What needs the user: the counters, their pickers, the quick look on one
//! (answered in place, the next a key away), and chats seen.

use super::*;
use harness_contracts::{AdapterEvent, ApprovalDecision, ApprovalOption, ClientEvent, Notification, NotificationId, NotificationKind, NotificationState, RequestId, RequestKind, RequestStatus, RuntimeRequest, SlotId, ThreadId};

fn note(id: &str, kind: NotificationKind, slot: &str, request: Option<&str>, at_ms: u64) -> ClientEvent {
    let notification = Notification {
        id: NotificationId::new(id),
        server: None,
        level: kind.level(),
        kind,
        slot: Some(SlotId::new(slot)),
        thread: Some(ThreadId::new("t")),
        request: request.map(RequestId::new),
        title: format!("{id} title"),
        detail: Some(format!("{id} detail")),
        at_ms,
        state: NotificationState::Open,
    };
    ClientEvent::NotificationUpdated { notification }
}

/// The lead's conversation, a reviewer sub-slot, the lead waiting on an
/// approval and a question, and the reviewer's finished turn.
fn waiting_app() -> (egui::Context, CockpitApp) {
    let (ctx, mut app) = full_app();
    app.chat = crate::chat::tests::conversation();
    let mut sub = crate::chat::tests::lead(None);
    (sub.id, sub.role, sub.name) = (SlotId::new("sub-1"), "sub".into(), "reviewer".into());
    app.chat.process_event(ClientEvent::SlotUpdated { slot: sub });
    let option = |id: &str, decision, label: &str| ApprovalOption { id: id.into(), decision, label: label.into(), warning: None };
    let request = RuntimeRequest {
        id: RequestId::new("r1"),
        thread_id: ThreadId::new("t"),
        turn_id: None,
        item_id: None,
        kind: RequestKind::Command,
        status: RequestStatus::Pending,
        title: "Run cargo test".into(),
        detail: None,
        options: vec![option("allow", ApprovalDecision::Accept, "Allow"), option("deny", ApprovalDecision::Decline, "Deny")],
        questions: Vec::new(),
        response: None,
    };
    app.chat.process_event(ClientEvent::Session { event: AdapterEvent::RequestUpdated { request } });
    for event in [note("req:r1", NotificationKind::Approval, "lead", Some("r1"), 1), note("req:r2", NotificationKind::Question, "lead", Some("r2"), 2), note("turn:u1", NotificationKind::TurnFinished, "sub-1", None, 3)] {
        app.chat.process_event(event);
    }
    focus(&ctx, &mut app, BufferKind::Graph);
    (ctx, app)
}

fn sent(app: &CockpitApp) -> Vec<String> {
    std::mem::take(&mut *app.net.sent.lock().unwrap())
}

fn looked_at(app: &CockpitApp) -> Option<&str> {
    app.looked_at().map(|id| id.as_str())
}

#[test]
fn the_counters_say_how_many_and_their_pickers_open_one_in_the_quick_look() {
    let (ctx, mut app) = waiting_app();
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains(&format!("{} 2", egui_phosphor::regular::BELL_RINGING)) && text.contains(&format!("{} 1", egui_phosphor::regular::BELL)), "{text}");
    let actions = from_outside(&ctx, &mut app, r#"{"op":"actions"}"#).unwrap();
    let waiting = actions.as_array().unwrap().iter().find(|a| a["id"] == "attention.waiting").expect("the waiting picker");
    assert!(waiting.to_string().contains("req:r1") && !waiting.to_string().contains("turn:u1"), "{waiting}");
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.waiting","value":"req:r2"}"#).unwrap();
    assert_eq!(looked_at(&app), Some("req:r2"));
    // The next waiting: the oldest.
    keys(&ctx, &mut app, "\u{1b}");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert!(app.quick_look.is_none(), "Escape closes it");
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.next_waiting"}"#).unwrap();
    assert_eq!(looked_at(&app), Some("req:r1"));
}

#[test]
fn in_the_quick_look_n_and_shift_n_go_through_them_and_y_answers_its_request() {
    let (ctx, mut app) = waiting_app();
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.next_waiting"}"#).unwrap();
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Run cargo test") && text.contains("1 of 3") && text.contains("Allow"), "the request to answer, in place: {text}");
    keys(&ctx, &mut app, "n");
    assert_eq!(looked_at(&app), Some("req:r2"), "n: the next");
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Its request is gone"), "{text}");
    keys(&ctx, &mut app, "n");
    keys(&ctx, &mut app, "n");
    assert_eq!((looked_at(&app), app.note.as_deref()), (Some("turn:u1"), Some("That was the last one.")));
    keys(&ctx, &mut app, "NN");
    assert_eq!(looked_at(&app), Some("req:r1"), "N: back");
    sent(&app);
    keys(&ctx, &mut app, "y");
    assert_eq!(sent(&app), ["POST /api/threads/t/requests/r1"], "y answers its request, as in the chat");
    // Answered (the server says so): the look says it's handled.
    let mut answered = app.chat.store.threads[&ThreadId::new("t")].request(&RequestId::new("r1")).unwrap().clone();
    answered.status = RequestStatus::Resolved;
    app.chat.process_event(ClientEvent::Session { event: AdapterEvent::RequestUpdated { request: answered } });
    app.chat.process_event(ClientEvent::NotificationUpdated { notification: Notification { state: NotificationState::Done, ..app.chat.store.notifications[&NotificationId::new("req:r1")].clone() } });
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Already handled"), "{text}");
    keys(&ctx, &mut app, "y");
    assert!(sent(&app).is_empty(), "nothing left to answer");
}

#[test]
fn the_look_opens_its_chat_and_puts_one_away() {
    let (ctx, mut app) = waiting_app();
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.updates","value":"turn:u1"}"#).unwrap();
    sent(&app);
    keys(&ctx, &mut app, "x");
    assert_eq!(sent(&app), ["POST /api/notifications/turn%3Au1/dismiss"]);
    assert!(app.quick_look.is_none(), "the last one: closed");
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.updates","value":"turn:u1"}"#).unwrap();
    keys(&ctx, &mut app, "o");
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.quick_look.is_none(), app.focused_kind(), app.chat.ui.slot.as_str()), (true, BufferKind::Chat, "sub-1"), "the reviewer's chat");
}

#[test]
fn a_chat_on_screen_says_its_updates_were_seen_now_and_then() {
    let (ctx, mut app) = waiting_app();
    sent(&app);
    frame_at(&ctx, &mut app, vec![], 10.0);
    assert!(!sent(&app).iter().any(|s| s.contains("seen")), "the chat isn't on screen");
    focus(&ctx, &mut app, BufferKind::Chat);
    app.chat.ui.slot = SlotId::new("sub-1");
    frame_at(&ctx, &mut app, vec![], 20.0);
    frame_at(&ctx, &mut app, vec![], 20.5);
    let seen: Vec<String> = sent(&app).into_iter().filter(|s| s.contains("seen")).collect();
    assert_eq!(seen, ["POST /api/notifications/seen"], "once, not every frame");
    frame_at(&ctx, &mut app, vec![], 23.0);
    assert_eq!(sent(&app).iter().filter(|s| s.contains("seen")).count(), 1, "again after a while, while it has updates");
    app.chat.process_event(ClientEvent::NotificationUpdated { notification: Notification { state: NotificationState::Dismissed, ..app.chat.store.notifications[&NotificationId::new("turn:u1")].clone() } });
    frame_at(&ctx, &mut app, vec![], 30.0);
    assert!(!sent(&app).iter().any(|s| s.contains("seen")), "none left: nothing said");
}
