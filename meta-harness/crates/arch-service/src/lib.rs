//! The architecture service: plans laid over the code, kept honest. Plan and
//! index storage, incremental ingestion, plan operations and checks,
//! user-only approvals, contract slices, and implementation verification.

mod checks;
mod contract;
mod ops;
mod service;
mod store;
mod verify;

pub use contract::{ContractSlice, Neighbor};
pub use ops::PlanOp;
pub use service::{Actor, ArchitectureService, ComponentDetail, IngestReport, Verification, ViewResult};
pub use store::{CodeIndexStore, FsIndex, FsPlans, MemoryIndex, MemoryPlans, PlanRepository};
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
