//! Things the harness is waiting on: approvals and structured questions. Options
//! come from the harness (an adapter may synthesize them when it has none); the
//! client shows them as-is and answers with the chosen option.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{ItemId, RequestId, ThreadId, TurnId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRequest {
    pub id: RequestId,
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    /// The tool-call item this request is about, if any.
    pub item_id: Option<ItemId>,
    pub kind: RequestKind,
    pub status: RequestStatus,
    pub title: String,
    pub detail: Option<String>,
    pub options: Vec<ApprovalOption>,
    pub questions: Vec<Question>,
    pub response: Option<RequestResponse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    Permission,
    Command,
    FileRead,
    FileChange,
    UserInput,
    McpElicitation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    Pending,
    Resolved,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    AcceptAlways,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalOption {
    /// Native option id, passed back verbatim in the response.
    pub id: String,
    pub decision: ApprovalDecision,
    pub label: String,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
    pub allow_custom_answer: bool,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionOption {
    pub label: String,
    pub description: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestResponse {
    /// Chosen `ApprovalOption::id`.
    pub option_id: Option<String>,
    /// Answers by `Question::id`.
    #[serde(default)]
    pub answers: BTreeMap<String, Vec<String>>,
}
