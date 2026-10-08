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
    /// Time for building and laying out one tree.
    pub tree_ms: u64,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerfLimits {
    pub full_index_ms: u64,
    pub one_file_ms: u64,
    pub tree_ms: u64,
    pub search_ms: u64,
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
        toml::Value::Table(base).try_into().map_err(|e| format!("limits: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // req: M.1
    #[test]
    fn limits_come_from_config_and_a_project_overrides_some() {
        let built_in = Limits::defaults().unwrap();
        assert!(built_in.cards_per_answer > 0 && built_in.perf.one_file_ms > 0);
        let mine = Limits::with_overrides("[limits]\ncards_per_answer = 50\n[limits.perf]\nsearch_ms = 9\n").unwrap();
        assert_eq!((mine.cards_per_answer, mine.perf.search_ms), (50, 9));
        assert_eq!((mine.tree_ms, mine.perf.full_index_ms), (built_in.tree_ms, built_in.perf.full_index_ms), "the rest is kept");
        assert!(Limits::with_overrides("[limits]\ncards_per_answer = \"many\"\n").is_err());
    }
}
