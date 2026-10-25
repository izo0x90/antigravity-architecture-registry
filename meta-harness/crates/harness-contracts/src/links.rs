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

/// What one tag or thing a message links hands an agent with it: the
/// links that hand it on (one per place the text links it, all to the
/// same), and its text as the agent is handed it (its heading, then each
/// thing it's on: whole, its start, or why it isn't there).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Piece {
    pub links: Vec<Link>,
    pub text: String,
}

impl Piece {
    /// The links of `pieces` in the text's order, and each piece's text:
    /// what a message shows of what went with it.
    pub fn split(pieces: Vec<Piece>) -> (Vec<Link>, Vec<String>) {
        let mut links = Vec::new();
        let mut texts = Vec::with_capacity(pieces.len());
        for p in pieces {
            links.extend(p.links);
            texts.push(p.text);
        }
        links.sort_by_key(|l| l.start);
        (links, texts)
    }
}

/// A message as an agent is handed it: its text, and a piece for each tag
/// or thing it links; and what the sender should be told (linked whole
/// but past the budget: sent as its start).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expanded {
    pub message: Message,
    pub pieces: Vec<Piece>,
    pub notes: Vec<String>,
}

impl Message {
    /// `head`, then this message (its links moved along).
    pub fn after(self, head: &str) -> Self {
        let by = head.chars().count();
        Self { text: format!("{head}{}", self.text), links: self.links.into_iter().map(|l| Link { start: l.start + by, end: l.end + by, ..l }).collect() }
    }

    /// Messages one after the other, `between` each, their links moved along.
    pub fn joined(messages: Vec<Message>, between: &str) -> Self {
        let mut out = Message::default();
        for (n, m) in messages.into_iter().enumerate() {
            let head = if n == 0 { out.text.clone() } else { format!("{}{between}", out.text) };
            let next = m.after(&head);
            out.links.extend(next.links);
            out.text = next.text;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(start: usize, end: usize) -> Link {
        Link { start, end, to: Linked::Tag { tag: TagId::new("x") }, include: Include::Auto }
    }

    #[test]
    fn links_move_with_the_text_before_them() {
        let m = Message { text: "see #x".into(), links: vec![link(4, 6)] };
        let after = m.clone().after("From é:\n");
        assert_eq!((after.text.as_str(), after.links[0].start), ("From é:\nsee #x", 12));
        let both = Message::joined(vec![m.clone(), m], "\n--\n");
        assert_eq!(both.text, "see #x\n--\nsee #x");
        assert_eq!(both.links.iter().map(|l| (l.start, l.end)).collect::<Vec<_>>(), vec![(4, 6), (14, 16)]);
        let said = |l: &Link| both.text.chars().skip(l.start).take(l.end - l.start).collect::<String>();
        assert!(both.links.iter().all(|l| said(l) == "#x"));
    }

    #[test]
    fn a_messages_pieces_give_its_links_in_order_and_their_texts() {
        let pieces = vec![Piece { links: vec![link(9, 11), link(0, 2)], text: "a".into() }, Piece { links: vec![link(4, 6)], text: "b".into() }];
        let (links, texts) = Piece::split(pieces);
        assert_eq!(links.iter().map(|l| l.start).collect::<Vec<_>>(), vec![0, 4, 9]);
        assert_eq!(texts, vec!["a", "b"]);
    }
}
