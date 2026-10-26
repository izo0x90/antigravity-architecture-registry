//! An agent's sub-agents in its chat: a strip above the typing box while
//! any is at work (a few each a chip, more folded into how many), a chip
//! a place (a tap: the cursor on where it started, its Open under it),
//! opened over the panels (its steps, stopping it alone where its harness
//! can), a line where it started (Enter opens it, `] a` goes to the next),
//! how many are at work on the live line and the panel's top line.

use super::*;
use crate::chat::tests::item;
use harness_contracts::{AdapterEvent, ClientEvent, ItemBody, ItemId, ItemStatus, Subagent, SubagentId, SubagentStatus, ThreadId, TurnId, TurnItem};

fn session(event: AdapterEvent) -> ClientEvent {
    ClientEvent::Session { event }
}

/// Sub-agent `n` (its item `i{at}`), `status`, stoppable alone or not.
fn subagent(n: u32, at: u32, status: SubagentStatus, can_cancel: bool) -> Subagent {
    Subagent {
        id: SubagentId::new(format!("a{n}")),
        thread_id: ThreadId::new("t"),
        item_id: ItemId::new(format!("i{at}")),
        title: format!("Helper {n}"),
        kind: Some("general-purpose".into()),
        description: Some(format!("Task {n}")),
        prompt: Some(format!("Do task {n} carefully.")),
        doing: Some(format!("Reading file{n}.rs")),
        status,
        can_cancel,
    }
}

/// A step of the sub-agent whose item is `i{parent}`: a call, `title`.
fn step(n: u32, parent: u32, title: &str) -> ClientEvent {
    session(AdapterEvent::ItemUpdated {
        item: TurnItem { id: ItemId::new(format!("i{n}")), thread_id: ThreadId::new("t"), turn_id: TurnId::new("turn"), native_id: None, parent_id: Some(ItemId::new(format!("i{parent}"))), ordinal: n, status: ItemStatus::Completed, title: Some(title.into()), body: ItemBody::ToolCall { kind: harness_contracts::ToolKind::Read, name: "read".into(), input: None, output: vec![], locations: vec![] } },
    })
}

/// A chat (the lead's) with `subagents`, each its item where it started.
fn chat_with(subagents: Vec<Subagent>) -> (egui::Context, CockpitApp) {
    let (ctx, mut app) = full_app();
    let mut events = crate::chat::tests::conversation_events();
    for s in &subagents {
        let at: u32 = s.item_id.as_str()[1..].parse().unwrap();
        events.push(item(at, ItemBody::Subagent { subagent_id: s.id.clone() }));
        events.push(session(AdapterEvent::SubagentUpdated { subagent: s.clone() }));
    }
    events.into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    focus(&ctx, &mut app, BufferKind::Chat);
    frame(&ctx, &mut app, vec![]);
    (ctx, app)
}

fn shown(output: &egui::FullOutput) -> String {
    texts(output)
}

#[test]
fn those_at_work_are_a_strip_of_chips_and_many_fold_into_how_many() {
    let (ctx, mut app) = chat_with(vec![subagent(1, 200, SubagentStatus::Running, true), subagent(2, 201, SubagentStatus::Waiting, false)]);
    let text = shown(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Helper 1 · Reading file1.rs") && text.contains("Helper 2 · Reading file2.rs"), "each a chip: {text}");
    // Where each started: a line, its state.
    let dot = egui_phosphor::regular::CIRCLE;
    assert!(text.contains(&format!("{dot} working")) && text.contains(&format!("{dot} waiting on you")), "{text}");
    // Many: one chip, how many; tapped, each.
    let (ctx, mut app) = chat_with((1..=4).map(|n| subagent(n, 200 + n, SubagentStatus::Running, false)).collect());
    let text = shown(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("4 sub-agents working") && !text.contains("Helper 1 · Reading"), "{text}");
    let at = drawn_text_at(&frame(&ctx, &mut app, vec![]), "4 sub-agents working");
    click(&ctx, &mut app, at);
    // (Taller: the stream makes room for it the next frame.)
    frame(&ctx, &mut app, vec![]);
    let text = shown(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Helper 1 · Reading file1.rs") && text.contains("Helper 4 · Reading file4.rs"), "{text}");
    // None at work (and a while since one ended): no strip.
    let (ctx, mut app) = chat_with(vec![subagent(1, 200, SubagentStatus::Completed, false)]);
    frame_at(&ctx, &mut app, vec![], 100.0);
    let text = shown(&frame_at(&ctx, &mut app, vec![], 120.0));
    assert!(!text.contains("Helper 1 ·") && text.contains("Helper 1"), "its line stays, no chip: {text}");
}

#[test]
fn a_chip_opens_it_over_the_panels_its_steps_as_they_come_and_it_stops_alone_where_it_can() {
    let (ctx, mut app) = chat_with(vec![subagent(1, 200, SubagentStatus::Running, true), subagent(2, 201, SubagentStatus::Running, false)]);
    app.desk.shared.agents.get_mut().apply(step(300, 200, "Read src/parse.rs"));
    let at = drawn_text_at(&frame(&ctx, &mut app, vec![]), "Helper 1 · Reading file1.rs");
    click(&ctx, &mut app, at);
    // A tap: the cursor where it started; its Open under it.
    let open = drawn_text_at(&frame(&ctx, &mut app, vec![]), "Open");
    click(&ctx, &mut app, open);
    frame(&ctx, &mut app, vec![]);
    let text = shown(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Task 1") && text.contains("Read src/parse.rs") && text.contains("Stop it"), "over the panels: {text}");
    // A step as it comes.
    app.desk.shared.agents.get_mut().apply(step(301, 200, "Read src/lex.rs"));
    assert!(shown(&frame(&ctx, &mut app, vec![])).contains("Read src/lex.rs"));
    // Stopped alone: asked of the server.
    sent(&app);
    let stop = drawn_text_at(&frame(&ctx, &mut app, vec![]), "Stop it");
    click(&ctx, &mut app, stop);
    assert!(sent(&app).iter().any(|c| c == "POST /api/threads/t/subagents/a1/cancel"), "{:?}", sent(&app));
    // A tap outside closes it.
    tap_corner(&ctx, &mut app);
    assert!(!shown(&frame(&ctx, &mut app, vec![])).contains("Task 1"));
    // One its harness can't stop alone: said, not offered.
    let at = drawn_text_at(&frame(&ctx, &mut app, vec![]), "Helper 2 · Reading file2.rs");
    click(&ctx, &mut app, at);
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    let text = shown(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("can't stop it alone") && !text.contains("Stop it"), "{text}");
}

#[test]
fn the_next_one_is_a_key_away_and_enter_opens_it() {
    let (ctx, mut app) = chat_with(vec![subagent(1, 200, SubagentStatus::Completed, false), subagent(2, 201, SubagentStatus::Running, false)]);
    keys(&ctx, &mut app, "]a");
    let first = app.desk.chat().ui.cursor;
    keys(&ctx, &mut app, "]a");
    assert!(app.desk.chat().ui.cursor > first && first.is_some(), "to the next: {first:?} {:?}", app.desk.chat().ui.cursor);
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert!(shown(&frame(&ctx, &mut app, vec![])).contains("Task 2"), "Enter opens it");
    // Where it started, from it: the chat's cursor on its line.
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
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

/// Where `words` were drawn (the first drawn that begins so).
fn drawn_text_at(output: &egui::FullOutput, words: &str) -> egui::Pos2 {
    let mut found = None;
    for clipped in &output.shapes {
        collect_text(&clipped.shape, &mut |text, rect| {
            if found.is_none() && text.contains(words) {
                found = Some(rect.center());
            }
        });
    }
    found.unwrap_or_else(|| panic!("{words:?} isn't drawn"))
}

fn collect_text(shape: &egui::Shape, out: &mut dyn FnMut(&str, egui::Rect)) {
    match shape {
        egui::Shape::Text(t) => out(t.galley.text(), egui::Rect::from_min_size(t.pos, t.galley.size())),
        egui::Shape::Vec(list) => list.iter().for_each(|s| collect_text(s, out)),
        _ => {}
    }
}

fn click(ctx: &egui::Context, app: &mut CockpitApp, at: egui::Pos2) {
    let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(ctx, app, vec![egui::Event::PointerMoved(at), button(true)]);
    frame(ctx, app, vec![button(false)]);
}

fn tap_corner(ctx: &egui::Context, app: &mut CockpitApp) {
    click(ctx, app, egui::pos2(4.0, SCREEN.y - 4.0));
}

