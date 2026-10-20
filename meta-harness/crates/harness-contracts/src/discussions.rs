//! Discussions: a question, or a change asked for, on one thing in the
//! project (`Subject`), and the back-and-forth on it: messages, an answer
//! (a change request's says what it changed), settled or reopened by the
//! user. Whose move it is follows from where it stands. A project server's.

use serde::{Deserialize, Serialize};

use crate::ids::DiscussionId;
use crate::tags::{Kept, Subject};

/// What a discussion asks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscussionKind {
    /// Answered when it's answered (nothing has to change).
    #[default]
    Question,
    /// Something to change: its answer says what was changed.
    ChangeRequest,
}

impl DiscussionKind {
    pub fn words(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::ChangeRequest => "change request",
        }
    }
}

/// Where a discussion stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscussionStatus {
    /// Waiting for an answer (started, or reopened).
    #[default]
    Open,
    /// Answered: waiting for the user to settle or reopen it.
    Answered,
    /// The user settled it.
    Settled,
}

/// What a message in a discussion is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaidKind {
    #[default]
    Message,
    /// An answer (a change request's names what it changed).
    Answer,
    /// The user settled it (what they said with it, if anything).
    Settled,
    /// The user reopened it: why.
    Reopened,
}

/// A thing a discussion names (what it's on, what an answer changed): where
/// it is, and its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Named {
    pub subject: Subject,
    /// Its name when it was named (`Subject::name`).
    #[serde(default)]
    pub what: String,
}

/// One message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Said {
    /// `user`, or an agent's slot.
    pub by: String,
    pub at_ms: u64,
    #[serde(default)]
    pub kind: SaidKind,
    pub text: String,
    /// An answer's: what it changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<Named>,
}

/// A discussion on a thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discussion {
    pub id: DiscussionId,
    /// What it's on: where it is, its name.
    pub on: Named,
    /// Where what it's on says is kept in the logs (a thing its owner
    /// doesn't keep for good: a terminal's output, a chat's turn).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<Kept>,
    #[serde(default)]
    pub kind: DiscussionKind,
    /// Who started it (`user`, or an agent's slot).
    pub by: String,
    /// The agent slot asked to answer it (`lead` unless another was named).
    pub asked: String,
    #[serde(default)]
    pub status: DiscussionStatus,
    /// Its messages, oldest first (the first: what was asked).
    pub said: Vec<Said>,
}

/// Whose move it is in a discussion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Move {
    /// The agent slot asked (it's open).
    Agent(String),
    /// The user (it's answered: settle or reopen it).
    User,
    /// No one's (settled).
    Nobody,
}

impl Discussion {
    /// Whose move it is: the agent asked while it's open, the user once it's
    /// answered, no one's once settled.
    pub fn whose_move(&self) -> Move {
        match self.status {
            DiscussionStatus::Open => Move::Agent(self.asked.clone()),
            DiscussionStatus::Answered => Move::User,
            DiscussionStatus::Settled => Move::Nobody,
        }
    }

    /// What was asked: its first message's text.
    pub fn asking(&self) -> &str {
        self.said.first().map_or("", |s| s.text.as_str())
    }
}
