//! The running turn's live line: what the agent is doing, how live it is
//! (receiving, waiting, quiet too long, cut off), in words, with what shows
//! it (`crate::indicator`) beside them. From what's already known: each
//! update's arrival (the thread's update count changing), the event
//! socket's state, the turn's latest item.

use super::*;
use crate::indicator::{Activity, Liveness, Next, Stage};

/// Updates in the last this-many seconds: receiving.
const RECEIVING_S: f64 = 1.5;
/// No update for this long: quiet too long.
const QUIET_S: f64 = 60.0;

impl ChatStreamState {
    /// Once a frame while drawn: an update arrived (the count changed), and
    /// since when the turn has run.
    pub(super) fn note_updates(&mut self, now: f64, connected: bool) {
        self.ui.connected = connected;
        let (activity, running) = self.thread().map_or((None, false), |t| (Some(t.activity), t.running_turn().is_some()));
        if activity != self.ui.seen_activity {
            if self.ui.seen_activity.is_some() {
                self.ui.updates.push_back(now);
            }
            self.ui.seen_activity = activity;
        }
        while self.ui.updates.front().is_some_and(|t| now - t > crate::indicator::TRACE_S) {
            self.ui.updates.pop_front();
        }
        self.ui.running_since = if running { self.ui.running_since.or(Some(now)) } else { None };
    }

    /// The agent at work now, as plain data.
    pub(super) fn activity(&self, now: f64) -> Activity {
        let last = self.ui.updates.back().copied().or(self.ui.running_since).unwrap_or(now);
        let since_last = now - last;
        let liveness = match since_last {
            _ if !self.ui.connected => Liveness::Disconnected,
            s if s < RECEIVING_S => Liveness::Receiving,
            s if s < QUIET_S => Liveness::Waiting,
            _ => Liveness::Quiet,
        };
        Activity { liveness, updates: self.ui.updates.iter().copied().collect(), since_last, doing: self.doing() }
    }

    /// What it's doing, from the running turn's latest item.
    fn doing(&self) -> String {
        let Some(view) = self.thread() else { return "Working".into() };
        let Some(turn) = view.running_turn() else { return "Working".into() };
        let Some(item) = view.items_of(&turn.id).last() else { return "Starting".into() };
        let file = |locations: &[harness_contracts::FileLocation]| locations.first().map(|l| l.path.rsplit('/').next().unwrap_or(&l.path).to_owned());
        match &item.body {
            ItemBody::Request { request_id } if view.request(request_id).is_some_and(|r| self.agents().waits(r)) => "Asking you".into(),
            ItemBody::Reasoning { .. } => "Thinking".into(),
            ItemBody::AssistantMessage { .. } => "Writing".into(),
            ItemBody::ToolCall { kind, locations, .. } => match (kind, file(locations)) {
                (ToolKind::Read, Some(f)) => format!("Reading {f}"),
                (ToolKind::Edit, Some(f)) => format!("Editing {f}"),
                (ToolKind::Read, None) => "Reading".into(),
                (ToolKind::Edit, None) => "Editing".into(),
                (ToolKind::Search, _) => "Searching".into(),
                (ToolKind::Execute, _) => "Running a command".into(),
                (ToolKind::Fetch, _) => "Fetching".into(),
                (ToolKind::Think, _) => "Thinking".into(),
                (ToolKind::Delete | ToolKind::Move, _) => "Changing files".into(),
                (ToolKind::Other, _) => item.title.clone().unwrap_or_else(|| "Using a tool".into()),
            },
            ItemBody::CommandExecution { .. } => "Running a command".into(),
            ItemBody::FileChange { path, .. } => format!("Changing {}", path.rsplit('/').next().unwrap_or(path)),
            _ => "Working".into(),
        }
    }

    /// The live line on the running turn's line: its words, then what
    /// shows it in a spot beside them; the next frame asked for when either
    /// moves (at most about 30 a second).
    pub(super) fn render_live(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        let activity = self.activity(now);
        let look = crate::theme::look();
        let (words, color) = match activity.liveness {
            Liveness::Receiving => (format!("{}…", activity.doing), colors().info),
            Liveness::Waiting => (format!("{}… {} s since anything", activity.doing, activity.since_last as u64), colors().muted),
            Liveness::Quiet => (format!("No word for {} min", (activity.since_last / 60.0) as u64), colors().warn),
            Liveness::Disconnected => ("The connection to the server is down".to_owned(), colors().warn),
        };
        ui.add(egui::Label::new(RichText::new(words).size(look.text_small).color(color)).truncate());
        let (spot, _) = ui.allocate_exact_size(egui::vec2(64.0, 12.0), egui::Sense::hover());
        // (The words say seconds: again within the second.)
        let mut next = 1.0f64;
        if let Some(live) = self.ui.live.as_mut() {
            let stage = Stage { spot, panel: ui.clip_rect(), items: self.ui.item_rects.clone() };
            if let Next::In(s) = live.draw(&ui.painter_at(spot), ui.painter(), &activity, &stage, now) {
                next = next.min(s);
            }
        }
        ui.ctx().request_repaint_after_secs(next.max(crate::indicator::FASTEST_S) as f32);
    }
}
