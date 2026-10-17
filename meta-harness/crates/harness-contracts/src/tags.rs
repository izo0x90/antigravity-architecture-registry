//! Tags: names the user (or an agent) gives things in the project, to find
//! them again and point agents at them. A tag is on subjects: durable
//! references to one thing each (some lines of a terminal, a command it
//! ran...), each kept with what it said when tagged. What a subject says
//! now is its owner's to tell. Tags are a project server's.

use serde::{Deserialize, Serialize};

use crate::ids::{TagId, TerminalId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    /// Stays when it's renamed.
    pub id: TagId,
    /// What it's called (`flaky-build`): no spaces, unique on its server.
    pub name: String,
    /// Which of the theme's tag colours (`tag_0`…`tag_7`).
    pub colour: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// What it's on, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<Tagged>,
}

/// The theme's tag colours: a tag's `colour` is below this.
pub const TAG_COLOURS: u8 = 8;

/// A subject a tag is on, with what it said when tagged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tagged {
    pub subject: Subject,
    pub said: String,
    /// When it was tagged (ms since 1970).
    pub at_ms: u64,
}

/// One thing in the project, durably: what a tag is on. Each kind is its
/// owner's (it says what one says now); kinds are added as owners support
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Subject {
    /// Lines of a terminal (`first` to `last`, by their numbers), in one
    /// start of its shell (`ScreenUpdate::shell`: a new start's lines are
    /// other lines).
    TerminalLines { terminal: TerminalId, shell: u64, first: u64, last: u64 },
    /// A command a terminal's shell ran (`TerminalCommand::id`), in one
    /// start of its shell.
    TerminalCommand { terminal: TerminalId, shell: u64, command: u64 },
}

/// A subject as its owner says it is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubjectNow {
    /// What and where it is, in a few words (`terminal t1, lines 120–124`).
    pub what: String,
    /// What it says now (none: it's gone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now: Option<String>,
}

/// A tag with each subject as it is now (none: its owner can't say, here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagRead {
    pub tag: Tag,
    pub now: Vec<Option<SubjectNow>>,
}
