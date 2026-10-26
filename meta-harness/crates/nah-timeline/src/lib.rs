//! The timeline: when the project's files changed, and who was working.
//!
//! Whenever any session's turn starts or ends, the project's files are
//! snapshotted (`nah_changes::Changes::snapshot`: a commit of their own,
//! the user's repository untouched; the last one again when nothing changed
//! since). Consecutive snapshots are windows, and the turns running during
//! one are who was working (none: the user, or a tool NAH doesn't run). A
//! turn's changes are its start's snapshot to its end's; others working
//! meanwhile are named, never guessed apart.
//!
//! Kept in the state database through `TimelineStore` (`nah-state`
//! implements it); let go by `Policy`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use harness_contracts::changes::{Comparison, Point, Timeline as Read, TurnChanges, Window, Working};
use harness_contracts::{AdapterEvent, ClientEvent, TurnStatus};
use harness_core::EventHub;
use nah_changes::{Changes, Snapshot};
use tokio::sync::broadcast::error::RecvError;

#[derive(Debug, thiserror::Error)]
pub enum TimelineError {
    #[error("timeline store: {0}")]
    Store(String),
    #[error(transparent)]
    Changes(#[from] nah_changes::ChangesError),
    #[error("{0}: write [timeline] keep_days and closed_session_days (whole days)")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, TimelineError>;

/// Where the timeline is kept.
pub trait TimelineStore: Send + Sync {
    /// Every snapshot kept, oldest first.
    fn snapshots(&self) -> std::result::Result<Vec<Kept>, String>;
    fn add_snapshot(&self, snapshot: &Kept) -> std::result::Result<(), String>;
    /// Every turn kept.
    fn turns(&self) -> std::result::Result<Vec<TurnSpan>, String>;
    /// Writes a turn: at its start, then again with its end.
    fn put_turn(&self, turn: &TurnSpan) -> std::result::Result<(), String>;
    /// A session ended (its turns go a while after).
    fn thread_closed(&self, thread: &str, at_ms: u64) -> std::result::Result<(), String>;
    fn forget(&self, snapshots: &[String], turns: &[String]) -> std::result::Result<(), String>;
}

/// A snapshot as kept: the one before it, and how many files changed since
/// that one (its window's count).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub id: String,
    pub at_ms: u64,
    pub previous: Option<String>,
    pub files: u32,
}

/// A turn as kept: whose it is, and the snapshots at its start and end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnSpan {
    pub turn: String,
    pub thread: String,
    /// Its slot's name (empty when the stream hadn't said).
    pub slot: String,
    pub started_ms: u64,
    pub start: String,
    pub ended_ms: Option<u64>,
    pub end: Option<String>,
    /// Files changed from its start to its end.
    pub files: Option<u32>,
    /// When its session ended.
    pub closed_ms: Option<u64>,
}

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// How long things are kept: snapshots and turns `keep_days`; a closed
/// session's turns `closed_session_days` after it closed (and snapshots no
/// kept turn needs, as long).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub keep_days: u64,
    pub closed_session_days: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self { keep_days: 14, closed_session_days: 1 }
    }
}

impl Policy {
    /// The `[timeline]` table of a project's `.nah/changes.toml` over the
    /// defaults.
    pub fn from_toml(text: &str) -> Result<Self> {
        #[derive(serde::Deserialize, Default)]
        #[serde(default, deny_unknown_fields)]
        struct File {
            timeline: Policy,
        }
        Ok(toml::from_str::<File>(text).map_err(|e| TimelineError::Config(e.to_string()))?.timeline)
    }
}

/// Who was working during `from..to` (`to`: none, up to `now`).
fn working(turns: &[TurnSpan], from: u64, to: u64, now: u64, but: Option<&str>) -> Vec<Working> {
    turns
        .iter()
        .filter(|t| but != Some(t.turn.as_str()) && t.started_ms < to && t.ended_ms.unwrap_or(now) > from)
        .map(|t| Working { slot: t.slot.clone(), thread: t.thread.clone(), turn: t.turn.clone() })
        .collect()
}

fn stored<T>(r: std::result::Result<T, String>) -> Result<T> {
    r.map_err(TimelineError::Store)
}

#[derive(Default)]
struct Inner {
    /// Each session's slot name, as the stream last said.
    slots: HashMap<String, String>,
    /// Turns started and not yet ended.
    running: HashSet<String>,
    /// The latest snapshot.
    last: Option<Snapshot>,
}

pub struct Timeline {
    changes: Arc<Changes>,
    store: Arc<dyn TimelineStore>,
    policy: Policy,
    inner: Mutex<Inner>,
}

impl Timeline {
    /// The timeline kept in `store`; turns it holds as running from before
    /// (their end wasn't seen) are let go.
    pub fn new(changes: Arc<Changes>, store: Arc<dyn TimelineStore>, policy: Policy) -> Result<Self> {
        let last = stored(store.snapshots())?.into_iter().max_by_key(|k| k.at_ms).map(|k| Snapshot { id: k.id, at_ms: k.at_ms });
        let dangling: Vec<String> = stored(store.turns())?.into_iter().filter(|t| t.end.is_none()).map(|t| t.turn).collect();
        stored(store.forget(&[], &dangling))?;
        Ok(Self { changes, store, policy, inner: Mutex::new(Inner { last, ..Default::default() }) })
    }

    /// Follows `events`: what's running now first, then each turn's start
    /// and end as they come.
    pub fn start(self, events: Arc<EventHub>) -> Arc<Self> {
        let timeline = Arc::new(self);
        Self::follow(Arc::downgrade(&timeline), events);
        timeline
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn follow(timeline: Weak<Self>, events: Arc<EventHub>) {
        tokio::spawn(async move {
            let (mut pending, mut live) = events.subscribe();
            loop {
                // (Each as of when it happened.)
                for stamped in std::mem::take(&mut pending).into_iter().filter(|s| Self::matters(&s.event)) {
                    let Some(t) = timeline.upgrade() else { return };
                    if let Err(e) = tokio::task::spawn_blocking(move || t.handle(&stamped.event, stamped.at_ms)).await {
                        tracing::warn!("the timeline couldn't take an event: {e}");
                    }
                }
                match live.recv().await {
                    Ok(event) => pending.push(event),
                    // Fell behind: from the stream's current state again
                    // (what's already running isn't started twice).
                    Err(RecvError::Lagged(missed)) => {
                        tracing::warn!("the timeline fell {missed} events behind: reading the current state");
                        (pending, live) = events.subscribe();
                    }
                    Err(RecvError::Closed) => return,
                }
            }
        });
    }

    /// The events the timeline reads.
    fn matters(event: &ClientEvent) -> bool {
        matches!(event, ClientEvent::SlotUpdated { .. } | ClientEvent::ThreadClosed { .. } | ClientEvent::Session { event: AdapterEvent::TurnUpdated { .. } })
    }

    /// What an event means for the timeline: a turn's start or end takes a
    /// snapshot (blocking: it reads the files). Failures are said, and the
    /// stream goes on.
    pub fn handle(&self, event: &ClientEvent, now: u64) {
        let done = match event {
            ClientEvent::SlotUpdated { slot } => {
                let mut inner = self.lock();
                inner.slots.retain(|_, name| name != &slot.name);
                for thread in slot.sessions.iter().filter_map(|s| s.thread.as_ref()) {
                    inner.slots.insert(thread.to_string(), slot.name.clone());
                }
                Ok(())
            }
            ClientEvent::ThreadClosed { thread_id } => stored(self.store.thread_closed(thread_id.as_str(), now)),
            ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn } } => {
                let id = turn.id.to_string();
                let running = self.lock().running.contains(&id);
                match (turn.status, running) {
                    (TurnStatus::Running, false) => self.turn_started(&id, turn.thread_id.as_str(), now),
                    (status, true) if status.is_terminal() => self.turn_ended(&id, now),
                    _ => Ok(()),
                }
            }
            _ => Ok(()),
        };
        if let Err(e) = done {
            tracing::warn!("the timeline: {e}");
        }
    }

    fn turn_started(&self, turn: &str, thread: &str, now: u64) -> Result<()> {
        let start = self.take_snapshot(&format!("turn {turn} started"), now)?;
        let slot = self.lock().slots.get(thread).cloned().unwrap_or_default();
        stored(self.store.put_turn(&TurnSpan { turn: turn.to_owned(), thread: thread.to_owned(), slot, started_ms: now, start: start.id, ended_ms: None, end: None, files: None, closed_ms: None }))?;
        self.lock().running.insert(turn.to_owned());
        Ok(())
    }

    fn turn_ended(&self, turn: &str, now: u64) -> Result<()> {
        self.lock().running.remove(turn);
        let Some(mut span) = stored(self.store.turns())?.into_iter().find(|t| t.turn == turn) else { return Ok(()) };
        let end = self.take_snapshot(&format!("turn {turn} ended"), now)?;
        let files = self.changes.changed_paths(&Comparison { from: Point::Snapshot(span.start.clone()), to: Point::Snapshot(end.id.clone()) })?.len() as u32;
        (span.ended_ms, span.end, span.files) = (Some(now), Some(end.id), Some(files));
        stored(self.store.put_turn(&span))
    }

    /// The files now, as a snapshot (the last one when nothing changed
    /// since); a new one is kept, and what's due is let go.
    fn take_snapshot(&self, why: &str, now: u64) -> Result<Snapshot> {
        let last = self.lock().last.clone();
        let (snapshot, new) = self.changes.snapshot(why, last.as_ref())?;
        if new {
            let files = match &last {
                Some(l) => self.changes.changed_paths(&Comparison { from: Point::Snapshot(l.id.clone()), to: Point::Snapshot(snapshot.id.clone()) })?.len() as u32,
                None => 0,
            };
            // When it was taken, by the same clock as the turns.
            stored(self.store.add_snapshot(&Kept { id: snapshot.id.clone(), at_ms: now, previous: last.map(|l| l.id), files }))?;
            self.lock().last = Some(snapshot.clone());
            self.prune(now)?;
        }
        Ok(snapshot)
    }

    /// Lets go what the policy no longer keeps: turns older than it keeps,
    /// or of sessions closed long enough; snapshots as old, or that no kept
    /// turn needs and older than a closed session is kept (never the latest).
    pub fn prune(&self, now: u64) -> Result<()> {
        let (keep, closed) = (self.policy.keep_days * DAY_MS, self.policy.closed_session_days * DAY_MS);
        let turns = stored(self.store.turns())?;
        let gone = |t: &TurnSpan| t.started_ms + keep < now || t.closed_ms.is_some_and(|c| c + closed < now);
        let (old, kept): (Vec<TurnSpan>, Vec<TurnSpan>) = turns.into_iter().partition(gone);
        let needed: HashSet<&str> = kept.iter().flat_map(|t| std::iter::once(t.start.as_str()).chain(t.end.as_deref())).collect();
        let latest = self.lock().last.as_ref().map(|s| s.id.clone());
        let snapshots: Vec<String> = stored(self.store.snapshots())?
            .into_iter()
            .filter(|s| Some(&s.id) != latest.as_ref() && (s.at_ms + keep < now || (!needed.contains(s.id.as_str()) && s.at_ms + closed < now)))
            .map(|s| s.id)
            .collect();
        if old.is_empty() && snapshots.is_empty() {
            return Ok(());
        }
        for id in &snapshots {
            self.changes.drop_snapshot(id)?;
        }
        stored(self.store.forget(&snapshots, &old.into_iter().map(|t| t.turn).collect::<Vec<_>>()))
    }

    /// The timeline as it is at `now`: the windows (the last up to the
    /// files now) and the turns' changes.
    pub fn read(&self, now: u64) -> Result<Read> {
        let mut snapshots = stored(self.store.snapshots())?;
        snapshots.sort_by_key(|s| s.at_ms);
        let running = self.lock().running.clone();
        let mut turns: Vec<TurnSpan> = stored(self.store.turns())?.into_iter().filter(|t| t.end.is_some() || running.contains(&t.turn)).collect();
        turns.sort_by(|a, b| (a.started_ms, &a.turn).cmp(&(b.started_ms, &b.turn)));
        let count = |from: &str, to: Point| -> Result<u32> { Ok(self.changes.changed_paths(&Comparison { from: Point::Snapshot(from.to_owned()), to })?.len() as u32) };
        let mut windows = Vec::with_capacity(snapshots.len());
        for pair in snapshots.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            // Counted when taken, unless the one before it was let go since.
            let files = if b.previous.as_ref() == Some(&a.id) { b.files } else { count(&a.id, Point::Snapshot(b.id.clone()))? };
            windows.push(Window { from: a.id.clone(), to: Some(b.id.clone()), started_ms: a.at_ms, ended_ms: Some(b.at_ms), working: working(&turns, a.at_ms, b.at_ms, now, None), files });
        }
        if let Some(last) = snapshots.last() {
            windows.push(Window { from: last.id.clone(), to: None, started_ms: last.at_ms, ended_ms: None, working: working(&turns, last.at_ms, now, now, None), files: count(&last.id, Point::Files)? });
        }
        let mut changes = Vec::with_capacity(turns.len());
        for t in &turns {
            let to = t.end.clone().map_or(Point::Files, Point::Snapshot);
            let files = match t.files {
                Some(f) => f,
                None => count(&t.start, to.clone())?,
            };
            changes.push(TurnChanges {
                thread: t.thread.clone(),
                turn: t.turn.clone(),
                comparison: Comparison { from: Point::Snapshot(t.start.clone()), to },
                files,
                alongside: working(&turns, t.started_ms, t.ended_ms.unwrap_or(now), now, Some(&t.turn)),
            });
        }
        Ok(Read { windows, turns: changes })
    }
}

#[cfg(test)]
mod tests;
