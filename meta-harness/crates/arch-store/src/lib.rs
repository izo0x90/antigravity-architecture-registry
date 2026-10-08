//! Where the analysis is kept: per source file, what our reader read from it
//! (with its size, time and hash) and what a language's indexer said about
//! it; and the combined result, which everything downstream reads (the
//! graph, and per file what each named place refers to). A pipeline run is
//! written in one transaction.
//!
//! The code is the source of truth, so the store is disposable: one that
//! was written by another version of the reader, or can't be read at all,
//! is thrown away and rebuilt from the code.

mod memory;
mod sqlite;

use std::collections::BTreeMap;

use arch_graph::RepoPath;
use code_analyzer::index::IndexedFile;
use code_analyzer::{Combined, FileExtract, Occurrence};

pub use memory::MemoryStore;
pub use sqlite::{Settings, SqliteStore};

/// Bumped whenever a store's layout changes.
pub const STORE_VERSION: u32 = 2;

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

/// Everything kept, as loaded at start-up.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub files: Vec<Stored>,
    pub indexed: Vec<IndexedFile>,
    /// The combined result and the key it was made under (see `Commit`).
    pub combined: Option<(String, Combined)>,
}

/// What a pipeline run writes, together: all of it or none.
#[derive(Clone, Copy, Debug, Default)]
pub struct Commit<'a> {
    pub files: &'a [Change],
    /// An indexer's run: these files' indexer output is replaced.
    pub indexed: Option<IndexedChange<'a>>,
    pub combined: Option<CombinedRows<'a>>,
}

#[derive(Clone, Copy, Debug)]
pub struct IndexedChange<'a> {
    /// Files whose indexer output is dropped first (those it was run over).
    pub drop: &'a [RepoPath],
    pub put: &'a [IndexedFile],
}

#[derive(Clone, Copy, Debug)]
pub struct CombinedRows<'a> {
    /// What the result depends on beyond the files (types, packs, the
    /// analyzer's version): a result made under another key is made again.
    pub key: &'a str,
    pub combined: &'a Combined,
    /// Replaces every file's occurrences.
    pub occurrences: &'a BTreeMap<RepoPath, Vec<Occurrence>>,
}

/// Keeps the analysis between runs.
pub trait FactStore: Send + Sync {
    /// Everything (at start-up).
    fn load(&self) -> Result<Loaded, StoreError>;
    /// Writes a run's changes together: all of them or none.
    fn commit(&self, commit: &Commit<'_>) -> Result<(), StoreError>;
    /// One file's occurrences, from the last combined result.
    fn occurrences(&self, file: &RepoPath) -> Result<Option<Vec<Occurrence>>, StoreError>;
    /// Forgets everything (the next run reads every file).
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
        let mut e = code_analyzer::extract_file(
            code_analyzer::PackSet::from_shipped_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")).unwrap().by_name("rust").unwrap(),
            &code_analyzer::CodeRules::default(),
            RepoPath::new(file).unwrap(),
            "m".into(),
            "pub fn f() {}\n",
        );
        e.content_hash = hash.into();
        e
    }

    fn indexed(file: &str, hash: &str) -> IndexedFile {
        IndexedFile { path: RepoPath::new(file).unwrap(), indexer: "x 1.0".into(), content_hash: hash.into(), symbols: vec!["s".into()], occurrences: vec![[0, 1, 0, 1]], defined: vec![] }
    }

    fn occurrence(file: &str) -> BTreeMap<RepoPath, Vec<Occurrence>> {
        BTreeMap::from([(RepoPath::new(file).unwrap(), vec![Occurrence { start: 0, end: 1, target: None, symbol: Some("s".into()), definition: true }])])
    }

    /// The contract every store keeps.
    pub fn exercise(store: &dyn FactStore) {
        assert_eq!(store.load().unwrap(), Loaded::default());
        let stamp = Stamp { size: 14, modified: 7 };
        let put = |f: &str, h: &str| Change::Put(Box::new(Stored { extract: extract(f, h), stamp }));
        let combined = Combined { external: vec![code_analyzer::ExternalCall { caller: arch_graph::ComponentId::new("rust:a"), callee: "x".into() }], ..Combined::default() };
        let a = RepoPath::new("src/a.rs").unwrap();
        store
            .commit(&Commit {
                files: &[put("src/a.rs", "h1"), put("src/b.rs", "h2")],
                indexed: Some(IndexedChange { drop: &[], put: &[indexed("src/a.rs", "h1")] }),
                combined: Some(CombinedRows { key: "k1", combined: &combined, occurrences: &occurrence("src/a.rs") }),
            })
            .unwrap();
        let mut loaded = store.load().unwrap();
        loaded.files.sort_by(|a, b| a.extract.file.cmp(&b.extract.file));
        assert_eq!(loaded.files.iter().map(|s| (s.extract.file.as_str(), s.extract.content_hash.as_str())).collect::<Vec<_>>(), vec![("src/a.rs", "h1"), ("src/b.rs", "h2")]);
        assert_eq!(loaded.files[0].extract, extract("src/a.rs", "h1"), "facts come back as they went in");
        assert_eq!(loaded.files[0].stamp, stamp);
        assert_eq!(loaded.indexed, [indexed("src/a.rs", "h1")]);
        assert_eq!(loaded.combined, Some(("k1".to_owned(), combined.clone())));
        assert_eq!(store.occurrences(&a).unwrap(), occurrence("src/a.rs").remove(&a));
        // One file changes, another is gone, the first is touched; an indexer
        // run replaces what it covered; the result and occurrences are replaced.
        let touched = Stamp { size: 14, modified: 9 };
        store
            .commit(&Commit {
                files: &[put("src/a.rs", "h3"), Change::Remove(RepoPath::new("src/b.rs").unwrap()), Change::Restamp { file: a.clone(), stamp: touched }],
                indexed: Some(IndexedChange { drop: std::slice::from_ref(&a), put: &[] }),
                combined: Some(CombinedRows { key: "k2", combined: &Combined::default(), occurrences: &occurrence("src/c.rs") }),
            })
            .unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.files.len(), 1);
        assert_eq!((loaded.files[0].extract.content_hash.as_str(), loaded.files[0].stamp), ("h3", touched));
        assert!(loaded.indexed.is_empty());
        assert_eq!(loaded.combined, Some(("k2".to_owned(), Combined::default())));
        assert_eq!(store.occurrences(&a).unwrap(), None, "every file's occurrences are replaced");
        // Without a result, the last one stays.
        store.commit(&Commit { files: &[put("src/d.rs", "h4")], ..Commit::default() }).unwrap();
        assert_eq!(store.load().unwrap().combined.map(|c| c.0), Some("k2".to_owned()));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), Loaded::default());
    }

    // req: M.11, AP.7
    #[test]
    fn the_memory_store_keeps_the_contract() {
        exercise(&MemoryStore::default());
    }
}
