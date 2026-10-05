//! The architecture service: plans laid over the code, kept honest. Plan and
//! index storage, incremental ingestion, plan operations and checks,
//! user-only approvals, contract slices, and implementation verification.

mod chain;
mod checks;
mod contract;
mod import;
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
pub use ops::PlanOp;
pub use queue::{BriefContext, Outcome, Queue, QueueConfig, QueueSettings, job_for, render_brief};
pub use progress::{Action, Downgrade, cascade, next_actionable, topological};
pub use arch_graph::{ComponentDetail, ViewResult};
pub use review::{ReviewNote, ReviewProvider, ReviewRequest, ReviewRun, ReviewerCatalog, ReviewerDef, ReviewerKind, ReviewsReport, Rule};
pub use service::{Actor, ArchitectureService, IngestReport, NewComment, Refresh, Reverified, Verification, affected, approval_blocker};
pub use tools::{Invocation, LogicChecks, ProcessRunner, ToolCatalog, ToolDef, ToolOutput, ToolRun, ToolRunner, ToolsReport};
pub use store::{CodeIndexStore, FsIndex, FsPlans, MemoryIndex, MemoryPlans, PlanRepository};
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

impl ServiceError {
    fn poisoned<T>(_: T) -> Self {
        Self::Poisoned
    }
}
