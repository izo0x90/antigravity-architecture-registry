//! The limits the service keeps to (from config: built-in defaults, then a
//! project's `.nah/limits.toml`).

use serde::{Deserialize, Serialize};

const DEFAULT_LIMITS: &str = include_str!("../config/limits.toml");

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Plans whose overlay on the code is kept built.
    pub plans_kept_laid_over: usize,
    /// Cards in one tree answer.
    pub cards_per_answer: usize,
    /// Lines to cards shown elsewhere in one tree answer.
    pub links_per_answer: usize,
    /// Cards a client holds at most (a tree with the branches opened
    /// since): over it, the opened branches furthest away close again.
    pub cards_held: usize,
    /// Trees sent to clients kept to merge what's opened next into (the
    /// most recent).
    pub trees_kept: usize,
    /// Time for walking one tree.
    pub tree_ms: u64,
    /// Time for laying one tree out (its own limit).
    pub layout_ms: u64,
    /// A request slower than this is logged.
    pub slow_request_ms: u64,
    /// Answers whose timings are kept for /api/perf.
    pub perf_kept: usize,
    /// Searching symbols, files and lines.
    pub search: SearchLimits,
    /// Where the facts read from the code are kept.
    pub store: StoreLimits,
    /// What the performance tests hold the code to.
    pub perf: PerfLimits,
    /// The file watcher.
    pub watch: WatchLimits,
    /// The languages' servers.
    pub servers: ServerLimits,
    pub pipeline: PipelineLimits,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerLimits {
    /// Requests sent together before waiting for their answers.
    pub requests_in_flight: usize,
    /// Files whose links one server is asked about at a time.
    pub files_at_once: usize,
    /// A warm server confirms one changed file's links within this.
    pub confirm_ms: u64,
    /// A server not ready by then shows as failed.
    pub ready_s: u64,
    /// A server that ended is started again after this, doubling each time
    /// up to the most.
    pub restart_after_ms: u64,
    pub restart_after_most_ms: u64,
    /// A refused file is asked again after a wait that doubles each time,
    /// up to this.
    pub retry_most_ms: u64,
    /// How often a confirmer looks again when nothing is ready or queued.
    pub poll_ms: u64,
    /// On shutdown, how long a server has to answer, then to exit.
    pub shutdown_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WatchLimits {
    /// Saves this close together are one batch.
    pub debounce_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchLimits {
    /// Hits in an answer when the asker doesn't say.
    pub results: usize,
    /// The most hits in one answer.
    pub most: usize,
    pub text_ms: u64,
    pub text_per_file: usize,
    pub text_max_file_kib: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoreLimits {
    pub synchronous: String,
    pub cache_kib: i64,
    /// A file changed more recently than this when read is read again next time.
    pub recent_change_ms: u64,
    /// A busy connection tries again after this (µs), until this (ms).
    pub busy_retry_us: u64,
    pub busy_give_up_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipelineLimits {
    /// The share of the cores its parallel work takes (at least one).
    pub cores_share: f64,
    /// How often progress is announced while files are read.
    pub progress_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerfLimits {
    pub full_index_ms: u64,
    /// A one-file change on this repo, until the new graph is visible.
    pub one_file_ms: u64,
    /// A one-file change on a big repo, as a share of its full index.
    pub one_file_share: f64,
    pub tree_ms: u64,
    pub search_ms: u64,
    /// The longest browser frame while panning a big tree.
    pub frame_ms: u64,
}

#[derive(Deserialize)]
struct File {
    limits: toml::Table,
}

impl Limits {
    pub fn defaults() -> Result<Self, String> {
        Self::from_tables(Self::table(DEFAULT_LIMITS)?, toml::Table::new())
    }

    /// The defaults with a project's overrides (only the keys it gives).
    pub fn with_overrides(text: &str) -> Result<Self, String> {
        Self::from_tables(Self::table(DEFAULT_LIMITS)?, Self::table(text)?)
    }

    fn table(text: &str) -> Result<toml::Table, String> {
        toml::from_str::<File>(text).map(|f| f.limits).map_err(|e| format!("limits: {e}"))
    }

    fn from_tables(mut base: toml::Table, overrides: toml::Table) -> Result<Self, String> {
        for (key, value) in overrides {
            match (base.get_mut(&key), value) {
                (Some(toml::Value::Table(inner)), toml::Value::Table(update)) => inner.extend(update),
                (_, value) => {
                    base.insert(key, value);
                }
            }
        }
        let limits: Limits = toml::Value::Table(base).try_into().map_err(|e| format!("limits: {e}"))?;
        // A client holds an answer and the branch it opens; closing other
        // branches can't make room when that alone is over the cap.
        if limits.cards_held < 2 * limits.cards_per_answer {
            return Err(format!(
                "limits: cards_held ({}) must be at least twice cards_per_answer ({}): a tree and the branch opened in it are held together",
                limits.cards_held, limits.cards_per_answer
            ));
        }
        Ok(limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_come_from_config_and_a_project_overrides_some() {
        let built_in = Limits::defaults().unwrap();
        assert!(built_in.cards_per_answer > 0 && built_in.perf.one_file_ms > 0);
        let mine = Limits::with_overrides("[limits]\ncards_per_answer = 50\n[limits.perf]\nsearch_ms = 9\n").unwrap();
        assert_eq!((mine.cards_per_answer, mine.perf.search_ms), (50, 9));
        assert_eq!((mine.tree_ms, mine.perf.full_index_ms), (built_in.tree_ms, built_in.perf.full_index_ms), "the rest is kept");
        assert!(Limits::with_overrides("[limits]\ncards_per_answer = \"many\"\n").is_err());
    }

    #[test]
    fn the_cards_held_always_fit_a_tree_and_the_branch_opened_in_it() {
        let too_few = Limits::with_overrides("[limits]\ncards_per_answer = 400\ncards_held = 700\n").unwrap_err();
        assert!(too_few.contains("cards_held (700) must be at least twice cards_per_answer (400)"), "{too_few}");
        assert!(Limits::with_overrides("[limits]\ncards_per_answer = 400\ncards_held = 800\n").is_ok());
    }
}
