//! For tests: discussions kept in memory, with every change made.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use harness_contracts::DiscussionId;

use crate::{Change, DiscussionStore, Stored};

#[derive(Default)]
pub struct MemoryStore {
    pub kept: Mutex<BTreeMap<DiscussionId, Stored>>,
    /// Who changed what, in order: (who, what was done).
    pub changes: Mutex<Vec<(String, String)>>,
    /// How many kept this build can't read (and the files they point into).
    pub unreadable: Mutex<(usize, BTreeSet<String>)>,
    numbered: std::sync::atomic::AtomicU64,
}

impl DiscussionStore for MemoryStore {
    fn discussions(&self) -> Result<Vec<Stored>, String> {
        Ok(self.kept.lock().unwrap().values().cloned().collect())
    }

    fn unreadable(&self) -> Result<usize, String> {
        Ok(self.unreadable.lock().unwrap().0)
    }

    fn kept_files(&self) -> Result<BTreeSet<String>, String> {
        let mut files: BTreeSet<String> =
            self.kept.lock().unwrap().values().filter_map(|d| d.kept.as_ref().map(|k| k.file.clone())).collect();
        files.extend(self.unreadable.lock().unwrap().1.iter().cloned());
        Ok(files)
    }

    fn number(&self) -> Result<u64, String> {
        Ok(self.numbered.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1)
    }

    fn put(&self, discussion: &Stored, change: Option<&Change>) -> Result<(), String> {
        self.kept.lock().unwrap().insert(discussion.id.clone(), discussion.clone());
        self.changes.lock().unwrap().extend(change.map(|c| (c.by.clone(), c.action.to_owned())));
        Ok(())
    }

    fn remove(&self, discussion: &Stored, change: &Change) -> Result<(), String> {
        self.kept.lock().unwrap().remove(&discussion.id);
        self.changes.lock().unwrap().push((change.by.clone(), change.action.to_owned()));
        Ok(())
    }
}
