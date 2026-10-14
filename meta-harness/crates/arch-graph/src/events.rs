//! Architecture events pushed to clients (the cockpit refreshes on them; the
//! agent's navigation requests reach the UI through them).

use serde::{Deserialize, Serialize};

use crate::{ComponentId, PlanId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReason {
    Ingested,
    PlanEdited,
    Approved,
    StageChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requester {
    Agent,
    User,
}

/// What to show (the agent pointing the user's graph somewhere): a view,
/// the code or a plan, a root and a way, a depth, a card to select. What
/// isn't given stays as the user had it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<crate::tree::ViewKind>,
    /// The code, or a plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::Source>,
    /// The card to start from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub way: Option<crate::tree::Way>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// The card to select (and point at) once it shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<String>,
    /// Which of the user's columns shows it, counted from 1 on the left
    /// (made a graph if it isn't one); none: the graph they used last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArchEvent {
    /// Code or plans changed; views showing these should refresh.
    Changed {
        reason: ChangeReason,
        #[serde(default)]
        plans: Vec<PlanId>,
        #[serde(default)]
        components: Vec<ComponentId>,
    },
    /// A plan check or implementation verification ran.
    Validated {
        plan: PlanId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        component: Option<ComponentId>,
        errors: usize,
        warnings: usize,
    },
    /// Show something in the graph (usually the agent driving the view).
    ViewRequested { request: Box<ViewRequest>, by: Requester },
    /// The work queue over a plan moved (a job started, finished, waits).
    QueueUpdated { snapshot: Box<crate::QueueSnapshot> },
    /// Code indexing started, moved on, finished or failed, or files changed
    /// on disk since they were read.
    Index { status: Box<crate::IndexStatus> },
}
