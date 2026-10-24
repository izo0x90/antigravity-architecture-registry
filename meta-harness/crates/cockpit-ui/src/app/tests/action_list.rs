//! The action list as it's used: `:` and a number, a row that can't run
//! here, the keys of where it was opened from, its one key line, and
//! nothing of it built while it's closed.

use super::*;

/// The keys that open it, as typed from where keys run actions.
fn open(ctx: &egui::Context, app: &mut CockpitApp, key_typed: &str) -> String {
    frame(ctx, app, key(key_typed));
    assert!(app.palette.is_open(), "{key_typed} opens the action list");
    frame(ctx, app, vec![]);
    texts(&frame(ctx, app, vec![]))
}

/// `:` and a number lists going to that line first, whatever else
/// matches (the highlight starts on it: Enter goes there).
#[test]
fn a_number_typed_lists_going_to_that_line_first() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    open(&ctx, &mut app, ":");
    frame(&ctx, &mut app, key("7"));
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Go to line 7") && app.palette.selected() == 0, "listed, first: {text}");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.palette.is_open(), app.desk.code().target_line), (false, Some(7)));
}

/// A row picked where it doesn't apply: one that asks for a value says
/// why not, as its key does, and the list stays; one that doesn't runs
/// (its view says why it can't).
#[test]
fn a_row_that_cant_run_here_says_why_and_one_without_a_value_runs() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Spec);
    let (offered, _) = app.sheet_of(app.palette_actions());
    let asks = |o: &&crate::actions::Offered| !matches!(o.bound.action.param, Param::None | Param::Toggle { .. });
    let refused = offered.iter().find(|o| !o.applies && asks(o) && o.bound.leader.is_some()).expect("one that asks for a value and doesn't apply here");
    let why = refused.refused().expect("refused");
    let refused = refused.bound.clone();
    let text = everything(&ctx, &mut app, &refused.action.label);
    assert!(text.contains(&refused.action.label), "{text}");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(app.palette.is_open() && app.palette.picking().is_none() && text.contains(&why), "said, and the list stays: {text}");
    // Its key says the same.
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    app.run_bound(&refused, None, &ctx);
    assert_eq!(app.note.as_deref(), Some(why.as_str()));

    let runs = offered.iter().find(|o| !o.applies && o.bound.action.param == Param::None && o.bound.origin == Origin::Shell).map(|o| o.bound.clone()).expect("one without a value that doesn't apply here");
    everything(&ctx, &mut app, &runs.action.label);
    app.ran.borrow_mut().clear();
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert!(!app.palette.is_open() && app.ran.borrow().contains(&runs.action.id), "{} ran: {:?}", runs.action.id, app.ran.borrow());
}

/// Opened from typing (the top bar's button), the list on everything has
/// the typing keys; Escape goes back to typing. Opened later from where
/// keys run actions, it has no mode's keys: nothing is left over.
#[test]
fn everything_lists_the_keys_of_where_it_was_opened_from_and_nothing_left_over() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    frame(&ctx, &mut app, key("i"));
    assert!(matches!(app.input_mode, InputMode::Insert { .. }));
    // The top bar's button for the action list.
    let button = *drawn_containing(&frame(&ctx, &mut app, vec![]), "Action list  :").first().expect("the top bar's button");
    let button = button.center();
    let tap = |pressed| egui::Event::PointerButton { pos: button, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(button)]);
    frame(&ctx, &mut app, vec![tap(true)]);
    frame(&ctx, &mut app, vec![tap(false)]);
    assert!(app.palette.is_open(), "the top bar's button opens it");
    frame(&ctx, &mut app, press(egui::Key::Tab, false));
    frame(&ctx, &mut app, key("leaves the field"));
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Every action and key") && text.contains("Typing: its keys"), "{text}");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    frame(&ctx, &mut app, vec![]);
    assert!(matches!(app.input_mode, InputMode::Insert { .. }), "back to typing: {:?}", app.input_mode);
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!(app.input_mode, InputMode::Normal);
    open(&ctx, &mut app, ":");
    frame(&ctx, &mut app, press(egui::Key::Tab, false));
    frame(&ctx, &mut app, key("leaves the field"));
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Every action and key") && !text.contains("its keys"), "{text}");
}

/// A value picked from the list goes back to the list as it was (on
/// everything, still on everything).
#[test]
fn back_from_a_value_is_the_list_as_it_was() {
    let (ctx, mut app) = app_with_graph();
    everything(&ctx, &mut app, "switch view");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.palette.picking(), Some("graph.view"));
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(app.palette.is_open() && app.palette.picking().is_none() && text.contains("Every action and key"), "{text}");
}

/// The list and the key bar say the same keys, the switch included, from
/// the frame it opens and after the switch; the view's menu's name finds
/// its actions.
#[test]
fn the_list_and_the_key_bar_say_one_line_and_a_menus_name_finds_its_actions() {
    let (ctx, mut app) = app_with_graph();
    let said = |app: &CockpitApp| {
        let actions = app.palette_actions();
        app.palette.keys_said(&app.config.keymap, |origin, id| actions.iter().find(|b| b.origin == origin && b.action.id == id))
    };
    let opened = texts(&frame(&ctx, &mut app, key(":")));
    let keys = said(&app);
    assert!(keys.iter().any(|k| k == "Tab: everything") && keys.iter().any(|k| k == "Enter picks"), "{keys:?}");
    for keys in &keys {
        assert!(opened.lines().any(|l| l == keys), "in the key bar from the frame it opens: {keys}: {opened}");
    }
    let tab = frame(&ctx, &mut app, press(egui::Key::Tab, false));
    assert!(tab.viewport_output.values().any(|v| v.repaint_delay.is_zero()), "drawn again at once");
    let switched = texts(&frame(&ctx, &mut app, vec![]));
    let keys = said(&app);
    assert!(keys.iter().any(|k| k == "Tab: what applies here") && switched.lines().any(|l| l == "Tab: what applies here"), "the key bar after the switch: {switched}");
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains(&keys.join(" · ")), "under the list: {text}");
    let text = everything(&ctx, &mut app, "graph actions");
    assert!(text.contains("Show the legend"), "the view's menu, by its name: {text}");
}

/// Closed and not asked for, nothing of the list is built; open, it is,
/// once a frame.
#[test]
fn nothing_of_the_list_is_built_while_its_closed() {
    let (ctx, mut app) = full_app();
    frame(&ctx, &mut app, vec![]);
    let before = app.listed.get();
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(app.listed.get(), before, "closed");
    open(&ctx, &mut app, ":");
    let open_at = app.listed.get();
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.listed.get(), open_at + 1, "open: once a frame");
}
