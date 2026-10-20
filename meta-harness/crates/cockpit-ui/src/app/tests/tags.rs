//! Tags in the cockpit, on a terminal's things: what's tagged at the
//! cursor (the unit, or another by Tab), where the tags show (the gutter,
//! the panel's top line), going to and through what's tagged, the Tags
//! buffer, and the palette that picks them.

use super::*;
use super::terminal::{answer_lines, command, not_typing, with_commands};

fn lines(terminal: &str, shell: u64, first: u64, last: u64) -> harness_contracts::Subject {
    harness_contracts::Subject::TerminalLines { terminal: harness_contracts::TerminalId::new(terminal), shell, first, last, command: None }
}

fn ran(terminal: &str, shell: u64, command: u64) -> harness_contracts::Subject {
    harness_contracts::Subject::TerminalCommand { terminal: harness_contracts::TerminalId::new(terminal), shell, command, typed: String::new() }
}

/// A tag (id `id`) on `on`, each named as the server names it.
fn tag_on(id: &str, name: &str, on: &[harness_contracts::Subject]) -> harness_contracts::Tag {
    let what = |s: &harness_contracts::Subject| match s {
        harness_contracts::Subject::TerminalLines { terminal, first, last, .. } => format!("terminal {terminal} lines {first}-{last}"),
        harness_contracts::Subject::TerminalCommand { terminal, command, .. } => format!("terminal {terminal} command {command}"),
        _ => String::new(),
    };
    let on = on.iter().map(|s| harness_contracts::Tagged { subject: s.clone(), what: what(s), at_ms: 0, by: "user".into(), kept: None }).collect();
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
    let flaky = crate::theme::colors().tag(2);
    let marks = bars(&output, flaky);
    let row = |line: u64| drawn_rects(&output, &format!("line {line}"))[0];
    assert_eq!(marks.len(), 2, "beside the command (its lines) and the line: {marks:?}");
    assert!(marks[0].top() < row(1003).center().y && row(1007).center().y < marks[0].bottom(), "the command's bar spans its lines: {marks:?}");
    assert!(marks[1].y_range().contains(row(1021).center().y));
    for mark in &marks {
        assert!(mark.right() < row(1003).left(), "in the gutter, never over the text: {mark:?}");
    }
    // The panel's top line names the tag on the cursor's line (and only
    // then: not on another).
    app.note = None;
    let header = |output: &egui::FullOutput| drawn_rects(output, "flaky").into_iter().filter(|r| r.bottom() < row(1003).top()).count();
    assert_eq!(header(&frame(&ctx, &mut app, vec![])), 1);
    keys(&ctx, &mut app, "k");
    assert_eq!(header(&frame(&ctx, &mut app, vec![])), 0, "line 1020 has no tag");
    // Its lines scrolled out of view: no bars.
    keys(&ctx, &mut app, "gg");
    answer_lines(&mut app);
    frame(&ctx, &mut app, vec![]);
    assert!(bars(&frame(&ctx, &mut app, vec![]), flaky).is_empty());
}

#[test]
fn on_a_line_of_output_the_command_is_tagged_and_tab_in_the_picker_tags_the_line_instead() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![command(1, 1003, Some(1008), "make")])]);
    not_typing(&ctx, &mut app);
    app.run_move(BufferKind::Terminal, Move::Motion(Motion::Line(1005)), None, &ctx);
    // M: its top line says what's tagged (the command, with all its output);
    // the next of the things there, by its key.
    keys(&ctx, &mut app, "M");
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    // Each by its one name: the name it's kept by (`Subject::name`'s words are its own test's).
    let line = harness_contracts::Subject::TerminalLines { terminal: harness_contracts::TerminalId::new("t1"), shell: 1, first: 1005, last: 1005, command: Some(1) };
    let command = harness_contracts::Subject::TerminalCommand { terminal: harness_contracts::TerminalId::new("t1"), shell: 1, command: 1, typed: "make".into() };
    let name = |s: &harness_contracts::Subject| s.name(&harness_contracts::Known::default());
    assert!(drawn.contains(&name(&command)) && drawn.contains(&name(&line)), "what's tagged, and what Tab tags instead: {drawn}");
    frame(&ctx, &mut app, press(egui::Key::Tab, false));
    frame(&ctx, &mut app, vec![]);
    keys(&ctx, &mut app, "seen");
    bodies(&app, "");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags"), [serde_json::json!({ "tag": "seen", "subject": line })], "Tab: the line");
    // Opened again: the unit again.
    keys(&ctx, &mut app, "M");
    keys(&ctx, &mut app, "seen");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags"), [serde_json::json!({ "tag": "seen", "subject": command })]);
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
    frame(&ctx, &mut app, vec![]);
    // On a line among the tagged lines: their tag's.
    keys(&ctx, &mut app, "kkk");
    assert_eq!(cursor(&app).as_deref(), Some("1020"));
    for (keys_, at) in [("]t", "1010"), ("]t", "1019"), ("[t", "1010"), ("[t", "1019")] {
        keys(&ctx, &mut app, keys_);
        frame(&ctx, &mut app, vec![]);
        assert_eq!(cursor(&app).as_deref(), Some(at), "{keys_}");
    }
    // Each a jump: ctrl+o goes back through them, line by line; ctrl+i forward.
    for (key, at) in [(egui::Key::O, "1010"), (egui::Key::O, "1019"), (egui::Key::O, "1010"), (egui::Key::I, "1019")] {
        ctrl(&ctx, &mut app, key);
        frame(&ctx, &mut app, vec![]);
        assert_eq!(cursor(&app).as_deref(), Some(at), "{key:?}");
    }
    for _ in 0..2 {
        ctrl(&ctx, &mut app, egui::Key::I);
    }
    frame(&ctx, &mut app, vec![]);
    assert_eq!(cursor(&app).as_deref(), Some("1019"));
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
    // ctrl+o: back where it was (its line, once its lines come again).
    ctrl(&ctx, &mut app, egui::Key::O);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.terminal().target(), Some(Target::Terminal(harness_contracts::TerminalId::new("t1"))));
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![])]);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(cursor(&app).as_deref(), Some("1023"));
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
fn the_tags_buffer_lists_tags_folded_reads_what_they_say_now_and_the_shells_tag_actions_edit_them() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![command(1, 1003, Some(1008), "make")])]);
    let mut tag = tag_on("x1", "flaky", &[lines("t1", 1, 1010, 1010), ran("t1", 1, 1)]);
    (tag.note, tag.noted_by) = ("fails on Mondays".into(), "lead".into());
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x0", "alpha", &[lines("t1", 1, 1001, 1001)]) });
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag.clone() });
    focus(&ctx, &mut app, BufferKind::Tags);
    assert_eq!(cursor(&app).as_deref(), Some("x0"));
    // Folded to start: a line per tag. What its things say now: read once
    // the cursor rests on it, not while it moves through.
    frame_at(&ctx, &mut app, vec![], 100.0);
    sent(&app);
    for (n, k) in ["j", "k", "j", "k"].into_iter().enumerate() {
        frame_at(&ctx, &mut app, key(k), 100.05 + n as f64 * 0.05);
    }
    assert!(!sent(&app).contains(&"GET /api/tags/x1".to_owned()), "moved through");
    frame_at(&ctx, &mut app, key("j"), 101.0);
    assert_eq!(cursor(&app).as_deref(), Some("x1"), "the next tag's line: the folded one's things don't show");
    frame_at(&ctx, &mut app, vec![], 101.5);
    assert!(sent(&app).contains(&"GET /api/tags/x1".to_owned()));
    let on = tag.on.iter().map(|t| {
        let (held, text) = if t.subject == ran("t1", 1, 1) { (harness_contracts::Held::Source, "make: 3 failed") } else { (harness_contracts::Held::Kept, "$ ls") };
        harness_contracts::TaggedNow { tagged: t.clone(), held, text: Some(text.to_owned()), lines: 1 }
    }).collect();
    assert!(app.desk.net.answer("GET /api/tags/x1", 200, &serde_json::to_value(harness_contracts::TagRead { tag: tag.clone(), on }).unwrap()));
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("fails on Mondays (lead)") && !drawn.contains("make: 3 failed"), "folded: {drawn}");
    // A tag's line has the shell's tag edits as its buttons.
    for button in ["Rename", "Colour", "Note", "Delete"] {
        assert!(drawn.contains(button), "{button}: {drawn}");
    }
    // Enter unfolds it: its things, where they went, who tagged them and when.
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    for words in ["terminal t1 command 1", "make: 3 failed", "as it was when tagged: changed or gone since", "tagged by you", " d ago"] {
        assert!(drawn.contains(words), "{words}: {drawn}");
    }
    // Edited with the shell's tag actions (what's there now, to edit).
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
    run(&ctx, &mut app, "rename the tag flaky", Some("-ci"));
    run(&ctx, &mut app, "write flaky's note", Some(", only CI"));
    run(&ctx, &mut app, "clear flaky's note", None);
    run(&ctx, &mut app, "give flaky another colour", Some(crate::theme::colors().tag_name(5)));
    assert_eq!(bodies(&app, "POST /api/tags/x1/edit"), [serde_json::json!({ "name": "flaky-ci" }), serde_json::json!({ "note": "fails on Mondays, only CI" }), serde_json::json!({ "note": "" }), serde_json::json!({ "colour": 5 })]);
    // On its second thing; the first taken off by someone else: still on it.
    keys(&ctx, &mut app, "jj");
    let on_make = cursor(&app);
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("Go to it") && drawn.contains("Take off"), "a thing's buttons: {drawn}");
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x1", "flaky", &[ran("t1", 1, 1)]) });
    frame(&ctx, &mut app, vec![]);
    assert_eq!(cursor(&app), on_make, "rows are what they're on, not where");
    run(&ctx, &mut app, "take flaky off this", None);
    assert_eq!(bodies(&app, "POST /api/tags/x1/untag"), [serde_json::json!({ "subject": ran("t1", 1, 1) })]);
    // Deleting asks first: kept, nothing sent; yes, deleted.
    run(&ctx, &mut app, "delete the tag flaky", Some("keep"));
    assert!(!sent(&app).iter().any(|s| s.starts_with("DELETE")));
    run(&ctx, &mut app, "delete the tag flaky", Some("delete"));
    assert!(sent(&app).contains(&"DELETE /api/tags/x1".to_owned()));
    // Backspace: up to its tag; z c folds it; z R unfolds them all, z M folds them.
    frame(&ctx, &mut app, press(egui::Key::Backspace, false));
    assert_eq!(cursor(&app).as_deref(), Some("x1"));
    keys(&ctx, &mut app, "zc");
    let rows = |app: &CockpitApp| app.desk.view(BufferKind::Tags).items().len();
    let folded = texts(&frame(&ctx, &mut app, vec![]));
    assert!(!folded.contains("terminal t1 command 1"), "{folded}");
    keys(&ctx, &mut app, "zR");
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("terminal t1 lines 1001-1001"), "every tag unfolded");
    keys(&ctx, &mut app, "zM");
    assert!(!texts(&frame(&ctx, &mut app, vec![])).contains("terminal t1 lines 1001-1001"));
    // Search finds a thing under a folded tag, and going there unfolds it.
    keys(&ctx, &mut app, "/");
    keys(&ctx, &mut app, "command 1");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert!(cursor(&app).is_some_and(|c| c.starts_with("x1 ")), "{:?}", cursor(&app));
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("terminal t1 command 1"));
    assert!(rows(&app) > 2);
    keys(&ctx, &mut app, "zM");
    // Enter on a folded tag unfolds it; again, to its first thing; on that,
    // the terminal, its cursor on the command.
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
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
    long.on[0].by = "a-lead-agent-slot-whose-name-goes-on-and-on-past-any-sensible-length-for-a-name".repeat(3);
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: long });
    frame(&ctx, &mut app, vec![]);
    let rect = |ctx: &egui::Context| crate::palette::Palette::drawn(ctx).expect("the palette is drawn");
    // Every picker: all actions, a search (tags), a choice whose rows say a lot.
    // From a list to a text field (go to line), it stays as tall.
    for (open, typed) in [(":", "tagwhat"), ("M", "fl"), ("gT", "fl\n"), (":", "go to line or\n")] {
        for _ in 0..2 {
            frame(&ctx, &mut app, press(egui::Key::Escape, false));
        }
        keys(&ctx, &mut app, open);
        frame(&ctx, &mut app, vec![]);
        let first = rect(&ctx);
        for c in typed.chars() {
            frame(&ctx, &mut app, if c == '\n' { press(egui::Key::Enter, false) } else { key(&c.to_string()) });
            frame(&ctx, &mut app, vec![]);
            assert_eq!(rect(&ctx), first, "{open} then {typed:?} ({c:?}, palette open: {}): the same place and size", app.palette.is_open());
        }
        assert!((first.center().x - SCREEN.x / 2.0).abs() < 2.0, "centred: {first:?}");
    }
}


#[test]
fn a_tag_on_what_you_are_on_is_edited_from_any_view_picking_which_when_there_are_several() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![command(1, 1003, Some(1008), "make")])]);
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x1", "flaky", &[ran("t1", 1, 1)]) });
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x2", "slow", &[lines("t1", 1, 1005, 1005)]) });
    not_typing(&ctx, &mut app);
    app.run_move(BufferKind::Terminal, Move::Motion(Motion::Line(1005)), None, &ctx);
    // Two tags there (the line's, its command's): which first, then its name.
    keys(&ctx, &mut app, ":");
    keys(&ctx, &mut app, "rename a tag here");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("flaky") && drawn.contains("slow"), "{drawn}");
    keys(&ctx, &mut app, "slow");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("Rename the tag slow"), "its name asked for next: {drawn}");
    bodies(&app, "");
    keys(&ctx, &mut app, "-ish");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags/x2/edit"), [serde_json::json!({ "name": "slow-ish" })], "what it was, edited");
    // On the prompt's line (only the command's tag there): no picking.
    app.run_move(BufferKind::Terminal, Move::Motion(Motion::Line(1003)), None, &ctx);
    keys(&ctx, &mut app, ":");
    keys(&ctx, &mut app, "give flaky another colour");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    keys(&ctx, &mut app, crate::theme::colors().tag_name(3));
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags/x1/edit"), [serde_json::json!({ "colour": 3 })]);
}

#[test]
fn a_hash_typed_in_a_prompt_opens_the_tag_picker_and_puts_the_name_after_it() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    frame(&ctx, &mut app, key("i"));
    let typed = |ctx: &egui::Context, app: &mut CockpitApp, text: &str| {
        for c in text.chars() {
            frame(ctx, app, vec![egui::Event::Text(c.to_string())]);
        }
        frame(ctx, app, vec![]);
    };
    // No tags yet: a # is a #.
    typed(&ctx, &mut app, "fix #");
    assert!(!app.palette.is_open());
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x1", "flaky", &[ran("t1", 1, 1)]) });
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x2", "deploy", &[ran("t1", 1, 2)]) });
    typed(&ctx, &mut app, "1 look at (#");
    assert!(app.palette.is_open(), "the tag picker");
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("flaky") && drawn.contains("deploy"), "{drawn}");
    keys(&ctx, &mut app, "fla");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert!(matches!(app.input_mode, InputMode::Insert { .. }), "typing on");
    // The caret after the name: what's typed next follows it.
    typed(&ctx, &mut app, ")");
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("fix #1 look at (#flaky )"), "in the box, as typed");
    // Escape out of the picker: the # stays, and typing goes on.
    typed(&ctx, &mut app, " #");
    assert!(app.palette.is_open());
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    frame(&ctx, &mut app, vec![]);
    assert!(!app.palette.is_open() && matches!(app.input_mode, InputMode::Insert { .. }), "back to typing");
    typed(&ctx, &mut app, "x");
    assert!(!app.palette.is_open(), "only a # just typed opens it");
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("fix #1 look at (#flaky ) #x"));
    // Mid-word, a # is just a #.
    typed(&ctx, &mut app, " issue#");
    assert!(!app.palette.is_open());
}

#[test]
fn each_tag_has_a_column_of_its_colour_in_the_gutter_and_more_than_fit_share_a_muted_one() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![command(1, 1003, Some(1008), "make")])]);
    for (n, name) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
        let mut tag = tag_on(&format!("x{n}"), name, &[lines("t1", 1, 1010 + n as u64, 1010 + n as u64), lines("t1", 1, 1020, 1020)]);
        tag.colour = n as u8;
        told(&app, harness_contracts::ClientEvent::TagUpdated { tag });
    }
    not_typing(&ctx, &mut app);
    // (The theme's colours, as the cockpit set them.)
    let theme = crate::theme::colors();
    let output = frame(&ctx, &mut app, vec![]);
    // Five tags on screen, four columns: three their own, two sharing the last, muted.
    let column = |colour| bars(&output, colour).first().map(|b| b.left());
    let own: Vec<f32> = (0..3).map(|n| column(theme.tag(n)).expect("a bar in its colour")).collect();
    assert!(own[0] < own[1] && own[1] < own[2], "side by side: {own:?}");
    let muted = bars(&output, theme.muted);
    assert!(muted.len() >= 2 && muted.iter().all(|b| b.left() > own[2]), "the rest, together: {muted:?}");
    assert!(column(theme.tag(3)).is_none() && column(theme.tag(4)).is_none());
    // A long press on a bar says its tags (a line two share: both).
    let shared = bars(&output, theme.tag(0)).into_iter().find(|b| b.y_range().contains(drawn_rects(&output, "line 1020")[0].center().y)).expect("line 1020's bar");
    let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos: shared.center(), force: None };
    frame_at(&ctx, &mut app, vec![touch(egui::TouchPhase::Start)], 70.0);
    frame_at(&ctx, &mut app, vec![], 70.8);
    frame_at(&ctx, &mut app, vec![touch(egui::TouchPhase::End)], 70.9);
    frame_at(&ctx, &mut app, vec![], 71.0);
    assert_eq!(app.note.as_deref(), Some("a"), "{:?}", app.note);
}

#[test]
fn an_edit_is_of_the_tag_it_was_asked_for_and_a_thing_is_taken_off_exactly() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![with_commands(1000, vec![])]);
    // Two ranges of one tag, one inside the other.
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x1", "wide", &[lines("t1", 1, 1005, 1020), lines("t1", 1, 1010, 1012)]) });
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag: tag_on("x2", "other", &[lines("t1", 1, 1001, 1001)]) });
    focus(&ctx, &mut app, BufferKind::Tags);
    // Delete asked of the first tag; it's deleted elsewhere meanwhile: the
    // answer deletes nothing else.
    keys(&ctx, &mut app, ":");
    keys(&ctx, &mut app, "delete the tag other");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    told(&app, harness_contracts::ClientEvent::TagRemoved { tag_id: harness_contracts::TagId::new("x2") });
    frame(&ctx, &mut app, vec![]);
    sent(&app);
    keys(&ctx, &mut app, "delete");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert!(!sent(&app).iter().any(|s| s.starts_with("DELETE")), "nothing deleted");
    assert!(app.note.as_deref().is_some_and(|n| n.contains("gone")), "{:?}", app.note);
    // The inner range's row: taken off that range, not the one around it.
    keys(&ctx, &mut app, "gg");
    keys(&ctx, &mut app, "zo");
    keys(&ctx, &mut app, "jj");
    bodies(&app, "");
    keys(&ctx, &mut app, ":");
    keys(&ctx, &mut app, "take wide off this");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags/x1/untag"), [serde_json::json!({ "subject": lines("t1", 1, 1010, 1012) })]);
}

/// The test conversation, its session named by the harness ("n1").
fn named_conversation() -> Vec<harness_contracts::ClientEvent> {
    use harness_contracts::{AdapterEvent, ClientEvent, NativeId};
    crate::chat::tests::conversation_events()
        .into_iter()
        .map(|event| match event {
            ClientEvent::Session { event: AdapterEvent::ThreadUpdated { mut thread } } => {
                thread.native_id = Some(NativeId::new("n1"));
                ClientEvent::Session { event: AdapterEvent::ThreadUpdated { thread } }
            }
            ClientEvent::SlotUpdated { mut slot } => {
                slot.sessions[0].native_id = Some(NativeId::new("n1"));
                ClientEvent::SlotUpdated { slot }
            }
            other => other,
        })
        .collect()
}

#[test]
fn in_a_chat_a_message_or_its_turn_is_tagged_shown_in_the_gutter_and_gone_to() {
    let (ctx, mut app) = full_app();
    for event in named_conversation() {
        told(&app, event);
    }
    focus(&ctx, &mut app, BufferKind::Chat);
    frame(&ctx, &mut app, vec![]);
    let answer = app.desk.view(BufferKind::Chat).items().into_iter().rev().find(|i| i.text.contains("Looking at it.")).map(|i| i.key).expect("the answer");
    app.desk.view_mut(BufferKind::Chat).move_to(&answer);
    // M: the message (the unit), Tab: its turn.
    keys(&ctx, &mut app, "M");
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    let message = harness_contracts::Subject::ChatMessage { session: harness_contracts::NativeId::new("n1"), turn: 1, message: 2 };
    let known = harness_contracts::Known { chat: harness_contracts::chat_who(Some("lead"), &harness_contracts::NativeId::new("n1")), ..harness_contracts::Known::default() };
    let turn = harness_contracts::Subject::ChatTurn { session: harness_contracts::NativeId::new("n1"), turn: 1 };
    assert!(drawn.contains(&message.name(&known)) && drawn.contains(&turn.name(&known)), "by the one name each has (its slot's chat): {drawn}");
    keys(&ctx, &mut app, "answer");
    bodies(&app, "");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags"), [serde_json::json!({ "tag": "answer", "subject": message })]);
    // Told: a bar beside the message, in the chat's gutter.
    let mut tag = tag_on("x1", "answer", &[]);
    tag.on.push(harness_contracts::Tagged { subject: message.clone(), what: "chat lead turn 1 message 2".into(), at_ms: 0, by: "user".into(), kept: None });
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag });
    frame(&ctx, &mut app, vec![]);
    let theme = crate::theme::colors();
    let output = frame(&ctx, &mut app, vec![]);
    let at = drawn_rects(&output, "Looking at it.")[0];
    let marks = bars(&output, theme.tag(2));
    assert!(marks.iter().any(|b| b.y_range().contains(at.center().y) && b.right() < at.left()), "{marks:?} beside {at:?}");
    // Gone to from elsewhere: the chat, its cursor on the message.
    keys(&ctx, &mut app, "G");
    frame(&ctx, &mut app, press(egui::Key::Backspace, false));
    keys(&ctx, &mut app, "gT");
    keys(&ctx, &mut app, "answer");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.focused_kind(), cursor(&app)), (BufferKind::Chat, Some(answer)));
}

#[test]
fn in_the_code_the_innermost_definition_is_tagged_tab_its_line_and_in_the_outline_its_component() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code_mut().move_to("3");
    let file = app.desk.code().current_file.clone();
    let within = app.desk.code().defs.iter().filter(|d| d.start <= 3 && 3 <= d.end).min_by_key(|d| d.end - d.start).cloned().expect("line 3 is in a definition");
    keys(&ctx, &mut app, "M");
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    let name = |s: harness_contracts::Subject| s.name(&harness_contracts::Known::default());
    let (component, line) = (harness_contracts::Subject::Component { id: within.id.clone(), was: None }, harness_contracts::Subject::CodeLines { path: file.clone(), first: 3, last: 3, snapshot: String::new() });
    assert!(drawn.contains(&name(component)) && drawn.contains(&name(line)), "{drawn}");
    keys(&ctx, &mut app, "seen");
    bodies(&app, "");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags"), [serde_json::json!({ "tag": "seen", "subject": harness_contracts::Subject::Component { id: within.id.clone(), was: None } })]);
    keys(&ctx, &mut app, "M");
    frame(&ctx, &mut app, press(egui::Key::Tab, false));
    frame(&ctx, &mut app, vec![]);
    keys(&ctx, &mut app, "seen");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags"), [serde_json::json!({ "tag": "seen", "subject": harness_contracts::Subject::CodeLines { path: file.clone(), first: 3, last: 3, snapshot: String::new() } })], "the files as they are (the server keeps them at a snapshot)");
    // Told: a bar beside the definition's lines.
    let mut tag = tag_on("x1", "seen", &[]);
    tag.on.push(harness_contracts::Tagged { subject: harness_contracts::Subject::Component { id: within.id.clone(), was: None }, what: within.name.clone(), at_ms: 0, by: "user".into(), kept: None });
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag });
    frame(&ctx, &mut app, vec![]);
    let theme = crate::theme::colors();
    assert!(!bars(&frame(&ctx, &mut app, vec![]), theme.tag(2)).is_empty(), "in the code's gutter");
    // The outline: the component on its row.
    focus(&ctx, &mut app, BufferKind::Tree);
    app.desk.tree_mut().move_to("rust:a::run");
    keys(&ctx, &mut app, "M");
    keys(&ctx, &mut app, "seen");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/tags"), [serde_json::json!({ "tag": "x1", "subject": harness_contracts::Subject::Component { id: arch_graph::ComponentId::new("rust:a::run"), was: None } })], "the tag there is, by its id");
}
