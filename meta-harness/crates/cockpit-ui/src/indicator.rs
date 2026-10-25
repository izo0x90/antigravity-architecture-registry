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
    /// It waits on you (a request of its): quiet, as it should be.
    Asking,
    /// No word for a long while.
    Quiet,
    /// The connection to the server is down.
    Disconnected,
}

/// What's happening, as plain data.
#[derive(Clone, Debug)]
pub struct Activity {
    pub liveness: Liveness,
    /// When its updates over the trace's span came (the clock's seconds),
    /// oldest first: each tenth of a second that had any, and how many.
    pub updates: Vec<(f64, u32)>,
    /// Seconds since the last of them (none: none came since it was known).
    pub since_last: Option<f64>,
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

/// The ones there are (the chat's setting picks one).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Heartbeat,
    None,
}

impl crate::config::Named for Kind {
    const WHAT: &'static str = "indicator";
    const ALL: &'static [Self] = &[Self::Heartbeat, Self::None];

    fn name(self) -> &'static str {
        match self {
            Self::Heartbeat => "heartbeat",
            Self::None => "none",
        }
    }

    fn words(self) -> &'static str {
        match self {
            Self::Heartbeat => "A heartbeat",
            Self::None => "Nothing",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Heartbeat => "A trace that blips with each update, beside the running turn's words",
            Self::None => "The words alone",
        }
    }
}

/// The one picked, and it (none: nothing shows).
pub struct Shown {
    kind: Kind,
    drawer: Option<Box<dyn Indicator>>,
}

impl Shown {
    pub fn of(kind: Kind) -> Self {
        let drawer: Option<Box<dyn Indicator>> = match kind {
            Kind::Heartbeat => Some(Box::new(Heartbeat)),
            Kind::None => None,
        };
        Self { kind, drawer }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn drawer(&mut self) -> Option<&mut (dyn Indicator + 'static)> {
        self.drawer.as_deref_mut()
    }
}

impl Default for Shown {
    fn default() -> Self {
        Self::of(Kind::default())
    }
}

/// A trace over the last few seconds with a blip where updates came, as
/// they came: its rhythm is the real one, as far as the trace's width
/// shows it (updates closer than a blip's width are one blip, as tall as
/// how many they are). Waiting, it lies flat, a slow breathing dot beside
/// it; waiting on you, flat and still; quiet too long or cut off, it dims,
/// amber.
pub struct Heartbeat;

/// The seconds the trace spans.
pub const TRACE_S: f64 = 10.0;

/// Updates in one blip that make it its full height (fewer, lower).
const FULL_BLIP: u32 = 8;

impl Indicator for Heartbeat {
    fn draw(&mut self, spot: &egui::Painter, _over: &egui::Painter, activity: &Activity, stage: &Stage, now: f64) -> Next {
        let (theme, look) = (crate::theme::colors(), crate::theme::look());
        let rect = stage.spot;
        let (line, faded) = match activity.liveness {
            Liveness::Receiving | Liveness::Waiting | Liveness::Asking => (theme.ok, false),
            Liveness::Quiet | Liveness::Disconnected => (theme.warn, true),
        };
        let color = if faded { line.gamma_multiply(0.5) } else { line };
        let mid = rect.center().y;
        // The trace, and the dot's room at its right end (in the spot).
        let most = look.line_bold + 1.0;
        let trace = Rect::from_min_max(rect.min, Pos2::new(rect.right() - 2.0 * most - look.space_xs, rect.max.y));
        let blip_w = look.space_sm;
        let x_at = |t: f64| trace.right() - blip_w - ((now - t) / TRACE_S) as f32 * (trace.width() - blip_w);
        // The base line, then the blips: each update where it came; ones
        // closer than a blip's width, one blip as tall as how many.
        spot.line_segment([Pos2::new(trace.left(), mid), Pos2::new(trace.right(), mid)], Stroke::new(look.line_thin, color.gamma_multiply(0.5)));
        let in_sight = || activity.updates.iter().filter(|(t, _)| now - *t <= TRACE_S);
        let mut blips: Vec<(f32, u32)> = Vec::new();
        for (t, n) in in_sight() {
            let x = x_at(*t);
            match blips.last_mut() {
                Some((at, count)) if x - *at < blip_w => *count += n,
                _ => blips.push((x, *n)),
            }
        }
        let tallest = rect.height() / 2.0 - look.space_hair;
        for (x, count) in blips {
            let height = tallest * (0.5 + 0.5 * (count.min(FULL_BLIP) as f32 / FULL_BLIP as f32));
            let blip = vec![Pos2::new(x, mid), Pos2::new(x + blip_w / 3.0, mid - height), Pos2::new(x + blip_w * 2.0 / 3.0, mid + height * 0.6), Pos2::new(x + blip_w, mid)];
            spot.add(egui::Shape::line(blip, Stroke::new(look.line_bold, color)));
        }
        // Waiting: a dot breathing slowly at its end.
        let waiting = activity.liveness == Liveness::Waiting;
        if waiting {
            let breath = ((now * std::f64::consts::TAU / BREATH_PERIOD_S).sin() * 0.5 + 0.5) as f32;
            spot.circle_filled(Pos2::new(rect.right() - most, mid), look.line_bold + breath, line.gamma_multiply(0.35 + 0.5 * breath));
        }
        // Moving: the trace a pixel at a time while a blip's in sight; the
        // dot breathing; else still.
        let pixel = TRACE_S / f64::from((trace.width() - blip_w).max(1.0));
        match (in_sight().next().is_some(), waiting) {
            (true, true) => Next::In(pixel.min(BREATH_S)),
            (true, false) => Next::In(pixel),
            (false, true) => Next::In(BREATH_S),
            (false, false) => Next::OnChange,
        }
    }
}

/// How often the breathing dot is drawn again, and how long one breath is.
const BREATH_S: f64 = 0.15;
const BREATH_PERIOD_S: f64 = 3.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// Moving while a blip is in sight or it waits, still otherwise; all it
    /// draws inside its spot (the breathing dot too); "none" draws nothing.
    #[test]
    fn the_heartbeat_moves_only_while_there_is_something_to_move_and_draws_in_its_spot() {
        let ctx = egui::Context::default();
        crate::theme::set(crate::config::UiConfig::with_overrides("").unwrap().theme, &ctx);
        let spot = Rect::from_min_size(Pos2::new(100.0, 100.0), egui::vec2(64.0, 12.0));
        let stage = Stage { spot, panel: Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 400.0)), items: Vec::new() };
        let mut next = Vec::new();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background()).with_clip_rect(spot);
            let mut beat = Heartbeat;
            let at = |liveness, updates: Vec<(f64, u32)>| Activity { liveness, updates, since_last: Some(0.0), doing: "Thinking".into() };
            next.push(beat.draw(&painter, &painter, &at(Liveness::Receiving, vec![(99.5, 3)]), &stage, 100.0));
            next.push(beat.draw(&painter, &painter, &at(Liveness::Waiting, vec![(80.0, 1)]), &stage, 100.0));
            next.push(beat.draw(&painter, &painter, &at(Liveness::Quiet, vec![(10.0, 1)]), &stage, 100.0));
        });
        assert!(matches!(next[..], [Next::In(a), Next::In(b), Next::OnChange] if a > FASTEST_S && a < 0.3 && b == BREATH_S), "{next:?}");
        let dot = output.shapes.iter().find(|s| matches!(s.shape, egui::Shape::Circle(_))).expect("the breathing dot");
        assert!(spot.contains_rect(dot.shape.visual_bounding_rect()), "inside its spot: {:?} in {spot:?}", dot.shape.visual_bounding_rect());
        assert!(output.shapes.iter().all(|s| spot.expand(1.0).contains_rect(s.shape.visual_bounding_rect())), "all of it in its spot");
        assert!(Shown::of(Kind::None).drawer().is_none() && Shown::of(Kind::Heartbeat).drawer().is_some());
    }
}
