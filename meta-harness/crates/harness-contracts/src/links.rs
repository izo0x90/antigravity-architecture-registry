//! Links: what a message pulls in. A message is its text and its links;
//! each link marks a span of the text (character offsets, `start..end`)
//! and points at a tag or a thing. Nothing reads the text for them: a
//! `#word` or `@word` that isn't a link is text.

use serde::{Deserialize, Serialize};

use crate::{Subject, TagId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub start: usize,
    pub end: usize,
    pub to: Linked,
    #[serde(default)]
    pub include: Include,
}

/// What a link points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Linked {
    /// A tag: what it's on.
    Tag { tag: TagId },
    /// One thing (a file, lines of one, a symbol...).
    Thing { subject: Subject },
}

/// Whether what's linked goes in whole, or as a pointer to read it by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Include {
    /// Whole when it's small, else a pointer.
    #[default]
    Auto,
    /// Whole (as it is when sent), unless the message's budget is spent.
    Whole,
    /// Its name, its size, its first lines, and how to read the rest.
    Pointer,
}

impl Include {
    /// The next, as the user cycles a link's.
    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::Whole,
            Self::Whole => Self::Pointer,
            Self::Pointer => Self::Auto,
        }
    }
}

/// A message as it's sent: its text, and what it links.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
}

impl Message {
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into(), links: Vec::new() }
    }

    /// Its text trimmed, its links moved with it (one cut off, or not a
    /// span of it: dropped).
    pub fn trimmed(self) -> Self {
        let lead = self.text.chars().take_while(|c| c.is_whitespace()).count();
        let text = self.text.trim().to_owned();
        let len = text.chars().count();
        let links = self.links.into_iter().filter(|l| l.start < l.end && l.start >= lead && l.end - lead <= len).map(|l| Link { start: l.start - lead, end: l.end - lead, ..l }).collect();
        Self { text, links }
    }

    /// Only its links that are spans of its text (as a client may send any).
    pub fn valid(self) -> Self {
        let len = self.text.chars().count();
        Self { links: self.links.into_iter().filter(|l| l.start < l.end && l.end <= len).collect(), ..self }
    }
}

/// A message of the user's as typed, beside the text the agent was handed
/// for it: its own text and links, and what its links handed on after it
/// (the handed text's rest, after its heading).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsTyped {
    pub message: Message,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handed_on: Option<String>,
}

impl From<&str> for Message {
    fn from(text: &str) -> Self {
        Self::text(text)
    }
}

impl From<String> for Message {
    fn from(text: String) -> Self {
        Self::text(text)
    }
}

impl From<&String> for Message {
    fn from(text: &String) -> Self {
        Self::text(text.clone())
    }
}

/// A message as an agent is handed it: the text with what's linked after
/// it (`handed_on`, apart: what follows its heading), and what the sender
/// should be told (linked whole but past the budget: sent as a pointer).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    pub handed_on: Option<String>,
    pub notes: Vec<String>,
}
