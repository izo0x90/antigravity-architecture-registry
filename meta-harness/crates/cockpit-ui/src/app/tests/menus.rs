//! The menus on a phone: a panel's groups wrap inside its width; a tap on
//! one opens its menu, whose entries fit the screen too.

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
    for group in groups {
        let drawn = drawn_containing(&output, group);
        assert!(!drawn.is_empty() && drawn.iter().all(|r| phone.contains_rect(*r)), "{group}: {drawn:?} on {phone:?}");
    }
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
