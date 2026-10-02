//! What an adapter can do. Clients and the server gate behavior on these flags
//! instead of on which harness is running. `Default` is the conservative "nothing".

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub sessions: SessionCapabilities,
    pub threads: ThreadCapabilities,
    pub turns: TurnCapabilities,
    pub streaming: StreamingCapabilities,
    pub tools: ToolCapabilities,
    pub approvals: ApprovalCapabilities,
    pub planning: PlanningCapabilities,
    pub subagents: SubagentCapabilities,
    pub runtime_policy: RuntimePolicyCapabilities,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCapabilities {
    pub model_switch_in_session: bool,
    pub runtime_mode_switch_in_session: bool,
    /// Can reopen a saved native session (`SessionConfig::resume`).
    pub resume: bool,
    /// Can list the harness's saved sessions (`SessionCatalog`).
    pub list_saved: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCapabilities {
    pub read_snapshot: bool,
    pub rollback: bool,
    pub fork: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCapabilities {
    pub interrupt: bool,
    pub active_steering: bool,
    pub queued_messages: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamingCapabilities {
    pub assistant_text: bool,
    pub reasoning: bool,
    pub tool_output: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCapabilities {
    pub mcp_tools: bool,
    pub client_file_system: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalCapabilities {
    pub command_approval: bool,
    pub file_change_approval: bool,
    /// Requests can only be answered while the session that raised them is alive.
    pub live_only: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanningCapabilities {
    pub todo_list: bool,
    pub proposed_plan: bool,
    pub structured_questions: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentCapabilities {
    pub supported: bool,
    pub lifecycle_events: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePolicyCapabilities {
    pub enforcement: PolicyEnforcement,
}

/// Where runtime modes are enforced: by the harness itself, or only at the points
/// the app mediates (permission requests, client file system).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyEnforcement {
    Native,
    #[default]
    ClientBoundary,
}
