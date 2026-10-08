//! How long things take, stage by stage. Every answer the service gives
//! (a tree, a search, an index run) is timed through `Timing`: each stage
//! runs in a tracing span and its time is kept. The last few answers are
//! kept for `/api/perf`; one slower than the limit is logged with what was
//! asked.

use std::collections::VecDeque;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// One stage's time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageTime {
    pub stage: String,
    pub ms: f64,
}

/// One answer, timed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timed {
    /// tree, expand, context, card, search, ingest.
    pub what: String,
    /// What was asked, in words.
    pub asked: String,
    /// When it finished (RFC 3339).
    pub at: String,
    pub ms: f64,
    pub stages: Vec<StageTime>,
    /// How big the answer was (cards, hits, bytes...), with its unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<(u64, String)>,
    /// Over the slow limit.
    pub slow: bool,
}

/// Times one answer; hand it back with `ArchitectureService::finish`.
pub struct Timing {
    what: &'static str,
    asked: String,
    started: Instant,
    stages: Vec<StageTime>,
    size: Option<(u64, String)>,
}

impl Timing {
    pub fn start(what: &'static str, asked: impl Into<String>) -> Self {
        Self { what, asked: asked.into(), started: Instant::now(), stages: Vec::new(), size: None }
    }

    /// Runs one stage (in a tracing span) and keeps its time.
    pub fn stage<T>(&mut self, stage: &'static str, run: impl FnOnce() -> T) -> T {
        let span = tracing::info_span!("stage", what = self.what, stage);
        let _in = span.enter();
        let started = Instant::now();
        let out = run();
        self.stages.push(StageTime { stage: stage.to_owned(), ms: ms(started) });
        out
    }

    /// How big the answer was.
    pub fn size(&mut self, count: usize, unit: &str) {
        self.size = Some((count as u64, unit.to_owned()));
    }

    /// Time so far.
    pub fn elapsed_ms(&self) -> f64 {
        ms(self.started)
    }

    pub(crate) fn done(self, slow_ms: u64, at: String) -> Timed {
        let total = ms(self.started);
        Timed { what: self.what.to_owned(), asked: self.asked, at, ms: total, stages: self.stages, size: self.size, slow: total > slow_ms as f64 }
    }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// The last answers, newest last.
#[derive(Default)]
pub(crate) struct Recent {
    kept: VecDeque<Timed>,
    /// The last index run (kept apart: requests would push it out).
    pub(crate) last_index: Option<Timed>,
}

impl Recent {
    pub(crate) fn push(&mut self, timed: Timed, keep: usize) {
        if timed.what == "ingest" {
            self.last_index = Some(timed.clone());
        }
        self.kept.push_back(timed);
        while self.kept.len() > keep {
            self.kept.pop_front();
        }
    }

    pub(crate) fn list(&self) -> Vec<Timed> {
        self.kept.iter().cloned().collect()
    }
}

/// What `/api/perf` shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerfReport {
    /// The last answers, newest last.
    pub recent: Vec<Timed>,
    pub last_index: Option<Timed>,
    pub graph: GraphSize,
    pub store: StoreSize,
    /// The server process's resident memory.
    pub memory_bytes: Option<u64>,
    /// The limits answers are held to.
    pub slow_request_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphSize {
    pub components: usize,
    pub relations: usize,
    /// Plans laid over the code and kept built.
    pub overlays: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoreSize {
    /// Source files whose facts are held.
    pub files: usize,
    /// The store on disk (0 when it's in memory).
    pub bytes: u64,
}
