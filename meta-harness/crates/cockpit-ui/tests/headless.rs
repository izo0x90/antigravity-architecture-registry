//! Drives the whole UI for a few frames without a window.

use cockpit_ui::alerts::{Backend, Clicked, Permission};
use cockpit_ui::app::CockpitApp;

/// Alerts as a test has them: none can be shown.
struct NoAlerts;

impl Backend for NoAlerts {
    fn permission(&self) -> Permission {
        Permission::Unavailable("a test shows no alerts")
    }
    fn ask(&self) {}
    fn out_of_sight(&self) -> bool {
        false
    }
    fn raise(&self, _: &str, _: &harness_contracts::Notification, _: Clicked) {}
}

#[test]
fn frames_render_and_leader_opens() {
    let ctx = egui::Context::default();
    let mut app = CockpitApp::new("http://127.0.0.1:9", std::sync::Arc::new(NoAlerts));
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
    assert_eq!(app.desk.panels.len(), 3);
    assert_eq!(app.input_mode, cockpit_ui::keymap::InputMode::Normal);
}
