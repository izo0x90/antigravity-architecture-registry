//! The state of code indexing, as clients see it: whether the server is
//! reading code (and how far it got), how the last run went, and which files
//! changed on disk since they were last read.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexState {
    /// Nothing running.
    #[default]
    Idle,
    Indexing,
    /// The last run stopped with an error.
    Failed,
}

/// How the last finished run went.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSummary {
    pub millis: u64,
    /// Files looked at, and of those re-read (the rest came from the cache).
    pub files: usize,
    pub parsed: usize,
    pub components: usize,
    pub relations: usize,
    /// Calls that couldn't be linked to anything for certain.
    pub unresolved: usize,
    /// When it finished (RFC 3339).
    pub at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexStatus {
    pub state: IndexState,
    /// Progress of the run under way: files done of `total`.
    #[serde(default)]
    pub done: usize,
    #[serde(default)]
    pub total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<IndexSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Repo paths changed on disk since they were last read (files, or a
    /// directory for everything under it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale: Vec<String>,
    /// Each language's own indexer: whether it's there, and how its last
    /// run went.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexers: Vec<IndexerStatus>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexerStatus {
    pub language: String,
    /// The indexer pack.
    pub indexer: String,
    pub state: IndexerState,
    /// The tool's version, as checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Files it described in its last run (as they are now).
    #[serde(default)]
    pub files: usize,
    /// Links it confirmed.
    #[serde(default)]
    pub proven: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexerState {
    /// Its pack isn't installed (or its tool is out of range): links in
    /// this language are matched by name.
    #[default]
    Missing,
    Ready,
    Running,
    Failed,
}

impl IndexStatus {
    /// Whether `file` changed on disk since it was read.
    pub fn is_stale(&self, file: &str) -> bool {
        self.stale.iter().any(|s| s == file || file.strip_prefix(s.as_str()).is_some_and(|rest| rest.starts_with('/')))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_files_and_directories_cover_what_is_under_them() {
        let status = IndexStatus { stale: vec!["src/a.rs".into(), "crates/x".into()], ..Default::default() };
        assert!(status.is_stale("src/a.rs") && status.is_stale("crates/x/src/lib.rs"));
        assert!(!status.is_stale("src/ab.rs") && !status.is_stale("crates/xy/lib.rs"));
    }
}
