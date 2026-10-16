//! The mouse, the trackpad and fingers, the same in every view: the wheel
//! moves (Shift: sideways), Ctrl or Cmd + wheel and pinching zoom where a
//! view zooms, a drag moves and a flick glides on, a tap selects and a
//! double tap opens, and fingers are fed in as the browser feeds them.

use super::*;

/// What the browser sends for one finger: a touch, and the pointer egui
/// makes of the first finger.
fn finger(phase: egui::TouchPhase, pos: egui::Pos2) -> Vec<egui::Event> {
    let touch = egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos, force: None };
    let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    match phase {
        egui::TouchPhase::Start => vec![button(true), touch],
        egui::TouchPhase::Move => vec![egui::Event::PointerMoved(pos), touch],
        _ => vec![button(false), egui::Event::PointerGone, touch],
    }
}

fn wheel(delta: egui::Vec2, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    vec![egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, modifiers }]
}

fn pan(app: &CockpitApp) -> egui::Vec2 {
    app.desk.graph.camera().pan
}

fn zoom(app: &CockpitApp) -> f32 {
    app.desk.graph.camera().zoom
}

#[test]
fn on_the_graph_the_wheel_moves_ctrl_wheel_and_pinching_zoom_and_a_flick_glides() {
    let (ctx, mut app) = app_with_graph();
    // Tall: room to move up, down and to the right (it starts at its left edge).
    app.desk.graph = crate::graph::tests::showing(crate::graph::tests::wide_tree());
    frame(&ctx, &mut app, vec![]);
    let middle = app.slot_rects[1].center();
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(middle)]);
    // The wheel and two fingers on a trackpad move it, Shift sideways.
    let before = pan(&app);
    frame(&ctx, &mut app, wheel(egui::vec2(0.0, -5.0), Default::default()));
    assert_eq!(pan(&app) - before, egui::vec2(0.0, -5.0), "down");
    frame(&ctx, &mut app, wheel(egui::vec2(3.0, 4.0), Default::default()));
    assert_eq!(pan(&app) - before, egui::vec2(3.0, -1.0), "any way");
    frame(&ctx, &mut app, wheel(egui::vec2(0.0, 5.0), egui::Modifiers::SHIFT));
    assert_eq!(pan(&app) - before, egui::vec2(8.0, -1.0), "Shift: sideways");
    // Ctrl (Cmd) + one notch: one step of the zoom keys, around the pointer.
    let (step, z) = (app.desk.graph.ui.zoom_step, zoom(&app));
    frame(&ctx, &mut app, wheel(egui::vec2(0.0, 100.0), egui::Modifiers::COMMAND));
    assert!((zoom(&app) - z * step).abs() < 1e-4, "{} after {z}", zoom(&app));
    // A pinch on a trackpad (the browser's Ctrl + wheel, as a zoom).
    let z = zoom(&app);
    frame(&ctx, &mut app, vec![egui::Event::Zoom(1.1)]);
    assert!((zoom(&app) - z * 1.1).abs() < 1e-4);
    // A drag moves it; let go moving and it glides on, slowing to a stop.
    let mut t = 1.0;
    let mut at = middle;
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame_at(&ctx, &mut app, vec![button(at, true)], t);
    for _ in 0..5 {
        (t, at) = (t + 0.016, at + egui::vec2(20.0, 0.0));
        frame_at(&ctx, &mut app, vec![egui::Event::PointerMoved(at)], t);
    }
    let dragged = pan(&app);
    frame_at(&ctx, &mut app, vec![button(at, false)], t + 0.016);
    let mut last = pan(&app);
    let mut glided = 0;
    for n in 2..200 {
        frame_at(&ctx, &mut app, vec![], t + 0.016 * n as f64);
        if pan(&app) == last {
            break;
        }
        (last, glided) = (pan(&app), glided + 1);
    }
    assert!(glided > 3 && last.x > dragged.x + 20.0, "glided on {} over {glided} frames", last.x - dragged.x);
    assert_eq!(last.y, dragged.y, "the way it was flicked");
}

#[test]
fn on_the_graph_fingers_tap_select_double_tap_go_in_drag_and_two_pinch_and_pan() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    let step = app.desk.graph.to_screen(&app.desk.graph.card("rust:a::step").unwrap().rect).center();
    let mut t = 1.0;
    let mut feed = |app: &mut CockpitApp, events: Vec<egui::Event>| {
        t += 0.05;
        frame_at(&ctx, app, events, t);
    };
    feed(&mut app, finger(egui::TouchPhase::Start, step));
    feed(&mut app, finger(egui::TouchPhase::End, step));
    assert_eq!(app.desk.graph.selection.as_deref(), Some("rust:a::step"), "a tap selects");
    feed(&mut app, finger(egui::TouchPhase::Start, step));
    feed(&mut app, finger(egui::TouchPhase::End, step));
    feed(&mut app, vec![]);
    assert_eq!(app.desk.graph.selection.as_deref(), Some("rust:a::save"), "a double tap opens it, as Enter: to what it calls");
    assert!(app.desk.jumps.can_back(), "a jump, as Enter's");
    // One finger drags it, sideways too: it moves, it isn't a swipe back.
    feed(&mut app, vec![]);
    let middle = app.slot_rects[1].center();
    let before = pan(&app);
    feed(&mut app, finger(egui::TouchPhase::Start, middle));
    feed(&mut app, finger(egui::TouchPhase::Move, middle + egui::vec2(60.0, 0.0)));
    feed(&mut app, finger(egui::TouchPhase::Move, middle + egui::vec2(160.0, 0.0)));
    feed(&mut app, finger(egui::TouchPhase::End, middle + egui::vec2(160.0, 0.0)));
    feed(&mut app, vec![]);
    assert!(pan(&app).x > before.x, "it moved (as far as the tree's edge lets it): {:?} from {before:?}", pan(&app));
    assert!(!app.desk.jumps.can_forward() && app.desk.graph.selection.as_deref() == Some("rust:a::save"), "not a swipe back");
    // Two fingers: spread to zoom around them, moved together to pan.
    let two = |phase, a: egui::Pos2, b: egui::Pos2| {
        let touch = |id, pos| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(id), phase, pos, force: None };
        let mut events = finger(phase, a);
        events.retain(|e| !matches!(e, egui::Event::Touch { .. }));
        events.extend([touch(1, a), touch(2, b)]);
        events
    };
    let (a, b) = (middle - egui::vec2(50.0, 0.0), middle + egui::vec2(50.0, 0.0));
    let (z, before) = (zoom(&app), pan(&app));
    feed(&mut app, two(egui::TouchPhase::Start, a, b));
    feed(&mut app, two(egui::TouchPhase::Move, a, b));
    feed(&mut app, two(egui::TouchPhase::Move, a - egui::vec2(25.0, 0.0), b + egui::vec2(25.0, 0.0)));
    assert!((zoom(&app) - z * 1.5).abs() < 1e-3, "spread by half: {} from {z}", zoom(&app));
    let z = zoom(&app);
    feed(&mut app, two(egui::TouchPhase::Move, a + egui::vec2(-25.0, 40.0), b + egui::vec2(25.0, 40.0)));
    assert!((zoom(&app) - z).abs() < 1e-4 && pan(&app).y > before.y + 30.0, "moved together: it pans");
    feed(&mut app, two(egui::TouchPhase::End, a + egui::vec2(-25.0, 40.0), b + egui::vec2(25.0, 40.0)));
    let lifted = pan(&app);
    feed(&mut app, vec![]);
    feed(&mut app, vec![]);
    assert_eq!(pan(&app), lifted, "lifting two fingers isn't a flick");
}

#[test]
fn a_swipe_where_the_view_moves_sideways_moves_it_instead() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    let wide = (1..=40).map(|n| format!("let line_{n} = \"{}\";", "x".repeat(400))).collect::<Vec<_>>().join("\n");
    app.desk.code.current_file = "src/wide.rs".into();
    app.desk.code.set_content(wide);
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    // A jump, to have somewhere to swipe back to.
    frame(&ctx, &mut app, key("G"));
    assert_eq!(app.desk.code.target_line, Some(40));
    let middle = app.slot_rects[app.desk.columns.focused()].center();
    let mut t = 5.0;
    let mut flick = |app: &mut CockpitApp, dx: f32| {
        for (phase, pos) in [(egui::TouchPhase::Start, middle), (egui::TouchPhase::Move, middle + egui::vec2(dx, 0.0)), (egui::TouchPhase::End, middle + egui::vec2(dx, 0.0))] {
            t += 0.05;
            frame_at(&ctx, app, finger(phase, pos), t);
        }
        t += 0.05;
        frame_at(&ctx, app, vec![], t);
    };
    flick(&mut app, -150.0);
    assert!(app.desk.code.viewport.offset.x > 100.0, "the finger moved the long lines: {}", app.desk.code.viewport.offset.x);
    flick(&mut app, 150.0);
    assert_eq!(app.desk.code.target_line, Some(40), "not a swipe back (where nothing moves sideways it is: the chat's swipes)");
}

#[test]
fn after_moving_the_graph_by_hand_a_key_starts_from_what_shows() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph = crate::graph::tests::showing(crate::graph::tests::wide_tree());
    frame(&ctx, &mut app, vec![]);
    keys(&ctx, &mut app, "gg");
    let first = app.desk.graph.selection.clone().unwrap();
    let middle = app.slot_rects[1].center();
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(middle)]);
    for n in 0..150 {
        frame_at(&ctx, &mut app, wheel(egui::vec2(0.0, -7.0), Default::default()), 1.0 + n as f64 * 0.02);
    }
    let shows = |app: &CockpitApp, key: &str| app.desk.graph.on_screen().iter().any(|c| c.key == key);
    assert!(!shows(&app, &first), "scrolled away from it");
    keys(&ctx, &mut app, "j");
    let now = app.desk.graph.selection.clone().unwrap();
    assert!(now != first && shows(&app, &now), "j went on from a card that shows: {now}");
}
