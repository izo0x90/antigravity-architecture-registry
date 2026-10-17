//! The terminal buffer in the cockpit: focusing it starts typing; every
//! key goes to its program but the leave keys; it shows what its socket
//! sends; each panel showing it holds its own socket at its own size, let
//! go when it isn't drawn.

use harness_contracts::{Mods, TermKey, TerminalInput, ToTerminal};

use super::*;
use crate::terminal::TerminalBuffer;

fn inputs(app: &CockpitApp) -> Vec<TerminalInput> {
    app.desk.terminal().sent().into_iter().filter_map(|s| if let ToTerminal::Input { input } = s { Some(input) } else { None }).collect()
}

fn text(t: &str) -> TerminalInput {
    TerminalInput::Text { text: t.into() }
}

fn named(key: TermKey, ctrl: bool) -> TerminalInput {
    TerminalInput::Key { key, mods: Mods { ctrl, ..Mods::default() } }
}

#[test]
fn focused_it_types_every_key_into_its_program_but_the_leave_keys() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Terminal);
    assert!(matches!(app.input_mode, InputMode::Insert { .. }), "focusing it starts typing");
    let bar = texts(&frame(&ctx, &mut app, vec![]));
    assert!(bar.contains("Escape Escape") && bar.contains("stops typing"), "the key bar says how to leave");
    frame(&ctx, &mut app, key("l"));
    frame(&ctx, &mut app, key("s"));
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, press(egui::Key::Tab, false));
    frame(&ctx, &mut app, press(egui::Key::ArrowUp, false));
    ctrl(&ctx, &mut app, egui::Key::C);
    assert_eq!(inputs(&app), [text("l"), text("s"), named(TermKey::Enter, false), named(TermKey::Tab, false), named(TermKey::Up, false), named(TermKey::Char('c'), true)], "keys the cockpit binds (Tab, Enter) are the program's while typing");
    assert!(matches!(app.input_mode, InputMode::Insert { .. }), "Tab didn't move to another panel");

    // Escape once: held for a moment (it may be the first of two), then the program's.
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 10.0);
    assert_eq!(inputs(&app).len(), 6, "held");
    frame_at(&ctx, &mut app, vec![], 10.5);
    assert_eq!(inputs(&app).last(), Some(&named(TermKey::Escape, false)), "the wait passed: vim gets its Escape");
    assert!(matches!(app.input_mode, InputMode::Insert { .. }));
    // Twice: typing stops, and the program gets neither.
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 20.0);
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 20.1);
    assert_eq!(app.input_mode, InputMode::Normal);
    assert_eq!(inputs(&app).len(), 7);
    // Not typing, keys are the cockpit's: i types again (and isn't typed).
    frame(&ctx, &mut app, key("i"));
    assert!(matches!(app.input_mode, InputMode::Insert { .. }));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(inputs(&app).len(), 7, "the i that started typing isn't the program's");
}

#[test]
fn a_text_field_still_leaves_on_one_escape() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    frame(&ctx, &mut app, key("i"));
    assert!(matches!(app.input_mode, InputMode::Insert { .. }));
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!(app.input_mode, InputMode::Normal, "the chat's field takes text: Escape leaves it");
}

#[test]
fn it_shows_the_screen_it_was_sent_and_says_its_size() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Terminal);
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("Cargo.toml  src"), "{drawn}");
    let sizes: Vec<_> = app.desk.terminal().sent().into_iter().filter(|s| matches!(s, ToTerminal::Size { .. })).collect();
    assert_eq!(sizes.len(), 1, "its size, said once: {sizes:?}");
}

#[test]
fn each_panel_showing_it_is_a_reader_and_one_not_drawn_lets_its_socket_go() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Terminal);
    // Another terminal buffer, in another panel: the same terminal.
    app.desk.panels.focus(0);
    app.new_view(BufferKind::Terminal);
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    let readers = app.desk.all_of::<TerminalBuffer>(BufferKind::Terminal).iter().filter(|t| t.sent().iter().any(|s| matches!(s, ToTerminal::Size { .. }))).count();
    assert_eq!(readers, 2, "two panels, two readers, each its own size");
    // Its panel shows something else (it's hidden): its socket goes.
    app.desk.panels.focus(0);
    app.new_view(BufferKind::Graph);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    let linked = app.desk.all_of::<TerminalBuffer>(BufferKind::Terminal).iter().filter(|t| t.linked()).count();
    assert_eq!(linked, 1, "the hidden one let its socket go");
}

#[test]
fn with_no_terminal_one_is_opened_and_shown_and_an_ended_one_starts_again() {
    let (ctx, mut app) = app_with_graph();
    focus(&ctx, &mut app, BufferKind::Terminal);
    let asked = sent(&app);
    assert!(asked.contains(&"GET /api/terminals".to_owned()) && !asked.contains(&"POST /api/terminals".to_owned()), "which there are asked first: none heard of isn't none: {asked:?}");
    app.desk.shared.terminals.get_mut().answered_list(Vec::new());
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert!(sent(&app).contains(&"POST /api/terminals".to_owned()), "the server has none: one is opened");
    app.desk.shared.terminals.get_mut().answered_open(Ok(crate::terminals::tests::terminal("t7")));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.terminal().target(), Some(Target::Terminal(harness_contracts::TerminalId::new("t7"))), "it shows the one opened");
    // Its shell ends: it says so, and Enter starts it again.
    let mut ended = crate::terminals::tests::terminal("t7");
    ended.status = harness_contracts::TerminalStatus::Ended { code: Some(2) };
    app.desk.shared.terminals.get_mut().apply(&harness_contracts::ClientEvent::TerminalUpdated { terminal: ended });
    app.input_mode = InputMode::Normal;
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("ended (2)"), "{drawn}");
    sent(&app);
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert!(sent(&app).contains(&"POST /api/terminals/t7/restart".to_owned()));
}

#[test]
fn the_pop_up_terminal_is_its_own_over_everything_and_its_key_hides_it() {
    let (ctx, mut app) = full_app();
    app.desk.shared.terminals.get_mut().answered_list(vec![crate::terminals::tests::terminal("t1")]);
    frame(&ctx, &mut app, vec![]);
    sent(&app);
    let pop = BufferId::new(BufferKind::Terminal, crate::quick_look::PANE);
    ctrl(&ctx, &mut app, egui::Key::Backtick);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.quick_look.as_ref().map(|q| q.buffer), Some(pop), "over everything, in the quick look");
    assert!(sent(&app).contains(&"POST /api/terminals".to_owned()), "a terminal of its own, not the panel's");
    app.desk.shared.terminals.get_mut().answered_open(Ok(crate::terminals::tests::terminal("t9")));
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(app.desk.view(BufferKind::Terminal).target(), Some(Target::Terminal(harness_contracts::TerminalId::new("t9"))));
    assert_eq!(app.input_mode, InputMode::Insert { at: Host::Look }, "focused, it types");
    // Leave typing; its key hides it.
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 30.0);
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 30.1);
    ctrl(&ctx, &mut app, egui::Key::Backtick);
    assert!(app.quick_look.is_none(), "the same key hides it");
    // Again: the same terminal, typing again; and kept for the next load.
    ctrl(&ctx, &mut app, egui::Key::Backtick);
    for _ in 0..2 {
        frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(app.desk.view(BufferKind::Terminal).target(), Some(Target::Terminal(harness_contracts::TerminalId::new("t9"))));
    assert!(!sent(&app).contains(&"POST /api/terminals".to_owned()), "no new one");
    assert_eq!(app.input_mode, InputMode::Insert { at: Host::Look });
    let kept = app.saved_ui().kept;
    assert!(kept.iter().any(|(b, v)| *b == pop && v == "t9"), "its terminal is kept: {kept:?}");
}
