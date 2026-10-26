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
/// same thing), and its text as the agent is handed it (its heading, then
/// each thing it's on: whole, its start, or why it isn't there).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Piece {
    pub links: Vec<Link>,
    pub text: String,
}

/// The links of a message's `pieces`, in the order its text has them.
pub fn links_of(pieces: &[Piece]) -> Vec<Link> {
    let mut links: Vec<Link> = pieces.iter().flat_map(|p| p.links.iter().cloned()).collect();
    links.sort_by_key(|l| l.start);
    links
}

/// What a piece's label starts with.
pub const LABEL_START: &str = "nah://";

/// The longest a label is read for (its subject's name, every span of its
/// links): a longer one isn't one.
const LABEL_MOST: usize = 16 * 1024;

impl Piece {
    /// Its label: what it links, each place the message's text links it
    /// (and how it goes in), and how long its text is, in bytes: the label
    /// and its text can be found again in any text they're written into
    /// (`nah://tag/flaky?at=4-10-auto,21-27-pointer&len=120`; a thing's:
    /// `nah://thing?subject=<its JSON>&at=…&len=…`). Its length comes last:
    /// the label ends with its digits.
    pub fn label(&self) -> String {
        let at: Vec<String> = self.links.iter().map(|l| format!("{}-{}-{}", l.start, l.end, include_word(l.include))).collect();
        let at = at.join(",");
        let len = self.text.len();
        match self.links.first().map(|l| &l.to) {
            Some(Linked::Tag { tag }) => format!("{LABEL_START}tag/{}?at={at}&len={len}", escaped(tag.as_str())),
            Some(Linked::Thing { subject }) => format!("{LABEL_START}thing?subject={}&at={at}&len={len}", escaped(&serde_json::to_string(subject).unwrap_or_default())),
            None => format!("{LABEL_START}none?len={len}"),
        }
    }

    /// The label at the start of `text`: its links, how long its text is,
    /// and how long the label is (none: no label of ours starts there). A
    /// label has no white space: it's never looked for past one.
    pub fn read_label(text: &str) -> Option<(Vec<Link>, usize, usize)> {
        let rest = text.strip_prefix(LABEL_START)?;
        // (It ends at its length's digits, never further than a label goes:
        // only what it is is read, whatever follows.)
        let most = (0..=rest.len().min(LABEL_MOST)).rev().find(|at| rest.is_char_boundary(*at))?;
        let len_at = rest[..most].find("&len=")?;
        let digits = rest[len_at + 5..].bytes().take(20).take_while(u8::is_ascii_digit).count();
        let word = &rest[..len_at + 5 + digits];
        if word.contains(char::is_whitespace) {
            return None;
        }
        let (path, query) = word.split_once('?')?;
        let len_at = query.find("&len=")?;
        let len: usize = query[len_at + 5..].parse().ok()?;
        let label_len = LABEL_START.len() + word.len();
        let mut subject = None;
        let mut at = None;
        for pair in query[..len_at].split('&') {
            match pair.split_once('=')? {
                ("subject", v) => subject = Some(unescaped(v)?),
                ("at", v) => at = Some(v),
                _ => return None,
            }
        }
        let to = match path.strip_prefix("tag/") {
            Some(tag) => Linked::Tag { tag: TagId::new(unescaped(tag)?) },
            None if path == "thing" => Linked::Thing { subject: serde_json::from_str(&subject?).ok()? },
            None => return None,
        };
        let links = at?
            .split(',')
            .map(|span| {
                let mut parts = span.splitn(3, '-');
                let (start, end) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
                let include = serde_json::from_value(serde_json::Value::String(parts.next()?.to_owned())).ok()?;
                (start < end).then(|| Link { start, end, to: to.clone(), include })
            })
            .collect::<Option<Vec<_>>>()?;
        Some((links, len, label_len))
    }
}

fn include_word(include: Include) -> String {
    match serde_json::to_value(include) {
        Ok(serde_json::Value::String(word)) => word,
        _ => String::new(),
    }
}

/// `text` with every byte but letters, digits and `-._~` as `%XX`.
fn escaped(text: &str) -> String {
    urlencoding::encode(text).into_owned()
}

/// `text` as `escaped` wrote it (none: a `%` isn't followed by two hex
/// digits, so it wasn't).
fn unescaped(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let well_formed = text.match_indices('%').all(|(at, _)| bytes.get(at + 1..at + 3).is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit)));
    well_formed.then(|| urlencoding::decode(text).ok()).flatten().map(std::borrow::Cow::into_owned)
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
        let (mut chars, between_chars) = (0, between.chars().count());
        for (n, m) in messages.into_iter().enumerate() {
            if n > 0 {
                out.text.push_str(between);
                chars += between_chars;
            }
            out.links.extend(m.links.into_iter().map(|l| Link { start: l.start + chars, end: l.end + chars, ..l }));
            chars += m.text.chars().count();
            out.text.push_str(&m.text);
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
        let both = Message::joined(vec![m.clone(), Message::text("é"), m], "\n--\n");
        assert_eq!(both.text, "see #x\n--\né\n--\nsee #x");
        assert_eq!(both.links.iter().map(|l| (l.start, l.end)).collect::<Vec<_>>(), vec![(4, 6), (19, 21)]);
        let said = |l: &Link| both.text.chars().skip(l.start).take(l.end - l.start).collect::<String>();
        assert!(both.links.iter().all(|l| said(l) == "#x"));
    }

    #[test]
    fn a_label_says_what_its_piece_links_where_and_how_long_it_is_and_reads_back() {
        let subject = crate::Subject::TerminalLines { terminal: crate::TerminalId::new("t 1"), shell: 1, first: 3, last: 9, command: None };
        let pieces = [
            Piece { links: vec![link(4, 10), Link { include: Include::Pointer, ..link(21, 27) }], text: "#x é\n(fails)".into() },
            Piece { links: vec![Link { start: 0, end: 4, to: Linked::Thing { subject }, include: Include::Whole }], text: String::new() },
            Piece { links: vec![Link { to: Linked::Tag { tag: TagId::new("a b/c?d&len=1") }, ..link(1, 2) }], text: "x".into() },
        ];
        assert_eq!(pieces[0].label(), format!("nah://tag/x?at=4-10-auto,21-27-pointer&len={}", pieces[0].text.len()));
        for p in &pieces {
            let label = p.label();
            assert!(!label.contains(|c: char| c.is_whitespace() || "()\"<>".contains(c)), "{label}: nothing an agent's own writing ends a label at");
            // Read back where it's written, followed by anything.
            let written = format!("{label}nah://more");
            assert_eq!(Piece::read_label(&written), Some((p.links.clone(), p.text.len(), label.len())), "{label}");
        }
        assert_eq!(Piece::read_label("nah://tag/x?at=9-3-auto&len=1"), None, "not a span");
        assert_eq!(Piece::read_label("nah://tag/x?at=1-3-auto"), None, "no length");
        assert_eq!(Piece::read_label("nah://tag/x?at=1-3-auto &len=1"), None, "past a space: not read");
        assert_eq!(Piece::read_label("https://tag/x?at=1-3-auto&len=1"), None, "not ours");
        for bad in ["%G1", "%4", "x%"] {
            assert_eq!(Piece::read_label(&format!("nah://tag/{bad}?at=1-3-auto&len=1")), None, "{bad}: not as we escape");
        }
    }
}
