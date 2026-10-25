//! Starting a discussion: one action of the shell for every buffer, on
//! the thing at the cursor, with the tag picker's way of picking it (its
//! top line says what; `Tab` picks the next of the things there: the line,
//! the command it's in). What it is (a question, a change request) is
//! picked first, then who's asked (with other agent slots than the lead),
//! then what it says is written in the typing box every conversation has.
//! Once the server has it, it opens.

use harness_contracts::DiscussionKind;

use super::*;
use crate::actions::{Choice, Group, When};
use crate::nav::Target;

impl CockpitApp {
    /// Starting a discussion where you are, and the steps after.
    pub(super) fn discuss_actions(&self, here: Here) -> Vec<Action> {
        use ShellCommand as S;
        let things = self.things_here(here);
        let level = self.level(&things);
        let chosen = things.picked(Some(level));
        let next = (things.at.len() > 1).then(|| &things.at[(level + 1) % things.at.len()]);
        let picking = self.palette.picking();
        let when = |ok: bool| if ok { When::Always } else { When::Unavailable };
        let kinds = vec![Choice::new("question", "A question").detail(Some("Answered without anything changing".into())), Choice::new("change_request", "A change request").detail(Some("Its answer names what changed".into()))];
        let mut out = vec![
            Action::new("discuss.start", "Ask about what you're on", "A question, or a change request, on the thing at the cursor (what it's on is said at the top)", S::Discuss)
                .param(Param::Choice(kinds))
                .about(chosen.map(|t| format!("Discuss {}", t.words)))
                .cycle(next.map(|_| "discuss.level"))
                .group(Group::Act)
                .importance(SECONDARY)
                .when(when(chosen.is_some())),
            Action::new("discuss.level", next.map_or_else(|| "Discuss something around it".to_owned(), |t| format!("Discuss {} instead", t.words)), "While starting a discussion: what it's on becomes the next of the things at the cursor (a line, the command it's in)", S::DiscussLevel)
                .group(Group::Act)
                .importance(MINOR)
                .when(when(picking == Some("discuss.start") && next.is_some())),
        ];
        // The discussions on what you're on (or on the mark picked): one opened.
        let here_ids: Vec<harness_contracts::DiscussionId> = if self.desk.discussions_here.is_empty() { self.notes_here(here).discussions.into_iter().map(|d| d.id).collect() } else { self.desk.discussions_here.clone() };
        let all = self.desk.shared.discussions.get();
        let choices: Vec<Choice> = here_ids.iter().filter_map(|id| all.get(id)).map(|d| Choice::new(d.id.as_str(), format!("{} {}: {}", d.id, d.kind.words(), crate::tags::first_line(d.asking()))).detail(Some(crate::conversation::whose(d).0))).collect();
        let available = !choices.is_empty();
        out.push(Action::new("discuss.open", "Open a discussion here", "One of those on what you're on, waiting on you first", S::DiscussOpen).param(Param::Choice(choices)).group(Group::GoTo).importance(MINOR).when(when(available)));
        if let Some(d) = &self.desk.discussing {
            let lead = harness_contracts::LEAD;
            let choices = self.slots_to_ask().into_iter().map(|(name, role)| Choice::new(name.clone(), name.clone()).detail(Some(if name == lead { "the lead (asked unless another is picked)".to_owned() } else { role }))).collect();
            out.push(Action::new("discuss.ask", format!("Who answers {} on {}?", d.kind.words(), d.words), "An agent slot: the lead, unless another is picked", S::DiscussAsk).param(Param::Choice(choices)).group(Group::Act).importance(MINOR));
            let asked = d.asked.as_ref().map(|a| format!(", asking {a}")).unwrap_or_default();
            out.push(Action::new("discuss.write", format!("{} on {}{asked}", crate::nav::capitalized(d.kind.words()), d.words), "What it asks (# links a tag, @ a file or a symbol)", S::DiscussWrite).param(Param::Writing { hint: "what you ask".into(), start: d.text.clone(), links: true }).group(Group::Act).importance(MINOR));
        }
        out
    }

    /// The agent slots a discussion can ask, by name: the lead first.
    fn slots_to_ask(&self) -> Vec<(String, String)> {
        let agents = self.desk.shared.agents.get();
        let mut slots: Vec<(String, String)> = agents.store().slots.values().map(|s| (s.name.clone(), s.role.clone())).collect();
        slots.sort_by_key(|(name, _)| (name != harness_contracts::LEAD, name.clone()));
        slots
    }

    /// Runs a step of starting a discussion.
    pub(super) fn run_discuss(&mut self, command: &ShellCommand, value: Option<String>, ctx: &egui::Context) {
        let here = self.here();
        match (command, value) {
            (ShellCommand::DiscussLevel, _) => {
                let things = self.things_here(here);
                if !things.at.is_empty() {
                    let next = (self.level(&things) + 1) % things.at.len();
                    self.desk.tag_level = Some((self.palette.opening(), things.at[next].subject.clone()));
                }
            }
            (ShellCommand::Discuss, Some(kind)) => {
                let things = self.things_here(here);
                let Some(thing) = things.picked(Some(self.level(&things))).cloned() else {
                    self.note = Some("Nothing here to discuss: the cursor isn't on something that can be.".into());
                    return;
                };
                let Ok(kind) = serde_json::from_value::<DiscussionKind>(serde_json::Value::String(kind.clone())) else {
                    self.note = Some(format!("{kind:?} isn't a kind of discussion: a question or a change request."));
                    return;
                };
                self.desk.discussing = Some(crate::app::desk::Discussing { on: thing.subject, words: thing.words, kind, asked: None, text: Default::default() });
                // Who's asked, when there's another slot than the lead to ask.
                let next = if self.slots_to_ask().len() > 1 { "discuss.ask" } else { "discuss.write" };
                self.run_by_id(next, None, ctx);
            }
            (ShellCommand::DiscussOpen, Some(id)) => {
                self.desk.discussions_here.clear();
                let jump = self.jump_here();
                self.land(None, &Target::Discussion(harness_contracts::DiscussionId::new(id)), jump, ctx);
            }
            (ShellCommand::DiscussAsk, Some(slot)) => {
                let Some(d) = self.desk.discussing.as_mut() else { return };
                d.asked = (slot != harness_contracts::LEAD).then_some(slot);
                self.run_by_id("discuss.write", None, ctx);
            }
            (ShellCommand::DiscussWrite, Some(text)) => {
                let Some(mut sending) = self.desk.discussing.take() else { return };
                if text.trim().is_empty() {
                    self.note = Some("A discussion asks something: nothing was started.".into());
                    return;
                }
                // The writing's message (its text and links); from outside, its text.
                sending.text = serde_json::from_str::<harness_contracts::Message>(&text).unwrap_or_else(|_| harness_contracts::Message::text(text.trim())).trimmed();
                let (subject, kind, asked) = (sending.on.clone(), sending.kind, sending.asked.clone());
                let message = sending.text.clone();
                self.desk.sending = Some(sending);
                let started = self.desk.started.clone();
                self.desk.net.discussion_start(&subject, kind, &message, asked.as_deref(), ctx.clone(), move |r| {
                    if let Ok(mut s) = started.lock() {
                        s.push(r);
                    }
                });
            }
            _ => {}
        }
    }

    /// Discussions started from here: each opens (over what you're on), or
    /// why not is said.
    pub(super) fn open_started(&mut self, ctx: &egui::Context) {
        let started = std::mem::take(&mut *self.desk.started.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for answer in started {
            match answer {
                Ok(d) => {
                    self.desk.sending = None;
                    let jump = self.jump_here();
                    self.land(None, &Target::Discussion(d.id), jump, ctx);
                }
                // Not started: what was written comes back, to send again.
                Err(e) => {
                    self.note = Some(format!("The discussion wasn't started: {}", e.message));
                    if let Some(sending) = self.desk.sending.take() {
                        self.desk.discussing = Some(sending);
                        self.run_by_id("discuss.write", None, ctx);
                    }
                }
            }
        }
    }
}
