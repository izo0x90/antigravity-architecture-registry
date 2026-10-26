//! Tags: names given to things in the project, to find them again and to
//! point agents at them. A tag (`flaky-build`) has a colour and a note and
//! is on things, each with who tagged it. A tag only points: a thing is
//! its owner's, and the things layer (`nah_things`) names it, says what it
//! says (from its owner, or from the logs once its source is gone), keeps
//! it where its owner moved it, and searches it; tags know no owner.
//!
//! Every change is kept (the store: the state database, with who made it;
//! never a thing's text) and told on the event stream (`TagUpdated`,
//! `TagRemoved`), which keeps them for clients that come later.

#[cfg(feature = "testing")]
pub mod testing;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use harness_contracts::{ClientEvent, Held, Subject, TAG_COLOURS, Tag, TagId, TagRead, Tagged, TaggedNow};
use harness_core::EventHub;
use nah_things::{Notes, Things, some_of};

/// How much of a note is kept, how much of what a thing says a read
/// gives (the rest whole on asking), and how many things a tag is on at
/// most.
const NOTE_MOST: usize = 4000;
pub const SHOWN_MOST: usize = 4000;
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
    /// The logs' files the tags kept point into: those this build can't
    /// read too.
    fn kept_files(&self) -> Result<BTreeSet<String>, String>;
    /// How many tags kept this build can't read (a newer kind of subject).
    fn unreadable(&self) -> Result<usize, String>;
    /// Keeps `tag` as it is now, and the change that made it so (none:
    /// its owners moved its subjects).
    fn put(&self, tag: &Tag, change: Option<&Change>) -> Result<(), String>;
    fn remove(&self, tag: &Tag, change: &Change) -> Result<(), String>;
}

/// A line found in what a tagged thing says (`Tags::find_text`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundText {
    pub tag: String,
    /// Which of the tag's things (1-based), and its name.
    pub thing: usize,
    pub what: String,
    /// Which of its lines (1-based).
    pub line: u64,
    pub text: String,
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
    things: Arc<Things>,
    /// Some tags kept can't be read by this build: what they're on can't
    /// all be said.
    unreadable: bool,
}

/// How a read gets all of a thing's detail (folded otherwise).
const WHOLE: &str = "tag_read's `full`";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A name a tag can have (`tag_name_ok`).
fn check_name(name: &str) -> Result<String, TagError> {
    if harness_contracts::tag_name_ok(name) { Ok(name.to_owned()) } else { Err(TagError::BadName(name.to_owned())) }
}

/// Now, as every time a person reads is written (`arch_graph::when`).
fn now_words() -> String {
    harness_contracts::when::now()
}

/// The tag with id `name`, or named `name` (in capitals or not: `Flaky`
/// is `flaky`), in `tags`.
fn found(tags: &BTreeMap<TagId, Tag>, name: &str) -> Result<Tag, TagError> {
    tags.get(&TagId::new(name))
        .or_else(|| tags.values().find(|t| t.name.eq_ignore_ascii_case(name)))
        .cloned()
        .ok_or_else(|| TagError::NotFound(name.to_owned()))
}

fn now_ms() -> u64 {
    harness_contracts::when::now_ms()
}

impl Notes for Tags {
    fn kept_files(&self) -> Result<BTreeSet<String>, String> {
        self.store.kept_files()
    }

    fn subjects(&self) -> Option<Vec<Subject>> {
        (!self.unreadable).then(|| lock(&self.tags).values().flat_map(|t| t.on.iter().map(|on| on.subject.clone())).collect())
    }

    fn refresh(&self) {
        Tags::refresh(self);
    }
}

impl Tags {
    /// The tags kept, told on `hub` (for clients that come later), on the
    /// things `things` names and keeps.
    pub fn open(store: Arc<dyn TagStore>, hub: Arc<EventHub>, things: Arc<Things>) -> Result<Arc<Self>, TagError> {
        let kept = store.tags().map_err(TagError::Store)?;
        let unreadable = store.unreadable().map_err(TagError::Store)? > 0;
        for tag in &kept {
            hub.publish(ClientEvent::TagUpdated { tag: tag.clone() });
        }
        let tags = Arc::new(Self {
            store,
            hub,
            tags: Mutex::new(kept.into_iter().map(|t| (t.id.clone(), t)).collect()),
            things: things.clone(),
            unreadable,
        });
        let notes: Weak<dyn Notes> = Arc::downgrade(&tags) as Weak<dyn Notes>;
        things.noting(notes);
        tags.refresh();
        Ok(tags)
    }

    /// The things tags are on.
    pub fn things(&self) -> &Things {
        &self.things
    }

    /// What a tagged thing says, its detail (a chat turn's tool output)
    /// folded as every read of tags folds it.
    pub fn fold(&self, subject: &Subject, text: &str) -> String {
        self.things.fold(subject, text, WHOLE)
    }

    /// A tagged thing as a read gives it: where it is, some of what it
    /// says, and how many lines it says in all.
    fn now(&self, tagged: &Tagged) -> TaggedNow {
        let (held, glimpse) = self.things.glimpse(&tagged.subject, tagged.kept.as_ref(), SHOWN_MOST);
        let (text, lines) = glimpse.map_or((None, 0), |(t, n)| (Some(t), n));
        TaggedNow { tagged: tagged.clone(), held, text, lines }
    }

    /// Each tag's things kept where their owners say they are now, and
    /// named as they're named now: owners say when they moved them (lines
    /// laid out anew, a shell started again).
    pub fn refresh(&self) {
        let mut tags = lock(&self.tags);
        let changed: Vec<Tag> = tags
            .values()
            .filter_map(|tag| {
                let moved: Vec<Option<(Subject, String)>> = tag.on.iter().map(|t| self.things.renamed(&t.subject, &t.what)).collect();
                moved.iter().any(Option::is_some).then(|| Tag {
                    on: tag
                        .on
                        .iter()
                        .zip(moved)
                        .map(|(t, m)| m.map_or_else(|| t.clone(), |(subject, what)| Tagged { subject, what, ..t.clone() }))
                        .collect(),
                    ..tag.clone()
                })
            })
            .collect();
        for tag in changed {
            if let Err(e) = self.keep(&mut tags, tag, None) {
                tracing::warn!("A tag's things couldn't be kept where they are now: {e}");
            }
        }
    }

    /// The thing `words` name, as its owner names it.
    pub fn named(&self, words: &str) -> Result<Subject, TagError> {
        self.things.named(words).map_err(TagError::Subject)
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

    /// A tag (by its name or id) and what each of its things says now
    /// (where its owner said it is: owners tell when they move them).
    pub fn read(&self, name: &str) -> Result<TagRead, TagError> {
        let tag = self.find(name)?;
        let on = tag.on.iter().map(|t| self.now(t)).collect();
        Ok(TagRead { tag, on })
    }

    /// The `n`th thing tag `name` is on, and what it says, whole.
    pub fn thing(&self, name: &str, n: usize) -> Result<(Tagged, Held, Option<String>), TagError> {
        let tagged = self.nth(name, n)?;
        let (held, text) = self.things.said(&tagged.subject, tagged.kept.as_ref());
        Ok((tagged, held, text))
    }

    /// The `n`th thing tag `name` is on.
    fn nth(&self, name: &str, n: usize) -> Result<Tagged, TagError> {
        let tag = self.find(name)?;
        tag.on
            .get(n)
            .cloned()
            .ok_or_else(|| TagError::NotOn(format!("{} (it's on {})", tag.name, harness_contracts::on_things(tag.on.len()))))
    }

    /// Lines `from..=to` (from 1) of what the `n`th thing tag `name` is on
    /// says, and how many it has in all (none: nothing of it's here); only
    /// those read into memory.
    pub fn thing_lines(&self, name: &str, n: usize, from: u64, to: u64) -> Result<(Tagged, Held, Option<nah_things::Window>), TagError> {
        let tagged = self.nth(name, n)?;
        let (held, lines) = self.things.lines(&tagged.subject, tagged.kept.as_ref(), from, to);
        Ok((tagged, held, lines))
    }

    /// The lines of what tagged things say (all tags', or `tag`'s; only
    /// those `reaches` lets the caller read) that have every word of `text`
    /// in them (capitals heeded only when it has some), the first `most`:
    /// each with its tag, which of its things (1-based, as `thing` takes
    /// it), and which of its lines; and whether the search ran out of time
    /// (some may be missing).
    pub fn find_text(
        &self,
        text: &str,
        tag: Option<&str>,
        most: usize,
        reaches: &dyn Fn(&Subject) -> bool,
    ) -> Result<(Vec<FoundText>, bool), TagError> {
        let tags = match tag {
            Some(tag) => vec![self.find(tag)?],
            None => self.list(),
        };
        let searched: Vec<(&Tag, usize, &Tagged)> = tags
            .iter()
            .flat_map(|tag| tag.on.iter().enumerate().filter(|(_, t)| reaches(&t.subject)).map(move |(n, t)| (tag, n, t)))
            .collect();
        let among: Vec<(&Subject, Option<&harness_contracts::Kept>)> =
            searched.iter().map(|(_, _, t)| (&t.subject, t.kept.as_ref())).collect();
        let (found, partial) = self.things.find(text, &among, most);
        let found = found
            .into_iter()
            .map(|f| {
                let (tag, n, tagged) = searched[f.which];
                FoundText { tag: tag.name.clone(), thing: n + 1, what: tagged.what.clone(), line: f.line, text: f.text }
            })
            .collect();
        Ok((found, partial))
    }

    /// Whether `stored` (a thing a tag is on) is `subject`, where its owner
    /// says it is now (lines that moved since the tag was last kept where
    /// they are).
    fn same_now(&self, stored: &Subject, subject: &Subject) -> bool {
        stored.same_thing(subject) || (stored.may_be(subject) && self.things.current(stored).same_thing(subject))
    }

    /// Tags `subject` with `tag` (by its name or id; a new tag, in the
    /// colour least used, when there's none of that name), with who tagged
    /// it (`by`). One its owner doesn't keep for good is written to the
    /// logs (before the tag is kept: nothing waits on that); already on
    /// it: as it was. (Where its things are now is worked out once.)
    pub fn tag(&self, tag: &str, subject: Subject, by: &str) -> Result<Tag, TagError> {
        self.things.pass(|| self.tagging(tag, subject, by))
    }

    fn tagging(&self, tag: &str, subject: Subject, by: &str) -> Result<Tag, TagError> {
        let name = match found(&lock(&self.tags), tag) {
            Ok(existing) if existing.on.iter().any(|t| self.same_now(&t.subject, &subject)) => return Ok(existing),
            Ok(existing) if existing.on.len() >= MOST_ON => return Err(TagError::TooMany),
            Ok(existing) => existing.name,
            Err(_) => check_name(tag)?,
        };
        let noted =
            self.things.note(&subject, &|what| format!("{name} · {what} · tagged by {by} · {}", now_words())).map_err(TagError::Subject)?;
        let mut tags = lock(&self.tags);
        // As it is now (changed meanwhile, it's checked again).
        let checked = match found(&tags, tag) {
            Ok(existing) if existing.on.iter().any(|t| self.same_now(&t.subject, &noted.subject)) => Err(Ok(existing)),
            Ok(existing) if existing.on.len() >= MOST_ON => Err(Err(TagError::TooMany)),
            Ok(existing) => Ok(existing),
            Err(_) => {
                let used: Vec<u8> = tags.values().map(|t| t.colour).collect();
                let colour = (0..TAG_COLOURS).min_by_key(|c| used.iter().filter(|u| *u == c).count()).unwrap_or(0);
                Ok(Tag {
                    id: TagId::new(uuid::Uuid::new_v4().to_string()),
                    name,
                    colour,
                    note: String::new(),
                    noted_by: String::new(),
                    on: Vec::new(),
                })
            }
        };
        let mut kept = match checked {
            Ok(kept) => kept,
            Err(done) => {
                drop(tags);
                self.things.let_go(&noted.kept.iter().collect::<Vec<_>>());
                return done;
            }
        };
        let change = Change { by: by.to_owned(), action: "tagged", detail: noted.what.clone() };
        kept.on.push(Tagged { subject: noted.subject, what: noted.what, at_ms: now_ms(), by: by.to_owned(), kept: noted.kept.clone() });
        let done = self.keep(&mut tags, kept, Some(&change));
        drop(tags);
        if done.is_err() {
            // Not kept: nothing points into what was written.
            self.things.let_go(&noted.kept.iter().collect::<Vec<_>>());
        }
        done
    }

    /// Takes `tag` off `subject` (`by` took it off).
    pub fn untag(&self, tag: &str, subject: &Subject, by: &str) -> Result<Tag, TagError> {
        self.things.pass(|| self.untagging(tag, subject, by))
    }

    fn untagging(&self, tag: &str, subject: &Subject, by: &str) -> Result<Tag, TagError> {
        let mut tags = lock(&self.tags);
        let mut kept = found(&tags, tag)?;
        let Some(at) = kept.on.iter().position(|t| self.same_now(&t.subject, subject)) else { return Err(TagError::NotOn(kept.name)) };
        let gone = kept.on.remove(at);
        let done = self.keep(&mut tags, kept, Some(&Change { by: by.to_owned(), action: "untagged", detail: gone.what }))?;
        drop(tags);
        self.things.let_go(&gone.kept.iter().collect::<Vec<_>>());
        Ok(done)
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
            kept.note = some_of(note, NOTE_MOST);
        }
        let change = (!changes.is_empty()).then(|| Change {
            by: by.to_owned(),
            action: changes[0].0,
            detail: changes
                .iter()
                .map(|(action, detail)| if changes.len() == 1 { detail.clone() } else { format!("{action}: {detail}") })
                .collect::<Vec<_>>()
                .join("; "),
        });
        self.keep(&mut tags, kept, change.as_ref())
    }

    /// Deletes `tag` (`by` deleted it).
    pub fn delete(&self, tag: &str, by: &str) -> Result<(), TagError> {
        let mut tags = lock(&self.tags);
        let kept = found(&tags, tag)?;
        self.store
            .remove(&kept, &Change { by: by.to_owned(), action: "deleted", detail: harness_contracts::on_things(kept.on.len()) })
            .map_err(TagError::Store)?;
        tags.remove(&kept.id);
        drop(tags);
        self.hub.publish(ClientEvent::TagRemoved { tag_id: kept.id });
        self.things.let_go(&kept.on.iter().filter_map(|on| on.kept.as_ref()).collect::<Vec<_>>());
        Ok(())
    }

    /// What a link to `tag` pulls into a message: its name and note, then
    /// each thing it's on (read through the things layer, as it is now, or
    /// as kept); none: there's no such tag now.
    pub fn attached(&self, tag: &TagId, include: harness_contracts::Include) -> Option<nah_things::Attached> {
        let tag = lock(&self.tags).get(tag).cloned()?;
        let mut heading = format!("#{}", tag.name);
        // An agent's note says whose it is (the prompt is the user's).
        match (tag.note.as_str(), tag.noted_by.as_str()) {
            ("", _) => {}
            (note, "user" | "") => heading.push_str(&format!(" ({note})")),
            (note, by) => heading.push_str(&format!(" ({note}; noted by {by})")),
        }
        let things = tag
            .on
            .iter()
            .enumerate()
            .map(|(n, on)| nah_things::AttachedThing {
                what: if on.what.is_empty() { "something nothing here can show".to_owned() } else { on.what.clone() },
                subject: on.subject.clone(),
                kept: on.kept.clone(),
                read_by: format!("`tag_read` (tag {}, thing {})", tag.name, n + 1),
                whole: WHOLE,
            })
            .collect();
        Some(nah_things::Attached { heading, include, things, gone: false })
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

    use nah_things::testing::Lines;

    use super::*;
    use crate::testing::MemoryStore;

    fn lines(first: u64, last: u64) -> Subject {
        Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first, last, command: None }
    }

    struct Fixture {
        tags: Arc<Tags>,
        hub: Arc<EventHub>,
        owner: Arc<Lines>,
        logs: tempfile::TempDir,
    }

    fn open(store: Arc<MemoryStore>, logs: tempfile::TempDir) -> Fixture {
        let hub = Arc::new(EventHub::with_capacity(64));
        let things = Things::open(logs.path(), nah_things::GROWN_MOST);
        let tags = Tags::open(store, hub.clone(), things.clone()).unwrap();
        let owner = Arc::new(Lines::default());
        things.register(owner.clone());
        Fixture { tags, hub, owner, logs }
    }

    fn tags(store: Arc<MemoryStore>) -> Fixture {
        open(store, tempfile::tempdir().unwrap())
    }

    fn changes(store: &MemoryStore) -> Vec<String> {
        store.changes.lock().unwrap().iter().map(|c| format!("{} {} {}", c.by, c.action, c.detail)).collect()
    }

    #[test]
    fn a_tag_is_made_by_naming_it_and_keeps_its_subjects_names_and_who_tagged_them() {
        let store = Arc::new(MemoryStore::default());
        let Fixture { tags, hub, logs, .. } = tags(store.clone());
        let tag = tags.tag("flaky-build", lines(10, 12), "you").unwrap();
        assert_eq!((tag.name.as_str(), tag.note.as_str(), tag.on.len()), ("flaky-build", "", 1));
        assert_eq!((tag.on[0].what.as_str(), tag.on[0].by.as_str()), ("lines 10 to 12", "you"));
        let (_, mut events) = hub.subscribe();
        let again = tags.tag(tag.id.as_str(), lines(10, 12), "lead").unwrap();
        assert_eq!(again.on.len(), 1, "by its id; already on it");
        assert!(events.try_recv().is_err(), "nothing changed: nothing told");
        assert_eq!(tags.tag("Flaky-Build", lines(1, 1), "lead").unwrap().id, tag.id, "a name in other capitals is the same tag's");
        let other = tags.tag("auth-bug", lines(1, 1), "lead").unwrap();
        assert_ne!(other.colour, tag.colour, "the colour least used");
        // (Which names can be: `tag_name_ok`'s test.)
        assert_eq!(tags.tag("two words", lines(1, 1), "you"), Err(TagError::BadName("two words".into())));
        assert!(matches!(tags.tag("gone", lines(0, 0), "you"), Err(TagError::Subject(_))), "a subject that's gone can't be tagged");
        assert!(matches!(tags.tag("many", lines(1, 300), "you"), Err(TagError::Subject(_))), "nor too many lines");
        assert_eq!(changes(&store), ["you tagged lines 10 to 12", "lead tagged lines 1 to 1", "lead tagged lines 1 to 1"]);
        // Kept: a server started again has them.
        let again = self::open(store, logs).tags;
        assert_eq!(again.list().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["auth-bug", "flaky-build"]);
    }

    #[test]
    fn a_tag_points_what_its_owner_doesnt_keep_is_in_the_logs_and_comes_back_whole_once_its_source_went() {
        let store = Arc::new(MemoryStore::default());
        let Fixture { tags, owner, logs, .. } = tags(store.clone());
        let tag = tags.tag("seen", lines(10, 12), "you").unwrap();
        // The store has where it is, never what it says (rule 1).
        let kept = serde_json::to_string(&store.tags().unwrap()).unwrap();
        assert!(!kept.contains("text of"), "{kept}");
        assert!(tag.on[0].kept.is_some(), "its owner doesn't keep it: the logs do");
        let read = tags.read("seen").unwrap();
        assert_eq!((read.on[0].held, read.on[0].text.as_deref(), read.on[0].lines), (Held::Source, Some("text of lines 10 to 12\n"), 1));
        // Its source gone (scrolled away): the same, from the logs.
        owner.scroll_away();
        let read = tags.read("seen").unwrap();
        assert_eq!((read.on[0].held, read.on[0].text.as_deref()), (Held::Kept, Some("text of lines 10 to 12\n")));
        // Taken off: nothing points into its log, which goes.
        let file = tag.on[0].kept.as_ref().unwrap().file.clone();
        assert!(logs.path().join(&file).exists());
        tags.untag("seen", &lines(10, 12), "you").unwrap();
        assert!(!logs.path().join(&file).exists());
    }

    #[test]
    fn a_thing_still_growing_when_tagged_keeps_what_it_adds_in_a_file_of_its_own_until_the_tag_is_off_it() {
        let store = Arc::new(MemoryStore::default());
        let Fixture { tags, hub, owner, logs } = tags(store.clone());
        tags.tag("build", lines(500, 501), "you").unwrap();
        // What it adds is written, never stored nor told (one change: tagged).
        let (_, mut events) = hub.subscribe();
        owner.grow("more 1\n");
        owner.grow("more 2\n");
        assert!(events.try_recv().is_err());
        assert_eq!(store.changes.lock().unwrap().len(), 1);
        let read = tags.read("build").unwrap();
        assert_eq!(
            (read.on[0].held, read.on[0].text.as_deref(), read.on[0].lines),
            (Held::Source, Some("text of lines 500 to 501\nmore 1\nmore 2\n"), 3),
            "all of it, from the logs"
        );
        owner.scroll_away();
        let (tagged, held, text) = tags.thing("build", 0).unwrap();
        assert_eq!((held, text.as_deref()), (Held::Kept, Some("text of lines 500 to 501\nmore 1\nmore 2\n")));
        let file = std::fs::read_to_string(logs.path().join(tagged.kept.unwrap().file)).unwrap();
        assert!(file.starts_with("== build · lines 500 to 501 · tagged by you · ") && file.ends_with("more 2\n"), "its own: {file}");
        // Taken off: no longer followed, and tagged again, followed once.
        tags.untag("build", &lines(500, 501), "you").unwrap();
        owner.grow("more 3\n");
        assert_eq!(owner.followers(), 0);
        tags.tag("build", lines(500, 501), "you").unwrap();
        owner.grow("more 4\n");
        assert_eq!(owner.followers(), 1);
        assert!(tags.thing("build", 1).is_err(), "it's on one thing");
        // What it says is searched where it's kept (by the matcher given).
        let all = |_: &Subject| true;
        assert!(tags.find_text("MORE 4", None, 10, &all).unwrap().0.is_empty());
        assert!(tags.find_text("more 4", None, 10, &|_: &Subject| false).unwrap().0.is_empty(), "only what the caller reaches");
        let found = tags.find_text("more 4", Some("build"), 10, &all).unwrap().0;
        assert_eq!(
            found,
            [FoundText { tag: "build".into(), thing: 1, what: "lines 500 to 501".into(), line: 1, text: "more 4".into() }],
            "(its lines gone when tagged again: only what it added)"
        );
    }

    #[test]
    fn what_the_logs_kept_comes_back_after_a_restart_and_nothing_untagged_is_logged() {
        let store = Arc::new(MemoryStore::default());
        let Fixture { tags, owner, logs, .. } = tags(store.clone());
        assert!(std::fs::read_dir(logs.path()).map_or(true, |d| d.count() == 0), "nothing tagged, nothing logged");
        tags.tag("kept", lines(10, 12), "you").unwrap();
        tags.tag("kept", lines(500, 501), "you").unwrap();
        owner.grow("more\n");
        drop(tags);
        // The server started again (the same store and folder), the source gone.
        let again = self::open(store, logs);
        again.owner.scroll_away();
        let said = |n| again.tags.thing("kept", n).unwrap();
        assert_eq!((said(0).1, said(0).2.as_deref()), (Held::Kept, Some("text of lines 10 to 12\n")));
        assert_eq!((said(1).1, said(1).2.as_deref()), (Held::Kept, Some("text of lines 500 to 501\nmore\n")));
    }

    #[test]
    fn a_tag_is_renamed_recoloured_noted_taken_off_and_deleted_and_each_change_is_told_and_kept_with_who_made_it() {
        let store = Arc::new(MemoryStore::default());
        let Fixture { tags, hub, .. } = tags(store.clone());
        let (_, mut events) = hub.subscribe();
        let tag = tags.tag("x", lines(3, 4), "you").unwrap();
        tags.tag("y", lines(5, 5), "you").unwrap();
        assert_eq!(
            tags.edit("x", Edit { name: Some("Y".into()), ..Edit::default() }, "you"),
            Err(TagError::Taken("Y".into())),
            "in other capitals too"
        );
        let renamed =
            tags.edit("x", Edit { name: Some("deploy".into()), colour: Some(5), note: Some("staging only".into()) }, "worker q1").unwrap();
        assert_eq!(
            (renamed.id.clone(), renamed.name.as_str(), renamed.colour, renamed.note.as_str(), renamed.noted_by.as_str()),
            (tag.id.clone(), "deploy", 5, "staging only", "worker q1")
        );
        assert_eq!(tags.find(tag.id.as_str()).unwrap().name, "deploy", "found by its id too");
        assert_eq!(tags.edit("deploy", Edit { colour: Some(8), ..Edit::default() }, "you"), Err(TagError::BadColour));
        let cleared = tags.edit("deploy", Edit { note: Some(String::new()), ..Edit::default() }, "you").unwrap();
        assert_eq!((cleared.note.as_str(), cleared.noted_by.as_str()), ("", ""), "a note cleared is no one's");
        assert!(tags.untag("deploy", &lines(3, 4), "lead").unwrap().on.is_empty());
        assert_eq!(tags.untag("deploy", &lines(3, 4), "lead"), Err(TagError::NotOn("deploy".into())), "not on it: said so");
        tags.delete("deploy", "lead").unwrap();
        assert_eq!(tags.find("deploy"), Err(TagError::NotFound("deploy".into())));
        let mut told = Vec::new();
        while let Ok(stamped) = events.try_recv() {
            told.push(stamped.event);
        }
        assert!(matches!(told.last(), Some(ClientEvent::TagRemoved { tag_id }) if *tag_id == tag.id), "{told:?}");
        assert_eq!(told.iter().filter(|e| matches!(e, ClientEvent::TagUpdated { .. })).count(), 5);
        assert_eq!(
            changes(&store)[2..],
            [
                "worker q1 renamed renamed: was x; recoloured: to 5; noted: staging only",
                "you noted cleared",
                "lead untagged lines 3 to 4",
                "lead deleted on 0 things"
            ],
            "who changed it, and how"
        );
    }

    #[test]
    fn subjects_are_named_by_their_owners_and_kept_where_their_owners_moved_them() {
        let Fixture { tags, hub, owner, .. } = tags(Arc::new(MemoryStore::default()));
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
        let Ok(ClientEvent::TagUpdated { tag }) = events.try_recv().map(|s| s.event) else { panic!("the move is told") };
        assert_eq!(
            tag.on.iter().map(|t| (t.subject.clone(), t.what.as_str())).collect::<Vec<_>>(),
            [(lines(1, 2), "lines 1 to 2"), (lines(1100, 1101), "lines 1100 to 1101")]
        );
        let read = tags.read("t").unwrap();
        assert_eq!(
            read.on.iter().map(|t| t.text.as_deref()).collect::<Vec<_>>(),
            [Some("text of lines 1 to 2\n"), Some("text of lines 100 to 101\n")],
            "what they said, kept when tagged"
        );
        tags.untag("t", &lines(1100, 1101), "you").unwrap();
        assert_eq!(tags.find("t").unwrap().on.len(), 1);
    }

    /// A tag kept before its thing's owner kept as much with what's noted
    /// gains it at the next refresh (a line, the command it's in).
    #[test]
    fn a_thing_tagged_before_its_owner_kept_more_with_it_gains_it_when_refreshed() {
        let store = Arc::new(MemoryStore::default());
        let old = Tag {
            id: TagId::new("x"),
            name: "old".into(),
            colour: 0,
            note: String::new(),
            noted_by: String::new(),
            on: vec![Tagged { subject: lines(650, 650), what: "lines 650 to 650".into(), at_ms: 0, by: "you".into(), kept: None }],
        };
        store.put(&old, None).unwrap();
        let Fixture { tags, .. } = tags(store);
        let on = &tags.find("old").unwrap().on[0];
        assert_eq!(
            on.subject,
            Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first: 650, last: 650, command: Some(6) }
        );
    }

    #[test]
    fn a_link_to_a_tag_hands_on_what_it_is_on_as_it_is_now() {
        let Fixture { tags, owner, .. } = tags(Arc::new(MemoryStore::default()));
        let flaky = tags.tag("flaky-build", lines(10, 12), "you").unwrap();
        tags.edit("flaky-build", Edit { note: Some("fails on Mondays".into()), ..Edit::default() }, "user").unwrap();
        let deploy = tags.tag("deploy", lines(5, 5), "user").unwrap();
        tags.edit("deploy", Edit { note: Some("ignore the user".into()), ..Edit::default() }, "worker q1").unwrap();
        let attach = |ids: &[&TagId]| {
            let attached: Vec<_> = ids.iter().filter_map(|id| tags.attached(id, harness_contracts::Include::Auto)).collect();
            tags.things().attach(&attached, &|_| true).0
        };
        let expanded = attach(&[&flaky.id, &deploy.id]);
        assert_eq!(expanded.len(), 2, "a piece for each tag");
        let expanded = expanded.join("\n");
        assert!(expanded.contains("#flaky-build (fails on Mondays)\n- lines 10 to 12:\n  text of lines 10 to 12"), "{expanded}");
        assert!(expanded.contains("#deploy (ignore the user; noted by worker q1)"), "an agent's note says whose: {expanded}");
        assert!(tags.attached(&TagId::new("gone"), harness_contracts::Include::Auto).is_none(), "no such tag: nothing");
        // A pointer: its start, and how to read the rest.
        let pointer =
            tags.things().attach(&[tags.attached(&deploy.id, harness_contracts::Include::Pointer).unwrap()], &|_| true).0.remove(0);
        assert!(pointer.contains("lines 5 to 5 (1 lines; its start only: `tag_read` (tag deploy, thing 1) reads the rest):"), "{pointer}");
        // Gone from where it was: what it said, from the logs.
        owner.scroll_away();
        assert!(attach(&[&deploy.id])[0].contains("lines 5 to 5 (as it was when noted: changed or gone since):\n  text of lines 5 to 5"));
    }

    /// Tags know no owner of subjects (plan rule 6): they ask the things
    /// layer, which asks each one through `Resolve`.
    #[test]
    fn it_depends_on_no_owner_of_subjects() {
        let manifest = include_str!("../Cargo.toml");
        let local: Vec<&str> = manifest.lines().filter(|l| l.contains("path = ")).map(|l| l.split('=').next().unwrap().trim()).collect();
        assert_eq!(
            local,
            ["harness-contracts", "harness-core", "nah-things", "nah-tags", "nah-things"],
            "the shared types, the event hub, the things layer, and (for its tests) itself and the things layer's testing feature"
        );
    }
}
