//! A chat's sub-agents: a strip above the typing box while any is at work
//! (or was just seen to end), a chip each that opens it in the quick look;
//! how many are at work, for the live line and the panel's top line;
//! moving from one to the next. Shown alone (`ChatView::alone`), the chat
//! is one sub-agent's: who it is and how it's doing above its own work.
//! What each is comes from the agents (`Seen`); its states are the status
//! vocabulary's.

use harness_contracts::ItemBody;

use super::*;
use super::live::{span_changes, span_words};
use crate::agents::working_words;
use crate::store::Seen;

/// Chips shown before they fold into one ("4 sub-agents working").
const CHIPS_MOST: usize = 3;
/// The folded strip's place (moved to, it shows each).
const EACH: &str = "subagents";

/// How long it ran, or has (seconds), as the server stamped it.
fn ran_s(seen: &Seen, now_ms: u64) -> f64 {
    let s = &seen.subagent;
    let end = s.ended_ms.unwrap_or(now_ms);
    end.saturating_sub(s.started_ms) as f64 / 1000.0
}

impl ChatStreamState {
    /// How many sub-agents of the thread it shows are at work.
    pub(crate) fn subagents_working(&self) -> usize {
        self.thread().map_or(0, |v| v.subagents_working())
    }

    /// The strip: each at work (or seen to end a moment ago) a chip, its
    /// state, name, what it's doing, how long; past a few, one chip with
    /// how many. Each chip is a place: a tap puts the cursor on where it
    /// started (its controls under it), a double tap (or Enter) opens it;
    /// the count's shows each.
    pub(super) fn render_subagents(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        let stays = self.settings.subagent_ended_s;
        let now_ms = self.agents().server_now_ms();
        let shown = |s: &&Seen| s.subagent.status.working() || s.ended_seen.is_some_and(|e| now - e < stays);
        // (None at work, none seen to end a moment ago: nothing to go
        // through.)
        let Some(count) = self.thread().map(|v| if v.subagents_working() == 0 && v.last_seen_ended().is_none_or(|e| now - e >= stays) { 0 } else { v.subagents().iter().filter(shown).count() }) else { return };
        // (Few enough again: folded again once there are more.)
        if count <= CHIPS_MOST {
            self.ui.subagents_unfolded = false;
        }
        if count == 0 {
            return;
        }
        let (unfolded, cursor) = (self.ui.subagents_unfolded, self.ui.cursor);
        let Some(view) = self.thread() else { return };
        // (When its words next change: a time's second, one seen to end
        // going.)
        let mut next = f64::INFINITY;
        let width = (ui.available_width() / 2.0).max(crate::theme::look().space_huge);
        let mut chips: Vec<(String, Rect)> = Vec::new();
        ui.horizontal_wrapped(|ui| {
            if count > CHIPS_MOST && !unfolded {
                let working = view.subagents_working();
                let words = if working > 0 { working_words(working) } else { format!("{count} sub-agents ended") };
                let rect = crate::status::pill(ui, &format!("{} {words}", icon::ROBOT), colors().text_strong, None, width).rect;
                chips.push((EACH.to_owned(), rect));
                return;
            }
            for seen in view.subagents().iter().filter(shown) {
                let s = &seen.subagent;
                let ran = ran_s(seen, now_ms);
                next = next.min(match seen.ended_seen {
                    Some(e) => stays - (now - e),
                    None => span_changes(ran),
                });
                let (glyph, word, colour) = crate::status::look(crate::status::subagent(s.status));
                let doing = seen.doing.as_deref().filter(|_| s.status == harness_contracts::SubagentStatus::Running).unwrap_or(&word);
                let at = view.shown_at(&s.item_id);
                let on = (at.is_some() && cursor == at).then(|| colors().selection);
                let rect = crate::status::pill(ui, &format!("{glyph} {} · {doing} · {}", s.title, span_words(ran)), colour, on, width).rect;
                if let Some(at) = at {
                    chips.push((at.to_string(), rect));
                }
            }
        });
        drop(view);
        if next.is_finite() {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(next.max(0.05)));
        }
        self.ui.spots.extend(chips.into_iter().map(|(key, rect)| Spot { key, rect }));
    }

    /// Shown alone: who the sub-agent is (its name, kind, how it's doing,
    /// how long), its task, what it's doing; what it was told and what it
    /// answered, folded; and, when its harness doesn't say which steps are
    /// its, that they're in the agent's chat.
    pub(super) fn render_subagent_head(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.ui.alone.clone() else { return };
        let look = crate::theme::look();
        let told_steps = self.instance().is_some_and(|i| i.capabilities.subagents.lifecycle_events);
        let now_ms = self.agents().server_now_ms();
        let Some(view) = self.thread() else { return };
        let Some(seen) = view.subagent(&id) else {
            ui.label(RichText::new("That sub-agent isn't in its chat now.").size(look.text_body).color(colors().muted));
            return;
        };
        let s = &seen.subagent;
        let ran = ran_s(seen, now_ms);
        if s.status.working() {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(span_changes(ran)));
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} {}", icon::ROBOT, s.title)).strong().size(look.text_title).color(colors().text_strong));
            if let Some(kind) = &s.kind {
                ui.label(RichText::new(kind).size(look.text_small).color(colors().muted));
            }
            crate::status::chip(ui, crate::status::subagent(s.status), None);
            // (Not under a second, ended: its times are a replay's, not
            // how long it ran.)
            if s.status.working() || ran >= 1.0 {
                ui.label(RichText::new(span_words(ran)).size(look.text_small).color(colors().muted));
            }
        });
        // Its task: as its harness says it, else its prompt's first line.
        if let Some(task) = s.description.clone().or_else(|| s.prompt.as_deref().map(crate::conversation::first_line)) {
            ui.label(RichText::new(task).size(look.text_body));
        }
        if let Some(doing) = seen.doing.as_ref().filter(|_| s.status == harness_contracts::SubagentStatus::Running) {
            ui.label(RichText::new(format!("Doing: {doing}")).size(look.text_small).color(colors().info));
        }
        let most = ui.available_height() / 3.0;
        for (said, text) in [("What it was told", &s.prompt), ("What it answered", &s.answer)] {
            if let Some(text) = text {
                egui::CollapsingHeader::new(RichText::new(said).size(look.text_small)).id_salt((said, &s.id)).show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt((said, &s.id, "text")).max_height(most).show(ui, |ui| {
                        ui.label(RichText::new(text).size(look.text_small));
                    });
                });
            }
        }
        drop(view);
        if self.shape().rows.is_empty() {
            let words = if told_steps { "Its steps show here as they come." } else { "Its harness doesn't say which steps are its: they're in the agent's chat." };
            ui.label(RichText::new(words).size(look.text_small).color(colors().muted));
        }
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

    /// The sub-agent whose item the cursor is on.
    pub(super) fn subagent_here(&self) -> Option<harness_contracts::SubagentId> {
        let view = self.thread()?;
        match &view.in_stream(self.ui.cursor?)?.body {
            ItemBody::Subagent { subagent_id } => Some(subagent_id.clone()),
            _ => None,
        }
    }

    /// The sub-agent whose item the cursor is on, opened (in the quick
    /// look, over what you're on).
    pub(super) fn open_subagent_here(&mut self) -> bool {
        let Some(id) = self.subagent_here() else { return false };
        self.asks.push(Ask::Show { target: Target::Subagent { slot: self.ui.slot.clone(), id }, in_: None });
        true
    }
}
