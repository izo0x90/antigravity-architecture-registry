//! For tests: tags kept in memory, and an owner of made-up terminal lines.

use std::sync::Mutex;

use harness_contracts::{Subject, SubjectNow, Tag, TagId};

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

/// Owns terminal lines: lines `a` to `b` say "lines a to b"; those from
/// line 0 are gone.
pub struct Lines;

impl Resolve for Lines {
    fn now(&self, subject: &Subject) -> Option<SubjectNow> {
        let Subject::TerminalLines { terminal, first, last, .. } = subject else { return None };
        Some(SubjectNow { what: format!("terminal {terminal}"), now: (*first > 0).then(|| format!("lines {first} to {last}")) })
    }
}
