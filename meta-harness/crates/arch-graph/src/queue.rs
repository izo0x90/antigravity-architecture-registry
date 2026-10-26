//! The work queue NAH runs over a plan: one job per component and stage,
//! in dependency order, each in its own agent session. These are the shared
//! shapes the server publishes and the cockpit shows.

use serde::{Deserialize, Serialize};

use crate::{ComponentId, Finding, PlanId, Stage};

/// What a session is asked to do for a component at its current stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Declared: write the spec; then the user approves the architecture.
    Design,
    /// Architecture approved: detail the implementation plan; then the user approves it.
    Plan,
    /// Plan approved: lay the structure with placeholder bodies.
    Skeleton,
    /// Fill the bodies until verification passes.
    Implement,
}

impl JobKind {
    pub const ALL: [JobKind; 4] = [JobKind::Design, JobKind::Plan, JobKind::Skeleton, JobKind::Implement];

    pub fn name(self) -> &'static str {
        match self {
            Self::Design => "design",
            Self::Plan => "plan",
            Self::Skeleton => "skeleton",
            Self::Implement => "implement",
        }
    }

    /// The stage the job aims for: reached by user approval (design, plan)
    /// or by verification (skeleton, implement).
    pub fn goal(self) -> Stage {
        match self {
            Self::Design => Stage::ArchApproved,
            Self::Plan => Stage::PlanApproved,
            Self::Skeleton => Stage::Skeleton,
            Self::Implement => Stage::Implemented,
        }
    }

    /// Whether the user approves the result (otherwise verification moves it).
    pub fn needs_approval(self) -> bool {
        matches!(self, Self::Design | Self::Plan)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobState {
    /// Waiting its turn (or for what it depends on).
    Queued,
    Running {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread: Option<String>,
    },
    /// Done; the user's approval moves it on.
    AwaitingApproval,
    /// The last attempt didn't check out; it runs again with the findings.
    Retrying,
    /// Out of attempts: needs the user (retry or change the plan).
    Failed,
    Done,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub component: ComponentId,
    pub kind: JobKind,
    pub state: JobState,
    /// Attempts made at this kind of job for this component.
    pub attempts: u32,
    /// Why the last attempt didn't pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStatus {
    Idle,
    Running,
    /// Nothing can run until the user approves something (or retries).
    WaitingForUser,
    /// Asked to stop; finishes the running job first.
    Stopping,
    /// Every component is implemented.
    Done,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueueSnapshot {
    pub plan: PlanId,
    pub status: QueueStatus,
    /// Every component's next job, in dependency order.
    pub jobs: Vec<Job>,
}
