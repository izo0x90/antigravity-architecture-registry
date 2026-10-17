//! Tags: names given to things in the project, to find them again and to
//! point agents at them. A tag (`flaky-build`) has a colour and a note and
//! is on subjects, each kept with what it said when tagged. What a subject
//! says now is its owner's to tell: owners (the terminals, ...) register a
//! `Resolve` at start-up and this crate knows none of them.
//!
//! Every change is kept (the store: the state database) and told on the
//! event stream (`TagUpdated`, `TagRemoved`), which keeps them for clients
//! that come later.

#[cfg(feature = "testing")]
pub mod testing;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use harness_contracts::{ClientEvent, Subject, SubjectNow, TAG_COLOURS, Tag, TagId, TagRead, Tagged};
use harness_core::EventHub;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TagError {
    #[error("there's no tag {0:?}")]
    NotFound(String),
    #[error("{0:?} can't be a tag's name: letters, digits and - _ . / only, no spaces, at most 40")]
    BadName(String),
    #[error("there's a tag named {0:?} already")]
    Taken(String),
    #[error("a tag's colour is one of 0 to {}", TAG_COLOURS - 1)]
    BadColour,
    #[error("that can't be tagged here: {0}")]
    Subject(String),
    #[error("tags couldn't be kept: {0}")]
    Store(String),
}

/// Where tags are kept (the project's state database).
pub trait TagStore: Send + Sync {
    fn tags(&self) -> Result<Vec<Tag>, String>;
    /// Keeps `tag` as it is now.
    fn put(&self, tag: &Tag) -> Result<(), String>;
    fn remove(&self, id: &TagId) -> Result<(), String>;
}

/// An owner of subjects: what one says now.
pub trait Resolve: Send + Sync {
    /// `subject` as it is now; none when it isn't this owner's kind.
    fn now(&self, subject: &Subject) -> Option<SubjectNow>;
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

/// A name a tag can have: `#name` finds it in a prompt.
fn check_name(name: &str) -> Result<String, TagError> {
    let ok = !name.is_empty() && name.chars().count() <= 40 && name.chars().all(|c| c.is_alphanumeric() || "-_./".contains(c));
    if ok { Ok(name.to_owned()) } else { Err(TagError::BadName(name.to_owned())) }
}

/// The tag named `name` (or with that id) in `tags`.
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

    /// What `subject` says now (none: no owner here can say).
    pub fn now(&self, subject: &Subject) -> Option<SubjectNow> {
        self.owners.read().unwrap_or_else(PoisonError::into_inner).iter().find_map(|o| o.now(subject))
    }

    /// Every tag, by name.
    pub fn list(&self) -> Vec<Tag> {
        let mut tags: Vec<Tag> = lock(&self.tags).values().cloned().collect();
        tags.sort_by(|a, b| a.name.cmp(&b.name));
        tags
    }

    /// The tag named `name` (or with that id).
    pub fn find(&self, name: &str) -> Result<Tag, TagError> {
        found(&lock(&self.tags), name)
    }

    /// A tag and what each of its subjects says now.
    pub fn read(&self, name: &str) -> Result<TagRead, TagError> {
        let tag = self.find(name)?;
        let now = tag.on.iter().map(|t| self.now(&t.subject)).collect();
        Ok(TagRead { tag, now })
    }

    /// Tags `subject` `name` (a new tag when there's none of that name,
    /// in the next colour): kept with what it says now. Already on it: as it was.
    pub fn tag(&self, name: &str, subject: Subject) -> Result<Tag, TagError> {
        let name = check_name(name)?;
        let said = self.now(&subject).ok_or_else(|| TagError::Subject("nothing here knows what it is".into()))?.now.ok_or_else(|| TagError::Subject("it's gone".into()))?;
        let mut tags = lock(&self.tags);
        let existing = tags.values().find(|t| t.name == name).cloned();
        let mut tag = existing.unwrap_or_else(|| {
            let used: Vec<u8> = tags.values().map(|t| t.colour).collect();
            let colour = (0..TAG_COLOURS).min_by_key(|c| used.iter().filter(|u| *u == c).count()).unwrap_or(0);
            Tag { id: TagId::new(uuid::Uuid::new_v4().to_string()), name, colour, note: String::new(), on: Vec::new() }
        });
        if !tag.on.iter().any(|t| t.subject == subject) {
            tag.on.push(Tagged { subject, said, at_ms: now_ms() });
        }
        self.keep(&mut tags, tag)
    }

    /// Takes `name` off `subject`.
    pub fn untag(&self, name: &str, subject: &Subject) -> Result<Tag, TagError> {
        let mut tags = lock(&self.tags);
        let mut tag = found(&tags, name)?;
        tag.on.retain(|t| t.subject != *subject);
        self.keep(&mut tags, tag)
    }

    pub fn edit(&self, name: &str, edit: Edit) -> Result<Tag, TagError> {
        let mut tags = lock(&self.tags);
        let mut tag = found(&tags, name)?;
        if let Some(new) = edit.name {
            let new = check_name(&new)?;
            if tags.values().any(|t| t.name == new && t.id != tag.id) {
                return Err(TagError::Taken(new));
            }
            tag.name = new;
        }
        if let Some(colour) = edit.colour {
            if colour >= TAG_COLOURS {
                return Err(TagError::BadColour);
            }
            tag.colour = colour;
        }
        if let Some(note) = edit.note {
            tag.note = note;
        }
        self.keep(&mut tags, tag)
    }

    pub fn delete(&self, name: &str) -> Result<(), TagError> {
        let mut tags = lock(&self.tags);
        let tag = found(&tags, name)?;
        self.store.remove(&tag.id).map_err(TagError::Store)?;
        tags.remove(&tag.id);
        self.hub.publish(ClientEvent::TagRemoved { tag_id: tag.id });
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
    fn a_tag_is_made_by_naming_it_and_keeps_what_its_subjects_said() {
        let store = Arc::new(MemoryStore::default());
        let (tags, _) = tags(store.clone());
        let tag = tags.tag("flaky-build", lines(10, 12)).unwrap();
        assert_eq!((tag.name.as_str(), tag.note.as_str(), tag.on.len()), ("flaky-build", "", 1));
        assert_eq!(tag.on[0].said, "lines 10 to 12");
        let again = tags.tag("flaky-build", lines(10, 12)).unwrap();
        assert_eq!(again.on.len(), 1, "already on it");
        let other = tags.tag("auth-bug", lines(1, 1)).unwrap();
        assert_ne!(other.colour, tag.colour, "the next colour");
        assert_eq!(tags.tag("two words", lines(1, 1)), Err(TagError::BadName("two words".into())));
        assert!(matches!(tags.tag("gone", lines(0, 0)), Err(TagError::Subject(_))), "a subject that's gone can't be tagged");
        // Kept: a server started again has them.
        let (again, _) = self::tags(store);
        assert_eq!(again.list().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["auth-bug", "flaky-build"]);
    }

    #[test]
    fn a_tag_is_renamed_recoloured_noted_taken_off_and_deleted_and_each_change_is_told() {
        let (tags, hub) = tags(Arc::new(MemoryStore::default()));
        let (_, mut events) = hub.subscribe();
        let tag = tags.tag("x", lines(3, 4)).unwrap();
        tags.tag("y", lines(5, 5)).unwrap();
        assert_eq!(tags.edit("x", Edit { name: Some("y".into()), ..Edit::default() }), Err(TagError::Taken("y".into())));
        let renamed = tags.edit("x", Edit { name: Some("deploy".into()), colour: Some(5), note: Some("staging only".into()) }).unwrap();
        assert_eq!((renamed.id.clone(), renamed.name.as_str(), renamed.colour, renamed.note.as_str()), (tag.id.clone(), "deploy", 5, "staging only"));
        assert_eq!(tags.find(tag.id.as_str()).unwrap().name, "deploy", "found by its id too");
        assert_eq!(tags.edit("deploy", Edit { colour: Some(8), ..Edit::default() }), Err(TagError::BadColour));
        assert!(tags.untag("deploy", &lines(3, 4)).unwrap().on.is_empty());
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
    fn reading_a_tag_says_what_each_subject_says_now() {
        let (tags, _) = tags(Arc::new(MemoryStore::default()));
        tags.tag("t", lines(1, 2)).unwrap();
        tags.tag("t", Subject::TerminalCommand { terminal: TerminalId::new("t1"), shell: 1, command: 3 }).unwrap_err();
        let read = tags.read("t").unwrap();
        assert_eq!(read.now, [Some(SubjectNow { what: "terminal t1".into(), now: Some("lines 1 to 2".into()) })]);
    }
}
