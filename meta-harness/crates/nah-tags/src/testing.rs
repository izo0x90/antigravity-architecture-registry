//! For tests: tags kept in memory (with the changes made).

use std::collections::BTreeSet;
use std::sync::Mutex;

use harness_contracts::Tag;

use crate::{Change, TagStore};

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

    fn unreadable(&self) -> Result<usize, String> {
        Ok(0)
    }

    fn kept_files(&self) -> Result<BTreeSet<String>, String> {
        Ok(self.tags.lock().unwrap().iter().flat_map(|t| &t.on).filter_map(|t| t.kept.as_ref().map(|k| k.file.clone())).collect())
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
