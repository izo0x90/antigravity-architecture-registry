//! Turn items: the units a client renders inside a turn. Adapters translate native
//! output into these; anything they cannot classify becomes `Unknown` with display
//! text, so nothing is silently dropped.

use serde::{Deserialize, Serialize};

use crate::ids::{ItemId, NativeId, PlanId, RequestId, SubagentId, ThreadId, TurnId};
use crate::plan::PlanStep;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnItem {
    pub id: ItemId,
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub native_id: Option<NativeId>,
    pub parent_id: Option<ItemId>,
    pub ordinal: u32,
    pub status: ItemStatus,
    pub title: Option<String>,
    pub body: ItemBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ItemBody {
    /// What you sent, as the agent was handed it (`text`), and as you
    /// typed it, when that's another: its own text and links (what was
    /// handed on with it follows it in `text`).
    UserMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        typed: Option<crate::Message>,
    },
    AssistantMessage { content: Vec<ContentBlock>, streaming: bool },
    Reasoning { text: String, streaming: bool },
    ToolCall {
        kind: ToolKind,
        name: String,
        input: Option<serde_json::Value>,
        output: Vec<ContentBlock>,
        locations: Vec<FileLocation>,
    },
    FileChange { path: String, diff: Option<Diff> },
    CommandExecution { command: String, output: Option<String>, exit_code: Option<i32> },
    ProposedPlan { plan_id: PlanId, markdown: String },
    TodoList { plan_id: PlanId, steps: Vec<PlanStep> },
    /// The item a request (approval/question) is shown in.
    Request { request_id: RequestId },
    Subagent { subagent_id: SubagentId },
    Notice { level: NoticeLevel, text: String },
    Error { message: String },
    /// Native output the adapter could not classify, rendered as text.
    Unknown { native_kind: String, display: String },
    /// A body type newer than this client understands.
    #[serde(other)]
    Unsupported,
}

impl ItemBody {
    /// What a message says, put together one way for every reader (the
    /// chat, `slot_read`, what's tagged and discussed): your text, or an
    /// answer's text parts a line apart. None: it isn't a message.
    pub fn said(&self) -> Option<String> {
        match self {
            Self::UserMessage { text, .. } => Some(text.clone()),
            Self::AssistantMessage { content, .. } => Some(content.iter().filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None }).collect::<Vec<_>>().join("\n")),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileLocation {
    pub path: String,
    pub line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    pub path: String,
    pub old_text: Option<String>,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ContentBlock {
    Text { text: String },
    Image { mime_type: String, data_base64: Option<String>, uri: Option<String> },
    ResourceLink { uri: String, name: Option<String> },
    Resource { uri: String, text: Option<String> },
    Diff(Diff),
    #[serde(other)]
    Unsupported,
}
