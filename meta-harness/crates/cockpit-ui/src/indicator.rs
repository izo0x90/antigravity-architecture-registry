//! What shows an agent at work, behind one small interface so there can be
//! several (a heartbeat now; others later). The chat hands one plain data
//! each frame: how live the agent is (receiving, waiting, quiet too long,
//! the connection down), when its recent updates came, what it's doing, in
//! a word; and the stage, as shapes: its spot on the turn's line, the
//! panel, the items on screen. It draws there (see-through over the panel,
//! clipped to it), takes no clicks or keys, and says when it next wants to
//! move; the chat asks for that frame, at most about 30 a second, and only
//! while it's drawn.

use egui::{Pos2, Rect, Stroke};

/// How live the agent is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Liveness {
    /// Something came in a moment ago.
    Receiving,
    /// Nothing new, the connection fine.
    Waiting,
    /// No word for a long while.
    Quiet,
    /// The connection to the server is down.
    Disconnected,
}

/// What's happening, as plain data.
#[derive(Clone, Debug)]
pub struct Activity {
    pub liveness: Liveness,
    /// When its recent updates came (the clock's seconds), oldest first.
    pub updates: Vec<f64>,
    /// Seconds since the last of them.
    pub since_last: f64,
    /// What it's doing, in a word or two ("Thinking", "Reading render.rs").
    pub doing: String,
}

/// Where it may draw: its own spot (on the turn's line), the panel (what
/// it's clipped to), the items on screen.
#[derive(Clone, Debug)]
pub struct Stage {
    pub spot: Rect,
    pub panel: Rect,
    pub items: Vec<Rect>,
}

/// When it next wants to be drawn again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Next {
    /// In this many seconds.
    In(f64),
    /// Not until something changes.
    OnChange,
}

/// The fastest it's drawn again for (about 30 a second).
pub const FASTEST_S: f64 = 1.0 / 30.0;

/// One way of showing the agent at work.
pub trait Indicator {
    /// Draws `activity` on `stage` at `now`: `spot` paints on its spot,
    /// `over` see-through over the panel (clipped to it).
    fn draw(&mut self, spot: &egui::Painter, over: &egui::Painter, activity: &Activity, stage: &Stage, now: f64) -> Next;
}

/// The ones there are, by name (the chat's setting picks one).
pub const NAMES: &[&str] = &["heartbeat", "none"];

/// The one named `name` (none for an unknown name).
pub fn by_name(name: &str) -> Option<Box<dyn Indicator>> {
    match name {
        "heartbeat" => Some(Box::new(Heartbeat)),
        _ => None,
    }
}

/// A trace over the last few seconds that blips once for each update as it
/// came: its rhythm is the real one. Waiting, it lies flat, a slow
/// breathing dot beside it; quiet too long or cut off, it dims, amber.
pub struct Heartbeat;

/// The seconds the trace spans.
pub const TRACE_S: f64 = 10.0;

impl Indicator for Heartbeat {
    fn draw(&mut self, spot: &egui::Painter, _over: &egui::Painter, activity: &Activity, stage: &Stage, now: f64) -> Next {
        let (theme, look) = (crate::theme::colors(), crate::theme::look());
        let rect = stage.spot;
        let (line, faded) = match activity.liveness {
            Liveness::Receiving | Liveness::Waiting => (theme.ok, false),
            Liveness::Quiet | Liveness::Disconnected => (theme.warn, true),
        };
        let color = if faded { line.gamma_multiply(0.5) } else { line };
        let mid = rect.center().y;
        let x_at = |t: f64| rect.right() - ((now - t) / TRACE_S) as f32 * rect.width();
        // The base line, then a blip for each update in sight.
        spot.line_segment([Pos2::new(rect.left(), mid), Pos2::new(rect.right(), mid)], Stroke::new(look.line_thin, color.gamma_multiply(0.5)));
        let height = rect.height() / 2.0 - 1.0;
        for t in activity.updates.iter().filter(|t| now - **t <= TRACE_S) {
            let x = x_at(*t);
            let blip = [Pos2::new(x - 2.0, mid), Pos2::new(x, mid - height), Pos2::new(x + 2.0, mid + height * 0.6), Pos2::new(x + 4.0, mid)];
            spot.add(egui::Shape::line(blip.to_vec(), Stroke::new(look.line_bold, color)));
        }
        // Waiting: a dot breathing slowly beside it.
        if activity.liveness == Liveness::Waiting {
            let breath = ((now * std::f64::consts::TAU / 3.0).sin() * 0.5 + 0.5) as f32;
            spot.circle_filled(Pos2::new(rect.right() + look.space_sm, mid), look.line_bold + breath, line.gamma_multiply(0.35 + 0.5 * breath));
        }
        // Moving while a blip is in sight or a dot breathes; else still.
        let moving = activity.updates.iter().any(|t| now - *t <= TRACE_S) || activity.liveness == Liveness::Waiting;
        if moving { Next::In(0.1) } else { Next::OnChange }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moving while a blip is in sight or it waits; still otherwise; and
    /// the named ones are there.
    #[test]
    fn the_heartbeat_moves_only_while_there_is_something_to_move() {
        let ctx = egui::Context::default();
        let stage = Stage { spot: Rect::from_min_size(Pos2::ZERO, egui::vec2(60.0, 12.0)), panel: Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 400.0)), items: Vec::new() };
        let mut next = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let mut beat = Heartbeat;
            let at = |liveness, updates: Vec<f64>| Activity { liveness, updates, since_last: 0.0, doing: "Thinking".into() };
            next.push(beat.draw(&painter, &painter, &at(Liveness::Receiving, vec![99.5]), &stage, 100.0));
            next.push(beat.draw(&painter, &painter, &at(Liveness::Waiting, vec![80.0]), &stage, 100.0));
            next.push(beat.draw(&painter, &painter, &at(Liveness::Quiet, vec![10.0]), &stage, 100.0));
        });
        assert_eq!(next, [Next::In(0.1), Next::In(0.1), Next::OnChange]);
        assert!(by_name("heartbeat").is_some() && by_name("none").is_none() && NAMES.contains(&"heartbeat"));
    }
}
