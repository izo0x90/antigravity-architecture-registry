//! Drives the whole UI for a few frames without a window.

use cockpit_ui::app::CockpitApp;

#[test]
fn frames_render_and_leader_opens() {
    let ctx = egui::Context::default();
    let mut app = CockpitApp::new("http://127.0.0.1:9");
    let input = |events: Vec<egui::Event>| egui::RawInput { events, ..Default::default() };
    let space = |pressed| egui::Event::Key { key: egui::Key::Space, physical_key: None, pressed, repeat: false, modifiers: Default::default() };
    for _ in 0..3 {
        let _ = ctx.run(input(Vec::new()), |ctx| app.frame(ctx));
    }
    let _ = ctx.run(input(vec![space(true)]), |ctx| app.frame(ctx));
    let _ = ctx.run(input(vec![space(false)]), |ctx| app.frame(ctx));
    assert_eq!(app.input_mode, cockpit_ui::keymap::InputMode::Leader);
    let _ = ctx.run(input(vec![egui::Event::Text("l".into())]), |ctx| app.frame(ctx));
    let _ = ctx.run(input(vec![egui::Event::Text("3".into())]), |ctx| app.frame(ctx));
    assert_eq!(app.column_count, 3);
    assert_eq!(app.input_mode, cockpit_ui::keymap::InputMode::Normal);
}
