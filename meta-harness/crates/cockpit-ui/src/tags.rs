//! The desk's tags: which the server has and what each is on (from the
//! event stream, its one source), changing them (what's asked,
//! `Ask::Tags`): tagging something, taking a tag off, renaming,
//! recolouring, noting, deleting; and what a tag's things say now, read
//! when asked. Kept on the desk (`Shared::tags`); buffers read it through a
//! reader, and know nothing of it: the shell tags what a buffer says it
//! shows (`Navigable::subject`), and draws tags on what's drawn
//! (`Navigable::drawn_at`). It knows no kind of subject: their names are
//! their owners' (`Tagged::what`).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use harness_contracts::{ClientEvent, Held, Subject, Tag, TagId, TagRead};

use crate::net::{NetClient, NetResult};

/// What's asked of the tags.
#[derive(Clone, Debug, PartialEq)]
pub enum TagAct {
    /// Tag `subject` with `tag` (a tag's id, or a new tag's name).
    Tag { tag: String, subject: Subject },
    Untag { tag: TagId, subject: Subject },
    Rename { tag: TagId, name: String },
    Recolour { tag: TagId, colour: u8 },
    /// Its note (empty: none).
    Note { tag: TagId, note: String },
    Delete { tag: TagId },
    /// What its things say now (read again).
    Read { tag: TagId },
}

/// A thing's name (its owner's: `Tagged::what`), or that nothing here
/// names it.
pub fn thing_name(what: &str) -> String {
    if what.is_empty() { "something this server can't show".into() } else { what.to_owned() }
}

/// Who tagged a thing, in words ("tagged by you"), as every place says who.
pub fn tagged_by(tagged: &harness_contracts::Tagged) -> String {
    if tagged.by.is_empty() { "tagged".into() } else { format!("tagged by {}", crate::conversation::who(&tagged.by)) }
}

/// A text's first line, cut short (what a list shows of what a thing said).
pub fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().chars().take(120).collect()
}

/// What each of a tag's things says (some of it) and where that came
/// from, as last read.
pub type Now = HashMap<Subject, (Held, Option<String>)>;

type Read = Arc<Mutex<Vec<(TagId, NetResult<TagRead>)>>>;

#[derive(Default)]
pub struct Tags {
    tags: BTreeMap<TagId, Tag>,
    /// Bumped whenever they change: what's made of them is made again
    /// only then.
    version: u64,
    /// Bumped when they're all told again (a resync): what was asked of
    /// them before is asked again.
    told_again: u64,
    now: BTreeMap<TagId, Now>,
    /// Why what a tag's things say now couldn't be read.
    unread: BTreeMap<TagId, String>,
    read: Read,
}

impl Tags {
    /// Takes what the server says about its tags.
    pub fn apply(&mut self, event: &ClientEvent) {
        match event {
            ClientEvent::TagUpdated { tag } => {
                self.tags.insert(tag.id.clone(), tag.clone());
            }
            ClientEvent::TagRemoved { tag_id } => {
                self.tags.remove(tag_id);
                self.now.remove(tag_id);
            }
            ClientEvent::Resync => {
                self.tags.clear();
                self.now.clear();
                self.unread.clear();
                self.told_again += 1;
            }
            _ => return,
        }
        self.version += 1;
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn told_again(&self) -> u64 {
        self.told_again
    }

    /// Every tag, by name.
    pub fn list(&self) -> Vec<&Tag> {
        let mut tags: Vec<&Tag> = self.tags.values().collect();
        tags.sort_by(|a, b| a.name.cmp(&b.name));
        tags
    }

    pub fn get(&self, id: &TagId) -> Option<&Tag> {
        self.tags.get(id)
    }

    /// A link's colour: its tag's, else the accent (a thing).
    pub fn link_colour(&self, to: &harness_contracts::Linked) -> egui::Color32 {
        match to {
            harness_contracts::Linked::Tag { tag } => self.get(tag).map_or(crate::theme::colors().accent, |t| crate::theme::colors().tag(t.colour)),
            harness_contracts::Linked::Thing { .. } => crate::theme::colors().accent,
        }
    }

    /// The tag named `name` (in capitals or not, as the server finds it).
    pub fn named(&self, name: &str) -> Option<&Tag> {
        self.tags.values().find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// The tags on `subject` (on it, or on something it's in: lines with
    /// it among them), by name, each with what it's on.
    pub fn on(&self, subject: &Subject) -> Vec<(&Tag, &Subject)> {
        self.list().into_iter().filter_map(|t| Some((t, &t.on.iter().find(|on| on.subject.contains(subject))?.subject))).collect()
    }

    /// What tag `id`'s things say now, as last read.
    pub fn now(&self, id: &TagId) -> Option<&Now> {
        self.now.get(id)
    }

    /// Why what tag `id`'s things say now couldn't be read.
    pub fn unread(&self, id: &TagId) -> Option<&str> {
        self.unread.get(id).map(String::as_str)
    }

    /// Does what was asked; what came of a change is said (`notes`): the
    /// change itself comes on the event stream.
    pub fn act(&mut self, act: TagAct, net: &NetClient, ctx: &egui::Context, notes: &Arc<Mutex<Vec<String>>>) {
        let ctx = ctx.clone();
        let say = |done: Box<dyn FnOnce(&Tag) -> String + Send>, failed: &'static str| {
            let notes = notes.clone();
            move |r: NetResult<Tag>| {
                if let Ok(mut notes) = notes.lock() {
                    notes.push(match r {
                        Ok(tag) => done(&tag),
                        Err(e) => format!("{failed}: {}", e.message),
                    });
                }
            }
        };
        match act {
            TagAct::Tag { tag, subject } => net.tag(&tag, &subject, ctx, say(Box::new(|t| format!("Tagged with {}.", t.name)), "Couldn't tag it")),
            TagAct::Untag { tag, subject } => net.untag(&tag, &subject, ctx, say(Box::new(|t| format!("Took {} off it.", t.name)), "Couldn't take the tag off")),
            TagAct::Rename { tag, name } => net.edit_tag(&tag, serde_json::json!({ "name": name }), ctx, say(Box::new(|t| format!("Renamed it {}.", t.name)), "Couldn't rename the tag")),
            TagAct::Recolour { tag, colour } => net.edit_tag(&tag, serde_json::json!({ "colour": colour }), ctx, say(Box::new(|t| format!("Gave {} another colour.", t.name)), "Couldn't recolour the tag")),
            TagAct::Note { tag, note } => {
                let cleared = note.is_empty();
                net.edit_tag(&tag, serde_json::json!({ "note": note }), ctx, say(Box::new(move |t| if cleared { format!("Cleared {}'s note.", t.name) } else { format!("Noted {}.", t.name) }), "Couldn't change the tag's note"));
            }
            TagAct::Delete { tag } => {
                let name = self.tags.get(&tag).map_or_else(|| "the tag".to_owned(), |t| t.name.clone());
                let notes = notes.clone();
                net.delete_tag(&tag, ctx, move |r| {
                    if let Ok(mut notes) = notes.lock() {
                        notes.push(match r {
                            Ok(()) => format!("Deleted {name}."),
                            Err(e) => format!("Couldn't delete {name}: {}", e.message),
                        });
                    }
                });
            }
            TagAct::Read { tag } => {
                let read = self.read.clone();
                net.read_tag(&tag.clone(), ctx, move |r| {
                    if let Ok(mut read) = read.lock() {
                        read.push((tag, r));
                    }
                });
            }
        }
    }

    /// Takes what was read.
    pub fn tick(&mut self) {
        let read = self.read.lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or_default();
        for (tag, r) in read {
            match r {
                Ok(read) => {
                    self.unread.remove(&tag);
                    self.now.insert(tag, read.on.into_iter().map(|t| (t.tagged.subject, (t.held, t.text))).collect());
                }
                Err(e) => {
                    self.unread.insert(tag, e.message);
                }
            }
            self.version += 1;
        }
    }
}
