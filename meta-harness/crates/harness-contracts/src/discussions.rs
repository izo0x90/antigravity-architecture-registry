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
    /// What the text links (the user's: a tag, a thing).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<crate::Link>,
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
    /// Who started it (`user`, or an agent's slot, by its id).
    pub by: String,
    /// Who's asked to answer it: an agent slot, by its id (the lead
    /// unless another was named), or `user` (an agent's question to the
    /// user).
    pub asked: String,
    #[serde(default)]
    pub status: DiscussionStatus,
    /// Its messages, oldest first (the first: what was asked).
    pub said: Vec<Said>,
    /// The agents in it, their slots' ids to their names as they were
    /// when they were in it (a slot closed since is still named).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub names: std::collections::BTreeMap<String, String>,
}

/// What a discussion is on says, as it is now (or as it was kept, once
/// it changed or went): its name, where it is, the start of it, how many
/// lines it has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnNow {
    pub what: String,
    pub held: crate::tags::Held,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub lines: u64,
}

/// Whose move it is in a discussion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Move {
    /// The agent slot asked (it's open).
    Agent(String),
    /// The user (it's answered: settle or reopen it; or it's open, asked
    /// of the user).
    User,
    /// No one's (settled).
    Nobody,
}

impl Discussion {
    /// Whose move it is: while it's open, whoever's asked, or, once the
    /// one asked has spoken last (asked something back), whoever started
    /// it; the user once it's answered; no one's once settled.
    pub fn whose_move(&self) -> Move {
        let who = |name: &str| if name == crate::USER { Move::User } else { Move::Agent(name.to_owned()) };
        match self.status {
            DiscussionStatus::Open if self.said.len() > 1 && self.said.last().is_some_and(|s| s.by == self.asked) => who(&self.by),
            DiscussionStatus::Open => who(&self.asked),
            DiscussionStatus::Answered => Move::User,
            DiscussionStatus::Settled => Move::Nobody,
        }
    }

    /// Who `by` (a slot's id, or `user`) is, by name.
    pub fn name_of<'a>(&'a self, by: &'a str) -> &'a str {
        self.names.get(by).map_or(by, String::as_str)
    }

    /// Who `by` is, as an agent reads it ("the user", a slot's name).
    pub fn to_agent<'a>(&'a self, by: &'a str) -> &'a str {
        by_to_agent(self.name_of(by))
    }

    /// Who `by` is, as the user reads it ("you", a slot's name).
    pub fn to_user<'a>(&'a self, by: &'a str) -> &'a str {
        by_to_user(self.name_of(by))
    }

    /// What was asked: its first message's text.
    pub fn asking(&self) -> &str {
        self.said.first().map_or("", |s| s.text.as_str())
    }
}

/// Who said or did something, as an agent reads it ("the user", a slot's
/// name): one way, wherever it's written to an agent.
pub fn by_to_agent(by: &str) -> &str {
    match by {
        "user" => "the user",
        "" => "someone",
        slot => slot,
    }
}

/// Who said or did something, as the user reads it ("you", a slot's name).
pub fn by_to_user(by: &str) -> &str {
    match by {
        "user" => "you",
        "" => "someone",
        slot => slot,
    }
}

impl SaidKind {
    /// What it was, in words ("answered").
    pub fn words(self) -> &'static str {
        match self {
            Self::Message => "said",
            Self::Answer => "answered",
            Self::Settled => "settled it",
            Self::Reopened => "reopened it",
        }
    }
}

impl DiscussionStatus {
    pub fn words(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Answered => "answered",
            Self::Settled => "settled",
        }
    }
}
