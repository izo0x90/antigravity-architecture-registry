//! What's over the panels, one stack: a tap outside the top one closes it
//! and reaches nothing under it; the way back out closes the top one only;
//! what's under a modal one takes no keys; the keyboard goes back where it
//! was.

use super::*;

/// A tap (press, then release) at `at`, at `time`.
fn tap(ctx: &egui::Context, app: &mut CockpitApp, at: egui::Pos2, time: f64) {
    let button =
        |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame_at(ctx, app, vec![egui::Event::PointerMoved(at), button(true)], time);
    frame_at(ctx, app, vec![button(false)], time + 0.05);
    frame_at(ctx, app, vec![], time + 0.1);
}

/// A point in the panel that isn't focused, near its bottom corner (below
/// what floats over the middle).
fn in_the_other_panel(app: &CockpitApp) -> egui::Pos2 {
    let other = (app.desk.panels.focused() + 1) % app.desk.panels.len();
    let rect = app.panel_rects[other];
    egui::pos2(rect.left() + 30.0, rect.bottom() - 40.0)
}

#[test]
fn a_tap_outside_closes_each_and_reaches_nothing_under_it() {
    type Opens = fn(&egui::Context, &mut CockpitApp);
    type IsOpen = fn(&CockpitApp) -> bool;
    let kinds: [(&str, Opens, IsOpen); 4] = [
        ("the action list", |ctx, app| drop(frame(ctx, app, key(":"))), |app| app.palette.is_open()),
        ("the menus", |ctx, app| app.open_menu(&[], ctx), |app| app.input_mode == InputMode::Leader),
        ("the issues", |_, app| app.feedback.is_issues_window_open = true, |app| app.feedback.is_issues_window_open),
        (
            "the quick look",
            |ctx, app| {
                crate::graph::tests::with_detail(app.desk.graph_mut(), "rust:a::step", "src/a.rs", 10, 20);
                app.desk.graph_mut().select(Some("rust:a::step".into()));
                frame_at(ctx, app, vec![], 1.0);
                peek_key(ctx, app, true, 1.0);
                peek_key(ctx, app, false, 1.05);
            },
            |app| app.quick_look.is_some(),
        ),
    ];
    for (what, open, is_open) in kinds {
        let (ctx, mut app) = app_with_graph();
        frame_at(&ctx, &mut app, vec![], 0.5);
        let focused = app.desk.panels.focused();
        open(&ctx, &mut app);
        frame_at(&ctx, &mut app, vec![], 2.0);
        frame_at(&ctx, &mut app, vec![], 2.1);
        assert!(is_open(&app), "{what}: open");
        let there = in_the_other_panel(&app);
        assert!(
            ctx.layer_id_at(there).is_none_or(|l| l.order == egui::Order::Background),
            "{what}: the tap is outside what's over the panels"
        );
        tap(&ctx, &mut app, there, 3.0);
        assert!(!is_open(&app), "{what}: a tap outside closes it");
        assert_eq!(app.desk.panels.focused(), focused, "{what}: the tap went no further (the panel under it isn't focused)");
        // The next tap is a tap.
        tap(&ctx, &mut app, there, 4.0);
        assert_ne!(app.desk.panels.focused(), focused, "{what}: then a tap there focuses it");
    }
}

#[test]
fn the_way_back_out_closes_the_top_one_only() {
    let (ctx, mut app) = app_with_graph();
    crate::graph::tests::with_detail(app.desk.graph_mut(), "rust:a::step", "src/a.rs", 10, 20);
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    frame_at(&ctx, &mut app, vec![], 1.0);
    peek_key(&ctx, &mut app, true, 1.0);
    peek_key(&ctx, &mut app, false, 1.05);
    frame_at(&ctx, &mut app, key(":"), 1.1);
    frame_at(&ctx, &mut app, vec![], 1.2);
    assert!(app.quick_look.is_some() && app.palette.is_open());
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 1.3);
    frame_at(&ctx, &mut app, vec![], 1.4);
    assert!(app.quick_look.is_some() && !app.palette.is_open(), "the list over the card: it alone");
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 1.5);
    assert!(app.quick_look.is_none(), "then the card");
    // One without a way back of its own (the issues): closed by it too.
    app.feedback.is_issues_window_open = true;
    frame_at(&ctx, &mut app, vec![], 2.0);
    frame_at(&ctx, &mut app, vec![], 2.1);
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 2.2);
    assert!(!app.feedback.is_issues_window_open);
}

#[test]
fn under_a_modal_one_the_panels_take_no_keys() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    let before = app.desk.view(app.focused_kind()).cursor_key();
    app.feedback.is_issues_window_open = true;
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, key("j"));
    frame(&ctx, &mut app, key("j"));
    assert_eq!(app.desk.view(app.focused_kind()).cursor_key(), before, "j went nowhere under the issues");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    frame(&ctx, &mut app, key("j"));
    assert_ne!(app.desk.view(app.focused_kind()).cursor_key(), before, "closed: j moves again");
}

#[test]
fn the_menus_closed_give_the_keyboard_back_where_it_was() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    frame(&ctx, &mut app, key("i"));
    assert!(matches!(app.input_mode, InputMode::Insert { .. }));
    let typing = app.input_mode;
    // Opened from typing (its button), left with the way back out: typing again.
    app.open_menu(&[], &ctx);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.input_mode, InputMode::Leader);
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!(app.input_mode, typing);
    // Left with a tap outside: the same.
    app.open_menu(&[], &ctx);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    let corner = egui::pos2(5.0, SCREEN.y / 2.0);
    let button =
        |pressed| egui::Event::PointerButton { pos: corner, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(corner), button(true)]);
    frame(&ctx, &mut app, vec![button(false)]);
    assert_eq!(app.input_mode, typing);
}
