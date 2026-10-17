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
    /// What it's called (`flaky-build`): letters, digits, `-` and `_`,
    /// unique on its server.
    pub name: String,
    /// Which of the theme's tag colours (`tag_0`…`tag_7`).
    pub colour: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Who wrote the note (`you`, or an agent's slot).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub noted_by: String,
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
    /// What it said when tagged (some of it, when it said a lot).
    pub said: String,
    /// When it was tagged (ms since 1970), and who tagged it (`you`, or an
    /// agent's slot).
    pub at_ms: u64,
    #[serde(default)]
    pub by: String,
}

/// One thing in the project, durably: what a tag is on. Each kind is its
/// owner's (it says what one says now, and names it); kinds are added as
/// owners support them, so whatever reads one allows for kinds it doesn't
/// know.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
#[non_exhaustive]
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
    /// Its name, as its owner names it (`terminal t1 lines 120-124`): it
    /// names it back, to tag or untag it.
    pub what: String,
    /// What it says now (none: it's gone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now: Option<String>,
}

/// What a tag is on, and each as it is now (none: no owner here can say).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagRead {
    pub tag: Tag,
    pub on: Vec<TaggedNow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaggedNow {
    pub tagged: Tagged,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now: Option<SubjectNow>,
}
