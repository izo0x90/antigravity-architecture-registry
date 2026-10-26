//! An agent's sub-agents in its chat: a strip above the typing box while
//! any is at work (a few each a chip, more folded into how many), a chip
//! a place (a tap: the cursor on where it started, its Open under it),
//! opened in the quick look (its own work alone, stopping it where its
//! harness can, where it started), a line where it started (its steps
//! folded under it; Enter opens it, `] a` goes to the next), how many are
//! at work on the live line and the panel's top line.

use super::*;
use crate::chat::tests::item;
use harness_contracts::{AdapterEvent, ClientEvent, ItemBody, ItemId, ItemStatus, Subagent, SubagentId, SubagentStatus, ThreadId, TurnId, TurnItem};

fn session(event: AdapterEvent) -> ClientEvent {
    ClientEvent::Session { event }
}

/// Sub-agent `n` (its item `i{at}`), `status`, stoppable alone or not,
/// started now.
fn subagent(n: u32, at: u32, status: SubagentStatus, can_cancel: bool) -> Subagent {
    Subagent {
        id: SubagentId::new(format!("a{n}")),
        thread_id: ThreadId::new("t"),
        item_id: ItemId::new(format!("i{at}")),
        title: format!("Helper {n}"),
        kind: Some("general-purpose".into()),
        description: Some(format!("Task {n}")),
        prompt: Some(format!("Do task {n} carefully.")),
        answer: None,
        status,
        started_ms: crate::attention::now_ms(),
        ended_ms: None,
        can_cancel,
    }
}

/// A step of the sub-agent whose item is `i{parent}`: a call, `title`.
fn step(n: u32, parent: u32, title: &str) -> ClientEvent {
    session(AdapterEvent::ItemUpdated {
        item: TurnItem { id: ItemId::new(format!("i{n}")), thread_id: ThreadId::new("t"), turn_id: TurnId::new("turn"), native_id: None, parent_id: Some(ItemId::new(format!("i{parent}"))), ordinal: n, status: ItemStatus::Completed, title: Some(title.into()), body: ItemBody::ToolCall { kind: harness_contracts::ToolKind::Read, name: "read".into(), input: None, output: vec![], locations: vec![] } },
    })
}

/// A chat (the lead's) with `subagents`, each its item where it started
/// and a step of its (reading `file{n}.rs`).
fn chat_with(subagents: Vec<Subagent>) -> (egui::Context, CockpitApp) {
    let (ctx, mut app) = full_app();
    let mut events = crate::chat::tests::conversation_events();
    for s in &subagents {
        let at: u32 = s.item_id.as_str()[1..].parse().unwrap();
        events.push(item(at, ItemBody::Subagent { subagent_id: s.id.clone() }));
        events.push(session(AdapterEvent::SubagentUpdated { subagent: s.clone() }));
        let n: u32 = s.id.as_str()[1..].parse().unwrap();
        events.push(step(at + 100, at, &format!("Read file{n}.rs")));
    }
    events.into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    focus(&ctx, &mut app, BufferKind::Chat);
    frame(&ctx, &mut app, vec![]);
    (ctx, app)
}

/// Where `words` are drawn (the first place).
fn at(output: &egui::FullOutput, words: &str) -> egui::Pos2 {
    drawn_containing(output, words).first().unwrap_or_else(|| panic!("{words:?} isn't drawn: {}", texts(output))).center()
}

#[test]
fn those_at_work_are_a_strip_of_chips_and_many_fold_into_how_many() {
    let (ctx, mut app) = chat_with(vec![subagent(1, 200, SubagentStatus::Running, true), subagent(2, 201, SubagentStatus::Waiting, false)]);
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Helper 1 · Read file1.rs") && text.contains("Helper 2 · waiting on you"), "each a chip, what it's doing from its steps: {text}");
    // Where each started: a line, its state; its step folded under it.
    assert!(text.contains("working") && text.contains("1 step by its sub-agent"), "{text}");
    // Many: one chip, how many; tapped, each; few again, folded again.
    let (ctx, mut app) = chat_with((1..=4).map(|n| subagent(n, 200 + n, SubagentStatus::Running, false)).collect());
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("4 sub-agents working") && !text.contains("Helper 1 · Read"), "{text}");
    let folded = at(&frame(&ctx, &mut app, vec![]), "4 sub-agents working");
    click(&ctx, &mut app, folded);
    frame(&ctx, &mut app, vec![]);
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Helper 1 · Read file1.rs") && text.contains("Helper 4 · Read file4.rs"), "{text}");
    // Two end, and their while in sight goes: few again; more again, folded.
    for n in 3..=4 {
        told(&app, session(AdapterEvent::SubagentUpdated { subagent: Subagent { ended_ms: Some(crate::attention::now_ms()), ..subagent(n, 200 + n, SubagentStatus::Completed, false) } }));
    }
    frame_at(&ctx, &mut app, vec![], 100.0);
    frame_at(&ctx, &mut app, vec![], 120.0);
    for n in 5..=6 {
        told(&app, item(200 + n, ItemBody::Subagent { subagent_id: SubagentId::new(format!("a{n}")) }));
        told(&app, session(AdapterEvent::SubagentUpdated { subagent: subagent(n, 200 + n, SubagentStatus::Running, false) }));
    }
    frame_at(&ctx, &mut app, vec![], 120.1);
    assert!(texts(&frame_at(&ctx, &mut app, vec![], 120.2)).contains("4 sub-agents working"), "folded again");
}

/// One seen to end stays a while (as the config says), then goes; one
/// that had ended before it was seen never shows.
#[test]
fn one_seen_to_end_stays_a_while_and_one_ended_before_never_shows() {
    let (ctx, mut app) = chat_with(vec![subagent(1, 200, SubagentStatus::Running, false), subagent(2, 201, SubagentStatus::Completed, false)]);
    frame_at(&ctx, &mut app, vec![], 10.0);
    told(&app, session(AdapterEvent::SubagentUpdated { subagent: Subagent { ended_ms: Some(crate::attention::now_ms()), ..subagent(1, 200, SubagentStatus::Completed, false) } }));
    let text = texts(&frame_at(&ctx, &mut app, vec![], 11.0));
    assert!(text.contains("Helper 1 · done") && !text.contains("Helper 2 ·"), "{text}");
    let text = texts(&frame_at(&ctx, &mut app, vec![], 30.0));
    assert!(!text.contains("Helper 1 ·") && text.contains("Helper 1"), "gone from the strip, its line stays: {text}");
}

#[test]
fn a_chip_opens_its_work_in_the_quick_look_and_it_stops_alone_where_it_can() {
    let (ctx, mut app) = chat_with(vec![subagent(1, 200, SubagentStatus::Running, true), subagent(2, 201, SubagentStatus::Running, false)]);
    let chip = at(&frame(&ctx, &mut app, vec![]), "Helper 1 · Read file1.rs");
    click(&ctx, &mut app, chip);
    // A tap: the cursor where it started; its Open under it.
    at(&frame(&ctx, &mut app, vec![]), "Open");
    // Typing to the agent meanwhile: opened, the quick look takes the keys.
    app.input_mode = InputMode::Insert { at: Host::Panel(app.desk.panels.focused()) };
    app.asked = Some(("chat.open_subagent".to_owned(), None));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(app.quick_look.is_some() && text.contains("Sub-agent of") && text.contains("Task 1") && text.contains("Read file1.rs") && text.contains("Stop it"), "in the quick look: {text}");
    assert!(!text.contains("Looking at it."), "its work alone: {text}");
    assert_eq!(app.input_mode, InputMode::Normal, "typing under it stopped");
    // A step as it comes.
    told(&app, step(301, 200, "Read src/lex.rs"));
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("Read src/lex.rs"));
    // Stopped alone: asked of the server, once.
    sent(&app);
    let stop = at(&frame(&ctx, &mut app, vec![]), "Stop it");
    click(&ctx, &mut app, stop);
    assert!(sent(&app).iter().any(|c| c == "POST /api/threads/t/subagents/a1/cancel"), "{:?}", sent(&app));
    let stop = at(&frame(&ctx, &mut app, vec![]), "Stop it");
    click(&ctx, &mut app, stop);
    assert!(sent(&app).is_empty(), "being stopped: not asked again");
    // A tap outside closes it.
    click(&ctx, &mut app, egui::pos2(4.0, SCREEN.y - 4.0));
    assert!(app.quick_look.is_none());
    // One its harness can't stop alone: its stop there, not offered.
    let chip = at(&frame(&ctx, &mut app, vec![]), "Helper 2 · Read file2.rs");
    click(&ctx, &mut app, chip);
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    let stop = at(&frame(&ctx, &mut app, vec![]), "Stop it");
    click(&ctx, &mut app, stop);
    assert!(sent(&app).is_empty(), "{:?}", sent(&app));
}

#[test]
fn the_next_one_is_a_key_away_enter_opens_it_and_where_it_started_is_too() {
    let (ctx, mut app) = chat_with(vec![subagent(1, 200, SubagentStatus::Completed, false), subagent(2, 201, SubagentStatus::Running, false)]);
    keys(&ctx, &mut app, "]a");
    let first = app.desk.chat().ui.cursor;
    keys(&ctx, &mut app, "]a");
    let second = app.desk.chat().ui.cursor;
    assert!(second > first && first.is_some(), "to the next: {first:?} {second:?}");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Task 2"), "Enter opens it: {:?} {text}", app.note);
    // Where it started, from it: the panel's chat, its cursor on its line.
    keys(&ctx, &mut app, "gw");
    assert!(app.quick_look.is_none());
    assert_eq!(app.desk.chat().ui.cursor, second);
    keys(&ctx, &mut app, "[a");
    assert_eq!(app.desk.chat().ui.cursor, first);
}

#[test]
fn how_many_are_at_work_is_said_on_the_live_line_and_the_panels_top_line() {
    let (_, app) = chat_with(vec![subagent(1, 200, SubagentStatus::Running, false), subagent(2, 201, SubagentStatus::Waiting, false), subagent(3, 202, SubagentStatus::Completed, false)]);
    assert_eq!(app.desk.chat().subagents_working(), 2);
    let status: Vec<String> = app.desk.view(BufferKind::Chat).status().into_iter().map(|s| s.text).collect();
    assert!(status.iter().any(|s| s.ends_with(" 2")), "{status:?}");
}
