//! The architecture service: plans laid over the code, kept honest. Plan and
//! index storage, incremental ingestion, plan operations and checks,
//! user-only approvals, contract slices, and implementation verification.

mod chain;
mod checks;
mod contract;
mod import;
mod limits;
mod perf;
mod pipeline;
mod ops;
mod progress;
mod queue;
mod review;
mod servers;
mod service;
mod store;
mod tools;
mod verify;

pub use contract::{ContractSlice, EffectiveData, FlowNeighbor, HeldInvariant, Neighbor};
pub use import::{ImportReport, ImportedComponent, Unmapped};
pub use limits::{Limits, PerfLimits, PipelineLimits, SearchLimits, ServerLimits, StoreLimits, WatchLimits};
pub use perf::{GraphSize, PerfReport, StageTime, StoreSize, Timed};
pub use arch_search::{Found, Hit, SearchKind};
pub use ops::PlanOp;
pub use queue::{BriefContext, Outcome, Queue, QueueConfig, QueueSettings, job_for, render_brief};
pub use progress::{Action, Downgrade, cascade, next_actionable, topological};
pub use arch_graph::ComponentDetail;
pub use review::{ReviewNote, ReviewProvider, ReviewRequest, ReviewRun, ReviewerCatalog, ReviewerDef, ReviewerKind, ReviewsReport, Rule};
pub use service::{Actor, ArchitectureService, Blocker, IngestReport, Refresh, Reverified, Verification, affected, only_changes};
pub use tools::{Invocation, LogicChecks, ProcessRunner, ToolCatalog, ToolDef, ToolOutput, ToolRun, ToolRunner, ToolsReport};
pub use store::{FsPlans, MemoryPlans, Mover, PlanRepository, cache_dir, open_facts, private_dirs, state_dir, store_settings, temporary_facts};
pub use arch_store::SqliteStore;
pub use chain::chain;
pub use checks::{CheckCx, blocking_for, check_plan, is_data_type, missing_details, schema_set, stage_gate};
pub use verify::signature_diff;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ServiceError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    InvalidOp(String),
    #[error("not allowed: {0}")]
    Forbidden(String),
    #[error("io: {0}")]
    Io(String),
    #[error("corrupt: {0}")]
    Corrupt(String),
    #[error("index: {0}")]
    Index(String),
    #[error("internal lock poisoned")]
    Poisoned,
}

impl From<arch_store::StoreError> for ServiceError {
    fn from(e: arch_store::StoreError) -> Self {
        Self::Io(e.to_string())
    }
}

impl ServiceError {
    fn poisoned<T>(_: T) -> Self {
        Self::Poisoned
    }
}
#[cfg(test)]
pub(crate) use test_packs::shipped_packs;

#[cfg(test)]
mod test_packs {
    /// The language packs shipped in this repo, without any project's or
    /// user's own (tests must not depend on the machine).
    pub fn shipped_packs() -> code_analyzer::PackSet {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
        let places = nah_packs::Places::isolated(&std::env::temp_dir().join(format!("nah-test-packs-{}", std::process::id())));
        let catalog = nah_packs::Catalog::load(&nah_packs::Distribution::from_dir(&dir).unwrap(), &places).unwrap();
        code_analyzer::PackSet::from_catalog(&catalog).unwrap()
    }
}
