//! The menus: on a phone, a panel's groups wrap inside its width and a tap
//! opens one; one with nothing usable isn't offered; the which-key menu by
//! tap and by key; a long press on a menu's button; the top bar's path.

use super::*;

#[test]
fn on_a_phone_the_groups_wrap_inside_the_panel_and_a_tap_opens_one() {
    let (ctx, mut app) = full_app();
    app.desk.panels = panels_of(&[BufferKind::Chat], 0);
    let phone = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(390.0, 844.0));
    let mut t = 10.0;
    let mut at = |ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>| {
        t += 0.05;
        ctx.run(egui::RawInput { events, time: Some(t), screen_rect: Some(phone), ..Default::default() }, |ctx| app.frame(ctx))
    };
    at(&ctx, &mut app, vec![]);
    let output = at(&ctx, &mut app, vec![]);
    let groups = ["Chat actions", "Session", "Buffer", "Panels"];
    let mut rows = Vec::new();
    for group in groups {
        let drawn = *drawn_containing(&output, group).first().unwrap_or_else(|| panic!("{group} isn't drawn"));
        assert!(phone.contains_rect(drawn), "{group}: {drawn:?} on {phone:?}");
        rows.push(drawn.top().round());
    }
    rows.dedup();
    assert!(rows.len() > 1, "wrapped onto more than one row: {rows:?}");
    let session = *drawn_containing(&output, "Session  Space s").first().expect("the Session menu's button");
    let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos: session.center(), force: None };
    let button = |pressed| egui::Event::PointerButton { pos: session.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    at(&ctx, &mut app, vec![touch(egui::TouchPhase::Start), egui::Event::PointerMoved(session.center()), button(true)]);
    at(&ctx, &mut app, vec![touch(egui::TouchPhase::End), button(false), egui::Event::PointerGone]);
    at(&ctx, &mut app, vec![]);
    assert_eq!((app.input_mode, app.leader_path.clone()), (InputMode::Leader, vec!['s']), "a tap opens its menu");
    let output = at(&ctx, &mut app, vec![]);
    let entries = drawn_containing(&output, "New session");
    assert!(!entries.is_empty() && entries.iter().all(|r| phone.contains_rect(*r)), "its entries on the screen: {entries:?}");
}

/// A menu's path is long: the top bar's mode is cut short in its place,
/// so the bar keeps its height and its right side doesn't move or get
/// drawn over, at a width with little room between them.
#[test]
fn a_menus_path_in_the_top_bar_moves_nothing() {
    let (ctx, mut app) = full_app();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1300.0, 1000.0));
    let at = |ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>| ctx.run(egui::RawInput { events, screen_rect: Some(screen), ..Default::default() }, |ctx| app.frame(ctx));
    at(&ctx, &mut app, vec![]);
    let normal = at(&ctx, &mut app, vec![]);
    let first = |output: &egui::FullOutput, words: &str| *drawn_containing(output, words).first().unwrap_or_else(|| panic!("{words:?} isn't drawn"));
    let (panels, group) = (first(&normal, "Panels: "), first(&normal, "Chat actions"));
    assert!(first(&normal, "NORMAL").max.x < panels.min.x, "the mode before the right side");
    app.open_menu(&[app.config.keymap.view_menu], &ctx);
    let menu = at(&ctx, &mut app, vec![]);
    assert_eq!(app.input_mode, InputMode::Leader);
    let mode = drawn_containing(&menu, "Space").into_iter().filter(|r| r.max.y < group.min.y).min_by(|a, b| a.min.x.total_cmp(&b.min.x)).expect("the mode, on the top bar's row");
    assert!(mode.max.x < panels.min.x, "cut short before the right side: {mode:?}, {panels:?}");
    assert_eq!((first(&menu, "Panels: "), first(&menu, "Chat actions")), (panels, group), "the right side and the panels stay where they were");
}

/// A menu shows on a panel's top line only while something in it can be
/// used there: the quick look's, on a card, not on nothing.
#[test]
fn a_menu_with_nothing_usable_here_isnt_offered() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    assert!(!drawn_containing(&frame(&ctx, &mut app, vec![]), "Quick look  Space q").is_empty(), "on a card");
    app.desk.graph_mut().select(None);
    frame(&ctx, &mut app, vec![]);
    assert!(drawn_containing(&frame(&ctx, &mut app, vec![]), "Quick look").is_empty(), "on nothing");
}

/// The which-key menu by tap and by key: an entry (its next key and its
/// label, one target) runs when tapped; Escape, or its button, goes back
/// one level, and from the top closes it.
#[test]
fn a_which_key_entry_runs_when_tapped_and_escape_goes_back_one_level() {
    let (ctx, mut app) = app_with_graph();
    let menu = app.config.keymap.view_menu;
    app.open_menu(&[menu], &ctx);
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    let text = texts(&output);
    assert!(text.lines().zip(text.lines().skip(1)).any(|(key, label)| key.chars().count() == 1 && label == "Show the legend"), "an entry says its next key alone: {text}");
    let was = app.desk.graph().legend_open;
    click_on(&ctx, &mut app, "Show the legend");
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.desk.graph().legend_open, app.input_mode), (!was, InputMode::Normal), "tapped: it ran, and the menu closed");

    app.open_menu(&[menu], &ctx);
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!((app.input_mode, app.leader_path.clone()), (InputMode::Leader, Vec::new()), "Escape: back one level");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!(app.input_mode, InputMode::Normal, "from the top: closed");
    app.open_menu(&[menu], &ctx);
    frame(&ctx, &mut app, vec![]);
    click_on(&ctx, &mut app, "Escape");
    assert_eq!((app.input_mode, app.leader_path.clone()), (InputMode::Leader, Vec::new()), "its button: back one level");
}

/// A finger held still on a menu's button says what it is and its keys,
/// as hovering it does (egui's tip, shown while held).
#[test]
fn a_finger_held_on_a_menu_button_says_what_it_is_and_its_keys() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    let button = drawn_containing(&output, "Graph actions  Space ,").first().expect("the view's menu button").center();
    let mut t = 10.0;
    let mut at = |ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>| {
        t += 0.1;
        frame_at(ctx, app, events, t)
    };
    let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos: button, force: None };
    at(&ctx, &mut app, vec![touch(egui::TouchPhase::Start), egui::Event::PointerMoved(button), egui::Event::PointerButton { pos: button, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }]);
    let mut said = String::new();
    for _ in 0..12 {
        said = texts(&at(&ctx, &mut app, vec![]));
    }
    assert!(said.contains("Graph actions (Space ,)"), "held: its tip: {said}");
}

/// The which-key menu is where it stays from the first frame it's shown
/// (it was drawn aside for a frame, measured narrower than it is).
#[test]
fn the_which_key_menu_doesnt_move_once_shown() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    app.open_menu(&[app.config.keymap.view_menu], &ctx);
    let mut shown = Vec::new();
    for _ in 0..4 {
        let output = frame(&ctx, &mut app, vec![]);
        shown.extend(drawn_containing(&output, "Show the legend"));
    }
    assert!(shown.len() >= 3 && shown.windows(2).all(|w| w[0] == w[1]), "the same place each frame: {shown:?}");
}
