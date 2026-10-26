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
    /// What this session lets you change, as the harness reports it.
    #[serde(default)]
    pub controls: SessionControls,
    /// Tool servers this session was started with.
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
}

/// The session's native modes and every config option the harness exposes
/// (model, thought level, ...), with current values. Rendered generically.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionControls {
    pub modes: Vec<ModeOption>,
    pub current_mode: Option<String>,
    pub config: Vec<ConfigControl>,
}

impl SessionControls {
    /// The config option that selects the model, if any.
    pub fn model_control(&self) -> Option<&ConfigControl> {
        self.config.iter().find(|c| c.category == ConfigCategory::Model)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigControl {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: ConfigCategory,
    pub kind: ConfigKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigCategory {
    Mode,
    Model,
    ModelConfig,
    ThoughtLevel,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ConfigKind {
    Select { choices: Vec<ConfigChoice>, current: String },
    Boolean { current: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
    /// Display group, when the harness groups its choices.
    pub group: Option<String>,
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
    /// Sent while another runs: it starts when those before it end.
    Queued,
    Running,
    Completed,
    Interrupted,
    Cancelled,
    Failed,
}

impl TurnStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
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

/// A sub-agent an agent started: who it is, what it was asked, how it's
/// doing, and what can be done about it. Its steps and messages are items
/// under its own (`item_id`) when the harness says which are its.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subagent {
    pub id: SubagentId,
    pub thread_id: ThreadId,
    /// Its item in the stream: where it started (its steps under it).
    pub item_id: ItemId,
    /// Its name (or its role), as the agent gave it.
    pub title: String,
    /// What kind of sub-agent it is, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Its task, in short.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// What it was last told to do, whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// What it handed back, when its harness sends that apart from its
    /// own messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    pub status: SubagentStatus,
    /// When it started, and ended (as the server saw each: a replayed one,
    /// when it was replayed).
    pub started_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
    /// It can be stopped on its own (its harness says so), the turn going on.
    #[serde(default)]
    pub can_cancel: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Running,
    /// Waiting on you (a request of its own).
    Waiting,
    /// Its work ended, its harness not saying how.
    Idle,
    Completed,
    Failed,
    Cancelled,
}

impl SubagentStatus {
    /// It's still at work (or waiting on you).
    pub fn working(self) -> bool {
        matches!(self, Self::Running | Self::Waiting)
    }
}

/// A session the harness saved itself; reopen it via `SessionConfig::resume`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSession {
    pub native_id: NativeId,
    pub title: Option<String>,
    pub cwd: Option<String>,
    /// RFC 3339 timestamp as reported by the harness.
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSessionPage {
    pub sessions: Vec<SavedSession>,
    pub next_cursor: Option<String>,
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

impl McpServer {
    /// As shown to clients: no secrets. A URL's query (a session's key) is
    /// dropped and header values are hidden.
    pub fn redacted(&self) -> Self {
        let hide = |headers: &[(String, String)]| headers.iter().map(|(name, _)| (name.clone(), "…".to_owned())).collect();
        let url_only = |url: &str| url.split_once('?').map_or(url, |(base, _)| base).to_owned();
        let transport = match &self.transport {
            McpTransport::Http { url, headers } => McpTransport::Http { url: url_only(url), headers: hide(headers) },
            McpTransport::Sse { url, headers } => McpTransport::Sse { url: url_only(url), headers: hide(headers) },
            McpTransport::Stdio { command, args, env } => {
                McpTransport::Stdio { command: command.clone(), args: args.clone(), env: hide(env) }
            }
        };
        Self { name: self.name.clone(), transport }
    }
}

/// The tool servers every session gets, besides any added for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolServers {
    /// The meta-harness's own (its architecture tools).
    pub ours: Vec<McpServer>,
    /// The project's (`.nah/mcp.toml`).
    pub project: Vec<McpServer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum McpTransport {
    Http {
        url: String,
        headers: Vec<(String, String)>,
    },
    /// The older HTTP transport: events on a stream, messages posted back.
    Sse {
        url: String,
        headers: Vec<(String, String)>,
    },
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInput {
    /// What the agent is handed.
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// What its links hand on with it, a piece each (an agent that takes
    /// pieces gets them as such).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pieces: Vec<crate::Piece>,
}

impl TurnInput {
    /// `text` alone.
    pub fn of(text: impl Into<String>) -> Self {
        Self { text: text.into(), attachments: Vec::new(), pieces: Vec::new() }
    }
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
    /// Native mode id from `SessionControls::modes`.
    #[serde(default)]
    pub mode: Option<String>,
    /// Values for `SessionControls::config` entries (string for selects, bool for booleans).
    #[serde(default)]
    pub config: Vec<crate::policy::OptionValue>,
}
