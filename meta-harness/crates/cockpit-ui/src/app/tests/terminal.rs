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
    let sizes = |app: &CockpitApp| app.desk.terminal().sent().into_iter().filter(|s| matches!(s, ToTerminal::Size { .. })).count();
    assert_eq!(sizes(&app), 1, "its size, said once");
    // Its panel narrowed bit by bit (an edge dragged): the size it ends at,
    // said once it's held still.
    let mut t = 100.0;
    for count in [4, 5, 6] {
        app.desk.panels.set_count(count, 12);
        t += 0.03;
        frame_at(&ctx, &mut app, vec![], t);
    }
    assert_eq!(sizes(&app), 1, "not while it changes");
    frame_at(&ctx, &mut app, vec![], t + 0.3);
    assert_eq!(sizes(&app), 2, "the one it ended at");
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

/// The lines the terminal asked its server for last, answered (it asks
/// one at a time; an answer it has already is let go).
fn answer_lines(app: &mut CockpitApp) {
    let asked = app.desk.terminal().sent().into_iter().filter_map(|s| if let ToTerminal::Lines { from, count } = s { Some((from, count)) } else { None }).next_back();
    if let Some((from, count)) = asked {
        app.desk.terminal_mut().arrive(vec![crate::terminal::tests::numbered_lines(from, count)]);
    }
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
    let meanwhile = drawn_lines(&frame(&ctx, &mut app, vec![]));
    assert_eq!(meanwhile.len(), read.len(), "until its lines come, what showed stands in: no blank rows");
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
    // Going to a line: by its number (as tags and agents name lines).
    app.run_move(BufferKind::Terminal, Move::Motion(Motion::Line(1010)), None, &ctx);
    assert_eq!(cursor(&app).as_deref(), Some("1010"));
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
    // Where the terminal is doesn't keep it (it came back after a reload).
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
    let at_prompt = |typed: &str, commands| {
        let harness_contracts::FromTerminal::Screen { mut update } = with_commands(1000, commands) else { unreachable!() };
        update.rows[23].1 = vec![harness_contracts::Run { text: format!("$ {typed}"), style: Default::default(), wide: false }];
        harness_contracts::FromTerminal::Screen { update }
    };
    app.desk.terminal_mut().arrive(vec![at_prompt("", vec![command(1, 1003, Some(1008), "make"), command(2, 1008, Some(1012), "make test"), waiting.clone()])]);
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
    // Something typed at the prompt, the cursor after it, or back at its
    // start (Home): not run over it, nor into it.
    let mut typed = command(3, 1023, None, "");
    (typed.output, typed.input_col) = (None, 0);
    for (screen, how) in [(at_prompt("rm -rf out", vec![typed]), "the cursor moved on"), (at_prompt("rm -rf out", vec![waiting.clone()]), "the cursor at its start")] {
        app.desk.terminal_mut().arrive(vec![screen]);
        frame(&ctx, &mut app, vec![]);
        let before = inputs(&app).len();
        keys(&ctx, &mut app, ":");
        keys(&ctx, &mut app, "run the command again");
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert_eq!(inputs(&app).len(), before, "{how}");
        assert!(app.note.as_deref().is_some_and(|n| n.starts_with("Not at an empty prompt")), "{how}: {:?}", app.note);
    }
}

fn lines(terminal: &str, shell: u64, first: u64, last: u64) -> harness_contracts::Subject {
    harness_contracts::Subject::TerminalLines { terminal: harness_contracts::TerminalId::new(terminal), shell, first, last }
}

fn ran(terminal: &str, shell: u64, command: u64) -> harness_contracts::Subject {
    harness_contracts::Subject::TerminalCommand { terminal: harness_contracts::TerminalId::new(terminal), shell, command }
}

/// A tag (id `id`) on `on`, each named as the server names it.
fn tag_on(id: &str, name: &str, on: &[harness_contracts::Subject]) -> harness_contracts::Tag {
    let what = |s: &harness_contracts::Subject| match s {
        harness_contracts::Subject::TerminalLines { terminal, first, last, .. } => format!("terminal {terminal} lines {first}-{last}"),
        harness_contracts::Subject::TerminalCommand { terminal, command, .. } => format!("terminal {terminal} command {command}"),
        _ => String::new(),
    };
    let on = on.iter().map(|s| harness_contracts::Tagged { subject: s.clone(), what: what(s), said: format!("said by {}", what(s)), at_ms: 0, by: "user".into() }).collect();
    harness_contracts::Tag { id: harness_contracts::TagId::new(id), name: name.into(), colour: 2, note: String::new(), noted_by: String::new(), on }
}

#[test]
fn what_the_cursor_is_on_is_tagged_from_the_shell_and_its_tags_show_where_it_is_drawn_once_the_server_says() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![command(1, 1003, Some(1008), "make")])]);
    not_typing(&ctx, &mut app);
    keys(&ctx, &mut app, "kk");
    assert_eq!(cursor(&app).as_deref(), Some("1021"));
    // M: the tag picker; a new tag by the name typed.
    keys(&ctx, &mut app, "M");
    keys(&ctx, &mut app, "flaky");
    bodies(&app, "");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let subject = lines("t1", 1, 1021, 1021);
    assert_eq!(bodies(&app, "POST /api/tags"), [serde_json::json!({ "tag": "flaky", "subject": subject })], "the cursor's line, in this start of its shell");
    // The reply says it's done; what it's on comes on the event stream (its one source).
    let tag = tag_on("x1", "flaky", &[subject.clone(), ran("t1", 1, 1)]);
    assert!(app.desk.net.answer("POST /api/tags", 200, &serde_json::to_value(&tag).unwrap()));
    let output = frame(&ctx, &mut app, vec![]);
    assert_eq!(app.note.as_deref(), Some("Tagged with flaky."));
    assert!(drawn_rects(&output, "flaky").is_empty(), "no chip from a reply");
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag });
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    let chips = drawn_rects(&output, "flaky");
    let row = |line: u64| drawn_rects(&output, &format!("line {line}"))[0];
    assert_eq!(chips.len(), 2, "on the line and on the command's prompt: {chips:?}");
    for line in [1003, 1021] {
        assert!(chips.iter().any(|chip| row(line).y_range().contains(chip.center().y)), "a chip on line {line}: {chips:?}, {:?}", row(line));
    }
    // Its lines scrolled out of view: no chips.
    keys(&ctx, &mut app, "gg");
    answer_lines(&mut app);
    frame(&ctx, &mut app, vec![]);
    assert!(drawn_rects(&frame(&ctx, &mut app, vec![]), "flaky").is_empty());
}

#[test]
fn lines_selected_with_the_mouse_are_what_is_tagged_and_a_click_or_a_long_press_is_on_a_line() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![])]);
    not_typing(&ctx, &mut app);
    let output = frame(&ctx, &mut app, vec![]);
    let (from, to) = (drawn_rects(&output, "line 1010")[0].center(), drawn_rects(&output, "line 1012")[0].center());
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    let mut t = 50.0;
    for events in [vec![egui::Event::PointerMoved(from)], vec![button(from, true)], vec![egui::Event::PointerMoved(from + egui::vec2(0.0, 8.0))], vec![egui::Event::PointerMoved(to)], vec![button(to, false)], vec![]] {
        t += 0.05;
        frame_at(&ctx, &mut app, events, t);
    }
    assert_eq!(cursor(&app).as_deref(), Some("1012"), "where the drag ended");
    bodies(&app, "");
    keys(&ctx, &mut app, "M");
    keys(&ctx, &mut app, "picked");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags"), [serde_json::json!({ "tag": "picked", "subject": lines("t1", 1, 1010, 1012) })]);
    // A click: the cursor on that line.
    let at = drawn_rects(&frame(&ctx, &mut app, vec![]), "line 1005")[0].center();
    t = 60.0;
    for events in [vec![egui::Event::PointerMoved(at)], vec![button(at, true)], vec![button(at, false)], vec![]] {
        t += 0.05;
        frame_at(&ctx, &mut app, events, t);
    }
    assert_eq!(cursor(&app).as_deref(), Some("1005"));
    assert_eq!(app.input_mode, InputMode::Normal);
    // A long press on a line (a phone): the cursor there.
    let at = drawn_rects(&frame_at(&ctx, &mut app, vec![], 61.0), "line 1008")[0].center();
    let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos: at, force: None };
    frame_at(&ctx, &mut app, vec![touch(egui::TouchPhase::Start)], 61.1);
    frame_at(&ctx, &mut app, vec![], 61.8);
    frame_at(&ctx, &mut app, vec![touch(egui::TouchPhase::End)], 61.9);
    frame_at(&ctx, &mut app, vec![], 62.0);
    assert_eq!(cursor(&app).as_deref(), Some("1008"));
}

#[test]
fn next_and_previous_go_through_the_tag_stepped_last_and_a_tag_on_lines_is_on_each_of_them() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![command(1, 1003, Some(1008), "make")])]);
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x1", "flaky", &[lines("t1", 1, 1021, 1021), ran("t1", 1, 1)]) });
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x2", "slow", &[lines("t1", 1, 1019, 1021), lines("t1", 1, 1010, 1010)]) });
    not_typing(&ctx, &mut app);
    let output = frame(&ctx, &mut app, vec![]);
    assert!(drawn_rects(&output, "line 1019")[0].y_range().contains(drawn_rects(&output, "slow")[0].center().y), "lines' chip: on their first");
    // On a line among the tagged lines: their tag's.
    keys(&ctx, &mut app, "kkk");
    assert_eq!(cursor(&app).as_deref(), Some("1020"));
    for (keys_, at) in [("]t", "1010"), ("]t", "1019"), ("[t", "1010"), ("[t", "1019")] {
        keys(&ctx, &mut app, keys_);
        frame(&ctx, &mut app, vec![]);
        assert_eq!(cursor(&app).as_deref(), Some(at), "{keys_}");
    }
    // On a line with both tags: the one stepped last (not the first by name).
    keys(&ctx, &mut app, "jj");
    assert_eq!(cursor(&app).as_deref(), Some("1021"));
    keys(&ctx, &mut app, "]t");
    frame(&ctx, &mut app, vec![]);
    assert_eq!(cursor(&app).as_deref(), Some("1010"), "slow's next, not flaky's");
    // Taken off, from a line among them: off the lines it's on.
    keys(&ctx, &mut app, "]tj");
    assert_eq!(cursor(&app).as_deref(), Some("1020"));
    bodies(&app, "");
    keys(&ctx, &mut app, ":");
    keys(&ctx, &mut app, "take a tag off");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags/x2/untag"), [serde_json::json!({ "subject": lines("t1", 1, 1019, 1021) })]);
}

#[test]
fn the_tag_picker_finds_tags_as_search_does_and_offers_a_new_one_first_when_none_has_its_name() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![])]);
    let mut noted = tag_on("x1", "ci-flaky", &[lines("t1", 1, 1010, 1010)]);
    noted.note = "fails on Mondays".into();
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: noted });
    not_typing(&ctx, &mut app);
    let rows = |app: &CockpitApp| -> Vec<String> {
        let Some(bound) = app.bound_actions().into_iter().find(|b| b.action.id == "tag.add") else { return Vec::new() };
        let Param::Search { found, .. } = &bound.action.param else { return Vec::new() };
        found.iter().map(|c| c.label.clone()).collect()
    };
    keys(&ctx, &mut app, "M");
    for (typed, expected) in [
        ("ci", vec!["New tag \u{201c}ci\u{201d}", "ci-flaky"]),
        ("cfl", vec!["New tag \u{201c}cfl\u{201d}", "ci-flaky"]),
        ("CI-Flaky", vec!["ci-flaky"]),
        ("mondays", vec!["New tag \u{201c}mondays\u{201d}", "ci-flaky"]),
        ("two words", vec![]),
    ] {
        for _ in 0..12 {
            frame(&ctx, &mut app, press(egui::Key::Backspace, false));
        }
        keys(&ctx, &mut app, typed);
        frame(&ctx, &mut app, vec![]);
        assert_eq!(rows(&app), expected, "{typed}");
    }
    // An existing tag, picked: by its id.
    for _ in 0..12 {
        frame(&ctx, &mut app, press(egui::Key::Backspace, false));
    }
    keys(&ctx, &mut app, "flaky");
    frame(&ctx, &mut app, press(egui::Key::ArrowDown, false));
    bodies(&app, "");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags").first().map(|b| b["tag"].clone()), Some(serde_json::json!("x1")));
    // Its whole name typed and Enter in one go (a paste, fast typing): the
    // rows for the whole name, not for what came before it.
    keys(&ctx, &mut app, "M");
    keys(&ctx, &mut app, "ci-flak");
    let mut last = key("y");
    last.extend(press(egui::Key::Enter, false));
    frame(&ctx, &mut app, last);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(bodies(&app, "POST /api/tags").first().map(|b| b["tag"].clone()), Some(serde_json::json!("x1")), "not a new tag \u{201c}ci-flak\u{201d}");
}

#[test]
fn a_go_to_a_tag_picks_the_tag_then_the_thing_lands_in_another_terminal_and_comes_back() {
    let (ctx, mut app) = full_app();
    // The server's terminals: t1, t2, and t4 (its shell didn't outlive the server).
    let mut t4 = crate::terminals::tests::terminal("t4");
    t4.status = harness_contracts::TerminalStatus::Ended { code: None };
    let both = serde_json::json!([crate::terminals::tests::terminal("t1"), crate::terminals::tests::terminal("t2"), t4]);
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.net.answer("GET /api/terminals", 200, &both));
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![])]);
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x1", "deploy", &[lines("t1", 1, 1010, 1010), ran("t2", 5, 2)]) });
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x2", "old", &[ran("t3", 1, 1)]) });
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x3", "before", &[ran("t4", 1, 1)]) });
    not_typing(&ctx, &mut app);
    // g T: the tag, by name (how many things, its note); then which thing.
    keys(&ctx, &mut app, "gT");
    keys(&ctx, &mut app, "deploy");
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("on 2 things"), "{drawn}");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("Go to which thing deploy is on?") && drawn.contains("terminal t2 command 2"), "{drawn}");
    keys(&ctx, &mut app, "t2");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.terminal().target(), Some(Target::Terminal(harness_contracts::TerminalId::new("t2"))));
    // Its lines come (that start of its shell's): the cursor on the command, not typing.
    let harness_contracts::FromTerminal::Screen { mut update } = with_commands(400, vec![command(2, 410, Some(412), "deploy")]) else { unreachable!() };
    update.shell = 5;
    app.desk.terminal_mut().arrive(vec![harness_contracts::FromTerminal::Screen { update }]);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(cursor(&app).as_deref(), Some("410"));
    assert_eq!(app.input_mode, InputMode::Normal, "landing on lines to look at them doesn't start typing");
    // ctrl+o: back in the terminal it was in.
    ctrl(&ctx, &mut app, egui::Key::O);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.terminal().target(), Some(Target::Terminal(harness_contracts::TerminalId::new("t1"))));
    // A tag on a closed terminal's command: said, and nothing moves.
    keys(&ctx, &mut app, "gT");
    keys(&ctx, &mut app, "old");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.note.as_deref(), Some("Terminal t3 is closed."));
    assert_eq!(app.desk.terminal().target(), Some(Target::Terminal(harness_contracts::TerminalId::new("t1"))));
    // One in a shell that didn't outlive the server: there, and said gone.
    keys(&ctx, &mut app, "gT");
    keys(&ctx, &mut app, "before");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.terminal().target(), Some(Target::Terminal(harness_contracts::TerminalId::new("t4"))));
    assert!(app.note.as_deref().is_some_and(|n| n.starts_with("It's gone from the terminal")), "{:?}", app.note);
}

#[test]
fn the_tags_buffer_lists_tags_and_what_they_are_on_reads_what_they_say_now_and_manages_them() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![command(1, 1003, Some(1008), "make")])]);
    let mut tag = tag_on("x1", "flaky", &[lines("t1", 1, 1010, 1010), ran("t1", 1, 1)]);
    (tag.note, tag.noted_by) = ("fails on Mondays".into(), "lead".into());
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x0", "alpha", &[lines("t1", 1, 1001, 1001)]) });
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag.clone() });
    focus(&ctx, &mut app, BufferKind::Tags);
    assert_eq!(cursor(&app).as_deref(), Some("x0"));
    // What its things say now: read once the cursor rests on it, not while
    // it moves through.
    frame_at(&ctx, &mut app, vec![], 100.0);
    sent(&app);
    for (n, k) in ["j", "j", "k", "k"].into_iter().enumerate() {
        frame_at(&ctx, &mut app, key(k), 100.05 + n as f64 * 0.05);
    }
    assert!(!sent(&app).contains(&"GET /api/tags/x1".to_owned()), "moved through");
    frame_at(&ctx, &mut app, key("j"), 101.0);
    frame_at(&ctx, &mut app, key("j"), 101.05);
    assert_eq!(cursor(&app).as_deref(), Some("x1"));
    frame_at(&ctx, &mut app, vec![], 101.5);
    assert!(sent(&app).contains(&"GET /api/tags/x1".to_owned()));
    let on = tag.on.iter().map(|t| harness_contracts::TaggedNow { tagged: t.clone(), now: (t.subject == ran("t1", 1, 1)).then(|| "make: 3 failed".to_owned()) }).collect();
    assert!(app.desk.net.answer("GET /api/tags/x1", 200, &serde_json::to_value(harness_contracts::TagRead { tag: tag.clone(), on }).unwrap()));
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    for words in ["fails on Mondays (lead)", "terminal t1 command 1", "now: make: 3 failed", "gone (what it said is kept)"] {
        assert!(drawn.contains(words), "{words}: {drawn}");
    }
    // Managed through its actions: each sends what it says.
    let run = |ctx: &egui::Context, app: &mut CockpitApp, action: &str, value: Option<&str>| {
        keys(ctx, app, ":");
        keys(ctx, app, action);
        frame(ctx, app, press(egui::Key::Enter, false));
        if let Some(value) = value {
            keys(ctx, app, value);
            frame(ctx, app, press(egui::Key::Enter, false));
        }
    };
    bodies(&app, "");
    run(&ctx, &mut app, "rename the tag", Some("flaky-ci"));
    run(&ctx, &mut app, "write the tag's note", Some("only CI"));
    run(&ctx, &mut app, "clear the tag's note", None);
    run(&ctx, &mut app, "give the tag another colour", Some(crate::theme::colors().tag_name(5)));
    assert_eq!(bodies(&app, "POST /api/tags/x1/edit"), [serde_json::json!({ "name": "flaky-ci" }), serde_json::json!({ "note": "only CI" }), serde_json::json!({ "note": "" }), serde_json::json!({ "colour": 5 })]);
    // On its second thing; the first taken off by someone else: still on it.
    keys(&ctx, &mut app, "jj");
    let on_make = cursor(&app);
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x1", "flaky", &[ran("t1", 1, 1)]) });
    frame(&ctx, &mut app, vec![]);
    assert_eq!(cursor(&app), on_make, "rows are what they're on, not where");
    run(&ctx, &mut app, "take the tag off this", None);
    assert_eq!(bodies(&app, "POST /api/tags/x1/untag"), [serde_json::json!({ "subject": ran("t1", 1, 1) })]);
    // Deleting asks first: kept, nothing sent; yes, deleted.
    run(&ctx, &mut app, "delete the tag", Some("keep"));
    assert!(!sent(&app).iter().any(|s| s.starts_with("DELETE")));
    run(&ctx, &mut app, "delete the tag", Some("delete"));
    assert!(sent(&app).contains(&"DELETE /api/tags/x1".to_owned()));
    // Enter on the thing: the terminal, its cursor on the command.
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.focused_kind(), BufferKind::Terminal);
    assert_eq!(cursor(&app).as_deref(), Some("1003"));
}

#[test]
fn the_palette_keeps_its_place_and_size_while_typing_and_whatever_its_rows_say() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![])]);
    let mut long = tag_on("x1", "flaky", &[lines("t1", 1, 1010, 1010), lines("t1", 1, 1011, 1011)]);
    long.note = "fails on Mondays when the build cache is cold and the network to the registry is slow, which is most Mondays".into();
    long.on[0].said = "error: could not compile `meta-harness` (test \"terminals_tests\") due to 1 previous error; 3 warnings emitted while linking the whole thing".repeat(3);
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: long });
    frame(&ctx, &mut app, vec![]);
    let rect = |ctx: &egui::Context| ctx.memory(|m| m.area_rect(egui::Id::new("command_palette"))).expect("the palette is drawn");
    // Every picker: all actions, a search (tags), a choice whose rows say a lot.
    for (open, typed) in [(":", "tagwhat"), ("M", "fl"), ("gT", "fl\n")] {
        for _ in 0..2 {
            frame(&ctx, &mut app, press(egui::Key::Escape, false));
        }
        keys(&ctx, &mut app, open);
        frame(&ctx, &mut app, vec![]);
        let first = rect(&ctx);
        for c in typed.chars() {
            frame(&ctx, &mut app, if c == '\n' { press(egui::Key::Enter, false) } else { key(&c.to_string()) });
            frame(&ctx, &mut app, vec![]);
            assert_eq!(rect(&ctx), first, "{open} then {typed:?}: the same place and size");
        }
        assert!((first.center().x - SCREEN.x / 2.0).abs() < 2.0, "centred: {first:?}");
    }
}
