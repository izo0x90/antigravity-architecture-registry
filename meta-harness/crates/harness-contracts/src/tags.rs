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

/// A subject a tag is on, with what it said when tagged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tagged {
    pub subject: Subject,
    /// Its name, as its owner names it (`terminal t1 lines 120-124`): it
    /// names it back, to tag or untag it. Kept up to date where it is now;
    /// empty: no owner here names it.
    #[serde(default)]
    pub what: String,
    /// What it said when tagged (some of it, when it said a lot).
    pub said: String,
    /// When it was tagged (ms since 1970), and who tagged it (`user`, or
    /// an agent's slot).
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

impl Subject {
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
/// most), so `#name` finds it in a prompt (punctuation after it isn't
/// part of it) and it goes in a link as it is; and never shaped like a
/// tag's id (a uuid), so it's never mistaken for one.
pub fn tag_name_ok(name: &str) -> bool {
    let uuid = name.len() == 36 && name.char_indices().all(|(at, c)| if [8, 13, 18, 23].contains(&at) { c == '-' } else { c.is_ascii_hexdigit() });
    !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !uuid
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
    /// What it says now (some of it, when it says a lot): none when it's
    /// gone, or when no owner here can say (`tagged.what` is empty then).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now: Option<String>,
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
        for ok in ["flaky-build", "a_1", "X"] {
            assert!(tag_name_ok(ok), "{ok}");
        }
        for bad in ["", "two words", "dots.too", "a/b", "café", "0b8f8a9e-3c1d-4f6e-9a2b-7c5d4e3f2a1b", &"x".repeat(41)] {
            assert!(!tag_name_ok(bad), "{bad}");
        }
    }
}
