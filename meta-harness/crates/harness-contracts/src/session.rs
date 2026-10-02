//! Threads, turns, subagents, usage, and the inputs that drive them.

use serde::{Deserialize, Serialize};

use crate::ids::{InstanceId, ItemId, NativeId, SubagentId, ThreadId, TurnId};
use crate::policy::{ModelSelection, RuntimePolicy};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: ThreadId,
    pub instance_id: InstanceId,
    pub native_id: Option<NativeId>,
    pub title: Option<String>,
    pub status: ThreadStatus,
    pub policy: RuntimePolicy,
    pub model: Option<ModelSelection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Starting,
    Idle,
    Running,
    WaitingForInput,
    Closed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub id: TurnId,
    pub thread_id: ThreadId,
    pub ordinal: u32,
    pub status: TurnStatus,
    pub failure: Option<Failure>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Running,
    Completed,
    Interrupted,
    Cancelled,
    Failed,
}

impl TurnStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub context_used_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subagent {
    pub id: SubagentId,
    pub thread_id: ThreadId,
    /// The tool-call item that spawned it, if any.
    pub parent_item_id: Option<ItemId>,
    pub title: String,
    pub description: Option<String>,
    pub status: SubagentStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Running,
    Idle,
    Completed,
    Failed,
    Cancelled,
}

/// Everything needed to open a session on an adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConfig {
    pub thread_id: ThreadId,
    pub policy: RuntimePolicy,
    pub model: Option<ModelSelection>,
    /// Native session to resume instead of starting fresh.
    pub resume: Option<NativeId>,
    /// Tool servers the harness should connect to; the adapter decides delivery.
    pub mcp_servers: Vec<McpServer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub name: String,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum McpTransport {
    Http { url: String, headers: Vec<(String, String)> },
    Stdio { command: String, args: Vec<String>, env: Vec<(String, String)> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInput {
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Attachment {
    File { path: String },
    Image { mime_type: String, data_base64: String },
}

/// Mid-session changes; each field is applied only if set and supported.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerInput {
    pub policy: Option<RuntimePolicy>,
    pub model: Option<ModelSelection>,
}
