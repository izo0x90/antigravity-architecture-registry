use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod arch;
pub use arch::*;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCursor {
    #[serde(alias = "session_id")]
    pub session_id: String,
    pub model: String,
    #[serde(alias = "runtime_mode")]
    pub runtime_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    #[serde(alias = "session_id")]
    pub session_id: String,
    pub model: String,
    #[serde(alias = "runtime_mode")]
    pub runtime_mode: String,
    pub cwd: String,
    #[serde(alias = "resume_cursor")]
    pub resume_cursor: SessionCursor,
}

/// Stream kind for content deltas matching T3 protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    ReasoningText,
    AssistantText,
}

/// Turn completion states
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Completed,
    Cancelled,
    Failed,
}

/// Tool call state inside ACP / Antigravity updates
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallState {
    #[serde(alias = "tool_call_id")]
    pub tool_call_id: String,
    pub kind: String,
    pub status: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    #[serde(default, alias = "is_mcp")]
    pub is_mcp: bool,
    #[serde(default, alias = "raw_output")]
    pub raw_output: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default, alias = "session_update")]
    pub session_update: Option<String>,
}

/// Permission option offered by native runtime
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    #[serde(alias = "option_id")]
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

/// Inbound permission request from native runtime
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    #[serde(alias = "session_id")]
    pub session_id: String,
    #[serde(alias = "tool_call")]
    pub tool_call: ToolCallState,
    pub options: Vec<PermissionOption>,
}

/// Resolution outcome for permission requests
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PermissionOutcome {
    Selected {
        #[serde(rename = "optionId", alias = "option_id")]
        option_id: String,
    },
    Cancelled,
}

/// Inbound events produced by native ACP runtime
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum NativeEvent {
    ThoughtDelta {
        text: String,
    },
    ContentDelta {
        text: String,
    },
    ToolCallUpdated {
        #[serde(rename = "toolCall", alias = "tool_call")]
        tool_call: ToolCallState,
    },
    AvailableCommandsUpdated {
        commands: Vec<String>,
    },
    ConnectionTerminated {
        detail: String,
    },
}

/// Outbound approval option presented to client
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalOption {
    pub decision: String,
    pub label: String,
}

/// Question option presented for user input interactions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionSpec {
    pub allow_custom_answer: bool,
    pub options: Vec<QuestionChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionChoice {
    pub value: String,
    pub label: String,
}

/// Canonical provider events emitted by the Adapter to client listeners
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ProviderRuntimeEvent {
    #[serde(rename = "content.delta")]
    ContentDelta {
        turn_id: String,
        payload: ContentDeltaPayload,
    },
    #[serde(rename = "item.completed")]
    ItemCompleted {
        turn_id: String,
        payload: ItemCompletedPayload,
    },
    #[serde(rename = "request.opened")]
    RequestOpened {
        request_id: String,
        payload: RequestOpenedPayload,
    },
    #[serde(rename = "user-input.requested")]
    UserInputRequested {
        request_id: String,
        payload: UserInputPayload,
    },
    #[serde(rename = "user-input.resolved")]
    UserInputResolved {
        request_id: String,
    },
    #[serde(rename = "turn.started")]
    TurnStarted {
        turn_id: String,
        payload: TurnStartedPayload,
    },
    #[serde(rename = "turn.completed")]
    TurnCompleted {
        turn_id: String,
        payload: TurnCompletedPayload,
    },
    #[serde(rename = "request.resolved")]
    RequestResolved {
        request_id: String,
    },
    #[serde(rename = "task.started")]
    TaskStarted {
        turn_id: String,
        payload: TaskPayload,
    },
    #[serde(rename = "task.progress")]
    TaskProgress {
        turn_id: String,
        payload: TaskPayload,
    },
    #[serde(rename = "task.updated")]
    TaskUpdated {
        turn_id: String,
        payload: TaskPayload,
    },
    #[serde(rename = "task.completed")]
    TaskCompleted {
        turn_id: String,
        payload: TaskPayload,
    },
    #[serde(rename = "session.exited")]
    SessionExited {
        thread_id: String,
    },
    #[serde(rename = "item.updated")]
    ItemUpdated {
        turn_id: String,
        payload: ItemCompletedPayload,
    },
    #[serde(rename = "architecture.focused")]
    ArchitectureFocused {
        root_id: String,
        depth: usize,
        direction: String,
        affected_components: Vec<String>,
    },
    #[serde(rename = "architecture.updated")]
    ArchitectureUpdated {
        component_id: String,
        status: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub thread_id: String,
    pub status: String,
    pub active_turn_id: Option<String>,
    pub model: String,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPayload {
    pub task_id: String,
    pub task_type: String,
    pub title: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_bypass: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentDeltaPayload {
    pub stream_kind: StreamKind,
    pub delta: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemCompletedPayload {
    pub item_type: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestOpenedPayload {
    pub tool_call: ToolCallState,
    pub options: Vec<ApprovalOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInputPayload {
    pub tool_call_id: String,
    pub questions: Vec<QuestionSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCompletedPayload {
    pub state: TurnState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Client approval decisions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecision {
    Accept,
    Decline,
    Cancel,
}

/// Client answers to user input questions
pub type UserInputAnswers = HashMap<String, String>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStartedPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSessionInput {
    pub thread_id: String,
    pub cwd: String,
    pub runtime_mode: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub resume_cursor: Option<SessionCursor>,
    #[serde(default)]
    pub resume_session_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendTurnInput {
    pub thread_id: String,
    pub input: String,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendTurnResult {
    pub turn_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResponse {
    #[serde(alias = "stop_reason")]
    pub stop_reason: String,
}

pub fn antigravity_permission_mode(runtime_mode: &str) -> &'static str {
    match runtime_mode {
        "full_access" | "full-access" | "yolo" => "yolo",
        "auto_accept_edits" | "auto-accept-edits" | "auto_edit" => "auto_edit",
        _ => "default",
    }
}

pub fn antigravity_approval_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            decision: "accept".to_string(),
            label: "Allow once".to_string(),
        },
        ApprovalOption {
            decision: "decline".to_string(),
            label: "Deny".to_string(),
        },
        ApprovalOption {
            decision: "cancel".to_string(),
            label: "Cancel".to_string(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Driver & Snapshot Shared Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSnapshot {
    pub status: String,
    pub auth_type: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSnapshot {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub is_legacy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashCommandSnapshot {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverSnapshot {
    pub status: String,
    pub installed: bool,
    pub version: Option<String>,
    pub auth: AuthSnapshot,
    pub models: Vec<ModelSnapshot>,
    pub slash_commands: Vec<SlashCommandSnapshot>,
    pub supports_text_generation: bool,
}
