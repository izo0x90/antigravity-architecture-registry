//! For tests: tags kept in memory (with the changes made), and an owner
//! of made-up terminal lines.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use harness_contracts::{Subject, Tag, TerminalId};

use crate::{Change, Resolve, TagStore};

#[derive(Default)]
pub struct MemoryStore {
    tags: Mutex<Vec<Tag>>,
    /// Each change kept, in order.
    pub changes: Mutex<Vec<Change>>,
}

impl TagStore for MemoryStore {
    fn tags(&self) -> Result<Vec<Tag>, String> {
        Ok(self.tags.lock().unwrap().clone())
    }

    fn put(&self, tag: &Tag, change: Option<&Change>) -> Result<(), String> {
        let mut tags = self.tags.lock().unwrap();
        tags.retain(|t| t.id != tag.id);
        tags.push(tag.clone());
        self.changes.lock().unwrap().extend(change.cloned());
        Ok(())
    }

    fn remove(&self, tag: &Tag, change: &Change) -> Result<(), String> {
        self.tags.lock().unwrap().retain(|t| t.id != tag.id);
        self.changes.lock().unwrap().push(change.clone());
        Ok(())
    }
}

/// Owns terminal t1's lines: lines `a` to `b` are named and say "lines a
/// to b"; those from line 0 are gone; those from 100 to 999 were laid out
/// anew (once `relay` is called) and are `relaid` thousand lines on now.
#[derive(Default)]
pub struct Lines {
    pub relaid: AtomicU64,
}

impl Lines {
    /// Lays lines 100 to 999 out anew (once more).
    pub fn relay(&self) {
        self.relaid.fetch_add(1, Ordering::SeqCst);
    }
}

impl Resolve for Lines {
    fn name(&self, subject: &Subject) -> Option<String> {
        let Subject::TerminalLines { first, last, .. } = subject else { return None };
        Some(format!("lines {first} to {last}"))
    }

    fn now(&self, subject: &Subject) -> Option<String> {
        let Subject::TerminalLines { first, .. } = subject else { return None };
        (*first > 0).then(|| self.name(subject)).flatten()
    }

    fn named(&self, words: &str) -> Option<Result<Subject, String>> {
        let (first, last) = words.strip_prefix("lines ")?.split_once(" to ")?;
        Some(first.parse().and_then(|first| Ok(Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first, last: last.parse()? })).map_err(|e| e.to_string()))
    }

    fn moved(&self, subject: &Subject) -> Option<Subject> {
        let Subject::TerminalLines { terminal, shell, first, last } = subject else { return None };
        let by = 1000 * self.relaid.load(Ordering::SeqCst);
        (by > 0 && (100..1000).contains(first)).then(|| Subject::TerminalLines { terminal: terminal.clone(), shell: *shell, first: first + by, last: last + by })
    }
}
