//! Where the facts read from the code are kept between runs: one row per
//! source file (what was read from it, and its size, time and hash when it
//! was read). The running service holds every file's facts in memory; the
//! store is only for starting again without re-reading the whole repo.
//!
//! The code is the source of truth, so the store is disposable: one that
//! was written by another version of the reader, or can't be read at all,
//! is thrown away and rebuilt from the code.

mod memory;
mod sqlite;

use arch_graph::RepoPath;
use arch_index::FileExtract;

pub use memory::MemoryStore;
pub use sqlite::{Settings, SqliteStore};

/// Bumped whenever a store's layout changes.
pub const STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StoreError {
    #[error("store: {0}")]
    Failed(String),
}

/// What tells a file changed without reading it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub size: u64,
    /// Modification time, in nanoseconds since the epoch (0: unknown).
    pub modified: i64,
}

impl Stamp {
    pub fn of(meta: &std::fs::Metadata) -> Self {
        let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i64);
        Self { size: meta.len(), modified }
    }
}

/// One file's facts as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub extract: FileExtract,
    pub stamp: Stamp,
}

/// One change to the store.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// A file read (again): its facts and stamp.
    Put(Box<Stored>),
    /// The same content, touched: only the stamp is new.
    Restamp { file: RepoPath, stamp: Stamp },
    /// A file that's gone.
    Remove(RepoPath),
}

/// Keeps every source file's facts between runs.
pub trait FactStore: Send + Sync {
    /// Every file's facts (at start-up).
    fn load(&self) -> Result<Vec<Stored>, StoreError>;
    /// Applies `changes` together: all of them or none.
    fn apply(&self, changes: &[Change]) -> Result<(), StoreError>;
    /// Forgets everything (the next read reads every file).
    fn clear(&self) -> Result<(), StoreError>;
    /// Its size on disk (0 in memory).
    fn bytes(&self) -> u64 {
        0
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn extract(file: &str, hash: &str) -> FileExtract {
        let mut e = arch_index::extract_file(
            arch_index::PackSet::built_in().unwrap().by_name("rust").unwrap(),
            &arch_index::CodeRules::default(),
            RepoPath::new(file).unwrap(),
            "m".into(),
            "pub fn f() {}\n",
        );
        e.content_hash = hash.into();
        e
    }

    /// The contract every store keeps.
    pub fn exercise(store: &dyn FactStore) {
        assert!(store.load().unwrap().is_empty());
        let stamp = Stamp { size: 14, modified: 7 };
        let put = |f: &str, h: &str| Change::Put(Box::new(Stored { extract: extract(f, h), stamp }));
        store.apply(&[put("src/a.rs", "h1"), put("src/b.rs", "h2")]).unwrap();
        let mut loaded = store.load().unwrap();
        loaded.sort_by(|a, b| a.extract.file.cmp(&b.extract.file));
        assert_eq!(loaded.iter().map(|s| (s.extract.file.as_str(), s.extract.content_hash.as_str())).collect::<Vec<_>>(), vec![("src/a.rs", "h1"), ("src/b.rs", "h2")]);
        assert_eq!(loaded[0].extract, extract("src/a.rs", "h1"), "facts come back as they went in");
        assert_eq!(loaded[0].stamp, stamp);
        // One file changes: only its row; another is gone; a third touched.
        let touched = Stamp { size: 14, modified: 9 };
        store.apply(&[put("src/a.rs", "h3"), Change::Remove(RepoPath::new("src/b.rs").unwrap()), Change::Restamp { file: RepoPath::new("src/a.rs").unwrap(), stamp: touched }]).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!((loaded[0].extract.content_hash.as_str(), loaded[0].stamp), ("h3", touched));
        store.clear().unwrap();
        assert!(store.load().unwrap().is_empty());
    }

    // req: M.11
    #[test]
    fn the_memory_store_keeps_the_contract() {
        exercise(&MemoryStore::default());
    }
}
