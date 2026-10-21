//! Tagging: one action of the shell for every buffer. A buffer only says
//! what things are at the item under its cursor (`Navigable::things`: a
//! line, the command it's in); the shell tags one of them (the unit, or
//! the one `Tab` picks in the tag picker, picking a tag or naming a new
//! one), takes a tag off any of them, goes to what a tag is on (a go-to,
//! as any other: the thing becomes a `Target`), steps through the rest
//! of a tag's things, and edits the tags there (rename, colour, note,
//! delete: of the one tag there, or of the one picked first). Where a
//! view's items are tags' own rows (`Navigable::tag_at`), these are their
//! buttons too.

use harness_contracts::{Subject, TagId, on_things};

use super::*;
use crate::app::TagEditing;
use crate::app::notes::{NotesHere, TagHere};
use crate::actions::{Choice, Group, When};
use crate::nav::{Item, Target, Things};
use crate::tags::{TagAct, first_line, thing_name};

impl CockpitApp {
    /// The things at the cursor where you are that a tag can be on.
    fn things_here(&self, here: Here) -> Things {
        let view = self.desk.view(here.kind);
        view.cursor_key().map(|key| view.things(&key)).unwrap_or_default()
    }

    /// Which of the things at the cursor tagging is on: the one picked in
    /// the tag picker open now, else the unit.
    fn level(&self, things: &Things) -> usize {
        let picked = self.desk.tag_level.as_ref().filter(|(opening, _)| *opening == self.palette.opening());
        picked.and_then(|(_, subject)| things.at.iter().position(|t| t.subject.same_thing(subject))).unwrap_or(things.unit)
    }

    /// The thing tagging is on.
    fn subject_here(&self, here: Here) -> Option<Subject> {
        let things = self.things_here(here);
        things.picked(Some(self.level(&things))).map(|t| t.subject.clone())
    }

    /// An edit of `tag`: its value asked for (a field starts with what's
    /// there), the tag it's of carried with it.
    fn edit_action(edit: TagEditing, tag: &TagHere) -> Action {
        use ShellCommand as S;
        use TagEditing as E;
        let theme = crate::theme::colors();
        let (label, param) = match edit {
            E::Rename => (format!("Rename the tag {}", tag.name), Param::Text { hint: "a new name (letters, digits, - and _)".into(), start: tag.name.clone() }),
            E::Recolour => (format!("Give {} another colour", tag.name), Param::Choice((0..harness_contracts::TAG_COLOURS).map(|i| Choice::new(i.to_string(), theme.tag_name(i)).swatch(theme.tag(i)).current(tag.colour == i)).collect())),
            E::Note => (format!("Write {}'s note", tag.name), Param::Writing { hint: "its note (as many lines as it takes)".into(), start: tag.note.clone() }),
            E::ClearNote => (format!("Clear {}'s note", tag.name), Param::None),
            E::Delete => (format!("Delete the tag {}", tag.name), Param::Choice(vec![Choice::new("yes", format!("Delete {} (from {})", tag.name, on_things(tag.count))), Choice::new("no", "Keep it")])),
        };
        Action::new(edit.id(), label, edit.detail(), S::TagEdit(edit, tag.id.clone())).param(param).group(Group::Act).importance(MINOR)
    }

    /// The edits of the tags at the cursor: of the one there (that an edit
    /// fits: a note to clear), its value asked for; of several, which first.
    /// Deleting a tag is the Tags view's (on its row).
    fn tag_edits(at: &NotesHere) -> Vec<Action> {
        use ShellCommand as S;
        use TagEditing as E;
        let theme = crate::theme::colors();
        let mut out = Vec::new();
        for edit in [E::Rename, E::Recolour, E::Note, E::ClearNote, E::Delete] {
            if edit == E::Delete && at.row.is_none() {
                continue;
            }
            let fitting: Vec<&TagHere> = at.tags.iter().filter(|t| edit != E::ClearNote || !t.note.is_empty()).collect();
            out.push(match fitting.as_slice() {
                [one] => Self::edit_action(edit, one),
                some => {
                    let choices = some.iter().map(|t| Choice::new(t.id.as_str(), t.name.clone()).colour(theme.tag(t.colour)).detail(Some(described(t.count, &t.note)))).collect();
                    let available = if some.is_empty() { When::Unavailable } else { When::Always };
                    Action::new(edit.id(), edit.which(), edit.detail(), S::TagEditWhich(edit)).param(Param::Choice(choices)).group(Group::Act).importance(MINOR).when(available)
                }
            });
        }
        out
    }

    /// Taking a tag off what you're on: the one there (in a view of tags,
    /// the row's), else which.
    fn untag_action(at: &NotesHere) -> Action {
        use ShellCommand as S;
        let theme = crate::theme::colors();
        let on: Vec<(&TagHere, &Subject)> = at.tags.iter().filter_map(|t| Some((t, t.on.as_ref()?))).collect();
        let one = match (&at.row, on.as_slice()) {
            (Some(_), [first, ..]) | (None, [first]) => Some(first),
            _ => None,
        };
        let action = match one {
            Some((tag, subject)) => Action::new("tag.remove", format!("Take {} off this", tag.name), "The tag stays on everything else", S::Untag(Some((tag.id.clone(), (*subject).clone())))),
            None => Action::new("tag.remove", "Take a tag off what you're on", "Pick which of its tags (on any of the things at the cursor)", S::Untag(None)).param(Param::Choice(on.iter().map(|(t, _)| Choice::new(t.id.as_str(), t.name.clone()).colour(theme.tag(t.colour))).collect())),
        };
        action.group(Group::Act).importance(MINOR).when(if on.is_empty() { When::Unavailable } else { When::Always })
    }

    /// The tag actions, where you are. The tag pickers' rows are made only
    /// while theirs is open (it's what shows them, afresh each frame).
    pub(super) fn tag_actions(&self, here: Here) -> Vec<Action> {
        use ShellCommand as S;
        let at = self.notes_here(here);
        let things = &at.things;
        let tags = self.desk.shared.tags.get();
        let when = |ok: bool| if ok { When::Always } else { When::Unavailable };
        let picking = self.palette.picking();
        let tagged = at.tags.iter().any(|t| t.on.is_some());
        // What's tagged (the picker's top line), and the next of the things
        // there (its cycle key).
        let level = self.level(things);
        let chosen = things.picked(Some(level));
        let next = (things.at.len() > 1).then(|| &things.at[(level + 1) % things.at.len()]);
        let mut out = vec![
            Action::new("tag.add", "Tag what you're on", "Pick a tag, or type a new one's name (what's tagged is said at the top)", S::Tag)
                .param(Param::Search { hint: "a tag's name (a new one is made)".into(), found: if picking == Some("tag.add") { self.tag_found("tag.add") } else { Vec::new() }, waiting: false })
                .about(chosen.map(|t| format!("Tag {}", t.words)))
                .cycle(next.map(|_| "tag.level"))
                .group(Group::Act)
                .importance(MINOR)
                .when(when(chosen.is_some()))
                .icon(egui_phosphor::regular::TAG),
            Action::new("tag.level", next.map_or_else(|| "Tag something around it".to_owned(), |t| format!("Tag {} instead", t.words)), "While tagging: what's tagged becomes the next of the things at the cursor (a line, the command it's in)", S::TagLevel)
                .group(Group::Act)
                .importance(MINOR)
                .when(when(picking == Some("tag.add") && next.is_some())),
            Self::untag_action(&at),
            Action::new("tag.goto", "Go to something tagged", "Pick the tag, then which thing it's on", S::TagGoTo).param(Param::Choice(tags.list().iter().map(|t| Choice::new(t.id.as_str(), t.name.clone()).colour(crate::theme::colors().tag(t.colour)).detail(Some(described(t.on.len(), &t.note)))).collect())).group(Group::GoTo).importance(MINOR),
            Action::new("tag.next", "Next with its tag", "The next thing the tag on this is on (the one stepped through last)", S::TagStep { forward: true }).group(Group::Navigate).importance(MINOR).when(when(tagged)),
            Action::new("tag.prev", "Previous with its tag", "The thing before, of those the tag on this is on", S::TagStep { forward: false }).group(Group::Navigate).importance(MINOR).when(when(tagged)),
        ];
        out.extend(Self::tag_edits(&at));
        // Where text is typed (a prompt): a tag's name, picked.
        let named: Vec<Choice> = if picking == Some("tag.insert") { self.tag_found("tag.insert").into_iter().filter(|c| tags.get(&TagId::new(&c.value)).is_some()).collect() } else { Vec::new() };
        out.push(
            Action::new("tag.insert", "Name a tag", "Its name where you're typing (after a #, a prompt hands the agent what the tag is on)", S::TagInsert)
                .param(Param::Search { hint: "a tag's name".into(), found: named, waiting: false })
                .group(Group::Act)
                .importance(MINOR)
                .when(when(self.desk.view(here.kind).takes_typed() && !tags.list().is_empty())),
        );
        // `g T` picked a tag on several things: which.
        if let Some(tag) = self.desk.tag_picking.as_ref().and_then(|id| tags.get(id)) {
            let choices = tag.on.iter().map(|on| Choice::new(serde_json::to_string(&on.subject).unwrap_or_default(), thing_name(&on.what)).detail(Some(crate::tags::tagged_by(on)))).collect();
            out.push(Action::new("tag.goto.pick", format!("Go to which thing {} is on?", tag.name), "It's on several: pick one", S::TagGoToPick).param(Param::Choice(choices)).group(Group::GoTo));
        }
        out
    }

    /// The tag picker's rows, for what's typed: the tags it finds (as
    /// search finds anywhere: by name, then by note), and first a new tag
    /// by that name (when it can be one, and none has it).
    fn tag_found(&self, search: &str) -> Vec<Choice> {
        let tags = self.desk.shared.tags.get();
        let list = tags.list();
        let query = self.desk.shell_searches.get(search).map_or("", |q| q.trim());
        let choice = |t: &harness_contracts::Tag| Choice::new(t.id.as_str(), t.name.clone()).colour(crate::theme::colors().tag(t.colour)).detail(Some(described(t.on.len(), &t.note)));
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
            (ShellCommand::TagLevel, _, _) => {
                let things = self.things_here(here);
                if !things.at.is_empty() {
                    let next = (self.level(&things) + 1) % things.at.len();
                    self.desk.tag_level = Some((self.palette.opening(), things.at[next].subject.clone()));
                }
            }
            (ShellCommand::Tag, Some(subject), Some(tag)) => {
                self.desk.shell_searches.remove("tag.add");
                self.ask_tags(TagAct::Tag { tag, subject }, ctx);
            }
            (ShellCommand::Untag(one), _, value) => {
                // The one it named, or the one picked of those there.
                let which = one.clone().or_else(|| value.and_then(|id| self.notes_here(here).tags.into_iter().find(|t| t.id.as_str() == id).and_then(|t| Some((t.id, t.on?)))));
                match which {
                    Some((tag, subject)) => self.ask_tags(TagAct::Untag { tag, subject }, ctx),
                    None => self.note = Some("That tag isn't on what you're on any more.".into()),
                }
            }
            (ShellCommand::TagInsert, _, Some(tag)) => {
                let Some(name) = self.desk.shared.tags.get().get(&TagId::new(&tag)).map(|t| t.name.clone()) else {
                    self.note = Some("That tag is gone.".into());
                    return;
                };
                self.desk.shell_searches.remove("tag.insert");
                self.desk.view_mut(here.kind).typed_in(&format!("#{name} "));
                // Back to typing where it was.
                self.input_mode = InputMode::Insert { at: here.at };
            }
            (ShellCommand::TagEditWhich(edit), _, Some(tag)) => {
                // The tag picked: its value asked for next (a note to clear: done).
                let Some(tag) = self.notes_here(here).tags.into_iter().find(|t| t.id.as_str() == tag) else {
                    self.note = Some("That tag is gone, or isn't here any more.".into());
                    return;
                };
                let action = Self::edit_action(*edit, &tag);
                if action.param == Param::None {
                    self.run_tag(&ShellCommand::TagEdit(*edit, tag.id), None, ctx);
                } else {
                    self.open_palette(Some(&crate::actions::Bound { action, origin: crate::actions::Origin::Shell, leader: None, keys: Vec::new(), gestures: Vec::new(), layer: crate::actions::Layer::On }));
                }
            }
            (ShellCommand::TagEdit(edit, tag), _, value) => {
                use TagEditing as E;
                if self.desk.shared.tags.get().get(tag).is_none() {
                    self.note = Some("That tag is gone (deleted since).".into());
                    return;
                }
                let tag = tag.clone();
                let act = match (edit, value) {
                    (E::Rename, Some(name)) => TagAct::Rename { tag, name: name.trim().to_owned() },
                    (E::Recolour, Some(colour)) => match colour.parse() {
                        Ok(colour) => TagAct::Recolour { tag, colour },
                        Err(_) => return,
                    },
                    (E::Note, Some(note)) => TagAct::Note { tag, note: note.trim().to_owned() },
                    (E::ClearNote, _) => TagAct::Note { tag, note: String::new() },
                    (E::Delete, Some(yes)) if yes == "yes" => TagAct::Delete { tag },
                    _ => return,
                };
                self.ask_tags(act, ctx);
            }
            (ShellCommand::Tag, None, _) => self.note = Some("Nothing here to tag: the cursor isn't on something that can be.".into()),
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
                let still = serde_json::from_str::<Subject>(&which).ok().filter(|s| self.desk.shared.tags.get().get(&tag).is_some_and(|t| t.on.iter().any(|on| on.subject.same_thing(s))));
                match still {
                    Some(subject) => {
                        self.desk.tag_stepping = Some(tag);
                        self.go_to_subject(here.kind, &subject);
                    }
                    None => self.note = Some("The tag isn't on that any more.".into()),
                }
            }
            (ShellCommand::TagStep { forward }, Some(_), _) => {
                let next = {
                    let here_tags = self.notes_here(here).tags;
                    let tags = self.desk.shared.tags.get();
                    // The tag stepped through last, when it's one of them.
                    let stepping = self.desk.tag_stepping.clone();
                    let on: Vec<(&TagId, &Subject)> = here_tags.iter().filter_map(|t| Some((&t.id, t.on.as_ref()?))).collect();
                    on.iter().find(|(id, _)| Some(*id) == stepping.as_ref()).or(on.first()).and_then(|(id, here)| {
                        let tag = tags.get(id)?;
                        let at = tag.on.iter().position(|on| on.subject.same_thing(here))?;
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
fn described(on: usize, note: &str) -> String {
    if note.is_empty() { on_things(on) } else { format!("{} · {}", on_things(on), first_line(note)) }
}

impl TagEditing {
    fn id(self) -> &'static str {
        match self {
            Self::Rename => "tag.rename",
            Self::Recolour => "tag.recolour",
            Self::Note => "tag.note",
            Self::ClearNote => "tag.note_clear",
            Self::Delete => "tag.delete",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Rename => "A new name for it (what it's on stays)",
            Self::Recolour => "One of the theme's tag colours",
            Self::Note => "What it's for, said wherever the tag is (agents read it too)",
            Self::ClearNote => "It has no note from now on",
            Self::Delete => "Off everything it's on, for good",
        }
    }

    /// Its label while which tag is still to be picked.
    fn which(self) -> &'static str {
        match self {
            Self::Rename => "Rename a tag here",
            Self::Recolour => "Give a tag here another colour",
            Self::Note => "Write a tag's note here",
            Self::ClearNote => "Clear a tag's note here",
            Self::Delete => "Delete a tag here",
        }
    }
}
