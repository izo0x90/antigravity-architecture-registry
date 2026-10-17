//! Tagging: one action of the shell for every buffer. A buffer only says
//! what the item under its cursor is (`Navigable::subject`); the shell tags
//! it (picking a tag, or naming a new one), takes a tag off it, goes to
//! what a tag is on, and steps through the rest of a tag's things.

use harness_contracts::{Subject, TagId};

use super::*;
use crate::actions::{Choice, Group, When};
use crate::tags::TagAct;

impl CockpitApp {
    /// What the cursor's on where you are, as something a tag can be on.
    fn subject_here(&self, here: Here) -> Option<Subject> {
        let view = self.desk.view(here.kind);
        view.subject(&view.cursor_key()?)
    }

    /// The tag actions, where you are.
    pub(super) fn tag_actions(&self, here: Here) -> Vec<Action> {
        use ShellCommand as S;
        let subject = self.subject_here(here);
        let tags = self.desk.shared.tags.get();
        let when = |ok: bool| if ok { When::Always } else { When::Unavailable };
        // Picking a tag for it: those whose names have what's typed in
        // them, and a new one by that name.
        let query = self.desk.tag_query.trim();
        let mut found: Vec<Choice> = tags.list().iter().filter(|t| query.is_empty() || t.name.contains(query)).map(|t| Choice::new(t.id.as_str(), t.name.clone()).detail(Some(things(t.on.len())))).collect();
        if !query.is_empty() && !tags.list().iter().any(|t| t.name == query) {
            found.push(Choice::new(query, format!("New tag \u{201c}{query}\u{201d}")));
        }
        let on_it: Vec<Choice> = subject.as_ref().map(|s| tags.on(s).iter().map(|t| Choice::new(t.id.as_str(), t.name.clone())).collect()).unwrap_or_default();
        let everything: Vec<Choice> = tags
            .list()
            .iter()
            .flat_map(|t| t.on.iter().enumerate().map(move |(n, on)| Choice::new(format!("{}#{n}", t.id), format!("{} · {}", t.name, crate::tags::in_words(&on.subject))).detail(Some(first_line(&on.said)))))
            .collect();
        let tagged = !on_it.is_empty();
        vec![
            Action::new("tag.add", "Tag what you're on", S::Tag).param(Param::Search { hint: "a tag's name (a new one is made)".into(), found, waiting: false }).group(Group::Act).importance(MINOR).when(when(subject.is_some())).icon(egui_phosphor::regular::TAG),
            Action::new("tag.remove", "Take a tag off what you're on", S::Untag).param(Param::Choice(on_it)).group(Group::Act).importance(MINOR).when(when(tagged)),
            Action::new("tag.goto", "Go to something tagged", S::TagGoTo).param(Param::Choice(everything)).group(Group::GoTo).importance(MINOR),
            Action::new("tag.next", "Next with its tag", S::TagStep { forward: true }).group(Group::Navigate).importance(MINOR).when(when(tagged)),
            Action::new("tag.prev", "Previous with its tag", S::TagStep { forward: false }).group(Group::Navigate).importance(MINOR).when(when(tagged)),
        ]
    }

    /// Runs a tag action.
    pub(super) fn run_tag(&mut self, command: &ShellCommand, value: Option<String>, ctx: &egui::Context) {
        let here = self.here();
        let subject = self.subject_here(here);
        match (command, subject, value) {
            (ShellCommand::Tag, Some(subject), Some(tag)) => {
                self.desk.tag_query.clear();
                self.ask_tags(TagAct::Tag { tag, subject }, ctx);
            }
            (ShellCommand::Untag, Some(subject), Some(tag)) => self.ask_tags(TagAct::Untag { tag: TagId::new(tag), subject }, ctx),
            (ShellCommand::Tag | ShellCommand::Untag, None, _) => self.note = Some("Nothing here to tag: the cursor isn't on something that can be (a terminal's line or command so far).".into()),
            (ShellCommand::TagGoTo, _, Some(which)) => {
                let found = which.rsplit_once('#').and_then(|(tag, n)| {
                    let tags = self.desk.shared.tags.get();
                    Some(tags.get(&TagId::new(tag))?.on.get(n.parse::<usize>().ok()?)?.subject.clone())
                });
                match found {
                    Some(subject) => self.go_to_subject(here.kind, subject),
                    None => self.note = Some("That tag changed: pick it again.".into()),
                }
            }
            (ShellCommand::TagStep { forward }, Some(subject), _) => {
                let next = {
                    let tags = self.desk.shared.tags.get();
                    tags.on(&subject).first().and_then(|tag| {
                        let at = tag.on.iter().position(|on| on.subject == subject)?;
                        let n = tag.on.len();
                        let next = if *forward { (at + 1) % n } else { (at + n - 1) % n };
                        (next != at).then(|| tag.on[next].subject.clone())
                    })
                };
                match next {
                    Some(subject) => self.go_to_subject(here.kind, subject),
                    None => self.note = Some("Its tag is on nothing else.".into()),
                }
            }
            _ => {}
        }
    }

    fn ask_tags(&mut self, act: TagAct, ctx: &egui::Context) {
        self.desk.shared.tags.get_mut().act(act, &self.desk.net, ctx);
    }

    /// Goes to `subject` in the buffer that shows its kind (the go-to path).
    fn go_to_subject(&mut self, from: BufferKind, subject: Subject) {
        match crate::tags::shown_in(&subject) {
            Some(kind) => {
                let jump = self.jump_of(from);
                self.show_target_in(kind, &crate::nav::Target::Subject(subject), jump);
            }
            None => self.note = Some("Nothing in this cockpit shows that kind of thing.".into()),
        }
    }
}

/// "on 3 things".
fn things(n: usize) -> String {
    match n {
        1 => "on 1 thing".into(),
        n => format!("on {n} things"),
    }
}

/// The first line of what something said (a picker's detail).
fn first_line(said: &str) -> String {
    said.lines().next().unwrap_or_default().chars().take(80).collect()
}
