//! Architecture events pushed to clients (the cockpit refreshes on them; the
//! agent's navigation requests reach the UI through them).

use serde::{Deserialize, Serialize};

use crate::{ComponentId, PlanId, ViewSpec};

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

/// What to show: a named view (or a full spec), optionally scoped to a plan,
/// rooted somewhere, with a component selected.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<ViewSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<ComponentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<ComponentId>,
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
}
