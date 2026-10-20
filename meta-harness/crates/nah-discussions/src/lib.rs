//! Discussions: the one discussion engine, for every kind of thing. A
//! discussion is on one thing (a `Subject`, as tags are): a question, or a
//! change asked for; its messages go back and forth; an answer (a change
//! request's says what it changed, and that's checked); only the user
//! settles or reopens one. Whose move it is follows from where it stands
//! (`Discussion::whose_move`). A thing its owner doesn't keep for good (a
//! terminal's output, a chat's turn) is kept in the things' logs when a
//! discussion starts on it, as tags keep it.
//!
//! Kept: the state database holds each discussion's structure and where
//! its messages' text is (`Stored`); the text is in files of its own
//! (`state/discussions/`, through the same log layer as the things' logs,
//! searched with FFF). Every change is told on the event stream
//! (`DiscussionUpdated`, `DiscussionRemoved`: all of it) and kept with who
//! made it. A plan asks the engine whether its component can be approved
//! (`blocker`). A discussion is a thing too (`Subject::Discussion`), owned
//! here: tagged, gone to, named from another.

#[cfg(feature = "testing")]
pub mod testing;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use harness_contracts::{ClientEvent, Discussion, DiscussionId, DiscussionKind, DiscussionStatus, Kept, Named, Said, SaidKind, Subject};
use harness_core::EventHub;
use nah_things::{Logs, Notes, Things};
use serde::{Deserialize, Serialize};

/// Who the user is in a discussion (the rest are agents' slots).
pub const USER: &str = "user";

/// The agent slot a discussion asks unless another is named.
pub const LEAD: &str = "lead";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DiscussionError {
    #[error("there's no discussion {0}")]
    NotFound(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Invalid(String),
    #[error("that can't be discussed here: {0}")]
    Subject(String),
    #[error("discussions couldn't be kept: {0}")]
    Store(String),
}

/// A message as kept: everything but its text, which is in the files
/// (`text`: where).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSaid {
    pub by: String,
    pub at_ms: u64,
    #[serde(default)]
    pub kind: SaidKind,
    pub text: Kept,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<Named>,
}

/// A discussion as kept (its messages' text in the files).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stored {
    pub id: DiscussionId,
    pub on: Named,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<Kept>,
    #[serde(default)]
    pub kind: DiscussionKind,
    pub by: String,
    pub asked: String,
    #[serde(default)]
    pub status: DiscussionStatus,
    pub said: Vec<StoredSaid>,
}

/// A line found in what's said (`Discussions::find`): in which discussion,
/// which of its messages (from 0), which of its lines (from 1), the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub discussion: DiscussionId,
    pub said: usize,
    pub line: u64,
    pub text: String,
}

/// Who changed a discussion, and how (kept with the change).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub by: String,
    /// `started`, `said`, `answered`, `settled`, `reopened`, `made a change request`, `deleted`.
    pub action: &'static str,
    /// What it's on, and what was done.
    pub detail: String,
}

/// Where discussions are kept (the project's state database).
pub trait DiscussionStore: Send + Sync {
    /// The discussions kept (one this build can't read is left as it is
    /// and not given).
    fn discussions(&self) -> Result<Vec<Stored>, String>;
    /// How many kept this build can't read.
    fn unreadable(&self) -> Result<usize, String>;
    /// The things' logs and the message files they point into: those of
    /// discussions this build can't read too.
    fn kept_files(&self) -> Result<BTreeSet<String>, String>;
    /// The next discussion's number: never one given before (one deleted
    /// keeps its number, so a note on it never points at another).
    fn number(&self) -> Result<u64, String>;
    /// Keeps `discussion` (`change`: who changed it; none when only where
    /// its thing is moved).
    fn put(&self, discussion: &Stored, change: Option<&Change>) -> Result<(), String>;
    fn remove(&self, discussion: &Stored, change: &Change) -> Result<(), String>;
}

/// A discussion with its text, and as it's kept.
struct Held {
    discussion: Discussion,
    stored: Stored,
}

pub struct Discussions {
    store: Arc<dyn DiscussionStore>,
    hub: Arc<EventHub>,
    things: Arc<Things>,
    /// The messages' text.
    said: Logs,
    held: Mutex<BTreeMap<DiscussionId, Held>>,
    /// Some kept can't be read by this build.
    unreadable: bool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Now, as a reader of the files reads a date.
fn now_words() -> String {
    httpdate::fmt_http_date(std::time::SystemTime::now())
}

fn text(raw: &str, what: &str) -> Result<String, DiscussionError> {
    let t = raw.trim();
    if t.is_empty() { Err(DiscussionError::Invalid(format!("{what} needs some text"))) } else { Ok(t.to_owned()) }
}

/// A message to add (when: as it's added).
fn said(by: &str, kind: SaidKind, text: String, changed: Vec<Named>) -> Said {
    Said { by: by.to_owned(), at_ms: 0, kind, text, changed }
}

fn user_only(by: &str, what: &str) -> Result<(), DiscussionError> {
    if by == USER { Ok(()) } else { Err(DiscussionError::Forbidden(format!("only the user {what}"))) }
}

impl Notes for Discussions {
    /// The things' logs the discussions point into (their messages' files
    /// are their own).
    fn kept_files(&self) -> Result<BTreeSet<String>, String> {
        self.store.kept_files()
    }

    fn subjects(&self) -> Option<Vec<Subject>> {
        (!self.unreadable).then(|| lock(&self.held).values().map(|h| h.discussion.on.subject.clone()).collect())
    }

    /// What they're on, where it is now and named as it's named now (asked
    /// of the owners with no discussion held).
    fn refresh(&self) {
        let on: Vec<(DiscussionId, Named)> = lock(&self.held).iter().map(|(id, h)| (id.clone(), h.discussion.on.clone())).collect();
        for (id, was) in on {
            let Some((subject, what)) = self.things.renamed(&was.subject, &was.what) else { continue };
            let mut held = lock(&self.held);
            // Changed meanwhile: it's refreshed next time.
            let Some(h) = held.get_mut(&id).filter(|h| h.discussion.on == was) else { continue };
            let (mut discussion, mut stored) = (h.discussion.clone(), h.stored.clone());
            discussion.on = Named { subject, what };
            stored.on = discussion.on.clone();
            if let Err(e) = self.store.put(&stored, None) {
                tracing::warn!("A discussion's thing couldn't be kept where it is now: {e}");
                continue;
            }
            *h = Held { discussion: discussion.clone(), stored };
            drop(held);
            self.hub.publish(ClientEvent::DiscussionUpdated { discussion });
        }
    }
}

impl Discussions {
    /// The discussions kept, told on `hub` (for clients that come later),
    /// on the things `things` names and keeps; their messages' text in
    /// `said` (a folder of its own).
    pub fn open(store: Arc<dyn DiscussionStore>, hub: Arc<EventHub>, things: Arc<Things>, said: &Path) -> Result<Arc<Self>, DiscussionError> {
        let said = Logs::open(said, "said");
        let unreadable = store.unreadable().map_err(DiscussionError::Store)? > 0;
        let mut held = BTreeMap::new();
        for stored in store.discussions().map_err(DiscussionError::Store)? {
            let discussion = Self::read_out(&said, &stored);
            hub.publish(ClientEvent::DiscussionUpdated { discussion: discussion.clone() });
            held.insert(stored.id.clone(), Held { discussion, stored });
        }
        let discussions = Arc::new(Self { store, hub, things: things.clone(), said, held: Mutex::new(held), unreadable });
        let notes: Weak<dyn Notes> = Arc::downgrade(&discussions) as Weak<dyn Notes>;
        things.noting(notes);
        discussions.refresh();
        // The message files no discussion points into (what a crash left).
        if !discussions.unreadable {
            let used: BTreeSet<String> = lock(&discussions.held).values().flat_map(|h| h.stored.said.iter().map(|s| s.text.file.clone())).collect();
            discussions.said.forget_all_but(&used);
        }
        Ok(discussions)
    }

    /// A kept discussion with its messages' text read in.
    fn read_out(said: &Logs, stored: &Stored) -> Discussion {
        let said = stored
            .said
            .iter()
            .map(|s| Said {
                by: s.by.clone(),
                at_ms: s.at_ms,
                kind: s.kind,
                text: match said.read(&s.text) {
                    Ok(text) => text,
                    Err(e) => format!("(its text couldn't be read: {e})"),
                },
                changed: s.changed.clone(),
            })
            .collect();
        Discussion { id: stored.id.clone(), on: stored.on.clone(), kept: stored.kept.clone(), kind: stored.kind, by: stored.by.clone(), asked: stored.asked.clone(), status: stored.status, said }
    }

    /// Every discussion, oldest first.
    pub fn list(&self) -> Vec<Discussion> {
        let mut out: Vec<Discussion> = lock(&self.held).values().map(|h| h.discussion.clone()).collect();
        out.sort_by_key(|d| d.said.first().map_or(0, |s| s.at_ms));
        out
    }

    pub fn get(&self, id: &DiscussionId) -> Result<Discussion, DiscussionError> {
        lock(&self.held).get(id).map(|h| h.discussion.clone()).ok_or_else(|| DiscussionError::NotFound(id.to_string()))
    }

    /// The discussions on `subject`, on something in it or something it's
    /// in (a component's: its sections' and relations' too; `touches`).
    pub fn on(&self, subject: &Subject) -> Vec<Discussion> {
        lock(&self.held).values().filter(|h| h.discussion.on.subject.touches(subject)).map(|h| h.discussion.clone()).collect()
    }

    /// The lines of the messages that have every word of `text` in them
    /// (searched with FFF where they're kept), at most `most` from each
    /// message, oldest discussion first; and whether the search ran out of
    /// time before it was through (some may be missing).
    pub fn find(&self, text: &str, most: usize) -> (Vec<Found>, bool) {
        let at: BTreeMap<String, (DiscussionId, usize)> = lock(&self.held).values().flat_map(|h| h.stored.said.iter().enumerate().map(|(i, s)| (s.text.file.clone(), (h.stored.id.clone(), i)))).collect();
        let files = at.keys().cloned().collect();
        let (lines, partial) = self.said.search(text, &files, most);
        let mut found: Vec<Found> = lines.into_iter().filter_map(|(file, line, text)| at.get(&file).map(|(id, said)| Found { discussion: id.clone(), said: *said, line: line - 1, text })).collect();
        let order: BTreeMap<DiscussionId, u64> = self.list().into_iter().enumerate().map(|(i, d)| (d.id, i as u64)).collect();
        found.sort_by_key(|f| (order.get(&f.discussion).copied(), f.said, f.line));
        (found, partial)
    }

    /// The things these discussions are on (to read what they say: the
    /// things layer gives it, from the logs once it's gone).
    pub fn things(&self) -> &Things {
        &self.things
    }

    /// A message written to the files: where it is.
    fn write(&self, id: &DiscussionId, what: &str, by: &str, kind: SaidKind, body: &str) -> Result<Kept, DiscussionError> {
        let header = format!("discussion {id} · {what} · {} by {by} · {}", match kind {
            SaidKind::Message => "said",
            SaidKind::Answer => "answered",
            SaidKind::Settled => "settled",
            SaidKind::Reopened => "reopened",
        }, now_words());
        self.said.write(&header, body).map_err(|e| DiscussionError::Store(e.to_string()))
    }

    /// Keeps `held` (stored with the change, then told).
    fn keep(&self, held: &mut BTreeMap<DiscussionId, Held>, discussion: Discussion, stored: Stored, change: &Change) -> Result<Discussion, DiscussionError> {
        self.store.put(&stored, Some(change)).map_err(DiscussionError::Store)?;
        held.insert(stored.id.clone(), Held { discussion: discussion.clone(), stored });
        self.hub.publish(ClientEvent::DiscussionUpdated { discussion: discussion.clone() });
        Ok(discussion)
    }

    /// Starts a discussion on `subject` (by `by`: the user, or an agent's
    /// slot): a question or a change request, asking `asked` (none: the
    /// lead). A thing its owner doesn't keep for good is kept in the logs.
    pub fn start(&self, subject: &Subject, kind: DiscussionKind, body: &str, by: &str, asked: Option<&str>) -> Result<Discussion, DiscussionError> {
        let body = text(body, "a discussion")?;
        let asked = asked.map(str::trim).filter(|a| !a.is_empty()).unwrap_or(LEAD).to_owned();
        // Noted first: no number is given to a start that fails on its thing
        // (what the logs keep of it says it's a discussion's, not which).
        let noted = self.things.note(subject, &|what| format!("a discussion · {what} · started by {by} · {}", now_words())).map_err(DiscussionError::Subject)?;
        let id = match self.store.number() {
            Ok(n) => DiscussionId::new(format!("d{n}")),
            Err(e) => {
                self.things.let_go(&noted.kept.iter().collect::<Vec<_>>());
                return Err(DiscussionError::Store(e));
            }
        };
        let on = Named { subject: noted.subject.clone(), what: noted.what.clone() };
        let written = match self.write(&id, &on.what, by, SaidKind::Message, &body) {
            Ok(written) => written,
            Err(e) => {
                self.things.let_go(&noted.kept.iter().collect::<Vec<_>>());
                return Err(e);
            }
        };
        let at_ms = now_ms();
        let discussion = Discussion { id: id.clone(), on: on.clone(), kept: noted.kept.clone(), kind, by: by.to_owned(), asked: asked.clone(), status: DiscussionStatus::Open, said: vec![Said { by: by.to_owned(), at_ms, kind: SaidKind::Message, text: body, changed: Vec::new() }] };
        let stored = Stored { id: id.clone(), on, kept: noted.kept.clone(), kind, by: by.to_owned(), asked, status: DiscussionStatus::Open, said: vec![StoredSaid { by: by.to_owned(), at_ms, kind: SaidKind::Message, text: written.clone(), changed: Vec::new() }] };
        let mut held = lock(&self.held);
        let done = self.keep(&mut held, discussion, stored, &Change { by: by.to_owned(), action: "started", detail: format!("{} on {}", kind.words(), noted.what) });
        drop(held);
        if done.is_err() {
            self.said.remove(&written.file);
            self.things.let_go(&noted.kept.iter().collect::<Vec<_>>());
        }
        done
    }

    /// Adds `said` to discussion `id` (once `check` passes, as it is
    /// then), leaving it at `status` (none: where it stands).
    fn add(&self, id: &DiscussionId, said: Said, status: Option<DiscussionStatus>, action: &'static str, check: &dyn Fn(&Discussion) -> Result<(), DiscussionError>) -> Result<Discussion, DiscussionError> {
        let Said { by, kind, text: body, changed, .. } = said;
        let what = {
            let held = lock(&self.held);
            let h = held.get(id).ok_or_else(|| DiscussionError::NotFound(id.to_string()))?;
            check(&h.discussion)?;
            h.discussion.on.what.clone()
        };
        let written = self.write(id, &what, &by, kind, &body)?;
        let mut held = lock(&self.held);
        let Some(h) = held.get(id) else {
            drop(held);
            self.said.remove(&written.file);
            return Err(DiscussionError::NotFound(id.to_string()));
        };
        // As it is now (changed meanwhile, it's checked again).
        if let Err(e) = check(&h.discussion) {
            drop(held);
            self.said.remove(&written.file);
            return Err(e);
        }
        let at_ms = now_ms();
        let (mut discussion, mut stored) = (h.discussion.clone(), h.stored.clone());
        stored.said.push(StoredSaid { by: by.clone(), at_ms, kind, text: written.clone(), changed: changed.clone() });
        discussion.said.push(Said { by: by.clone(), at_ms, kind, text: body, changed });
        if let Some(status) = status {
            discussion.status = status;
            stored.status = status;
        }
        let done = self.keep(&mut held, discussion, stored, &Change { by, action, detail: format!("{id} on {what}") });
        if done.is_err() {
            self.said.remove(&written.file);
        }
        done
    }

    /// A message in discussion `id` (the user's, or an agent's): it stays
    /// where it stands.
    pub fn reply(&self, id: &DiscussionId, body: &str, by: &str) -> Result<Discussion, DiscussionError> {
        let body = text(body, "a reply")?;
        self.add(id, said(by, SaidKind::Message, body, Vec::new()), None, "said", &|d| {
            if d.status == DiscussionStatus::Settled { Err(DiscussionError::Invalid(format!("{id} is settled: the user reopens it first"))) } else { Ok(()) }
        })
    }

    /// The answer to discussion `id`, naming what it changed (`changed`, by
    /// their names): a change request's has to name a change to what it's
    /// on (or what's in it), each one changed as its owner says.
    pub fn answer(&self, id: &DiscussionId, body: &str, changed: &[String], by: &str) -> Result<Discussion, DiscussionError> {
        let body = text(body, "an answer")?;
        let mut named = Vec::new();
        for words in changed {
            let subject = self.things.named(words).map_err(DiscussionError::Invalid)?;
            if self.things.changed(&subject) == Some(false) {
                return Err(DiscussionError::Invalid(format!("{words} hasn't changed since it was last reviewed: change it first, then name it")));
            }
            let what = self.things.name(&subject).unwrap_or_else(|| words.clone());
            named.push(Named { subject, what });
        }
        let names = named.clone();
        self.add(id, said(by, SaidKind::Answer, body, named), Some(DiscussionStatus::Answered), "answered", &move |d| {
            if d.status == DiscussionStatus::Settled {
                return Err(DiscussionError::Invalid(format!("{id} is settled")));
            }
            if d.kind == DiscussionKind::ChangeRequest && !names.iter().any(|n| n.subject.touches(&d.on.subject)) {
                return Err(DiscussionError::Invalid(format!("{id} asks for a change to {}: name what you changed there (`changed`)", d.on.what)));
            }
            Ok(())
        })
    }

    /// The user settles discussion `id` (a change request once it's
    /// answered with what changed), saying why if they like.
    pub fn settle(&self, id: &DiscussionId, body: &str, by: &str) -> Result<Discussion, DiscussionError> {
        user_only(by, "settles a discussion: answer it instead")?;
        self.add(id, said(by, SaidKind::Settled, body.trim().to_owned(), Vec::new()), Some(DiscussionStatus::Settled), "settled", &|d| match (d.kind, d.status) {
            (_, DiscussionStatus::Settled) => Err(DiscussionError::Invalid(format!("{id} is settled already"))),
            (DiscussionKind::ChangeRequest, DiscussionStatus::Open) => Err(DiscussionError::Invalid(format!("{id} asks for a change: it's settled once it's answered with what changed"))),
            _ => Ok(()),
        })
    }

    /// The user reopens discussion `id`: why.
    pub fn reopen(&self, id: &DiscussionId, why: &str, by: &str) -> Result<Discussion, DiscussionError> {
        user_only(by, "reopens a discussion")?;
        let why = text(why, "reopening it (why)")?;
        self.add(id, said(by, SaidKind::Reopened, why, Vec::new()), Some(DiscussionStatus::Open), "reopened", &|d| {
            if d.status == DiscussionStatus::Open { Err(DiscussionError::Invalid(format!("{id} is open already"))) } else { Ok(()) }
        })
    }

    /// The user makes discussion `id` a change request (it's open again:
    /// its answer has to name what changed).
    pub fn ask_for_change(&self, id: &DiscussionId, by: &str) -> Result<Discussion, DiscussionError> {
        user_only(by, "makes a discussion a change request")?;
        let mut held = lock(&self.held);
        let h = held.get(id).ok_or_else(|| DiscussionError::NotFound(id.to_string()))?;
        if h.discussion.kind == DiscussionKind::ChangeRequest {
            return Err(DiscussionError::Invalid(format!("{id} asks for a change already")));
        }
        let (mut discussion, mut stored) = (h.discussion.clone(), h.stored.clone());
        discussion.kind = DiscussionKind::ChangeRequest;
        stored.kind = DiscussionKind::ChangeRequest;
        discussion.status = DiscussionStatus::Open;
        stored.status = DiscussionStatus::Open;
        let what = discussion.on.what.clone();
        self.keep(&mut held, discussion, stored, &Change { by: by.to_owned(), action: "made a change request", detail: format!("{id} on {what}") })
    }

    /// The user deletes discussion `id`: its messages' text and what the
    /// logs kept of its thing go.
    pub fn delete(&self, id: &DiscussionId, by: &str) -> Result<(), DiscussionError> {
        user_only(by, "deletes a discussion")?;
        let mut held = lock(&self.held);
        let h = held.get(id).ok_or_else(|| DiscussionError::NotFound(id.to_string()))?;
        self.store.remove(&h.stored, &Change { by: by.to_owned(), action: "deleted", detail: format!("{id} on {}", h.discussion.on.what) }).map_err(DiscussionError::Store)?;
        let Some(gone) = held.remove(id) else { return Ok(()) };
        drop(held);
        self.hub.publish(ClientEvent::DiscussionRemoved { discussion_id: id.clone() });
        for said in &gone.stored.said {
            self.said.remove(&said.text.file);
        }
        self.things.let_go(&gone.stored.kept.iter().collect::<Vec<_>>());
        Ok(())
    }

    /// Why a plan's component can't be approved yet (discussions on it,
    /// its sections, its relations or the whole plan that aren't settled):
    /// none when it can.
    pub fn blocker(&self, plan: &arch_graph::PlanId, id: &arch_graph::ComponentId) -> Option<String> {
        let component = Subject::PlanComponent { plan: plan.clone(), id: id.clone() };
        let open: Vec<String> = lock(&self.held)
            .values()
            .map(|h| &h.discussion)
            .filter(|d| d.status != DiscussionStatus::Settled && d.on.subject.touches(&component))
            .map(|d| match d.status {
                DiscussionStatus::Answered => format!("{} on {} (answered: settle or reopen it)", d.id, d.on.what),
                _ => format!("{} on {} (open: \"{}\")", d.id, d.on.what, d.asking().chars().take(60).collect::<String>()),
            })
            .collect();
        if open.is_empty() { None } else { Some(format!("{id} has discussions not settled: {}", open.join("; "))) }
    }
}

/// A discussion as a thing: named `discussion d1`, saying its messages.
impl nah_things::Resolve for Discussions {
    fn owns(&self, subject: &Subject) -> bool {
        matches!(subject, Subject::Discussion { .. })
    }

    fn name(&self, subject: &Subject) -> String {
        subject.name(&harness_contracts::Known::default())
    }

    fn now(&self, subject: &Subject) -> Option<String> {
        let Subject::Discussion { id } = subject else { return None };
        let d = self.get(id).ok()?;
        let status = match d.status {
            DiscussionStatus::Open => "open",
            DiscussionStatus::Answered => "answered",
            DiscussionStatus::Settled => "settled",
        };
        let mut out = format!("{} on {} ({status})\n", d.kind.words(), d.on.what);
        for said in &d.said {
            out.push_str(&format!("{}: {}\n", said.by, said.text));
        }
        Some(out)
    }

    fn named(&self, words: &str) -> Option<Result<Subject, String>> {
        let id = DiscussionId::new(words.trim().strip_prefix("discussion ")?.trim());
        Some(if self.get(&id).is_ok() { Ok(Subject::Discussion { id }) } else { Err(format!("there's no discussion {id}")) })
    }

    fn there(&self, subject: &Subject) -> bool {
        matches!(subject, Subject::Discussion { id } if self.get(id).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use arch_graph::{ComponentId, PlanId};
    use harness_contracts::{Move, TerminalId};
    use nah_things::Resolve;
    use nah_things::testing::Lines;

    use super::*;
    use crate::testing::MemoryStore;

    fn lines(first: u64, last: u64) -> Subject {
        Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first, last, command: None }
    }

    fn plan() -> PlanId {
        PlanId::new("p").unwrap()
    }

    fn component(id: &str) -> Subject {
        Subject::PlanComponent { plan: plan(), id: ComponentId::new(id) }
    }

    /// Owns plan p's components (`component a`…), each there; those in
    /// `changed` changed since the plan was last reviewed.
    #[derive(Default)]
    struct Parts {
        changed: Mutex<Vec<String>>,
    }

    impl Resolve for Parts {
        fn owns(&self, subject: &Subject) -> bool {
            matches!(subject, Subject::PlanComponent { .. } | Subject::PlanSection { .. } | Subject::Plan { .. })
        }
        fn name(&self, subject: &Subject) -> String {
            subject.name(&harness_contracts::Known::default())
        }
        fn now(&self, subject: &Subject) -> Option<String> {
            Some(self.name(subject))
        }
        fn named(&self, words: &str) -> Option<Result<Subject, String>> {
            let rest = words.strip_prefix("plan p")?.trim();
            Some(Ok(match rest.split_whitespace().collect::<Vec<_>>().as_slice() {
                [] => Subject::Plan { plan: plan() },
                ["component", id] => component(id),
                ["component", id, "section", s] => Subject::PlanSection { plan: plan(), id: ComponentId::new(*id), section: (*s).to_owned() },
                _ => return Some(Err(format!("no {rest}"))),
            }))
        }
        fn lasts(&self, _: &Subject) -> bool {
            true
        }
        fn changed(&self, subject: &Subject) -> Option<bool> {
            let (Subject::PlanComponent { id, .. } | Subject::PlanSection { id, .. }) = subject else { return None };
            Some(self.changed.lock().unwrap().iter().any(|c| c == id.as_str()))
        }
    }

    struct Fixture {
        discussions: Arc<Discussions>,
        things: Arc<Things>,
        hub: Arc<EventHub>,
        owner: Arc<Lines>,
        parts: Arc<Parts>,
        dir: tempfile::TempDir,
    }

    /// The engine over `store`, its files and logs in `dir`, as a server
    /// starts it.
    fn open(store: Arc<MemoryStore>, dir: tempfile::TempDir) -> Fixture {
        let hub = Arc::new(EventHub::with_capacity(64));
        let things = Things::open(&dir.path().join("tagged"), nah_things::GROWN_MOST);
        let owner = Arc::new(Lines::default());
        things.register(owner.clone());
        let parts = Arc::new(Parts::default());
        things.register(parts.clone());
        let discussions = Discussions::open(store, hub.clone(), things.clone(), &dir.path().join("discussions")).unwrap();
        things.register(discussions.clone());
        things.forget_unused();
        Fixture { discussions, things, hub, owner, parts, dir }
    }

    fn engine(store: Arc<MemoryStore>) -> Fixture {
        open(store, tempfile::tempdir().unwrap())
    }

    fn changes(store: &MemoryStore) -> Vec<String> {
        store.changes.lock().unwrap().iter().map(|(by, action)| format!("{by} {action}")).collect()
    }

    #[test]
    fn a_question_is_asked_answered_settled_and_reopened_and_every_step_is_kept_and_told() {
        let store = Arc::new(MemoryStore::default());
        let Fixture { discussions: d, hub, dir: _dir, .. } = engine(store.clone());
        let (_, mut events) = hub.subscribe();
        let started = d.start(&lines(10, 12), DiscussionKind::Question, "  why does this fail?  ", USER, None).unwrap();
        assert_eq!((started.id.as_str(), started.on.what.as_str(), started.asked.as_str(), started.asking()), ("d1", "lines 10 to 12", LEAD, "why does this fail?"));
        assert_eq!(started.whose_move(), Move::Agent(LEAD.into()), "open: the agent asked");
        assert!(matches!(events.try_recv(), Ok(ClientEvent::DiscussionUpdated { discussion }) if discussion == started), "told whole");
        d.reply(&started.id, "which run?", "lead").unwrap();
        d.reply(&started.id, "the second", USER).unwrap();
        assert_eq!(d.get(&started.id).unwrap().status, DiscussionStatus::Open, "a reply leaves it where it stands");
        let answered = d.answer(&started.id, "a race in the test", &[], "lead").unwrap();
        assert_eq!((answered.status, answered.whose_move()), (DiscussionStatus::Answered, Move::User), "answered: the user's move");
        let settled = d.settle(&started.id, "", USER).unwrap();
        assert_eq!((settled.status, settled.whose_move()), (DiscussionStatus::Settled, Move::Nobody));
        assert!(d.reply(&started.id, "more", "lead").is_err(), "settled: nothing said until it's reopened");
        assert!(d.reopen(&started.id, "  ", USER).is_err(), "reopened: why");
        let reopened = d.reopen(&started.id, "it fails again", USER).unwrap();
        assert_eq!((reopened.status, reopened.said.last().map(|s| (s.kind, s.text.as_str()))), (DiscussionStatus::Open, Some((SaidKind::Reopened, "it fails again"))));
        let texts: Vec<&str> = reopened.said.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["why does this fail?", "which run?", "the second", "a race in the test", "", "it fails again"], "every message, in order");
        assert_eq!(changes(&store), ["user started", "lead said", "user said", "lead answered", "user settled", "user reopened"], "each change with who made it");
        let other = d.start(&lines(20, 20), DiscussionKind::Question, "and this?", "lead", Some("worker-1")).unwrap();
        assert_eq!((other.id.as_str(), other.asked.as_str(), other.by.as_str()), ("d2", "worker-1", "lead"), "an agent starts one too, asking another slot");
        assert_eq!(d.on(&lines(10, 12)).len(), 1, "the discussions on a thing");
        assert!(d.start(&lines(1, 1), DiscussionKind::Question, " ", USER, None).is_err(), "it asks something");
        assert!(d.start(&lines(0, 0), DiscussionKind::Question, "gone?", USER, None).is_err(), "a thing that's gone");
        assert_eq!(d.start(&lines(30, 30), DiscussionKind::Question, "next?", USER, None).unwrap().id.as_str(), "d3", "no number given to a start that failed");
    }

    #[test]
    fn only_the_user_settles_reopens_deletes_or_asks_for_a_change_and_a_change_request_needs_an_answer_naming_its_change() {
        let store = Arc::new(MemoryStore::default());
        let Fixture { discussions: d, parts, dir: _dir, .. } = engine(store);
        let q = d.start(&component("a"), DiscussionKind::Question, "is this right?", USER, None).unwrap();
        d.answer(&q.id, "yes", &[], "lead").unwrap();
        for refused in [d.settle(&q.id, "", "lead"), d.reopen(&q.id, "no", "lead"), d.ask_for_change(&q.id, "lead")] {
            assert!(matches!(refused, Err(DiscussionError::Forbidden(_))), "{refused:?}");
        }
        assert!(matches!(d.delete(&q.id, "lead"), Err(DiscussionError::Forbidden(_))));
        let asked = d.ask_for_change(&q.id, USER).unwrap();
        assert_eq!((asked.kind, asked.status), (DiscussionKind::ChangeRequest, DiscussionStatus::Open), "a change request, open again");
        assert!(d.settle(&q.id, "", USER).is_err(), "a change request is settled once it's answered");
        let unnamed = d.answer(&q.id, "done", &[], "lead").unwrap_err().to_string();
        assert!(unnamed.contains("name what you changed"), "{unnamed}");
        let elsewhere = d.answer(&q.id, "done", &["plan p component b".into()], "lead");
        assert!(elsewhere.is_err(), "what changed is what it's on (or in it)");
        let unchanged = d.answer(&q.id, "done", &["plan p component a section logic".into()], "lead").unwrap_err().to_string();
        assert!(unchanged.contains("hasn't changed"), "{unchanged}");
        parts.changed.lock().unwrap().push("a".into());
        let answered = d.answer(&q.id, "done", &["plan p component a section logic".into()], "lead").unwrap();
        assert_eq!(answered.said.last().unwrap().changed[0].what, "plan p component a section logic", "what it changed, named");
        d.settle(&q.id, "thanks", USER).unwrap();
        d.delete(&q.id, USER).unwrap();
        assert!(matches!(d.get(&q.id), Err(DiscussionError::NotFound(_))));
        let next = d.start(&component("a"), DiscussionKind::Question, "again", USER, None).unwrap();
        assert_eq!(next.id.as_str(), "d2", "a deleted discussion's number isn't given again");
    }

    #[test]
    fn a_component_isnt_approved_while_a_discussion_on_it_its_sections_its_relations_or_the_plan_isnt_settled() {
        let Fixture { discussions: d, parts, dir: _dir, .. } = engine(Arc::new(MemoryStore::default()));
        let (p, a) = (plan(), ComponentId::new("a"));
        assert_eq!(d.blocker(&p, &a), None);
        let section = d.start(&Subject::PlanSection { plan: p.clone(), id: a.clone(), section: "logic".into() }, DiscussionKind::ChangeRequest, "split step 2", USER, None).unwrap();
        let blocked = d.blocker(&p, &a).unwrap();
        assert!(blocked.contains("d1 on plan p component a section logic (open: \"split step 2\")"), "{blocked}");
        assert_eq!(d.blocker(&p, &ComponentId::new("b")), None, "another component's discussions don't");
        parts.changed.lock().unwrap().push("a".into());
        d.answer(&section.id, "split", &["plan p component a".into()], "lead").unwrap();
        assert!(d.blocker(&p, &a).unwrap().contains("answered: settle or reopen it"), "answered isn't settled");
        d.settle(&section.id, "", USER).unwrap();
        assert_eq!(d.blocker(&p, &a), None);
        let whole = d.start(&Subject::Plan { plan: p.clone() }, DiscussionKind::Question, "why this order?", USER, None).unwrap();
        assert!(d.blocker(&p, &ComponentId::new("b")).is_some(), "one on the whole plan holds every component");
        d.delete(&whole.id, USER).unwrap();
        let relation = Subject::PlanRelation { plan: p.clone(), relation: arch_graph::RelationKind::Calls, source: ComponentId::new("b"), target: a.clone() };
        // A relation's discussion, kept as started (no owner of relations here).
        assert!(Subject::touches(&relation, &component("a")), "a relation touches both its ends");
    }

    #[test]
    fn a_discussion_on_lines_that_move_follows_them_and_one_on_output_gone_from_the_terminal_still_says_it() {
        let store = Arc::new(MemoryStore::default());
        let Fixture { discussions: d, things, hub, owner, dir: _dir, .. } = engine(store.clone());
        let moving = d.start(&lines(100, 102), DiscussionKind::Question, "what's this?", USER, None).unwrap();
        assert!(moving.kept.is_some(), "what a terminal says is kept in the logs");
        let (_, mut events) = hub.subscribe();
        owner.relay();
        things.moved();
        let moved = d.get(&moving.id).unwrap();
        assert_eq!((moved.on.subject.clone(), moved.on.what.as_str()), (lines(1100, 1102), "lines 1100 to 1102"), "where they are now");
        assert_eq!(store.kept.lock().unwrap()[&moving.id].on, moved.on, "kept where they are now");
        assert!(matches!(events.try_recv(), Ok(ClientEvent::DiscussionUpdated { discussion }) if discussion.on == moved.on), "told");
        let gone = d.start(&lines(10, 12), DiscussionKind::Question, "and this?", USER, None).unwrap();
        owner.scroll_away();
        let (held, said) = things.said(&gone.on.subject, gone.kept.as_ref());
        assert_eq!((held, said.as_deref()), (harness_contracts::Held::Kept, Some("text of lines 10 to 12\n")), "from the logs once it's gone");
        d.delete(&gone.id, USER).unwrap();
        let log = _dir.path().join("tagged").join(&gone.kept.as_ref().unwrap().file);
        assert!(!log.exists(), "deleted: what the logs kept of it goes");
    }

    #[test]
    fn discussions_come_back_whole_after_a_restart_and_a_discussion_is_a_thing_too() {
        let store = Arc::new(MemoryStore::default());
        let dir = tempfile::tempdir().unwrap();
        let Fixture { discussions: d, dir, .. } = open(store.clone(), dir);
        let first = d.start(&lines(10, 12), DiscussionKind::Question, "why?\nsecond line", USER, None).unwrap();
        d.reply(&first.id, "because", "lead").unwrap();
        let before = d.get(&first.id).unwrap();
        drop(d);
        // What a crash left: a message file nothing points into.
        std::fs::write(dir.path().join("discussions").join("said-999.log"), "== stray\n").unwrap();
        let Fixture { discussions: d, things, dir: _dir, .. } = open(store.clone(), dir);
        assert_eq!(d.get(&first.id).unwrap(), before, "the same, text and all");
        let left: Vec<String> = std::fs::read_dir(things_dir(&d)).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(!left.iter().any(|f| f == "said-999.log"), "what no discussion points into goes: {left:?}");
        let named = things.named("discussion d1").unwrap();
        assert_eq!(named, Subject::Discussion { id: first.id.clone() }, "named");
        assert!(things.name(&named).is_some_and(|n| n == "discussion d1"));
        let said = things.said(&named, None).1.unwrap();
        assert!(said.contains("user: why?\nsecond line") && said.contains("lead: because"), "{said}");
        assert!(things.named("discussion d9").is_err(), "one that isn't there");
        let on_it = d.start(&named, DiscussionKind::Question, "is d1 still open?", USER, None).unwrap();
        assert_eq!(on_it.on.what, "discussion d1", "a discussion on a discussion");
        let (found, partial) = d.find("line second", 5);
        assert_eq!((found, partial), (vec![Found { discussion: first.id.clone(), said: 0, line: 2, text: "second line".into() }], false), "what's said, searched: every word, any order");
        assert_eq!(d.find("discussion", 5).0, [], "headings aren't found");
    }

    fn things_dir(d: &Discussions) -> std::path::PathBuf {
        d.said.dir().to_path_buf()
    }
}
