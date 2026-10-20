//! Tagging: one action of the shell for every buffer. A buffer only says
//! what the item under its cursor is (`Navigable::subject`); the shell tags
//! it (picking a tag, or naming a new one), takes a tag off it, goes to
//! what a tag is on (a go-to, as any other: the thing becomes a `Target`),
//! and steps through the rest of a tag's things.

use harness_contracts::{Subject, TagId, on_things};

use super::*;
use crate::actions::{Choice, Group, When};
use crate::nav::{Item, Target};
use crate::tags::{TagAct, first_line, thing_name};

impl CockpitApp {
    /// What the cursor's on where you are, as something a tag can be on.
    fn subject_here(&self, here: Here) -> Option<Subject> {
        let view = self.desk.view(here.kind);
        view.subject(&view.cursor_key()?)
    }

    /// The tag actions, where you are. The tag picker's rows are made only
    /// while the palette is open (it's what shows them, afresh each frame).
    pub(super) fn tag_actions(&self, here: Here) -> Vec<Action> {
        use ShellCommand as S;
        let subject = self.subject_here(here);
        let tags = self.desk.shared.tags.get();
        let when = |ok: bool| if ok { When::Always } else { When::Unavailable };
        let picking = self.palette.is_open();
        let on_it = subject.as_ref().map(|s| tags.on(s)).unwrap_or_default();
        let tagged = !on_it.is_empty();
        let mut out = vec![
            Action::new("tag.add", "Tag what you're on", "Pick a tag, or type a new one's name", S::Tag).param(Param::Search { hint: "a tag's name (a new one is made)".into(), found: if picking { self.tag_found() } else { Vec::new() }, waiting: false }).group(Group::Act).importance(MINOR).when(when(subject.is_some())).icon(egui_phosphor::regular::TAG),
            Action::new("tag.remove", "Take a tag off what you're on", "Pick which of its tags", S::Untag).param(Param::Choice(on_it.iter().map(|(t, _)| Choice::new(t.id.as_str(), t.name.clone())).collect())).group(Group::Act).importance(MINOR).when(when(tagged)),
            Action::new("tag.goto", "Go to something tagged", "Pick the tag, then which thing it's on", S::TagGoTo).param(Param::Choice(tags.list().iter().map(|t| Choice::new(t.id.as_str(), t.name.clone()).detail(Some(described(t)))).collect())).group(Group::GoTo).importance(MINOR),
            Action::new("tag.next", "Next with its tag", "The next thing the tag on this is on (the one stepped through last)", S::TagStep { forward: true }).group(Group::Navigate).importance(MINOR).when(when(tagged)),
            Action::new("tag.prev", "Previous with its tag", "The thing before, of those the tag on this is on", S::TagStep { forward: false }).group(Group::Navigate).importance(MINOR).when(when(tagged)),
        ];
        // `g T` picked a tag on several things: which.
        if let Some(tag) = self.desk.tag_picking.as_ref().and_then(|id| tags.get(id)) {
            let choices = tag.on.iter().map(|on| Choice::new(serde_json::to_string(&on.subject).unwrap_or_default(), thing_name(&on.what)).detail(Some(first_line(&on.said)))).collect();
            out.push(Action::new("tag.goto.pick", format!("Go to which thing {} is on?", tag.name), "It's on several: pick one", S::TagGoToPick).param(Param::Choice(choices)).group(Group::GoTo));
        }
        out
    }

    /// The tag picker's rows, for what's typed: the tags it finds (as
    /// search finds anywhere: by name, then by note), and first a new tag
    /// by that name (when it can be one, and none has it).
    fn tag_found(&self) -> Vec<Choice> {
        let tags = self.desk.shared.tags.get();
        let list = tags.list();
        let query = self.desk.shell_searches.get("tag.add").map_or("", |q| q.trim());
        let choice = |t: &harness_contracts::Tag| Choice::new(t.id.as_str(), t.name.clone()).detail(Some(described(t)));
        if query.is_empty() {
            return list.into_iter().map(choice).collect();
        }
        let items: Vec<Item> = list.iter().map(|t| Item::new(t.id.as_str(), t.name.clone(), t.note.clone())).collect();
        // The tag of that name (in any capitals) first; else a new one.
        let named = tags.named(query);
        let mut found: Vec<Choice> = named.into_iter().map(choice).collect();
        if named.is_none() && harness_contracts::tag_name_ok(query) {
            found.push(Choice::new(query, format!("New tag \u{201c}{query}\u{201d}")));
        }
        found.extend(crate::nav::rank_matches(&items, query).into_iter().filter(|i| Some(list[*i].id.clone()) != named.map(|t| t.id.clone())).map(|i| choice(list[i])));
        found
    }

    /// Runs a tag action.
    pub(super) fn run_tag(&mut self, command: &ShellCommand, value: Option<String>, ctx: &egui::Context) {
        let here = self.here();
        let subject = self.subject_here(here);
        match (command, subject, value) {
            (ShellCommand::Tag, Some(subject), Some(tag)) => {
                self.desk.shell_searches.remove("tag.add");
                self.ask_tags(TagAct::Tag { tag, subject }, ctx);
            }
            (ShellCommand::Untag, Some(subject), Some(tag)) => {
                let tag = TagId::new(tag);
                // What the tag is on: the thing, or lines it's among.
                let on = self.desk.shared.tags.get().on(&subject).into_iter().find(|(t, _)| t.id == tag).map(|(_, on)| on.clone());
                if let Some(subject) = on {
                    self.ask_tags(TagAct::Untag { tag, subject }, ctx);
                }
            }
            (ShellCommand::Tag | ShellCommand::Untag, None, _) => self.note = Some("Nothing here to tag: the cursor isn't on something that can be.".into()),
            (ShellCommand::TagGoTo, _, Some(tag)) => {
                let tag = TagId::new(tag);
                let on: Vec<Subject> = self.desk.shared.tags.get().get(&tag).map(|t| t.on.iter().map(|on| on.subject.clone()).collect()).unwrap_or_default();
                match on.as_slice() {
                    [] => self.note = Some("It's on nothing.".into()),
                    [one] => {
                        self.desk.tag_stepping = Some(tag);
                        self.go_to_subject(here.kind, one);
                    }
                    _ => {
                        self.desk.tag_picking = Some(tag);
                        self.run_by_id("tag.goto.pick", None, ctx);
                    }
                }
            }
            (ShellCommand::TagGoToPick, _, Some(which)) => {
                let Some(tag) = self.desk.tag_picking.take() else { return };
                let still = serde_json::from_str::<Subject>(&which).ok().filter(|s| self.desk.shared.tags.get().get(&tag).is_some_and(|t| t.on.iter().any(|on| on.subject == *s)));
                match still {
                    Some(subject) => {
                        self.desk.tag_stepping = Some(tag);
                        self.go_to_subject(here.kind, &subject);
                    }
                    None => self.note = Some("The tag isn't on that any more.".into()),
                }
            }
            (ShellCommand::TagStep { forward }, Some(subject), _) => {
                let next = {
                    let tags = self.desk.shared.tags.get();
                    let on = tags.on(&subject);
                    // The tag stepped through last, when it's one of them.
                    let stepping = self.desk.tag_stepping.clone();
                    on.iter().find(|(t, _)| Some(&t.id) == stepping.as_ref()).or(on.first()).and_then(|(tag, here)| {
                        let at = tag.on.iter().position(|on| on.subject == **here)?;
                        let n = tag.on.len();
                        let next = if *forward { (at + 1) % n } else { (at + n - 1) % n };
                        (next != at).then(|| (tag.id.clone(), tag.on[next].subject.clone()))
                    })
                };
                match next {
                    Some((tag, subject)) => {
                        self.desk.tag_stepping = Some(tag);
                        self.go_to_subject(here.kind, &subject);
                    }
                    None => self.note = Some("Its tag is on nothing else.".into()),
                }
            }
            _ => {}
        }
    }

    fn ask_tags(&mut self, act: TagAct, ctx: &egui::Context) {
        self.desk.shared.tags.get_mut().act(act, &self.desk.net, ctx, &self.notes_later);
    }

    /// Goes to `subject` where it's shown (the go-to path).
    fn go_to_subject(&mut self, from: BufferKind, subject: &Subject) {
        let jump = self.jump_of(from);
        match Target::of_subject(subject) {
            Some(target) => self.show_where_shown(&target, jump),
            None => self.note = Some("Nothing in this cockpit shows that kind of thing.".into()),
        }
    }
}

/// A tag in a picker's detail: how many things it's on, and its note.
fn described(tag: &harness_contracts::Tag) -> String {
    if tag.note.is_empty() { on_things(tag.on.len()) } else { format!("{} · {}", on_things(tag.on.len()), first_line(&tag.note)) }
}
