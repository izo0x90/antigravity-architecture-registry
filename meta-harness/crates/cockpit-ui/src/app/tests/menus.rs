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
    let session = drawn_containing(&output, "Session  s")[0];
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
