//! For tests: tags kept in memory, and an owner of made-up terminal lines.

use std::sync::Mutex;

use harness_contracts::{Subject, SubjectNow, Tag, TagId, TerminalId};

use crate::{Resolve, TagStore};

#[derive(Default)]
pub struct MemoryStore(Mutex<Vec<Tag>>);

impl TagStore for MemoryStore {
    fn tags(&self) -> Result<Vec<Tag>, String> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn put(&self, tag: &Tag) -> Result<(), String> {
        let mut tags = self.0.lock().unwrap();
        tags.retain(|t| t.id != tag.id);
        tags.push(tag.clone());
        Ok(())
    }

    fn remove(&self, id: &TagId) -> Result<(), String> {
        self.0.lock().unwrap().retain(|t| t.id != *id);
        Ok(())
    }
}

/// Owns terminal t1's lines: lines `a` to `b` are named and say "lines a
/// to b"; those from line 0 are gone; those from 100 to 999 were laid out
/// anew and are 1000 lines on now.
pub struct Lines;

impl Resolve for Lines {
    fn now(&self, subject: &Subject) -> Option<SubjectNow> {
        let Subject::TerminalLines { first, last, .. } = subject else { return None };
        let words = format!("lines {first} to {last}");
        Some(SubjectNow { what: words.clone(), now: (*first > 0).then_some(words) })
    }

    fn named(&self, words: &str) -> Option<Result<Subject, String>> {
        let (first, last) = words.strip_prefix("lines ")?.split_once(" to ")?;
        Some(first.parse().and_then(|first| Ok(Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first, last: last.parse()? })).map_err(|e| e.to_string()))
    }

    fn moved(&self, subject: &Subject) -> Option<Subject> {
        let Subject::TerminalLines { terminal, shell, first, last } = subject else { return None };
        (100..1000).contains(first).then(|| Subject::TerminalLines { terminal: terminal.clone(), shell: *shell, first: first + 1000, last: last + 1000 })
    }
}
