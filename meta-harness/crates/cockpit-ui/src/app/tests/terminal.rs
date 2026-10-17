//! The terminal buffer in the cockpit: focusing it starts typing; every
//! key goes to its program but the leave keys; it shows what its socket
//! sends; each panel showing it holds its own socket at its own size, let
//! go when it isn't drawn.

use harness_contracts::{Mods, TermKey, TerminalInput, ToTerminal};

use super::*;
use crate::hosted::Hosted;
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

/// Out of typing, in the terminal, its cursor on the live screen's last line.
fn not_typing(ctx: &egui::Context, app: &mut CockpitApp) {
    focus(ctx, app, BufferKind::Terminal);
    frame_at(ctx, app, press(egui::Key::Escape, false), 40.0);
    frame_at(ctx, app, press(egui::Key::Escape, false), 40.1);
    assert_eq!(app.input_mode, InputMode::Normal);
}

#[test]
fn search_goes_through_the_whole_scrollback_on_the_server() {
    let (ctx, mut app) = full_app();
    // 600 lines of scrollback above the screen, none of it fetched.
    app.desk.terminal_mut().arrive(vec![crate::terminal::tests::screen_under(&["$ ls", "Cargo.toml  src", "$ "], 600)]);
    not_typing(&ctx, &mut app);
    keys(&ctx, &mut app, "/make");
    let asked: Vec<String> = app.desk.terminal().sent().into_iter().filter_map(|s| if let ToTerminal::Search { query } = s { Some(query) } else { None }).collect();
    assert_eq!(asked.last().map(String::as_str), Some("make"), "the server is asked as it's typed: {asked:?}");
    // The server found it 500 lines up: line 100.
    let found = harness_contracts::Found { line: 100, col: 2, len: 4, text: "$ make all".into() };
    app.desk.terminal_mut().arrive(vec![harness_contracts::FromTerminal::Found { query: "make".into(), found: vec![found] }]);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, key("n"));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(cursor(&app).as_deref(), Some("100"), "n goes to the line found, far up");
}

#[test]
fn a_file_on_a_terminal_line_is_gone_to_in_the_code() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![crate::terminal::tests::screen(&["$ cargo build", "error: src/b.rs:9:5: expected `;`", "$ "])]);
    not_typing(&ctx, &mut app);
    let spots = app.desk.terminal().spots();
    let link = spots.iter().find(|s| s.key.ends_with(":7")).expect("the file's hint, at its column");
    app.desk.view_mut(BufferKind::Terminal).move_to(&link.key.clone());
    assert_eq!(app.desk.terminal().target(), Some(Target::Span { file: "src/b.rs".into(), start: 9, end: 9 }), "on the file, the target is its line");
    frame(&ctx, &mut app, key("g"));
    frame(&ctx, &mut app, key("d"));
    frame(&ctx, &mut app, vec![]);
    let calls = sent(&app);
    assert!(calls.iter().any(|c| c.contains("src/b.rs") || c.contains("src%2Fb.rs")), "the code view opens the file: {calls:?} {:?} {:?}", app.note, app.desk.view(app.focused_kind()).target());
}

/// The lines the terminal asked its server for, answered.
fn answer_lines(app: &mut CockpitApp) {
    let asked: Vec<(u64, u32)> = app.desk.terminal().sent().into_iter().filter_map(|s| if let ToTerminal::Lines { from, count } = s { Some((from, count)) } else { None }).collect();
    let answers = asked.into_iter().map(|(from, count)| crate::terminal::tests::numbered_lines(from, count)).collect();
    app.desk.terminal_mut().arrive(answers);
}

/// The terminal's lines drawn this frame, by number.
fn drawn_lines(output: &egui::FullOutput) -> Vec<u64> {
    texts(output).lines().filter_map(|l| l.strip_prefix("line ")?.parse().ok()).collect()
}

#[test]
fn every_frame_draws_whole_rows_and_the_live_screen_follows_the_output() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Terminal);
    // Output as fast as it comes: a line more each frame (Enter held down).
    for top in 1000..1060 {
        app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered(top, 0)]);
        let drawn = drawn_lines(&frame(&ctx, &mut app, vec![]));
        answer_lines(&mut app);
        assert!(drawn.contains(&(top + 23)), "the live screen's last line shows, every frame: {drawn:?}");
        assert!(drawn.windows(2).all(|w| w[1] == w[0] + 1), "whole rows, none missing: {drawn:?}");
    }
}

#[test]
fn scrolled_back_it_stays_on_what_is_read_while_output_arrives_and_typing_goes_back_to_it() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered(1000, 0)]);
    not_typing(&ctx, &mut app);
    answer_lines(&mut app);
    for _ in 0..100 {
        frame(&ctx, &mut app, key("k"));
        answer_lines(&mut app);
    }
    let read = drawn_lines(&frame(&ctx, &mut app, vec![]));
    assert!(!read.contains(&1023), "scrolled back off the live screen: {read:?}");
    for top in 1001..1200 {
        app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered(top, top.saturating_sub(1000))]);
        frame(&ctx, &mut app, vec![]);
        answer_lines(&mut app);
    }
    assert_eq!(drawn_lines(&frame(&ctx, &mut app, vec![])), read, "output arrived: what's read stays put");
    // Past what's kept: its lines gone, it shows the oldest there is.
    app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered(2000, 1500)]);
    frame(&ctx, &mut app, vec![]);
    answer_lines(&mut app);
    let drawn = drawn_lines(&frame(&ctx, &mut app, vec![]));
    assert_eq!(drawn.first(), Some(&1500), "{drawn:?}");
    // Typing: back to the live screen.
    frame(&ctx, &mut app, key("i"));
    frame(&ctx, &mut app, key("x"));
    frame(&ctx, &mut app, vec![]);
    answer_lines(&mut app);
    let drawn = drawn_lines(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains(&2023), "{drawn:?}");
}

#[test]
fn going_far_back_keeps_what_showed_until_the_lines_come() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered(5000, 0)]);
    not_typing(&ctx, &mut app);
    answer_lines(&mut app);
    let before = drawn_lines(&frame(&ctx, &mut app, vec![]));
    frame(&ctx, &mut app, key("g"));
    let waiting = drawn_lines(&frame(&ctx, &mut app, key("g")));
    assert_eq!(waiting, before, "the top's lines haven't come: what showed stays, no blank rows");
    answer_lines(&mut app);
    let drawn = drawn_lines(&frame(&ctx, &mut app, vec![]));
    assert_eq!(drawn.first(), Some(&0), "they came: the top shows");
}

#[test]
fn drawing_with_ten_thousand_lines_of_scrollback_costs_what_a_screenful_does() {
    let (ctx, mut app) = full_app();
    not_typing(&ctx, &mut app);
    // The time a frame takes, at most, over many (scrolling up a line each).
    let slowest = |app: &mut CockpitApp| {
        (0..30)
            .map(|_| {
                let start = std::time::Instant::now();
                frame(&ctx, app, key("k"));
                start.elapsed()
            })
            .max()
            .unwrap()
    };
    let screenful = slowest(&mut app);
    app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered(10_000, 0), crate::terminal::tests::numbered_lines(0, 10_000)]);
    frame(&ctx, &mut app, key("G"));
    let ten_thousand = slowest(&mut app);
    eprintln!("slowest frame: {screenful:?} with a screenful, {ten_thousand:?} with 10,000 lines");
    assert!(ten_thousand < screenful * 3 + std::time::Duration::from_millis(10), "{ten_thousand:?} against {screenful:?}");
}

#[test]
fn scrolled_back_while_typing_the_programs_cursor_stays_on_the_grid_for_the_browser() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered(1000, 0)]);
    focus(&ctx, &mut app, BufferKind::Terminal);
    let grid = app.desk.terminal().typing_rect().expect("drawn");
    let wheel = vec![egui::Event::PointerMoved(grid.center()), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, 5000.0), modifiers: egui::Modifiers::NONE }];
    frame(&ctx, &mut app, wheel);
    let ime = frame(&ctx, &mut app, vec![]).platform_output.ime.expect("typing: where its cursor is");
    assert!(matches!(app.input_mode, InputMode::Insert { .. }), "still typing");
    assert!(!app.desk.terminal().status().is_empty(), "scrolled back");
    // Off the grid, the browser scrolls the page to its hidden text field.
    assert!(grid.contains(ime.cursor_rect.center()), "{:?} on {grid:?}", ime.cursor_rect);
}

#[test]
fn following_the_live_screen_it_never_holds_back_however_fast_the_output() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Terminal);
    // Its window is taller than the screen (a smaller reader sets the size):
    // forty lines a frame scroll past what it has above the screen.
    for top in (1000..2000).step_by(40) {
        app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered(top, 0)]);
        let drawn = drawn_lines(&frame(&ctx, &mut app, vec![]));
        answer_lines(&mut app);
        assert_eq!(drawn.last(), Some(&(top + 23)), "the live screen's last line, at once");
    }
}

#[test]
fn a_new_width_or_a_new_start_of_the_shell_keeps_the_view_sensible() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered(1000, 0)]);
    not_typing(&ctx, &mut app);
    answer_lines(&mut app);
    for _ in 0..100 {
        frame(&ctx, &mut app, key("k"));
        answer_lines(&mut app);
    }
    let read = drawn_lines(&frame(&ctx, &mut app, vec![]));
    let back = 1024 - read.last().unwrap();
    // Narrower: every line laid out anew, numbered past the old ones.
    let harness_contracts::FromTerminal::Screen { mut update } = crate::terminal::tests::numbered(5000, 4000) else { unreachable!() };
    update.size.cols = 60;
    app.desk.terminal_mut().arrive(vec![harness_contracts::FromTerminal::Screen { update }]);
    frame(&ctx, &mut app, vec![]);
    answer_lines(&mut app);
    let drawn = drawn_lines(&frame(&ctx, &mut app, vec![]));
    assert_eq!(5024 - drawn.last().unwrap(), back, "as far from the live screen as before: {drawn:?}");
    // A new start of its shell: the live screen.
    let harness_contracts::FromTerminal::Screen { mut update } = crate::terminal::tests::numbered(30, 0) else { unreachable!() };
    (update.size.cols, update.shell) = (60, 2);
    app.desk.terminal_mut().arrive(vec![harness_contracts::FromTerminal::Screen { update }]);
    frame(&ctx, &mut app, vec![]);
    answer_lines(&mut app);
    let drawn = drawn_lines(&frame(&ctx, &mut app, vec![]));
    assert_eq!(drawn.last(), Some(&53), "{drawn:?}");
}

#[test]
fn the_cursor_is_there_only_while_not_typing_from_the_programs_line_and_never_on_a_line_gone() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered(1000, 0)]);
    focus(&ctx, &mut app, BufferKind::Terminal);
    assert_eq!(cursor(&app), None, "typing: no cursor");
    not_typing(&ctx, &mut app);
    // The screen helper's program cursor is on its last row.
    assert_eq!(cursor(&app).as_deref(), Some("1023"), "on the program's line");
    keys(&ctx, &mut app, "kk");
    assert_eq!(cursor(&app).as_deref(), Some("1021"));
    frame(&ctx, &mut app, key("i"));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(cursor(&app), None, "typing drops it");
    assert!(app.desk.terminal().status().is_empty(), "and the view follows the live screen");
    not_typing(&ctx, &mut app);
    keys(&ctx, &mut app, "kk");
    // Its line goes (a clear): the nearest there is.
    app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered(1100, 1100)]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(cursor(&app).as_deref(), Some("1100"));
    // Where the terminal is doesn't keep it.
    let place = app.desk.view(BufferKind::Terminal).place();
    app.desk.view_mut(BufferKind::Terminal).leave();
    app.desk.view_mut(BufferKind::Terminal).restore(&place);
    assert_eq!(cursor(&app), None);
}

/// A screen from line `top` with the shell's `commands` on it.
fn with_commands(top: u64, commands: Vec<harness_contracts::TerminalCommand>) -> harness_contracts::FromTerminal {
    let harness_contracts::FromTerminal::Screen { mut update } = crate::terminal::tests::numbered(top, 0) else { unreachable!() };
    update.commands = commands;
    harness_contracts::FromTerminal::Screen { update }
}

fn command(id: u64, prompt: u64, end: Option<u64>, text: &str) -> harness_contracts::TerminalCommand {
    harness_contracts::TerminalCommand { id, prompt, input: prompt, input_col: 2, output: Some(prompt + 1), end, text: text.into(), code: Some(0), ran_ms: Some(5) }
}

#[test]
fn not_typing_it_moves_by_command_and_copies_one_s_output_or_runs_it_again() {
    let (ctx, mut app) = full_app();
    // The third waits at an empty prompt on the screen's last line (where the program's cursor is).
    let mut waiting = command(3, 1023, None, "");
    waiting.output = None;
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![command(1, 1003, Some(1008), "make"), command(2, 1008, Some(1012), "make test"), waiting])]);
    not_typing(&ctx, &mut app);
    keys(&ctx, &mut app, "[[");
    assert_eq!(cursor(&app).as_deref(), Some("1008"), "from the program's line (in the third, running), the one before");
    keys(&ctx, &mut app, "[[");
    assert_eq!(cursor(&app).as_deref(), Some("1003"));
    keys(&ctx, &mut app, "]]");
    assert_eq!(cursor(&app).as_deref(), Some("1008"));
    // Its output, as the server has it, copied when it comes.
    keys(&ctx, &mut app, ":");
    keys(&ctx, &mut app, "copy the command's output");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert!(app.desk.terminal().sent().contains(&ToTerminal::CommandOutput { command: 2 }), "asked for");
    app.desk.terminal_mut().arrive(vec![harness_contracts::FromTerminal::CommandOutput { command: 2, text: Some("ok 1\nok 2".into()) }]);
    let out = frame(&ctx, &mut app, vec![]);
    let copied: Vec<&String> = out.platform_output.commands.iter().filter_map(|c| if let egui::OutputCommand::CopyText(t) = c { Some(t) } else { None }).collect();
    assert_eq!(copied, ["ok 1\nok 2"]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.note.as_deref(), Some("Copied its output."));
    // Run again: typed in, as the user would, at the empty prompt.
    keys(&ctx, &mut app, ":");
    keys(&ctx, &mut app, "run the command again");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(inputs(&app)[inputs(&app).len() - 2..], [text("make test"), named(TermKey::Enter, false)]);
    // Something typed at the prompt (its cursor moved on): not run over it.
    let mut typed = command(3, 1023, None, "");
    (typed.output, typed.input_col) = (None, 0);
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![typed])]);
    frame(&ctx, &mut app, vec![]);
    let before = inputs(&app).len();
    keys(&ctx, &mut app, ":");
    keys(&ctx, &mut app, "run the command again");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(inputs(&app).len(), before);
    assert!(app.note.as_deref().is_some_and(|n| n.starts_with("Not at an empty prompt")), "{:?}", app.note);
}
