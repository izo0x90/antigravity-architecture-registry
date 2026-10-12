//! The state of code indexing, as clients see it: whether the server is
//! reading code (and how far it got), how the last run went, and which files
//! changed on disk since they were last read, and each language's server.

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
    /// Each language's server: whether it's running, and if not, why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<ServerStatus>,
}

/// One language's server, as clients see it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerStatus {
    pub language: String,
    /// The server pack.
    pub server: String,
    pub state: ServerState,
    /// The server's version, as it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Why it isn't ready (every state but ready has one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Positions it gave that couldn't be turned into places in a file.
    #[serde(default)]
    pub dropped_positions: usize,
    /// The language's files, and the links its server proved in them.
    #[serde(default)]
    pub files: usize,
    #[serde(default)]
    pub proven: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerState {
    /// Its pack isn't installed: links in this language stay unconfirmed.
    #[default]
    NotInstalled,
    /// Installed, but its version isn't one the pack supports.
    OutOfRange,
    /// Installed, but its check fails.
    Broken,
    /// Running, still loading the project.
    Starting,
    Ready,
    /// It ended or couldn't start; started again after a wait.
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
