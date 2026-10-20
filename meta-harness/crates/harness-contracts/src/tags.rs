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
    /// Who wrote the note (`user`, or an agent's slot).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub noted_by: String,
    /// What it's on, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<Tagged>,
}

/// How many things a tag is on, in words ("on 3 things").
pub fn on_things(n: usize) -> String {
    if n == 1 { "on 1 thing".into() } else { format!("on {n} things") }
}

/// The theme's tag colours: a tag's `colour` is below this.
pub const TAG_COLOURS: u8 = 8;

/// A subject a tag is on. A tag points: what a thing says is never kept
/// with it, but given by its owner (from its source, or, for a thing its
/// owner doesn't keep for good, from the tags' own logs: `kept`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tagged {
    pub subject: Subject,
    /// Its name, as its owner names it (`terminal t1 lines 120-124`): it
    /// names it back, to tag or untag it. Kept up to date where it is now;
    /// empty: no owner here names it.
    #[serde(default)]
    pub what: String,
    /// When it was tagged (ms since 1970), and who tagged it (`user`, or
    /// an agent's slot).
    pub at_ms: u64,
    #[serde(default)]
    pub by: String,
    /// Where what it says is in the tags' logs, for a thing its owner
    /// doesn't keep (a terminal's output, a chat's message): none for one
    /// that lasts (code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<Kept>,
}

/// Where a thing's text is in the tags' logs: pieces of files, in order
/// (a thing still growing when tagged, a command running, adds pieces).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    pub pieces: Vec<Piece>,
}

/// A piece of a tags' log file: `len` bytes from byte `start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Piece {
    pub file: String,
    pub start: u64,
    pub len: u64,
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

/// How many of a terminal's lines a tag is on at most, at once.
pub const TAGGED_LINES: u64 = 200;

impl Subject {
    /// Why it can't be tagged as it is (lines the wrong way round, too
    /// many of them): none when it can.
    pub fn unfit(&self) -> Option<String> {
        match self {
            Self::TerminalLines { first, last, .. } if last < first => Some("its last line is before its first".into()),
            Self::TerminalLines { first, last, .. } if last - first >= TAGGED_LINES => Some(format!("at most {TAGGED_LINES} lines at once")),
            _ => None,
        }
    }

    /// Whether it is `other`, or has it in it (lines of a terminal have
    /// each of their lines in them).
    pub fn contains(&self, other: &Subject) -> bool {
        match (self, other) {
            (Self::TerminalLines { terminal, shell, first, last }, Self::TerminalLines { terminal: t, shell: s, first: f, last: l }) => terminal == t && shell == s && first <= f && l <= last,
            _ => self == other,
        }
    }
}

/// Whether `name` can be a tag's: letters, digits, `-` and `_` (40 at
/// most, ending in a letter or digit), so `#name` finds it in a prompt
/// (punctuation after it isn't part of it) and it goes in a link as it
/// is; and never shaped like a tag's id (a uuid), so it's never mistaken
/// for one.
pub fn tag_name_ok(name: &str) -> bool {
    let uuid = name.len() == 36 && name.char_indices().all(|(at, c)| if [8, 13, 18, 23].contains(&at) { c == '-' } else { c.is_ascii_hexdigit() });
    let ends = name.ends_with(|c: char| c.is_ascii_alphanumeric());
    ends && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !uuid
}

/// What a tag is on, and each as it is now.
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
    /// Where what it says came from.
    pub held: Held,
    /// What it says (some of it, when it says a lot: `lines` says how
    /// much there is): none when it's gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// How many lines it says in all.
    #[serde(default)]
    pub lines: u64,
}

/// Where what a tagged thing says comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Held {
    /// Its source: it's still there.
    Source,
    /// The tags' logs: its source is gone (scrolled away, a shell ended),
    /// what it said when tagged is kept.
    Kept,
    /// Nowhere: it's gone, and nothing was kept (or no owner here).
    Gone,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_contain_their_lines_and_names_are_checked() {
        let lines = |first, last| Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first, last };
        assert!(lines(10, 12).contains(&lines(11, 11)) && lines(10, 12).contains(&lines(10, 12)));
        assert!(!lines(10, 12).contains(&lines(12, 13)) && !lines(11, 11).contains(&lines(10, 12)));
        assert!(!lines(10, 12).contains(&Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 2, first: 11, last: 11 }), "another start's");
        let command = Subject::TerminalCommand { terminal: TerminalId::new("t1"), shell: 1, command: 3 };
        assert!(command.contains(&command.clone()) && !lines(0, 99).contains(&command));
        assert_eq!((lines(1, 200).unfit(), lines(1, 201).unfit().is_some(), lines(5, 4).unfit().is_some()), (None, true, true), "200 lines at most, the right way round");
        for ok in ["flaky-build", "a_1", "X"] {
            assert!(tag_name_ok(ok), "{ok}");
        }
        for bad in ["", "two words", "dots.too", "a/b", "café", "wip_", "build-", "0b8f8a9e-3c1d-4f6e-9a2b-7c5d4e3f2a1b", &"x".repeat(41)] {
            assert!(!tag_name_ok(bad), "{bad}");
        }
    }
}
