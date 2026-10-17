//! Tags: names given to things in the project, to find them again and to
//! point agents at them. A tag (`flaky-build`) has a colour and a note and
//! is on subjects, each kept with what it said when tagged and who tagged
//! it. A subject is its owner's: what it says now, its name in words (to
//! tag it by), and where it is now when its owner moved it (lines laid out
//! anew). Owners (the terminals, ...) register a `Resolve` when they're
//! made, and this crate knows none of them.
//!
//! Every change is kept (the store: the state database) and told on the
//! event stream (`TagUpdated`, `TagRemoved`), which keeps them for clients
//! that come later.

#[cfg(feature = "testing")]
pub mod testing;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};

use harness_contracts::{ClientEvent, Subject, SubjectNow, TAG_COLOURS, Tag, TagId, TagRead, Tagged, TaggedNow};
use harness_core::EventHub;

/// How much of what a subject said (and of a note) is kept, and how many
/// things a tag is on at most.
const SAID_MOST: usize = 4000;
pub const MOST_ON: usize = 500;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TagError {
    #[error("there's no tag {0:?}")]
    NotFound(String),
    #[error("{0:?} can't be a tag's name: letters, digits, - and _ only (at most 40)")]
    BadName(String),
    #[error("there's a tag named {0:?} already")]
    Taken(String),
    #[error("a tag's colour is one of 0 to {}", TAG_COLOURS - 1)]
    BadColour,
    #[error("that can't be tagged here: {0}")]
    Subject(String),
    #[error("{0} isn't on that")]
    NotOn(String),
    #[error("a tag is on {MOST_ON} things at most")]
    TooMany,
    #[error("tags couldn't be kept: {0}")]
    Store(String),
}

/// Where tags are kept (the project's state database).
pub trait TagStore: Send + Sync {
    /// The tags kept (one this build can't read, a newer kind of subject,
    /// is left as it is and not given).
    fn tags(&self) -> Result<Vec<Tag>, String>;
    /// Keeps `tag` as it is now.
    fn put(&self, tag: &Tag) -> Result<(), String>;
    fn remove(&self, id: &TagId) -> Result<(), String>;
}

/// An owner of subjects: what one says now, and its name in words. Each
/// answers none for what isn't its own.
pub trait Resolve: Send + Sync {
    /// `subject` as it is now (its name, and what it says).
    fn now(&self, subject: &Subject) -> Option<SubjectNow>;
    /// The subject `words` name (as `now` names them): an error says why
    /// they don't rightly name one of its own.
    fn named(&self, words: &str) -> Option<Result<Subject, String>>;
    /// Where `subject` is now, when its owner moved it since (lines laid
    /// out anew): none when it's where it was.
    fn moved(&self, _subject: &Subject) -> Option<Subject> {
        None
    }
}

/// A change to a tag.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edit {
    pub name: Option<String>,
    pub colour: Option<u8>,
    pub note: Option<String>,
}

pub struct Tags {
    store: Arc<dyn TagStore>,
    hub: Arc<EventHub>,
    tags: Mutex<BTreeMap<TagId, Tag>>,
    owners: RwLock<Vec<Arc<dyn Resolve>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A name a tag can have: `#name` finds it in a prompt (punctuation after
/// it isn't part of it), it goes in a link as it is, and it's never
/// mistaken for a tag's id.
fn check_name(name: &str) -> Result<String, TagError> {
    let ok = !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && uuid::Uuid::parse_str(name).is_err();
    if ok { Ok(name.to_owned()) } else { Err(TagError::BadName(name.to_owned())) }
}

/// What's kept of a text: some of it, when it's long.
fn kept_text(text: String) -> String {
    match text.char_indices().nth(SAID_MOST) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

/// The tag with id `name`, or named `name`, in `tags`.
fn found(tags: &BTreeMap<TagId, Tag>, name: &str) -> Result<Tag, TagError> {
    tags.get(&TagId::new(name)).or_else(|| tags.values().find(|t| t.name == name)).cloned().ok_or_else(|| TagError::NotFound(name.to_owned()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Tags {
    /// The tags kept, told on `hub` (for clients that come later).
    pub fn open(store: Arc<dyn TagStore>, hub: Arc<EventHub>) -> Result<Self, TagError> {
        let kept = store.tags().map_err(TagError::Store)?;
        for tag in &kept {
            hub.publish(ClientEvent::TagUpdated { tag: tag.clone() });
        }
        let tags = Mutex::new(kept.into_iter().map(|t| (t.id.clone(), t)).collect());
        Ok(Self { store, hub, tags, owners: RwLock::default() })
    }

    /// An owner of subjects, from now on.
    pub fn register(&self, owner: Arc<dyn Resolve>) {
        self.owners.write().unwrap_or_else(PoisonError::into_inner).push(owner);
    }

    fn owners(&self) -> RwLockReadGuard<'_, Vec<Arc<dyn Resolve>>> {
        self.owners.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// What `subject` says now (none: no owner here can say).
    pub fn now(&self, subject: &Subject) -> Option<SubjectNow> {
        self.owners().iter().find_map(|o| o.now(subject))
    }

    /// The subject `words` name, as its owner names it.
    pub fn named(&self, words: &str) -> Result<Subject, TagError> {
        self.owners().iter().find_map(|o| o.named(words)).unwrap_or_else(|| Err(format!("nothing here names {words:?}"))).map_err(TagError::Subject)
    }

    /// Where `subject` is now (where it was, when its owner didn't move it).
    fn current(&self, subject: &Subject) -> Subject {
        self.owners().iter().find_map(|o| o.moved(subject)).unwrap_or_else(|| subject.clone())
    }

    /// Every tag, by name.
    pub fn list(&self) -> Vec<Tag> {
        let mut tags: Vec<Tag> = lock(&self.tags).values().cloned().collect();
        tags.sort_by(|a, b| a.name.cmp(&b.name));
        tags
    }

    /// The tag with id `name`, or named `name`.
    pub fn find(&self, name: &str) -> Result<Tag, TagError> {
        found(&lock(&self.tags), name)
    }

    /// A tag (by its name or id) and what each of its subjects says now;
    /// subjects their owners moved are kept where they are now.
    pub fn read(&self, name: &str) -> Result<TagRead, TagError> {
        let mut tag = self.find(name)?;
        if tag.on.iter().any(|t| self.current(&t.subject) != t.subject) {
            let mut tags = lock(&self.tags);
            let mut kept = found(&tags, tag.id.as_str())?;
            for t in &mut kept.on {
                t.subject = self.current(&t.subject);
            }
            tag = self.keep(&mut tags, kept)?;
        }
        let on = tag.on.iter().map(|t| TaggedNow { tagged: t.clone(), now: self.now(&t.subject) }).collect();
        Ok(TagRead { tag, on })
    }

    /// Tags `subject` with `tag` (by its name or id; a new tag, in the
    /// colour least used, when there's none of that name), kept with what
    /// it says now and who tagged it (`by`). Already on it: as it was.
    pub fn tag(&self, tag: &str, subject: Subject, by: &str) -> Result<Tag, TagError> {
        let subject = self.current(&subject);
        let said = self.now(&subject).ok_or_else(|| TagError::Subject("nothing here knows what it is".into()))?.now.ok_or_else(|| TagError::Subject("it's gone".into()))?;
        let mut tags = lock(&self.tags);
        let mut kept = match found(&tags, tag) {
            Ok(existing) => existing,
            Err(_) => {
                let name = check_name(tag)?;
                let used: Vec<u8> = tags.values().map(|t| t.colour).collect();
                let colour = (0..TAG_COLOURS).min_by_key(|c| used.iter().filter(|u| *u == c).count()).unwrap_or(0);
                Tag { id: TagId::new(uuid::Uuid::new_v4().to_string()), name, colour, note: String::new(), noted_by: String::new(), on: Vec::new() }
            }
        };
        if kept.on.iter().any(|t| self.current(&t.subject) == subject) {
            return Ok(kept);
        }
        if kept.on.len() >= MOST_ON {
            return Err(TagError::TooMany);
        }
        kept.on.push(Tagged { subject, said: kept_text(said), at_ms: now_ms(), by: by.to_owned() });
        self.keep(&mut tags, kept)
    }

    /// Takes `tag` off `subject`.
    pub fn untag(&self, tag: &str, subject: &Subject) -> Result<Tag, TagError> {
        let subject = self.current(subject);
        let mut tags = lock(&self.tags);
        let mut kept = found(&tags, tag)?;
        let before = kept.on.len();
        kept.on.retain(|t| t.subject != subject && self.current(&t.subject) != subject);
        if kept.on.len() == before {
            return Err(TagError::NotOn(kept.name));
        }
        self.keep(&mut tags, kept)
    }

    /// Renames, recolours or notes `tag` (by its name or id); `by` wrote the note.
    pub fn edit(&self, tag: &str, edit: Edit, by: &str) -> Result<Tag, TagError> {
        let mut tags = lock(&self.tags);
        let mut kept = found(&tags, tag)?;
        if let Some(new) = edit.name {
            let new = check_name(&new)?;
            if tags.values().any(|t| t.name == new && t.id != kept.id) {
                return Err(TagError::Taken(new));
            }
            kept.name = new;
        }
        if let Some(colour) = edit.colour {
            if colour >= TAG_COLOURS {
                return Err(TagError::BadColour);
            }
            kept.colour = colour;
        }
        if let Some(note) = edit.note {
            kept.noted_by = if note.is_empty() { String::new() } else { by.to_owned() };
            kept.note = kept_text(note);
        }
        self.keep(&mut tags, kept)
    }

    pub fn delete(&self, tag: &str) -> Result<(), TagError> {
        let mut tags = lock(&self.tags);
        let kept = found(&tags, tag)?;
        self.store.remove(&kept.id).map_err(TagError::Store)?;
        tags.remove(&kept.id);
        self.hub.publish(ClientEvent::TagRemoved { tag_id: kept.id });
        Ok(())
    }

    /// Keeps `tag` (stored, then held, then told).
    fn keep(&self, tags: &mut BTreeMap<TagId, Tag>, tag: Tag) -> Result<Tag, TagError> {
        self.store.put(&tag).map_err(TagError::Store)?;
        tags.insert(tag.id.clone(), tag.clone());
        self.hub.publish(ClientEvent::TagUpdated { tag: tag.clone() });
        Ok(tag)
    }
}

#[cfg(test)]
mod tests {
    use harness_contracts::TerminalId;

    use super::*;
    use crate::testing::{Lines, MemoryStore};

    fn lines(first: u64, last: u64) -> Subject {
        Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first, last }
    }

    fn tags(store: Arc<MemoryStore>) -> (Tags, Arc<EventHub>) {
        let hub = Arc::new(EventHub::with_capacity(64));
        let tags = Tags::open(store, hub.clone()).unwrap();
        tags.register(Arc::new(Lines));
        (tags, hub)
    }

    #[test]
    fn a_tag_is_made_by_naming_it_and_keeps_what_its_subjects_said_and_who_tagged_them() {
        let store = Arc::new(MemoryStore::default());
        let (tags, hub) = tags(store.clone());
        let tag = tags.tag("flaky-build", lines(10, 12), "you").unwrap();
        assert_eq!((tag.name.as_str(), tag.note.as_str(), tag.on.len()), ("flaky-build", "", 1));
        assert_eq!((tag.on[0].said.as_str(), tag.on[0].by.as_str()), ("lines 10 to 12", "you"));
        let (_, mut events) = hub.subscribe();
        let again = tags.tag(tag.id.as_str(), lines(10, 12), "lead").unwrap();
        assert_eq!(again.on.len(), 1, "by its id; already on it");
        assert!(events.try_recv().is_err(), "nothing changed: nothing told");
        let other = tags.tag("auth-bug", lines(1, 1), "lead").unwrap();
        assert_ne!(other.colour, tag.colour, "the colour least used");
        for bad in ["two words", "dots.too", "a/b", "café", &uuid::Uuid::new_v4().to_string()] {
            assert_eq!(tags.tag(bad, lines(1, 1), "you"), Err(TagError::BadName(bad.into())), "{bad}");
        }
        assert!(matches!(tags.tag("gone", lines(0, 0), "you"), Err(TagError::Subject(_))), "a subject that's gone can't be tagged");
        // Kept: a server started again has them.
        let (again, _) = self::tags(store);
        assert_eq!(again.list().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["auth-bug", "flaky-build"]);
    }

    #[test]
    fn a_tag_is_renamed_recoloured_noted_taken_off_and_deleted_and_each_change_is_told() {
        let (tags, hub) = tags(Arc::new(MemoryStore::default()));
        let (_, mut events) = hub.subscribe();
        let tag = tags.tag("x", lines(3, 4), "you").unwrap();
        tags.tag("y", lines(5, 5), "you").unwrap();
        assert_eq!(tags.edit("x", Edit { name: Some("y".into()), ..Edit::default() }, "you"), Err(TagError::Taken("y".into())));
        let renamed = tags.edit("x", Edit { name: Some("deploy".into()), colour: Some(5), note: Some("staging only".into()) }, "worker q1").unwrap();
        assert_eq!((renamed.id.clone(), renamed.name.as_str(), renamed.colour, renamed.note.as_str(), renamed.noted_by.as_str()), (tag.id.clone(), "deploy", 5, "staging only", "worker q1"));
        assert_eq!(tags.find(tag.id.as_str()).unwrap().name, "deploy", "found by its id too");
        assert_eq!(tags.edit("deploy", Edit { colour: Some(8), ..Edit::default() }, "you"), Err(TagError::BadColour));
        assert!(tags.untag("deploy", &lines(3, 4)).unwrap().on.is_empty());
        assert_eq!(tags.untag("deploy", &lines(3, 4)), Err(TagError::NotOn("deploy".into())), "not on it: said so");
        tags.delete("deploy").unwrap();
        assert_eq!(tags.find("deploy"), Err(TagError::NotFound("deploy".into())));
        let mut told = Vec::new();
        while let Ok(event) = events.try_recv() {
            told.push(event);
        }
        assert!(matches!(told.last(), Some(ClientEvent::TagRemoved { tag_id }) if *tag_id == tag.id), "{told:?}");
        assert_eq!(told.iter().filter(|e| matches!(e, ClientEvent::TagUpdated { .. })).count(), 4);
    }

    #[test]
    fn subjects_are_named_by_their_owners_and_moved_where_their_owners_moved_them() {
        let (tags, _) = tags(Arc::new(MemoryStore::default()));
        assert_eq!(tags.named("lines 7 to 9"), Ok(lines(7, 9)));
        assert!(matches!(tags.named("a chat message"), Err(TagError::Subject(_))), "no owner names it");
        // Lines from 100 on, the owner says, moved 1000 on (laid out anew).
        tags.tag("t", lines(1, 2), "you").unwrap();
        let mut kept = tags.find("t").unwrap();
        kept.on.push(Tagged { subject: lines(100, 101), said: "before".into(), at_ms: 0, by: "you".into() });
        tags.keep(&mut lock(&tags.tags), kept).unwrap();
        let read = tags.read("t").unwrap();
        assert_eq!(read.on.iter().map(|t| t.tagged.subject.clone()).collect::<Vec<_>>(), [lines(1, 2), lines(1100, 1101)], "kept where they are now");
        assert_eq!(read.on[0].now, Some(SubjectNow { what: "lines 1 to 2".into(), now: Some("lines 1 to 2".into()) }));
        tags.untag("t", &lines(100, 101)).unwrap();
        assert_eq!(tags.find("t").unwrap().on.len(), 1, "taken off by where it was, too");
    }

    #[test]
    fn what_a_subject_said_is_kept_up_to_a_limit() {
        assert_eq!(kept_text("é".repeat(SAID_MOST + 10)).chars().count(), SAID_MOST + 1, "cut, with …");
        assert_eq!(kept_text("short".into()), "short");
    }
}
