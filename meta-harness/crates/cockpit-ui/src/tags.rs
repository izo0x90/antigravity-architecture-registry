//! The desk's tags: which the server has (from the event stream) and what
//! each is on, and changing them (what's asked, `Ask::Tags`): tagging
//! something, taking a tag off, renaming, recolouring, noting, deleting;
//! what a tag's things say now, read when asked. Kept on the desk
//! (`Shared::tags`); buffers read it through a reader, and know nothing of
//! it: the shell tags what a buffer says it shows (`Navigable::subject`),
//! and draws tags on what's drawn (`Navigable::drawn_at`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use harness_contracts::{ClientEvent, Subject, Tag, TagId, TagRead};

use crate::kind::BufferKind;
use crate::net::{NetClient, NetResult};

/// A subject in a few words (as a go-to says where it goes).
pub fn in_words(subject: &Subject) -> String {
    match subject {
        Subject::TerminalLines { terminal, first, last, .. } if first == last => format!("terminal {terminal}, line {first}"),
        Subject::TerminalLines { terminal, first, last, .. } => format!("terminal {terminal}, lines {first} to {last}"),
        Subject::TerminalCommand { terminal, command, .. } => format!("terminal {terminal}, command {command}"),
        _ => "something this cockpit doesn't know".into(),
    }
}

/// The kind of buffer that shows a subject (none: none here does).
pub fn shown_in(subject: &Subject) -> Option<BufferKind> {
    match subject {
        Subject::TerminalLines { .. } | Subject::TerminalCommand { .. } => Some(BufferKind::Terminal),
        _ => None,
    }
}

/// What's asked of the tags.
#[derive(Clone, Debug, PartialEq)]
pub enum TagAct {
    /// Tag `subject` with `tag` (a tag's id, or a new tag's name).
    Tag { tag: String, subject: Subject },
    Untag { tag: TagId, subject: Subject },
    Rename { tag: TagId, name: String },
    Recolour { tag: TagId, colour: u8 },
    Note { tag: TagId, note: String },
    Delete { tag: TagId },
    /// What its things say now (read again).
    Read { tag: TagId },
}

enum Reply {
    Changed(String, NetResult<Tag>),
    Deleted(NetResult<()>),
    Read(NetResult<TagRead>),
}

type Mailbox = Arc<Mutex<Vec<Reply>>>;

/// A reply's answer, posted to `mailbox` as `reply` makes it.
fn posting<T>(mailbox: &Mailbox, reply: impl FnOnce(NetResult<T>) -> Reply + Send + 'static) -> impl FnOnce(NetResult<T>) + Send + 'static {
    let mailbox = mailbox.clone();
    move |r| {
        if let Ok(mut m) = mailbox.lock() {
            m.push(reply(r));
        }
    }
}

#[derive(Default)]
pub struct Tags {
    tags: BTreeMap<TagId, Tag>,
    /// What each tag's things say now, as last read.
    read: BTreeMap<TagId, TagRead>,
    /// What went wrong last, and what went right (said to the user once).
    said: Option<Result<String, String>>,
    mailbox: Mailbox,
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
                self.read.remove(tag_id);
            }
            ClientEvent::Resync => {
                self.tags.clear();
                self.read.clear();
            }
            _ => {}
        }
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

    /// The tags on `subject`.
    pub fn on(&self, subject: &Subject) -> Vec<&Tag> {
        self.list().into_iter().filter(|t| t.on.iter().any(|on| on.subject == *subject)).collect()
    }

    /// What tag `id`'s things say now, as last read.
    pub fn read(&self, id: &TagId) -> Option<&TagRead> {
        self.read.get(id)
    }

    /// What came of the last change (once).
    pub fn take_said(&mut self) -> Option<Result<String, String>> {
        self.said.take()
    }

    /// Does what was asked.
    pub fn act(&mut self, act: TagAct, net: &NetClient, ctx: &egui::Context) {
        let ctx = ctx.clone();
        let changed = |done: &'static str| posting(&self.mailbox, move |r| Reply::Changed(done.to_owned(), r));
        match act {
            TagAct::Tag { tag, subject } => net.tag(&tag, &subject, ctx, changed("Tagged")),
            TagAct::Untag { tag, subject } => net.untag(&tag, &subject, ctx, changed("Taken off")),
            TagAct::Rename { tag, name } => net.edit_tag(&tag, serde_json::json!({ "name": name }), ctx, changed("Renamed")),
            TagAct::Recolour { tag, colour } => net.edit_tag(&tag, serde_json::json!({ "colour": colour }), ctx, changed("Recoloured")),
            TagAct::Note { tag, note } => net.edit_tag(&tag, serde_json::json!({ "note": note }), ctx, changed("Noted")),
            TagAct::Delete { tag } => net.delete_tag(&tag, ctx, posting(&self.mailbox, Reply::Deleted)),
            TagAct::Read { tag } => net.read_tag(&tag, ctx, posting(&self.mailbox, Reply::Read)),
        }
    }

    /// Takes the replies to what was asked.
    pub fn tick(&mut self) {
        let replies = self.mailbox.lock().map(|mut m| std::mem::take(&mut *m)).unwrap_or_default();
        for reply in replies {
            match reply {
                Reply::Changed(done, Ok(tag)) => {
                    self.said = Some(Ok(format!("{done}: {}.", tag.name)));
                    self.tags.insert(tag.id.clone(), tag);
                }
                Reply::Deleted(Ok(())) => self.said = Some(Ok("Deleted.".into())),
                Reply::Read(Ok(read)) => {
                    self.read.insert(read.tag.id.clone(), read);
                }
                Reply::Changed(_, Err(e)) | Reply::Deleted(Err(e)) | Reply::Read(Err(e)) => self.said = Some(Err(e.to_string())),
            }
        }
    }
}
