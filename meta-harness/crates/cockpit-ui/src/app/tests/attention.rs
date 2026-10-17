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
        terminal: None,
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
    crate::chat::tests::conversation_events().into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    let mut sub = crate::chat::tests::lead(None);
    (sub.id, sub.role, sub.name) = (SlotId::new("sub-1"), "sub".into(), "reviewer".into());
    app.desk.shared.agents.get_mut().apply(ClientEvent::SlotUpdated { slot: sub });
    let option = |id: &str, decision, label: &str| ApprovalOption { id: id.into(), decision, label: label.into(), warning: None };
    let request = RuntimeRequest {
        id: RequestId::new("r1"),
        thread_id: ThreadId::new("t"),
        turn_id: None,
        item_id: None,
        kind: RequestKind::Approval { action: harness_contracts::ToolAction::Command },
        status: RequestStatus::Pending,
        title: "Run cargo test".into(),
        detail: None,
        options: vec![option("allow", ApprovalDecision::Accept, "Allow"), option("deny", ApprovalDecision::Decline, "Deny")],
        questions: Vec::new(),
        response: None,
    };
    app.desk.shared.agents.get_mut().apply(ClientEvent::Session { event: AdapterEvent::RequestUpdated { request } });
    for event in [note("req:r1", NotificationKind::Approval, "lead", Some("r1"), 1), note("req:r2", NotificationKind::Question, "lead", Some("r2"), 2), note("turn:u1", NotificationKind::TurnFinished, "sub-1", None, 3)] {
        app.desk.shared.agents.get_mut().apply(event);
    }
    focus(&ctx, &mut app, BufferKind::Graph);
    (ctx, app)
}

fn looked_at(app: &CockpitApp) -> Option<String> {
    app.looked_at().map(|id| id.to_string())
}

#[test]
fn the_bell_and_the_tray_say_how_many_and_their_lists_open_one_in_the_quick_look() {
    let (ctx, mut app) = waiting_app();
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains(&format!("{} 2", egui_phosphor::regular::BELL)) && text.contains(&format!("{} 1", egui_phosphor::regular::TRAY)), "one bell (system notifications off: plain), one tray: {text}");
    let actions = from_outside(&ctx, &mut app, r#"{"op":"actions"}"#).unwrap();
    let waiting = actions.as_array().unwrap().iter().find(|a| a["id"] == "attention.waiting").expect("the waiting picker");
    assert!(waiting.to_string().contains("req:r1") && !waiting.to_string().contains("turn:u1"), "{waiting}");
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.waiting","value":"here/req:r2"}"#).unwrap();
    assert_eq!(looked_at(&app).as_deref(), Some("req:r2"));
    // The next waiting: the oldest.
    keys(&ctx, &mut app, "\u{1b}");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert!(app.quick_look.is_none(), "Escape closes it");
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.next_waiting"}"#).unwrap();
    assert_eq!(looked_at(&app).as_deref(), Some("req:r1"));
}

#[test]
fn in_the_quick_look_shift_j_and_k_go_through_them_and_y_answers_its_request() {
    let (ctx, mut app) = waiting_app();
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.next_waiting"}"#).unwrap();
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Run cargo test") && text.contains("1 of 3") && text.contains("Allow"), "the request to answer, in place: {text}");
    keys(&ctx, &mut app, "J");
    assert_eq!(looked_at(&app).as_deref(), Some("req:r2"), "J: the next");
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Its request is gone"), "{text}");
    keys(&ctx, &mut app, "J");
    keys(&ctx, &mut app, "J");
    assert_eq!((looked_at(&app).as_deref(), app.note.as_deref()), (Some("turn:u1"), Some("That was the last one.")));
    keys(&ctx, &mut app, "KK");
    assert_eq!(looked_at(&app).as_deref(), Some("req:r1"), "K: back");
    sent(&app);
    keys(&ctx, &mut app, "y");
    assert_eq!(sent(&app), ["POST /api/threads/t/requests/r1"], "y answers its request, as in the chat");
    // Answered (the server says so): the look says it's handled.
    let mut answered = app.desk.shared.agents.get().store().threads[&ThreadId::new("t")].request(&RequestId::new("r1")).unwrap().clone();
    answered.status = RequestStatus::Resolved;
    app.desk.shared.agents.get_mut().apply(ClientEvent::Session { event: AdapterEvent::RequestUpdated { request: answered } });
    let done = Notification { state: NotificationState::Done, ..app.desk.shared.agents.get().store().notifications[&NotificationId::new("req:r1")].clone() };
    app.desk.shared.agents.get_mut().apply(ClientEvent::NotificationUpdated { notification: done });
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Already handled"), "{text}");
    keys(&ctx, &mut app, "y");
    assert!(sent(&app).is_empty(), "nothing left to answer");
}

#[test]
fn the_look_opens_its_chat_and_puts_one_away() {
    let (ctx, mut app) = waiting_app();
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.updates","value":"here/turn:u1"}"#).unwrap();
    sent(&app);
    keys(&ctx, &mut app, "x");
    assert_eq!(sent(&app), ["POST /api/notifications/turn%3Au1/dismiss"]);
    assert!(app.quick_look.is_none(), "the last one: closed");
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.updates","value":"here/turn:u1"}"#).unwrap();
    keys(&ctx, &mut app, "o");
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.quick_look.is_none(), app.focused_kind(), app.desk.chat().ui.slot.as_str()), (true, BufferKind::Chat, "sub-1"), "the reviewer's chat");
}

#[test]
fn a_chat_on_screen_says_its_updates_were_seen_now_and_then() {
    let (ctx, mut app) = waiting_app();
    sent(&app);
    frame_at(&ctx, &mut app, vec![], 10.0);
    assert!(!sent(&app).iter().any(|s| s.contains("seen")), "the chat isn't on screen");
    focus(&ctx, &mut app, BufferKind::Chat);
    app.desk.chat_mut().ui.slot = SlotId::new("sub-1");
    frame_at(&ctx, &mut app, vec![], 20.0);
    frame_at(&ctx, &mut app, vec![], 20.5);
    let seen: Vec<String> = sent(&app).into_iter().filter(|s| s.contains("seen")).collect();
    assert_eq!(seen, ["POST /api/notifications/seen"], "once, not every frame");
    frame_at(&ctx, &mut app, vec![], 23.0);
    assert_eq!(sent(&app).iter().filter(|s| s.contains("seen")).count(), 1, "again after a while, while it has updates");
    let dismissed = Notification { state: NotificationState::Dismissed, ..app.desk.shared.agents.get().store().notifications[&NotificationId::new("turn:u1")].clone() };
    app.desk.shared.agents.get_mut().apply(ClientEvent::NotificationUpdated { notification: dismissed });
    frame_at(&ctx, &mut app, vec![], 30.0);
    assert!(!sent(&app).iter().any(|s| s.contains("seen")), "none left: nothing said");
}

#[test]
fn the_look_has_the_keyboard_its_keys_move_and_label_its_parts_and_nothing_reaches_the_column_under_it() {
    let (ctx, mut app) = waiting_app();
    let before = selected(&app).to_owned();
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.next_waiting"}"#).unwrap();
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.focused_kind(), BufferKind::Inbox, "the inbox it holds has the keys");
    assert_eq!(app.desk.inbox().cursor_key().as_deref(), Some("1"), "it opens on what waits on you: its request");
    // Its answers show under it; clicked, one answers.
    sent(&app);
    click_on(&ctx, &mut app, "Deny");
    assert_eq!(sent(&app), ["POST /api/threads/t/requests/r1"], "Deny, under its request");
    keys(&ctx, &mut app, "k");
    assert_eq!((selected(&app), app.desk.inbox().cursor_key().as_deref()), (before.as_str(), Some("0")), "k moves over its parts, not the graph under it");
    let under: Vec<String> = crate::actions::ActionProvider::actions(app.desk.inbox()).into_iter().filter(|a| a.at().is_some_and(|at| at == crate::actions::At::Item)).map(|a| a.id).collect();
    assert!(under.is_empty(), "off its request, its answers aren't under anything: {under:?}");
    // f labels what's in it; the bar offers its keys.
    keys(&ctx, &mut app, "gg");
    frame(&ctx, &mut app, key("f"));
    let Some(Capture::Hint(hint)) = &app.capture else { panic!("hint mode") };
    assert_eq!(hint.labels.len(), app.desk.inbox().spots().len(), "its parts, labelled");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    let bound = app.bound_actions();
    let bar: Vec<String> = app.bar_keys(&bound).iter().map(|b| b.action.id.clone()).collect();
    assert!(bar.iter().any(|id| id == "inbox.next") && bar.iter().any(|id| id.starts_with("request.")), "{bar:?}");
}

#[test]
fn several_answers_of_one_kind_are_numbered_not_all_y() {
    let (ctx, mut app) = waiting_app();
    let option = |id: &str, label: &str| ApprovalOption { id: id.into(), decision: ApprovalDecision::Accept, label: label.into(), warning: None };
    let mut asked = app.desk.shared.agents.get().store().threads[&ThreadId::new("t")].request(&RequestId::new("r1")).unwrap().clone();
    asked.options = vec![option("yes", "Yes, it worked"), option("no", "Still nothing")];
    app.desk.shared.agents.get_mut().apply(ClientEvent::Session { event: AdapterEvent::RequestUpdated { request: asked } });
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.next_waiting"}"#).unwrap();
    frame(&ctx, &mut app, vec![]);
    let bound = app.bound_actions();
    let answers: Vec<String> = bound.iter().filter(|b| b.action.id.starts_with("request.r1.")).map(|b| b.key_hint()).collect();
    assert_eq!(answers, ["Space r 1", "Space r 2"], "numbered under r, neither on y");
}

#[test]
fn the_bells_look_says_whether_system_notifications_reach_you_and_its_list_switches_them() {
    let (ctx, mut app) = waiting_app();
    let backend = std::sync::Arc::new(crate::alerts::tests::Recorded::default());
    app.alerts = crate::alerts::Alerts::with(backend.clone());
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.waiting","value":"alerts"}"#).unwrap();
    assert!(backend.asked.load(std::sync::atomic::Ordering::Relaxed), "turned on from the list: the platform is asked");
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains(&format!("{} 2 •", egui_phosphor::regular::BELL)), "on, not allowed yet: a dot: {text}");
    *backend.permission.lock().unwrap() = Some(crate::alerts::Permission::Granted);
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains(&format!("{} 2", egui_phosphor::regular::BELL_RINGING)), "on: ringing: {text}");
}

/// Pointer and touch, the same in every view: a click on an item puts the
/// cursor there; a double click opens it (Enter).
#[test]
fn a_click_puts_the_cursor_on_an_item_and_a_double_click_opens_it() {
    let (ctx, mut app) = waiting_app();
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.next_waiting"}"#).unwrap();
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.inbox().cursor_key().as_deref(), Some("1"), "on its request");
    click_on(&ctx, &mut app, "req:r1 title");
    assert_eq!(app.desk.inbox().cursor_key().as_deref(), Some("0"), "clicked: on what it says");
    assert!(app.quick_look.is_some(), "a click moves, it doesn't open");
    double_click_on(&ctx, &mut app, "req:r1 title");
    assert_eq!((app.quick_look.is_none(), app.desk.panels.focused_kind()), (true, BufferKind::Chat), "a double click opens it: its chat");
}
