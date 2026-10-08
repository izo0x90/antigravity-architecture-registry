//! The architecture service: plans laid over the code, kept honest. Plan and
//! index storage, incremental ingestion, plan operations and checks,
//! user-only approvals, contract slices, and implementation verification.

mod chain;
mod checks;
mod contract;
mod import;
mod limits;
mod ops;
mod progress;
mod queue;
mod review;
mod service;
mod store;
mod tools;
mod verify;

pub use contract::{ContractSlice, EffectiveData, FlowNeighbor, HeldInvariant, Neighbor};
pub use import::{ImportReport, ImportedComponent, Unmapped};
pub use limits::{Limits, PerfLimits, StoreLimits};
pub use ops::PlanOp;
pub use queue::{BriefContext, Outcome, Queue, QueueConfig, QueueSettings, job_for, render_brief};
pub use progress::{Action, Downgrade, cascade, next_actionable, topological};
pub use arch_graph::ComponentDetail;
pub use review::{ReviewNote, ReviewProvider, ReviewRequest, ReviewRun, ReviewerCatalog, ReviewerDef, ReviewerKind, ReviewsReport, Rule};
pub use service::{Actor, ArchitectureService, IngestReport, NewComment, Refresh, Reverified, Verification, affected, approval_blocker};
pub use tools::{Invocation, LogicChecks, ProcessRunner, ToolCatalog, ToolDef, ToolOutput, ToolRun, ToolRunner, ToolsReport};
pub use store::{FsPlans, MemoryPlans, PlanRepository, open_facts};
pub use arch_store::{FactStore, MemoryStore, SqliteStore};
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
