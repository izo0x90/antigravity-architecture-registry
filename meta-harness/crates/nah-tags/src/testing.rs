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

/// Owns terminal t1's lines: lines `a` to `b` are named "lines a to b"
/// and say "text of lines a to b"; those from line 0 are gone; those from 100 to 999 were laid out
/// anew (once `relay` is called) and are `relaid` thousand lines on now.
/// It doesn't keep them for good: once `scroll_away` is called, none is
/// there. Lines from 500 on are still growing: `grow` adds to them.
#[derive(Default)]
pub struct Lines {
    pub relaid: AtomicU64,
    gone: std::sync::atomic::AtomicBool,
    growing: Mutex<Vec<crate::More>>,
}

impl Lines {
    /// Lays lines 100 to 999 out anew (once more).
    pub fn relay(&self) {
        self.relaid.fetch_add(1, Ordering::SeqCst);
    }

    /// Every line goes (past what's kept).
    pub fn scroll_away(&self) {
        self.gone.store(true, Ordering::SeqCst);
    }

    /// The growing lines add `text`.
    pub fn grow(&self, text: &str) {
        for more in self.growing.lock().unwrap().iter_mut() {
            more(text);
        }
    }
}

impl Resolve for Lines {
    fn name(&self, subject: &Subject) -> Option<String> {
        let Subject::TerminalLines { first, last, .. } = subject else { return None };
        Some(format!("lines {first} to {last}"))
    }

    fn now(&self, subject: &Subject) -> Option<String> {
        let Subject::TerminalLines { first, .. } = subject else { return None };
        let Subject::TerminalLines { last, .. } = subject else { return None };
        (*first > 0 && !self.gone.load(Ordering::SeqCst)).then(|| format!("text of lines {first} to {last}\n"))
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

    fn lasts(&self, _subject: &Subject) -> bool {
        false
    }

    fn follow(&self, subject: &Subject, more: crate::More) -> bool {
        let Subject::TerminalLines { first, .. } = subject else { return false };
        if *first < 500 {
            return false;
        }
        let mut more = more;
        more(&self.now(subject).unwrap_or_default());
        self.growing.lock().unwrap().push(more);
        true
    }
}
