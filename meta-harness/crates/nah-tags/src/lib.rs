//! Tags: names given to things in the project, to find them again and to
//! point agents at them. A tag (`flaky-build`) has a colour and a note and
//! is on subjects, each kept with what it said when tagged and who tagged
//! it. A subject is its owner's: what it says now, its name in words (to
//! tag it by), and where it is now when its owner moved it (lines laid out
//! anew). Owners (the terminals, ...) register a `Resolve` when they're
//! made, and say when they moved their subjects (`refresh`); this crate
//! knows none of them.
//!
//! Every change is kept (the store: the state database, with who made it)
//! and told on the event stream (`TagUpdated`, `TagRemoved`), which keeps
//! them for clients that come later.

#[cfg(feature = "testing")]
pub mod testing;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};

use harness_contracts::{ClientEvent, Subject, TAG_COLOURS, Tag, TagId, TagRead, Tagged, TaggedNow};
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

/// Who changed a tag, and how (kept with the change).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// `user`, or an agent's slot.
    pub by: String,
    /// `tagged`, `untagged`, `renamed`, `recoloured`, `noted`, `deleted`.
    pub action: &'static str,
    /// What it was done to, or what changed (`terminal t1 lines 3-4`,
    /// `was flaky`).
    pub detail: String,
}

/// Where tags are kept (the project's state database).
pub trait TagStore: Send + Sync {
    /// The tags kept (one this build can't read, a newer kind of subject,
    /// is left as it is and not given).
    fn tags(&self) -> Result<Vec<Tag>, String>;
    /// Keeps `tag` as it is now, and the change that made it so (none:
    /// its owners moved its subjects).
    fn put(&self, tag: &Tag, change: Option<&Change>) -> Result<(), String>;
    fn remove(&self, tag: &Tag, change: &Change) -> Result<(), String>;
}

/// An owner of subjects: its name for one, what one says now. Each
/// answers none for what isn't its own.
pub trait Resolve: Send + Sync {
    /// `subject`'s name (`terminal t1 lines 120-124`), when it's its own.
    fn name(&self, subject: &Subject) -> Option<String>;
    /// What `subject` says now (none: it's gone, or it isn't its own).
    fn now(&self, subject: &Subject) -> Option<String>;
    /// The subject `words` name (as `name` names them): an error says why
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

/// A name a tag can have (`tag_name_ok`).
fn check_name(name: &str) -> Result<String, TagError> {
    if harness_contracts::tag_name_ok(name) { Ok(name.to_owned()) } else { Err(TagError::BadName(name.to_owned())) }
}

/// What's kept of a text: some of it, when it's long.
fn kept_text(text: String) -> String {
    match text.char_indices().nth(SAID_MOST) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

/// The tag with id `name`, or named `name` (in capitals or not: `Flaky`
/// is `flaky`), in `tags`.
fn found(tags: &BTreeMap<TagId, Tag>, name: &str) -> Result<Tag, TagError> {
    tags.get(&TagId::new(name)).or_else(|| tags.values().find(|t| t.name.eq_ignore_ascii_case(name))).cloned().ok_or_else(|| TagError::NotFound(name.to_owned()))
}

/// How much a prompt's tags add to it at most (roughly).
const EXPANDED_MOST: usize = 16_000;

/// The `#name`s in `text`: a `#` at a word's start, then a tag name's
/// letters (what follows them, a full stop say, isn't part of it).
fn hash_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut prev: Option<char> = None;
    for (at, c) in text.char_indices() {
        if c == '#' && prev.is_none_or(|p| p.is_whitespace() || "([{\"'".contains(p)) {
            let name: String = text[at + 1..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
            let name = name.trim_end_matches(['-', '_']).to_owned();
            if !name.is_empty() {
                out.push(name);
            }
        }
        prev = Some(c);
    }
    out
}

/// `text` indented under a list item.
fn indent(text: &str) -> String {
    text.lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n")
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

    /// An owner of subjects, from now on (its subjects named as it names
    /// them now).
    pub fn register(&self, owner: Arc<dyn Resolve>) {
        self.owners.write().unwrap_or_else(PoisonError::into_inner).push(owner);
        self.refresh();
    }

    fn owners(&self) -> RwLockReadGuard<'_, Vec<Arc<dyn Resolve>>> {
        self.owners.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// `subject`'s name, as its owner names it (none: no owner here).
    fn name(&self, subject: &Subject) -> Option<String> {
        self.owners().iter().find_map(|o| o.name(subject))
    }

    /// What `subject` says now (some of it, when it says a lot; none: it's
    /// gone, or no owner here can say).
    fn now(&self, subject: &Subject) -> Option<String> {
        self.owners().iter().find_map(|o| o.now(subject)).map(kept_text)
    }

    /// `tagged` where its owner says it is now, named as it's named now
    /// (none: as it was).
    fn moved(&self, tagged: &Tagged) -> Option<Tagged> {
        let subject = self.current(&tagged.subject);
        let what = self.name(&subject).unwrap_or_else(|| tagged.what.clone());
        (subject != tagged.subject || what != tagged.what).then(|| Tagged { subject, what, ..tagged.clone() })
    }

    /// Each tag's subjects kept where their owners say they are now, and
    /// named as they're named now: an owner says when it moved them
    /// (lines laid out anew, a shell started again).
    pub fn refresh(&self) {
        let mut tags = lock(&self.tags);
        let changed: Vec<Tag> = tags
            .values()
            .filter_map(|tag| {
                let moved: Vec<Option<Tagged>> = tag.on.iter().map(|t| self.moved(t)).collect();
                moved.iter().any(Option::is_some).then(|| Tag { on: tag.on.iter().zip(moved).map(|(t, m)| m.unwrap_or_else(|| t.clone())).collect(), ..tag.clone() })
            })
            .collect();
        for tag in changed {
            if let Err(e) = self.keep(&mut tags, tag, None) {
                tracing::warn!("A tag's things couldn't be kept where they are now: {e}");
            }
        }
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

    /// A tag (by its name or id) and what each of its subjects says now
    /// (where its owner says it is now).
    pub fn read(&self, name: &str) -> Result<TagRead, TagError> {
        self.refresh();
        let tag = self.find(name)?;
        let on = tag.on.iter().map(|t| TaggedNow { tagged: t.clone(), now: self.now(&t.subject) }).collect();
        Ok(TagRead { tag, on })
    }

    /// Tags `subject` with `tag` (by its name or id; a new tag, in the
    /// colour least used, when there's none of that name), kept with what
    /// it says now and who tagged it (`by`). Already on it: as it was.
    pub fn tag(&self, tag: &str, subject: Subject, by: &str) -> Result<Tag, TagError> {
        let subject = self.current(&subject);
        let what = self.name(&subject).ok_or_else(|| TagError::Subject("nothing here knows what it is".into()))?;
        let said = self.now(&subject).ok_or_else(|| TagError::Subject("it's gone".into()))?;
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
        let change = Change { by: by.to_owned(), action: "tagged", detail: what.clone() };
        kept.on.push(Tagged { subject, what, said: kept_text(said), at_ms: now_ms(), by: by.to_owned() });
        self.keep(&mut tags, kept, Some(&change))
    }

    /// Takes `tag` off `subject` (`by` took it off).
    pub fn untag(&self, tag: &str, subject: &Subject, by: &str) -> Result<Tag, TagError> {
        let subject = self.current(subject);
        let mut tags = lock(&self.tags);
        let mut kept = found(&tags, tag)?;
        let Some(at) = kept.on.iter().position(|t| t.subject == subject || self.current(&t.subject) == subject) else { return Err(TagError::NotOn(kept.name)) };
        let gone = kept.on.remove(at);
        self.keep(&mut tags, kept, Some(&Change { by: by.to_owned(), action: "untagged", detail: gone.what }))
    }

    /// Renames, recolours or notes `tag` (by its name or id): `by` changed it.
    pub fn edit(&self, tag: &str, edit: Edit, by: &str) -> Result<Tag, TagError> {
        let mut tags = lock(&self.tags);
        let mut kept = found(&tags, tag)?;
        let mut changes = Vec::new();
        if let Some(new) = edit.name {
            let new = check_name(&new)?;
            if tags.values().any(|t| t.name.eq_ignore_ascii_case(&new) && t.id != kept.id) {
                return Err(TagError::Taken(new));
            }
            changes.push(("renamed", format!("was {}", kept.name)));
            kept.name = new;
        }
        if let Some(colour) = edit.colour {
            if colour >= TAG_COLOURS {
                return Err(TagError::BadColour);
            }
            changes.push(("recoloured", format!("to {colour}")));
            kept.colour = colour;
        }
        if let Some(note) = edit.note {
            changes.push(("noted", if note.is_empty() { "cleared".into() } else { note.clone() }));
            kept.noted_by = if note.is_empty() { String::new() } else { by.to_owned() };
            kept.note = kept_text(note);
        }
        let change = (!changes.is_empty()).then(|| Change {
            by: by.to_owned(),
            action: changes[0].0,
            detail: changes.iter().map(|(action, detail)| if changes.len() == 1 { detail.clone() } else { format!("{action}: {detail}") }).collect::<Vec<_>>().join("; "),
        });
        self.keep(&mut tags, kept, change.as_ref())
    }

    /// Deletes `tag` (`by` deleted it).
    pub fn delete(&self, tag: &str, by: &str) -> Result<(), TagError> {
        let mut tags = lock(&self.tags);
        let kept = found(&tags, tag)?;
        self.store.remove(&kept, &Change { by: by.to_owned(), action: "deleted", detail: harness_contracts::on_things(kept.on.len()) }).map_err(TagError::Store)?;
        tags.remove(&kept.id);
        self.hub.publish(ClientEvent::TagRemoved { tag_id: kept.id });
        Ok(())
    }

    /// `text` with what the tags it names (`#name`) are on, as they are now,
    /// after it: what a prompt hands an agent. A `#word` no tag has is left
    /// as it is.
    pub fn expand(&self, text: &str) -> String {
        let mut named: Vec<String> = Vec::new();
        for name in hash_names(text) {
            if !named.contains(&name) && self.find(&name).is_ok_and(|t| t.name == name) {
                named.push(name);
            }
        }
        if named.is_empty() {
            return text.to_owned();
        }
        let mut out = format!("{text}\n\n---\nWhat the tags named above are on, as they are now:");
        for name in named {
            let Ok(read) = self.read(&name) else { continue };
            out.push_str(&format!("\n\n#{}", read.tag.name));
            if !read.tag.note.is_empty() {
                out.push_str(&format!(" ({})", read.tag.note));
            }
            if read.on.is_empty() {
                out.push_str("\n(on nothing now)");
            }
            for on in read.on {
                let (what, text) = match (on.tagged.what, on.now) {
                    (what, _) if what.is_empty() => ("something nothing here can show".to_owned(), String::new()),
                    (what, Some(now)) => (what, now),
                    (what, None) => (format!("{what} (gone; as it was when tagged)"), on.tagged.said),
                };
                out.push_str(&format!("\n- {what}:\n{}", indent(&text)));
                if out.len() > EXPANDED_MOST {
                    out.push_str("\n(the rest left out: tag_read gives all of it)");
                    return out;
                }
            }
        }
        out
    }

    /// Keeps `tag` (stored with the change that made it so, then held,
    /// then told).
    fn keep(&self, tags: &mut BTreeMap<TagId, Tag>, tag: Tag, change: Option<&Change>) -> Result<Tag, TagError> {
        self.store.put(&tag, change).map_err(TagError::Store)?;
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

    fn tags(store: Arc<MemoryStore>) -> (Tags, Arc<EventHub>, Arc<Lines>) {
        let hub = Arc::new(EventHub::with_capacity(64));
        let tags = Tags::open(store, hub.clone()).unwrap();
        let owner = Arc::new(Lines::default());
        tags.register(owner.clone());
        (tags, hub, owner)
    }

    fn changes(store: &MemoryStore) -> Vec<String> {
        store.changes.lock().unwrap().iter().map(|c| format!("{} {} {}", c.by, c.action, c.detail)).collect()
    }

    #[test]
    fn a_tag_is_made_by_naming_it_and_keeps_its_subjects_names_what_they_said_and_who_tagged_them() {
        let store = Arc::new(MemoryStore::default());
        let (tags, hub, _) = tags(store.clone());
        let tag = tags.tag("flaky-build", lines(10, 12), "you").unwrap();
        assert_eq!((tag.name.as_str(), tag.note.as_str(), tag.on.len()), ("flaky-build", "", 1));
        assert_eq!((tag.on[0].what.as_str(), tag.on[0].said.as_str(), tag.on[0].by.as_str()), ("lines 10 to 12", "lines 10 to 12", "you"));
        let (_, mut events) = hub.subscribe();
        let again = tags.tag(tag.id.as_str(), lines(10, 12), "lead").unwrap();
        assert_eq!(again.on.len(), 1, "by its id; already on it");
        assert!(events.try_recv().is_err(), "nothing changed: nothing told");
        assert_eq!(tags.tag("Flaky-Build", lines(1, 1), "lead").unwrap().id, tag.id, "a name in other capitals is the same tag's");
        let other = tags.tag("auth-bug", lines(1, 1), "lead").unwrap();
        assert_ne!(other.colour, tag.colour, "the colour least used");
        for bad in ["two words", "dots.too", "a/b", "café", &uuid::Uuid::new_v4().to_string()] {
            assert_eq!(tags.tag(bad, lines(1, 1), "you"), Err(TagError::BadName(bad.into())), "{bad}");
        }
        assert!(matches!(tags.tag("gone", lines(0, 0), "you"), Err(TagError::Subject(_))), "a subject that's gone can't be tagged");
        assert_eq!(changes(&store), ["you tagged lines 10 to 12", "lead tagged lines 1 to 1", "lead tagged lines 1 to 1"]);
        // Kept: a server started again has them.
        let (again, ..) = self::tags(store);
        assert_eq!(again.list().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["auth-bug", "flaky-build"]);
    }

    #[test]
    fn a_tag_is_renamed_recoloured_noted_taken_off_and_deleted_and_each_change_is_told_and_kept_with_who_made_it() {
        let store = Arc::new(MemoryStore::default());
        let (tags, hub, _) = tags(store.clone());
        let (_, mut events) = hub.subscribe();
        let tag = tags.tag("x", lines(3, 4), "you").unwrap();
        tags.tag("y", lines(5, 5), "you").unwrap();
        assert_eq!(tags.edit("x", Edit { name: Some("Y".into()), ..Edit::default() }, "you"), Err(TagError::Taken("Y".into())), "in other capitals too");
        let renamed = tags.edit("x", Edit { name: Some("deploy".into()), colour: Some(5), note: Some("staging only".into()) }, "worker q1").unwrap();
        assert_eq!((renamed.id.clone(), renamed.name.as_str(), renamed.colour, renamed.note.as_str(), renamed.noted_by.as_str()), (tag.id.clone(), "deploy", 5, "staging only", "worker q1"));
        assert_eq!(tags.find(tag.id.as_str()).unwrap().name, "deploy", "found by its id too");
        assert_eq!(tags.edit("deploy", Edit { colour: Some(8), ..Edit::default() }, "you"), Err(TagError::BadColour));
        let cleared = tags.edit("deploy", Edit { note: Some(String::new()), ..Edit::default() }, "you").unwrap();
        assert_eq!((cleared.note.as_str(), cleared.noted_by.as_str()), ("", ""), "a note cleared is no one's");
        assert!(tags.untag("deploy", &lines(3, 4), "lead").unwrap().on.is_empty());
        assert_eq!(tags.untag("deploy", &lines(3, 4), "lead"), Err(TagError::NotOn("deploy".into())), "not on it: said so");
        tags.delete("deploy", "lead").unwrap();
        assert_eq!(tags.find("deploy"), Err(TagError::NotFound("deploy".into())));
        let mut told = Vec::new();
        while let Ok(event) = events.try_recv() {
            told.push(event);
        }
        assert!(matches!(told.last(), Some(ClientEvent::TagRemoved { tag_id }) if *tag_id == tag.id), "{told:?}");
        assert_eq!(told.iter().filter(|e| matches!(e, ClientEvent::TagUpdated { .. })).count(), 5);
        assert_eq!(
            changes(&store)[2..],
            ["worker q1 renamed renamed: was x; recoloured: to 5; noted: staging only", "you noted cleared", "lead untagged lines 3 to 4", "lead deleted on 0 things"],
            "who changed it, and how"
        );
    }

    #[test]
    fn subjects_are_named_by_their_owners_and_kept_where_their_owners_moved_them() {
        let (tags, hub, owner) = tags(Arc::new(MemoryStore::default()));
        assert_eq!(tags.named("lines 7 to 9"), Ok(lines(7, 9)));
        assert!(matches!(tags.named("a chat message"), Err(TagError::Subject(_))), "no owner names it");
        tags.tag("t", lines(1, 2), "you").unwrap();
        tags.tag("t", lines(100, 101), "you").unwrap();
        let (_, mut events) = hub.subscribe();
        tags.refresh();
        assert!(events.try_recv().is_err(), "nothing moved: nothing told");
        // Lines from 100 on, the owner says, moved 1000 on (laid out anew):
        // it says so, and they're kept where they are now, named so.
        owner.relay();
        tags.refresh();
        let Ok(ClientEvent::TagUpdated { tag }) = events.try_recv() else { panic!("the move is told") };
        assert_eq!(tag.on.iter().map(|t| (t.subject.clone(), t.what.as_str())).collect::<Vec<_>>(), [(lines(1, 2), "lines 1 to 2"), (lines(1100, 1101), "lines 1100 to 1101")]);
        assert_eq!(tag.on[1].said, "lines 100 to 101", "what it said when tagged stays");
        let read = tags.read("t").unwrap();
        assert_eq!(read.on.iter().map(|t| t.now.as_deref()).collect::<Vec<_>>(), [Some("lines 1 to 2"), Some("lines 1100 to 1101")]);
        tags.untag("t", &lines(1100, 1101), "you").unwrap();
        assert_eq!(tags.find("t").unwrap().on.len(), 1);
    }

    #[test]
    fn a_prompt_naming_tags_hands_on_what_they_are_on_as_they_are_now() {
        let (tags, ..) = tags(Arc::new(MemoryStore::default()));
        tags.tag("flaky-build", lines(10, 12), "you").unwrap();
        tags.edit("flaky-build", Edit { note: Some("fails on Mondays".into()), ..Edit::default() }, "you").unwrap();
        let text = "Look at #flaky-build. Is it #1 or #nothing?";
        assert_eq!(hash_names(text), ["flaky-build", "1", "nothing"]);
        let expanded = tags.expand(text);
        assert!(expanded.starts_with(text), "the prompt as it was, first");
        assert!(expanded.contains("#flaky-build (fails on Mondays)\n- lines 10 to 12:\n  lines 10 to 12"), "{expanded}");
        assert!(!expanded.contains("#nothing ("), "a #word no tag has is left alone");
        assert_eq!(tags.expand("no tags here, not even mid#word"), "no tags here, not even mid#word");
    }

    #[test]
    fn what_a_subject_said_is_kept_up_to_a_limit() {
        assert_eq!(kept_text("é".repeat(SAID_MOST + 10)).chars().count(), SAID_MOST + 1, "cut, with …");
        assert_eq!(kept_text("short".into()), "short");
    }
}
