//! Starting a discussion: one action of the shell for every buffer, on
//! the thing at the cursor, with the tag picker's way of picking it (its
//! top line says what; `Tab` picks the next of the things there: the line,
//! the command it's in). What it is (a question, a change asked for) is
//! picked first, then what it says is written in the typing box every
//! conversation has. Once the server has it, it opens.

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
        let kinds = vec![Choice::new("question", "A question").detail(Some("Answered without anything changing".into())), Choice::new("change_request", "A change asked for").detail(Some("Its answer names what changed".into()))];
        let mut out = vec![
            Action::new("discuss.start", "Ask about what you're on", "A question, or a change asked for, on the thing at the cursor (what it's on is said at the top)", S::Discuss)
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
        if let Some((_, what, kind)) = &self.desk.discussing {
            out.push(Action::new("discuss.write", format!("{} on {what}", capitalized(kind.words())), "What it asks (# names a tag)", S::DiscussWrite).param(Param::Writing { hint: "what you ask".into(), start: String::new() }).group(Group::Act).importance(MINOR));
        }
        out
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
                let kind = if kind == "change_request" { DiscussionKind::ChangeRequest } else { DiscussionKind::Question };
                self.desk.discussing = Some((thing.subject, thing.words, kind));
                self.run_by_id("discuss.write", None, ctx);
            }
            (ShellCommand::DiscussWrite, Some(text)) => {
                let Some((subject, _, kind)) = self.desk.discussing.take() else { return };
                if text.trim().is_empty() {
                    self.note = Some("A discussion asks something: nothing was started.".into());
                    return;
                }
                let started = self.desk.started.clone();
                self.desk.net.discussion_start(&subject, kind, text.trim(), ctx.clone(), move |r| {
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
                    let jump = self.jump_here();
                    self.land(None, &Target::Discussion(d.id), jump, ctx);
                }
                Err(e) => self.note = Some(format!("The discussion wasn't started: {}", e.message)),
            }
        }
    }
}

/// `text` with its first letter a capital.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}
