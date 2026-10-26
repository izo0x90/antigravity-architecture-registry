//! A chat's sub-agents: a strip above the typing box while any is at work
//! (or has just ended), a chip each that opens it over the panels; how
//! many are at work, for the live line and the panel's top line; moving
//! from one to the next. What each is comes from the agents (`Seen`).

use egui::Color32;
use harness_contracts::{ItemBody, SubagentStatus};

use super::*;
use crate::store::Seen;

/// How long one that ended stays in the strip, in seconds.
const ENDED_STAYS: f64 = 6.0;
/// Chips shown before they fold into one ("4 sub-agents working").
const CHIPS_MOST: usize = 3;
/// The folded strip's place (moved to, it shows each).
const EACH: &str = "subagents";

/// Its state in a word, and its colour.
pub(crate) fn state_words(status: SubagentStatus) -> (&'static str, Color32) {
    match status {
        SubagentStatus::Running => ("working", colors().info),
        SubagentStatus::Waiting => ("waiting on you", colors().warn),
        SubagentStatus::Completed => ("done", colors().ok),
        SubagentStatus::Idle => ("ended", colors().muted),
        SubagentStatus::Failed => ("failed", colors().error),
        SubagentStatus::Cancelled => ("stopped", colors().muted),
    }
}

/// How long it ran (or has, at `now`).
pub(crate) fn elapsed(seen: &Seen, now: f64) -> String {
    let s = (seen.ended.unwrap_or(now) - seen.since).max(0.0) as u64;
    if s < 60 { format!("{s} s") } else { format!("{} min", s / 60) }
}

impl ChatStreamState {
    /// How many sub-agents of the thread it shows are at work.
    pub(crate) fn subagents_working(&self) -> usize {
        self.thread().map_or(0, |v| v.subagents().iter().filter(|s| s.subagent.status.working()).count())
    }

    /// The strip: each at work (or just ended) a chip, its name, its state,
    /// what it's doing, how long; past a few, one chip with how many. Each
    /// chip is a place: a tap puts the cursor on where it started (its
    /// controls under it), a double tap (or Enter) opens it; the count's
    /// shows each.
    pub(super) fn render_subagents(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        let Some(view) = self.thread() else { return };
        let shown: Vec<(Seen, Option<usize>)> = view.subagents().iter().filter(|s| s.subagent.status.working() || s.ended.is_some_and(|e| now - e < ENDED_STAYS)).map(|s| (s.clone(), view.shown_at(&s.subagent.item_id))).collect();
        drop(view);
        if shown.is_empty() {
            return;
        }
        // (Its times change each second; one that ended goes after a while.)
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        let look = crate::theme::look();
        let tall = ui.spacing().interact_size.y.max(look.text_title * 1.8);
        let font = egui::FontId::proportional(look.text_body);
        let mut chips: Vec<(String, egui::Rect)> = Vec::new();
        ui.horizontal_wrapped(|ui| {
            // Each as wide as its words, at most half the strip; one that
            // doesn't fit the row goes to the next.
            let chip = |ui: &mut egui::Ui, job: egui::text::LayoutJob, on: bool| {
                let pad = 2.0 * (look.space_m + look.line_thin);
                let wide = ui.fonts_mut(|f| f.layout_job(job.clone())).size().x + pad;
                let width = wide.min((ui.max_rect().width() / 2.0).max(pad * 4.0));
                if ui.available_width() < width {
                    ui.end_row();
                }
                let stroke = egui::Stroke::new(look.line_thin, if on { colors().focus } else { colors().border });
                ui.allocate_ui(egui::vec2(width, tall), |ui| {
                    egui::Frame::NONE.fill(colors().overlay_raised).stroke(stroke).corner_radius(look.radius_m).inner_margin(crate::theme::margin2(look.space_m, look.space_s)).show(ui, |ui| {
                        ui.set_min_height(tall - 2.0 * look.space_s);
                        ui.set_max_width(width - pad);
                        ui.add(egui::Label::new(job).truncate());
                    })
                    .response
                    .rect
                })
                .inner
            };
            if shown.len() > CHIPS_MOST && !self.ui.subagents_unfolded {
                let working = shown.iter().filter(|(s, _)| s.subagent.status.working()).count();
                let job = egui::text::LayoutJob::simple_singleline(format!("{} {working} sub-agents working", icon::ROBOT), font.clone(), colors().text_strong);
                chips.push((EACH.to_owned(), chip(ui, job, false)));
                return;
            }
            for (seen, at) in &shown {
                let s = &seen.subagent;
                let (state, colour) = state_words(s.status);
                let mut words = vec![s.title.clone()];
                words.push(s.doing.clone().filter(|_| s.status.working()).unwrap_or_else(|| state.to_owned()));
                words.push(elapsed(seen, now));
                let mut job = egui::text::LayoutJob::default();
                job.append(&format!("{} ", icon::CIRCLE), 0.0, egui::TextFormat::simple(font.clone(), colour));
                job.append(&words.join(" · "), 0.0, egui::TextFormat::simple(font.clone(), colors().text_strong));
                let rect = chip(ui, job, at.is_some() && self.ui.cursor == *at);
                if let Some(at) = at {
                    chips.push((at.to_string(), rect));
                }
            }
        });
        self.ui.spots.extend(chips.into_iter().map(|(key, rect)| Spot { key, rect }));
    }

    /// The strip's count, as a place: moved to, it shows each.
    pub(super) fn subagents_each(&mut self, key: &str) -> bool {
        if key != EACH {
            return false;
        }
        self.ui.subagents_unfolded = true;
        true
    }

    /// The next sub-agent's item in the stream (`by` 1), or the one before
    /// (-1), from the cursor: the cursor goes to it.
    pub(super) fn step_to_subagent(&mut self, by: i32) -> Step {
        let forward = by > 0;
        let Some(view) = self.thread() else { return Step::Nothing("No sub-agents here.".into()) };
        let starts: Vec<usize> = view.subagents().iter().filter_map(|s| view.shown_at(&s.subagent.item_id)).collect();
        drop(view);
        let from = self.ui.cursor;
        let next = match forward {
            true => starts.iter().copied().filter(|&at| from.is_none_or(|c| at > c)).min(),
            false => starts.iter().copied().filter(|&at| from.is_none_or(|c| at < c)).max(),
        };
        match next {
            Some(at) => {
                self.put_cursor(at, crate::viewport::Show::Middle, render::Reveal::Row);
                Step::Done
            }
            None if starts.is_empty() => Step::Nothing("No sub-agents here.".into()),
            None => Step::Nothing(if forward { "No sub-agent after this." } else { "No sub-agent before this." }.into()),
        }
    }

    /// The cursor is on where a sub-agent started.
    pub(super) fn subagent_here(&self) -> bool {
        self.thread().is_some_and(|v| matches!(self.ui.cursor.and_then(|at| v.in_stream(at)).map(|i| &i.body), Some(ItemBody::Subagent { .. })))
    }

    /// The sub-agent whose item the cursor is on, opened over the panels.
    pub(super) fn open_subagent_here(&mut self) -> bool {
        let Some(view) = self.thread() else { return false };
        let Some(ItemBody::Subagent { subagent_id }) = self.ui.cursor.and_then(|at| view.in_stream(at)).map(|i| i.body.clone()) else { return false };
        drop(view);
        self.asks.push(Ask::Subagent { slot: self.ui.slot.clone(), id: subagent_id });
        true
    }
}
