//! Where the analysis is kept: the plan's
//! tables, every row belonging to one file. A change writes, in one
//! transaction, only the changed tables of the files it touched; start-up
//! reads the rows back. Reads other than start-up use their own connection
//! (WAL), so they never wait on a write.
//!
//! The code is the source of truth, so the store is disposable: one with
//! another layout (`PRAGMA user_version`), or that can't be read at all, is
//! thrown away and made again from the code.

mod sqlite;

pub use sqlite::{RefTo, Settings, SqliteStore};

/// Bumped whenever the tables change.
pub const STORE_VERSION: i64 = 4;

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

/// Everything kept, as loaded at start-up (highlights are read per file,
/// when shown).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub files: Vec<code_analyzer::rows::FileRows>,
    /// What the rows were linked under (types, packs, linker): other
    /// fingerprints mean every file is linked again.
    pub fingerprints: Option<String>,
}
