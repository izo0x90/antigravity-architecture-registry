//! A store in memory (tests, and embedding without a disk).

use std::collections::BTreeMap;
use std::sync::Mutex;

use arch_graph::RepoPath;
use code_analyzer::Occurrence;
use code_analyzer::index::IndexedFile;

use crate::{Change, Commit, FactStore, Loaded, StoreError, Stored};

#[derive(Default)]
struct Rows {
    files: BTreeMap<RepoPath, Stored>,
    indexed: BTreeMap<RepoPath, IndexedFile>,
    combined: Option<(String, code_analyzer::Combined)>,
    occurrences: BTreeMap<RepoPath, Vec<Occurrence>>,
}

#[derive(Default)]
pub struct MemoryStore(Mutex<Rows>);

impl MemoryStore {
    fn rows(&self) -> Result<std::sync::MutexGuard<'_, Rows>, StoreError> {
        self.0.lock().map_err(|_| StoreError::Failed("lock poisoned".into()))
    }
}

impl FactStore for MemoryStore {
    fn load(&self) -> Result<Loaded, StoreError> {
        let rows = self.rows()?;
        Ok(Loaded { files: rows.files.values().cloned().collect(), indexed: rows.indexed.values().cloned().collect(), combined: rows.combined.clone() })
    }

    fn commit(&self, commit: &Commit<'_>) -> Result<(), StoreError> {
        let mut rows = self.rows()?;
        for change in commit.files {
            match change {
                Change::Put(stored) => {
                    rows.files.insert(stored.extract.file.clone(), (**stored).clone());
                }
                Change::Restamp { file, stamp } => {
                    if let Some(row) = rows.files.get_mut(file) {
                        row.stamp = *stamp;
                    }
                }
                Change::Remove(file) => {
                    rows.files.remove(file);
                }
            }
        }
        if let Some(indexed) = commit.indexed {
            for file in indexed.drop {
                rows.indexed.remove(file);
            }
            for file in indexed.put {
                rows.indexed.insert(file.path.clone(), file.clone());
            }
        }
        if let Some(c) = commit.combined {
            rows.combined = Some((c.key.to_owned(), c.combined.clone()));
            rows.occurrences = c.occurrences.clone();
        }
        Ok(())
    }

    fn occurrences(&self, file: &RepoPath) -> Result<Option<Vec<Occurrence>>, StoreError> {
        Ok(self.rows()?.occurrences.get(file).cloned())
    }

    fn clear(&self) -> Result<(), StoreError> {
        *self.rows()? = Rows::default();
        Ok(())
    }
}
