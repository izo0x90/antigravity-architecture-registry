//! The shell's tests; helpers here, tests by topic below.

mod audits;
mod bindings;
mod keys_snapshot;
mod layout;
mod look;
mod moves;
mod outside;
mod split;
mod views;

use super::*;

use crate::graph::tests::{loading, sample_tree, showing};
use arch_graph::{CodeAnchor, PlanId, Provenance, Relation, RelationKind, RepoPath, Source, Span};

fn texts(output: &egui::FullOutput) -> String {
    fn collect(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(list) => list.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }
    let mut out = String::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut out);
    }
    out
}

/// Clicks the drawn text `label` (a button's or a link's).
fn click_on(ctx: &egui::Context, app: &mut CockpitApp, label: &str) {
    fn find(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == label => Some(egui::Rect::from_min_size(t.pos, t.galley.size())),
            egui::Shape::Vec(list) => list.iter().find_map(|s| find(s, label)),
            _ => None,
        }
    }
    let output = frame(ctx, app, vec![]);
    let at = output.shapes.iter().find_map(|c| find(&c.shape, label)).unwrap_or_else(|| panic!("{label:?} isn't drawn: {}", texts(&output))).center();
    let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame(ctx, app, vec![egui::Event::PointerMoved(at)]);
    frame(ctx, app, vec![button(true)]);
    frame(ctx, app, vec![button(false)]);
}

/// Calls from outside, answered: each runs frames as eframe does (the
/// call's input fed in first) until it's answered.
fn from_outside(ctx: &egui::Context, app: &mut CockpitApp, call: &str) -> crate::remote::Answer {
    let door = app.remote.door(ctx);
    let answer = std::rc::Rc::new(std::cell::RefCell::new(None));
    let kept = answer.clone();
    door.knock(call, Box::new(move |a| *kept.borrow_mut() = Some(a)));
    for _ in 0..10 {
        let mut raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)), ..Default::default() };
        app.remote.feed(ctx, &mut raw);
        let _ = ctx.run(raw, |ctx| app.frame(ctx));
        if let Some(answer) = answer.borrow_mut().take() {
            return answer;
        }
    }
    panic!("{call} wasn't answered");
}

/// Columns showing these kinds, one view each.
fn columns_of(kinds: &[BufferKind], focused: usize) -> crate::columns::Columns {
    crate::columns::Columns::new(kinds.iter().map(|k| Pane::new(*k, 0)).collect(), focused)
}

/// An app with the graph in slot 2 (focused) showing the sample view.
fn app_with_graph() -> (egui::Context, CockpitApp) {
    let ctx = egui::Context::default();
    let mut app = CockpitApp::new("http://127.0.0.1:9");
    app.initialized = true; // skip network bootstrapping
    app.graph = showing(sample_tree());
    app.columns.focus(1);
    (ctx, app)
}

const SCREEN: egui::Vec2 = egui::vec2(1600.0, 1000.0);

fn frame(ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>) -> egui::FullOutput {
    ctx.run(egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)), ..Default::default() }, |ctx| app.frame(ctx))
}

fn frame_at(ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>, time: f64) -> egui::FullOutput {
    ctx.run(egui::RawInput { events, time: Some(time), screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)), ..Default::default() }, |ctx| app.frame(ctx))
}

fn key(text: &str) -> Vec<egui::Event> {
    vec![egui::Event::Text(text.into())]
}

/// Types each character in its own frame.
fn keys(ctx: &egui::Context, app: &mut CockpitApp, text: &str) {
    for c in text.chars() {
        frame(ctx, app, key(&c.to_string()));
    }
}

fn press(key: egui::Key, ctrl: bool) -> Vec<egui::Event> {
    let modifiers = egui::Modifiers { ctrl, command: ctrl, ..Default::default() };
    vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]
}

/// ctrl+key (modifiers are held for the whole frame).
fn ctrl(ctx: &egui::Context, app: &mut CockpitApp, key: egui::Key) {
    let modifiers = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
    let raw = egui::RawInput { events: press(key, true), modifiers, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)), ..Default::default() };
    let _ = ctx.run(raw, |ctx| app.frame(ctx));
}

fn selected(app: &CockpitApp) -> &str {
    app.graph.selection.as_ref().map_or("", |s| s.as_str())
}

fn call(a: &str, b: &str, order: u32, line: u32) -> Relation {
    let mut r = Relation::new(RelationKind::Calls, ComponentId::new(a), ComponentId::new(b), Provenance::OBSERVED);
    r.order = Some(order);
    r.sites = vec![Span { start_line: line, end_line: line, start_byte: 0, end_byte: 1 }];
    r
}

/// Every view with something in it: the graph (slot 2, focused) on plan
/// `p`, the outline, a code file with definitions and calls, a spec
/// (pinned on step), the queue and a conversation.
fn full_app() -> (egui::Context, CockpitApp) {
    let (ctx, mut app) = app_with_graph();
    let plan = PlanId::new("p").unwrap();
    app.graph.nav.source = Source::Plan(plan.clone());
    app.tree.set_result(crate::tree::tests::outline());
    app.code = crate::code::tests::sample();
    let step = ComponentId::new("rust:a::step");
    app.spec.pinned = true;
    app.spec.show(step.clone());
    app.spec.source = Source::Plan(plan.clone());
    app.spec.detail = Some(crate::spec::tests::detail(&step, &ComponentId::new("rust:a::save")));
    app.queue.tick(&app.net, &ctx, Some(&plan));
    // Enough jobs and messages that half a page means something.
    let mut snapshot = crate::queue::tests::snapshot(arch_graph::QueueStatus::Idle);
    for i in 0..30 {
        snapshot.jobs.push(arch_graph::Job { component: ComponentId::new(format!("rust:q{i}")), kind: arch_graph::JobKind::Implement, state: arch_graph::JobState::Queued, attempts: 0, findings: vec![] });
    }
    app.queue.on_event(&arch_graph::ArchEvent::QueueUpdated { snapshot: Box::new(snapshot) });
    app.chat = crate::chat::tests::conversation();
    for n in 4..40 {
        app.chat.process_event(crate::chat::tests::item(n, harness_contracts::ItemBody::UserMessage { text: format!("message {n}") }));
    }
    app.columns.set_count(3, 12);
    (ctx, app)
}

/// Moves the view of `kind` in use to the focused slot and draws it (so
/// it knows its size).
fn focus(ctx: &egui::Context, app: &mut CockpitApp, kind: BufferKind) {
    let pane = app.used_pane(kind);
    app.columns.place(app.columns.focused(), pane);
    app.use_pane(pane);
    for _ in 0..3 {
        frame(ctx, app, vec![]);
    }
}

fn cursor(app: &CockpitApp) -> Option<String> {
    app.view(app.focused_kind()).cursor_key()
}

/// The quick look's key, down (and typed) or let go, at `time`.
fn peek_key(ctx: &egui::Context, app: &mut CockpitApp, pressed: bool, time: f64) {
    let mut events = vec![egui::Event::Key { key: egui::Key::Q, physical_key: None, pressed, repeat: false, modifiers: Default::default() }];
    if pressed {
        events.push(egui::Event::Text("q".into()));
    }
    frame_at(ctx, app, events, time);
}

/// The agent moving the graph, in column `column` if it names one.
fn agent_moves(app: &mut CockpitApp, select: &str, column: Option<u32>) {
    let request = arch_graph::ViewRequest { select: Some(select.into()), column, ..Default::default() };
    let event = arch_graph::ArchEvent::ViewRequested { request: Box::new(request), by: arch_graph::Requester::Agent };
    app.net.incoming.lock().unwrap().push(harness_contracts::ClientEvent::Architecture { event });
}

/// Where the kept keys live (moves when they're given out again).
fn kept_keys(app: &CockpitApp) -> *const Keys {
    app.bound_cache.borrow().as_ref().map(|(_, keys)| keys.as_ptr()).unwrap()
}

fn index_event(status: arch_graph::IndexStatus) -> harness_contracts::ClientEvent {
    harness_contracts::ClientEvent::Architecture { event: arch_graph::ArchEvent::Index { status: Box::new(status) } }
}
