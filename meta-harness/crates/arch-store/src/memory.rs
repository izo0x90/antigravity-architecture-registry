//! A store in memory (tests, and embedding without a disk).

use std::collections::BTreeMap;
use std::sync::Mutex;

use arch_graph::RepoPath;

use crate::{Change, FactStore, StoreError, Stored};

#[derive(Default)]
pub struct MemoryStore(Mutex<BTreeMap<RepoPath, Stored>>);

impl FactStore for MemoryStore {
    fn load(&self) -> Result<Vec<Stored>, StoreError> {
        Ok(self.0.lock().map_err(|_| StoreError::Failed("lock poisoned".into()))?.values().cloned().collect())
    }

    fn apply(&self, changes: &[Change]) -> Result<(), StoreError> {
        let mut rows = self.0.lock().map_err(|_| StoreError::Failed("lock poisoned".into()))?;
        for change in changes {
            match change {
                Change::Put(stored) => {
                    rows.insert(stored.extract.file.clone(), (**stored).clone());
                }
                Change::Restamp { file, stamp } => {
                    if let Some(row) = rows.get_mut(file) {
                        row.stamp = *stamp;
                    }
                }
                Change::Remove(file) => {
                    rows.remove(file);
                }
            }
        }
        Ok(())
    }

    fn clear(&self) -> Result<(), StoreError> {
        self.0.lock().map_err(|_| StoreError::Failed("lock poisoned".into()))?.clear();
        Ok(())
    }
}
