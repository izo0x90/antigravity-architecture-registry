//! The running turn's live line: what the agent is doing, how live it is
//! (receiving, waiting, quiet too long, cut off), in words, with what shows
//! it (`crate::indicator`) beside them. From what's already known: when
//! the agent's updates came (kept with its thread, `ThreadView::arrivals`),
//! whether the server's socket is up, the running turn's latest item, and
//! whether its harness can run.

use super::*;
use crate::indicator::{Activity, Liveness, Next, Stage};

/// The indicator's spot beside the words.
pub(super) fn spot() -> Vec2 {
    let look = crate::theme::look();
    Vec2::new(look.space_huge + look.space_xl, look.text_label)
}

/// A while, in words: "12 s", "3 min".
fn span_words(seconds: f64) -> String {
    if seconds < 60.0 { format!("{} s", seconds as u64) } else { format!("{} min", (seconds / 60.0) as u64) }
}

/// When a while's words next change (a second's, or a minute's, turn).
fn span_changes(seconds: f64) -> f64 {
    if seconds < 60.0 { 1.0 - seconds.fract() } else { 60.0 - seconds % 60.0 }
}

impl ChatStreamState {
    /// The agent at work at `now`, as plain data: its updates over the
    /// trace's span.
    pub(super) fn activity(&self, view: &ThreadView, now: f64) -> Activity {
        let (receiving, quiet) = (self.settings.receiving_s, self.settings.quiet_after_s);
        let since_last = view.arrivals().back().map(|(t, _)| (now - t).max(0.0));
        let asking = view.requests().iter().any(|r| self.agents().waits(r));
        let liveness = match since_last {
            _ if !self.connected => Liveness::Disconnected,
            _ if asking => Liveness::Asking,
            // (Nothing's come since it was known: no time to say.)
            None => Liveness::Waiting,
            Some(s) if s < receiving => Liveness::Receiving,
            Some(s) if s < quiet => Liveness::Waiting,
            Some(_) => Liveness::Quiet,
        };
        let updates = view.arrivals().iter().filter(|(t, _)| now - *t <= crate::indicator::TRACE_S).copied().collect();
        Activity { liveness, updates, since_last, doing: self.doing(view) }
    }

    /// What it's doing, from the running turn's latest item.
    fn doing(&self, view: &ThreadView) -> String {
        let Some(turn) = view.running() else { return words::WORKING.into() };
        let Some(item) = view.items_of(&turn.id).next_back() else { return words::STARTING.into() };
        match &item.body {
            ItemBody::AssistantMessage { .. } => words::WRITING.into(),
            _ => words::doing(item).unwrap_or_else(|| words::WORKING.into()),
        }
    }

    /// Whether the agent can still be reached, in words (for when it's
    /// been quiet too long): its slot's, its harness's state.
    fn reach(&self) -> Option<String> {
        if let Some(slot) = self.slot()
            && let SlotStatus::Unavailable { reason, .. } = &slot.status
        {
            return Some(format!("it can't run: {reason}"));
        }
        Some(match &self.instance()?.status {
            ProviderStatus::Ready => "the server still reaches it".into(),
            other => format!("its harness {}", crate::agents::status_words(other)),
        })
    }

    /// The live line's words for `activity`, their colour, and when they
    /// next change.
    pub(super) fn live_words(&self, activity: &Activity) -> (String, Color32, Next) {
        let since = activity.since_last.unwrap_or_default();
        match activity.liveness {
            Liveness::Receiving => (format!("{}…", activity.doing), colors().info, Next::In(self.settings.receiving_s - since)),
            Liveness::Waiting if activity.since_last.is_none() => (format!("{}…", activity.doing), colors().muted, Next::OnChange),
            Liveness::Waiting => (format!("{}… {} since anything", activity.doing, span_words(since)), colors().muted, Next::In(span_changes(since).min(self.settings.quiet_after_s - since))),
            Liveness::Asking => ("Waiting on you".to_owned(), colors().info, Next::OnChange),
            Liveness::Quiet => {
                let reach = self.reach().map(|r| format!(" · {r}")).unwrap_or_default();
                (format!("No word for {}{reach}", span_words(since)), colors().warn, Next::In(span_changes(since)))
            }
            Liveness::Disconnected => ("The connection to the server is down".to_owned(), colors().warn, Next::OnChange),
        }
    }

    /// The running turn's live line in `ui` (a turn's line), for
    /// `activity`: its words in the room left beside the indicator's spot
    /// (none when there's no room for both). The spot, when there is one.
    pub(super) fn render_live(&self, ui: &mut egui::Ui, activity: &Activity) -> Option<Rect> {
        let (words, color, _) = self.live_words(activity);
        let look = crate::theme::look();
        let spot = spot();
        let room = ui.available_width() - spot.x - ui.spacing().item_spacing.x;
        let label = egui::Label::new(RichText::new(words).size(look.text_small).color(color)).truncate();
        if room < spot.x {
            ui.add(label);
            return None;
        }
        ui.allocate_ui(Vec2::new(room, spot.y), |ui| ui.add(label));
        Some(ui.allocate_exact_size(spot, egui::Sense::hover()).0)
    }

    /// After the stream's drawn: the live line's indicator where its spot
    /// shows (on the running turn's line; that line scrolled past while
    /// its turn's still in sight, pinned at the stream's top), on the
    /// stage (the panel, the items on screen), see-through over the panel;
    /// the next frame asked for when it or the words next move (gently;
    /// nothing asked while it's out of sight; the words alone when there's
    /// no room for the indicator).
    pub(super) fn show_live(&mut self, ui: &mut egui::Ui, visible: Rect, panel: Rect, items: &[Rect]) {
        let Some(live) = self.ui.live_at.take() else { return };
        let spot = if visible.intersects(live.line) {
            live.spot
        } else if live.line.bottom() <= visible.top() && live.turn_bottom > visible.top() + live.line.height() {
            self.pin_live(ui, visible, &live)
        } else {
            return;
        };
        let (now, activity) = (ui.input(|i| i.time), &live.activity);
        let (_, _, mut next) = self.live_words(activity);
        if let Some(spot) = spot
            && let Some(drawer) = self.ui.indicator.drawer()
        {
            let stage = Stage { spot, panel, items: items.iter().filter(|r| r.intersects(visible)).copied().collect() };
            let over = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Middle, ui.id().with("live over"))).with_clip_rect(panel);
            next = sooner(next, drawer.draw(&ui.painter_at(spot.intersect(visible)), &over, activity, &stage, now));
        }
        if let Next::In(s) = next {
            ui.ctx().request_repaint_after_secs(s.max(crate::indicator::FASTEST_S) as f32);
        }
    }

    /// The running turn's line again at the stream's top, over what's
    /// there (its own line scrolled past). Its spot.
    fn pin_live(&mut self, ui: &mut egui::Ui, visible: Rect, live: &render::LiveAt) -> Option<Rect> {
        let turn = self.thread()?.turn(&live.turn)?.clone();
        let bar = Rect::from_min_size(visible.min, Vec2::new(visible.width(), live.line.height()));
        ui.painter().rect_filled(bar, 0.0, colors().panel);
        let mut pinned = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
        self.render_turn_line(&mut pinned, &turn, Some(&live.activity)).1
    }
}

/// The sooner of two.
fn sooner(a: Next, b: Next) -> Next {
    match (a, b) {
        (Next::In(a), Next::In(b)) => Next::In(a.min(b)),
        (Next::In(s), Next::OnChange) | (Next::OnChange, Next::In(s)) => Next::In(s),
        (Next::OnChange, Next::OnChange) => Next::OnChange,
    }
}
