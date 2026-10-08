//! The architecture graph's domain model: a language-generic type system,
//! components anchored to code, relations by kind with provenance, plans laid
//! over the code, validation reports, and the three views as trees. Pure data and logic: no I/O,
//! so the server and the WASM cockpit share it.

mod anchor;
mod component;
mod cycles;
mod events;
mod graph;
mod lifecycle;
mod plan;
mod queue;
mod relation;
mod report;
mod source;
pub mod index;
pub mod review;
pub mod schema;
pub mod tree;
mod types;
mod wire;

pub use anchor::{CodeAnchor, ProposedLocation, RepoPath, Span};
pub use events::{ArchEvent, ChangeReason, Requester, ViewRequest};
pub use index::{IndexState, IndexStatus, IndexSummary, IndexerState, IndexerStatus};
pub use component::{Component, ComponentId, ImplementationSpec, Invariant, InvariantKind, LogicStep, Param, Schemas, SideEffect, Signature, Task, ToolRef};
pub use graph::{Alignment, CallChain, ChainStep, Graph};
pub use lifecycle::{Stage, Status, can_transition};
pub use queue::{Job, JobKind, JobState, QueueSnapshot, QueueStatus};
pub use plan::{Approval, ApprovalKind, Plan, REVOKED_BY, PlanId, Reference, SCHEMA_VERSION, Workflow, check_schema_version};
pub use relation::{Provenance, Relation, RelationId, RelationKind};
pub use schema::{Dsl, RefError, SchemaSet};
pub use review::{Answer, Author, Change, Comment, CommentAnchor, CommentId, CommentKind, CommentStatus, PlanDiff, Reply, ReviewSnapshot};
pub use source::Source;
pub use report::{CodeRef, Finding, FindingCode, Report, Severity, Target};
pub use types::{RequiredField, ResolvedType, TypeDef, TypeSystem};
pub use wire::{ComponentDetail, FileView};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ModelError {
    #[error("config: {0}")]
    Config(String),
    #[error("unknown type '{name}' ({context})")]
    UnknownType { name: String, context: String },
    #[error("type inheritance cycle: {0}")]
    TypeCycle(String),
    #[error("invalid repository path '{0}' (must be relative, inside the repo)")]
    InvalidPath(String),
    #[error("invalid id '{0}'")]
    InvalidId(String),
    #[error("component {id}: {reason}")]
    Location { id: ComponentId, reason: String },
    #[error("{what} has schema version {found}; this build reads version {supported}")]
    SchemaVersion { what: String, found: u32, supported: u32 },
    #[error("format: {0}")]
    Format(String),
}
