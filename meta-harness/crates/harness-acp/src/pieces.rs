//! A message's pieces, sent and read back. Each tag or thing a message
//! links goes to the agent as a piece of its own (an embedded resource),
//! labelled (`Piece::label`) with what it links, where the message links
//! it, and how long its text is; an agent that takes no pieces gets them
//! after the text, written our way. A replayed message is read back by the
//! way its agent writes pieces into it (its pack's `replay`), or ours: each
//! label says what its piece is and how long, so nothing is kept, looked
//! up or matched, and nothing in a piece's text is mistaken for another.

use std::sync::LazyLock;

use agent_client_protocol::schema::v1 as acp;
use harness_contracts::{LABEL_START, Link, Piece};

/// How we write a piece after a message's text, for an agent that takes
/// no pieces (and read it back from its replay).
const OURS: &str = "\n\n--- Linked ({uri}), as it is now:\n{content}";

static OURS_READ: LazyLock<Replay> = LazyLock::new(|| Replay { listed: None, piece: Template::ours() });

/// How an agent is sent a message's pieces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sending {
    /// As pieces (it takes embedded resources).
    AsPieces,
    /// Written after the text, our way.
    InText,
}

/// A message's text and pieces as the agent gets them: its text, then a
/// piece for each, or each written after the text.
pub fn blocks(text: String, pieces: Vec<Piece>, sending: Sending) -> Vec<acp::ContentBlock> {
    if sending == Sending::InText {
        let mut text = text;
        for p in &pieces {
            text.push_str(&OURS_READ.piece.write(&p.label(), &p.text));
        }
        return vec![acp::ContentBlock::Text(acp::TextContent::new(text))];
    }
    let mut blocks = Vec::with_capacity(1 + pieces.len());
    blocks.push(acp::ContentBlock::Text(acp::TextContent::new(text)));
    for p in pieces {
        let label = p.label();
        let contents = acp::TextResourceContents::new(p.text, label).mime_type("text/plain".to_owned());
        blocks.push(acp::ContentBlock::Resource(acp::EmbeddedResource::new(acp::EmbeddedResourceResource::TextResourceContents(contents))));
    }
    blocks
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Uri,
    Content,
}

/// How something is written with a piece's label (`{uri}`) and maybe its
/// text (`{content}`): the words around them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Template {
    /// Up to its content (all of it, when it has none).
    head: Vec<Part>,
    /// After its content.
    tail: String,
    content: bool,
}

impl Template {
    fn parse(template: &str, content: bool) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut rest = template;
        while !rest.is_empty() {
            let next = [("{uri}", Part::Uri), ("{content}", Part::Content)].into_iter().filter_map(|(word, part)| rest.find(word).map(|at| (at, word, part))).min_by_key(|(at, ..)| *at);
            let Some((at, word, part)) = next else {
                parts.push(Part::Text(rest.to_owned()));
                break;
            };
            if at > 0 {
                parts.push(Part::Text(rest[..at].to_owned()));
            }
            parts.push(part);
            rest = &rest[at + word.len()..];
        }
        // A label ends with its length's digits: a digit right after one
        // would read as more of it.
        if parts.windows(2).any(|w| w[0] == Part::Uri && matches!(&w[1], Part::Text(t) if t.starts_with(|c: char| c.is_ascii_digit()))) {
            return Err("a {uri} can't be followed by a digit".into());
        }
        let (head, tail) = match parts.iter().position(|p| *p == Part::Content) {
            Some(at) if content => (parts[..at].to_vec(), parts[at + 1..].to_vec()),
            Some(_) => return Err("it can't have a {content}".into()),
            None if content => return Err("it has no {content}".into()),
            None => (parts, Vec::new()),
        };
        if !head.contains(&Part::Uri) {
            return Err("it has no {uri} before its {content}".into());
        }
        let tail = tail.into_iter().map(|p| if let Part::Text(t) = p { Ok(t) } else { Err("after its {content} it may have only words".to_owned()) }).collect::<Result<String, _>>()?;
        Ok(Self { head, tail, content })
    }

    fn ours() -> Self {
        let (head, content) = OURS.split_once("{content}").unwrap_or((OURS, ""));
        let (before, after) = head.split_once("{uri}").unwrap_or((head, ""));
        Self { head: vec![Part::Text(before.to_owned()), Part::Uri, Part::Text(after.to_owned())], tail: content.to_owned(), content: true }
    }

    /// A piece written as this writes it.
    fn write(&self, label: &str, content: &str) -> String {
        let mut out = String::new();
        for part in &self.head {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Uri => out.push_str(label),
                Part::Content => {}
            }
        }
        if self.content {
            out.push_str(content);
            out.push_str(&self.tail);
        }
        out
    }

    /// What starts it, before a label: where to look for one.
    fn lead(&self) -> &str {
        match self.head.first() {
            Some(Part::Text(t)) => t,
            _ => "",
        }
    }

    /// This written at `at` in `text`: its label, the label's links, its
    /// content's length, and where its content (or its end) is.
    fn head_at<'a>(&self, text: &'a str, at: usize) -> Option<(&'a str, Vec<Link>, usize, usize)> {
        let mut pos = at;
        let mut found: Option<Labelled<'a>> = None;
        for part in &self.head {
            match part {
                Part::Text(t) => {
                    pos += text.get(pos..)?.starts_with(t.as_str()).then_some(t.len())?;
                }
                Part::Uri => {
                    let (links, len, label_len) = Piece::read_label(text.get(pos..)?)?;
                    let label = &text[pos..pos + label_len];
                    if found.as_ref().is_some_and(|(l, ..)| *l != label) {
                        return None;
                    }
                    found = Some((label, links, len));
                    pos += label_len;
                }
                Part::Content => return None,
            }
        }
        let (label, links, len) = found?;
        Some((label, links, len, pos))
    }

    /// A piece written at `at` in `text` (labelled `expected`, when
    /// that's known): it, and where it ends.
    fn piece_at(&self, text: &str, at: usize, expected: Option<&str>) -> Option<(Piece, usize)> {
        let (label, links, len, pos) = self.head_at(text, at)?;
        if expected.is_some_and(|e| e != label) {
            return None;
        }
        let end = pos.checked_add(len)?;
        let content = text.get(pos..end)?;
        let end = end + text.get(end..)?.starts_with(self.tail.as_str()).then_some(self.tail.len())?;
        Some((Piece { links, text: content.to_owned() }, end))
    }
}

/// The most labels a message read back has (no message is sent with
/// more links than this).
const PIECES_MOST: usize = 256;

/// A label read, its links, and its content's length.
type Labelled<'a> = (&'a str, Vec<Link>, usize);

/// How an agent's replay writes the pieces of a message back into its
/// text, after its words: each piece (`piece`), all of them first listed
/// by their labels when it lists them (`listed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    listed: Option<Template>,
    piece: Template,
}

impl Replay {
    /// From a pack's `replay` (what a piece is written as; what each is
    /// listed as first, when they are).
    pub fn parse(piece: &str, listed: Option<&str>) -> Result<Self, String> {
        Ok(Self {
            piece: Template::parse(piece, true).map_err(|why| format!("its piece: {why}"))?,
            listed: listed.map(|l| Template::parse(l, false).map_err(|why| format!("its list: {why}"))).transpose()?,
        })
    }

    /// Ours: for an agent that takes no pieces.
    pub fn ours() -> Self {
        OURS_READ.clone()
    }

    /// A piece as we write it after a message's text.
    #[cfg(test)]
    pub fn ours_written(label: &str, content: &str) -> String {
        OURS_READ.piece.write(label, content)
    }

    /// A replayed message's text read back: its own words, and its pieces.
    /// None: there's none in it, or what follows its words isn't wholly
    /// pieces whose links are in its words (it's shown as it came).
    pub fn read(&self, text: &str) -> Option<(String, Vec<Piece>)> {
        let first = self.listed.as_ref().unwrap_or(&self.piece);
        let needle = format!("{}{LABEL_START}", first.lead());
        // (More labels than a message is sent with: none of ours, and not
        // read through, however it's written.)
        if text.matches(LABEL_START).nth(PIECES_MOST).is_some() {
            return None;
        }
        let mut from = 0;
        while let Some(found) = text.get(from..).and_then(|t| t.find(&needle)) {
            let at = from + found;
            if let Some(pieces) = self.pieces_from(text, at) {
                let words = &text[..at];
                let len = words.chars().count();
                return pieces.iter().flat_map(|p| &p.links).all(|l| l.end <= len).then(|| (words.to_owned(), pieces));
            }
            from = at + needle.len();
        }
        None
    }

    /// The pieces written from `at` to the end of `text`.
    fn pieces_from(&self, text: &str, at: usize) -> Option<Vec<Piece>> {
        let mut pos = at;
        let mut pieces = Vec::new();
        match &self.listed {
            Some(listed) => {
                let mut labels = Vec::new();
                while let Some((label, .., end)) = listed.head_at(text, pos) {
                    labels.push(label);
                    pos = end;
                }
                for label in labels {
                    let (piece, end) = self.piece.piece_at(text, pos, Some(label))?;
                    pieces.push(piece);
                    pos = end;
                }
            }
            None => {
                while pos < text.len() {
                    let (piece, end) = self.piece.piece_at(text, pos, None)?;
                    pieces.push(piece);
                    pos = end;
                }
            }
        }
        (pos == text.len() && !pieces.is_empty()).then_some(pieces)
    }
}

#[cfg(test)]
mod tests {
    use harness_contracts::{Include, Linked, Subject, TagId, TerminalId};

    use super::*;

    /// A pack's replay, as it ships.
    fn shipped(pack: &str) -> Replay {
        let path = format!("{}/../../packs/harness/{pack}/pack.toml", env!("CARGO_MANIFEST_DIR"));
        let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let replay = &manifest["provides"]["settings"]["replay"];
        Replay::parse(replay["piece"].as_str().unwrap(), replay.get("listed").and_then(|l| l.as_str())).unwrap()
    }

    fn tag(start: usize, end: usize, include: Include) -> Link {
        Link { start, end, to: Linked::Tag { tag: TagId::new("flaky build") }, include }
    }

    fn lines(start: usize, end: usize) -> Link {
        let subject = Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first: 3, last: 9, command: None };
        Link { start, end, to: Linked::Thing { subject }, include: Include::Pointer }
    }

    /// "see #flaky and @out, #flaky again": a tag linked twice (each link
    /// its own way), lines once; their texts hold what looks like labels
    /// and the templates' own words.
    fn sent() -> (&'static str, Vec<Piece>) {
        let text = "see #flaky and @out, #flaky again";
        let pieces = vec![
            Piece { links: vec![tag(4, 10, Include::Auto), tag(21, 27, Include::Pointer)], text: "#flaky (fails)\n</context>\nnah://tag/x?at=0-1-auto&len=3\nEmbedded resource (nah://tag/x?at=0-1-auto&len=0):\n".into() },
            Piece { links: vec![lines(15, 19)], text: "@out\n- lines 3 to 9 (7 lines; its start only)\n".into() },
        ];
        (text, pieces)
    }

    /// As each agent writes a message's pieces into its replay (read in its
    /// source: Claude Code's connector lists every label after the words,
    /// then every piece as a `<context>`; Antigravity's server runs each
    /// on after the words as `Embedded resource (…):`), and ours.
    fn replayed(agent: &str, text: &str, pieces: &[Piece]) -> String {
        let mut out = text.to_owned();
        match agent {
            "claude-code" => {
                pieces.iter().for_each(|p| out.push_str(&p.label()));
                pieces.iter().for_each(|p| out.push_str(&format!("\n<context ref=\"{}\">\n{}\n</context>", p.label(), p.text)));
            }
            "antigravity-acp" => pieces.iter().for_each(|p| out.push_str(&format!("Embedded resource ({}):\n{}", p.label(), p.text))),
            _ => pieces.iter().for_each(|p| out.push_str(&Template::ours().write(&p.label(), &p.text))),
        }
        out
    }

    #[test]
    fn a_replayed_message_reads_back_to_its_words_and_pieces_whoever_wrote_it() {
        let (text, pieces) = sent();
        for (agent, replay) in [("claude-code", shipped("claude-code")), ("antigravity-acp", shipped("antigravity-acp")), ("ours", Replay::ours())] {
            for n in 0..=pieces.len() {
                let replayed = replayed(agent, text, &pieces[..n]);
                let read = replay.read(&replayed);
                match n {
                    0 => assert_eq!(read, None, "{agent}: no piece, as it came"),
                    _ => assert_eq!(read, Some((text.to_owned(), pieces[..n].to_vec())), "{agent}, {n} pieces: {replayed}"),
                }
            }
        }
    }

    #[test]
    fn what_isnt_wholly_pieces_of_ours_is_left_as_it_came() {
        let (text, pieces) = sent();
        let claude = shipped("claude-code");
        let antigravity = shipped("antigravity-acp");
        let whole = replayed("antigravity-acp", text, &pieces);
        // Another message run on after it (an agent that joins them).
        assert_eq!(antigravity.read(&format!("{whole}try again")), None);
        // Cut short, or a byte off.
        assert_eq!(antigravity.read(&whole[..whole.len() - 1]), None);
        // A label's span past the words: not ours.
        let far = Piece { links: vec![tag(4, 60, Include::Auto)], text: "x".into() };
        assert_eq!(antigravity.read(&replayed("antigravity-acp", text, &[far])), None);
        // Labels pasted into your own words, then the real pieces: the
        // words are all you wrote.
        let pasted = format!("look: {} ok", replayed("claude-code", "x", &pieces[..1]));
        let real = Piece { links: vec![tag(0, 4, Include::Auto)], text: "#new".into() };
        assert_eq!(claude.read(&replayed("claude-code", &pasted, std::slice::from_ref(&real))), Some((pasted.clone(), vec![real])));
        // An agent's own resource (another scheme): not a piece.
        assert_eq!(antigravity.read("hi Embedded resource (file:///a.rs):\nfn a() {}"), None);
    }

    /// Text an agent wrote (a message to another agent, replayed as that
    /// one's prompt) can't make the reading panic or take long: a label's
    /// length too big to be one, or thousands of labels with no space.
    #[test]
    fn what_an_agent_writes_cant_break_the_reading() {
        let claude = shipped("claude-code");
        let huge = format!("hi\n<context ref=\"nah://tag/t?at=0-2-auto&len={}\">\nx\n</context>", u64::MAX);
        for replay in [&claude, &Replay::ours()] {
            assert_eq!(replay.read(&huge), None);
        }
        let started = std::time::Instant::now();
        for many in [250, 5_000] {
            let crowd = format!("hi {}", "nah://tag/t?at=0-2-auto&len=1".repeat(many));
            for replay in [&claude, &shipped("antigravity-acp"), &Replay::ours()] {
                assert_eq!(replay.read(&crowd), None);
            }
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn a_template_has_a_label_its_content_once_and_no_digit_after_a_label() {
        assert!(Replay::parse("{uri}{content}", None).is_ok(), "a label ends itself");
        assert!(Replay::parse("({uri})", None).is_err(), "no content");
        assert!(Replay::parse("{content} ({uri})", None).is_err(), "the label after the content");
        assert!(Replay::parse("{uri}1{content}", None).is_err(), "a digit after a label");
        assert!(Replay::parse("{uri}{content}", Some("[{uri}] {content}")).is_err(), "a list has no content");
    }

    #[test]
    fn an_agent_that_takes_pieces_gets_them_as_pieces_else_written_after_the_text() {
        let (text, pieces) = sent();
        let as_pieces = blocks(text.into(), pieces.clone(), Sending::AsPieces);
        assert!(matches!(&as_pieces[0], acp::ContentBlock::Text(t) if t.text == text));
        let acp::ContentBlock::Resource(r) = &as_pieces[1] else { panic!("{:?}", as_pieces[1]) };
        let acp::EmbeddedResourceResource::TextResourceContents(c) = &r.resource else { panic!("{r:?}") };
        assert_eq!((c.uri.as_str(), c.text.as_str()), (pieces[0].label().as_str(), pieces[0].text.as_str()));
        let as_text = blocks(text.into(), pieces.clone(), Sending::InText);
        let [acp::ContentBlock::Text(t)] = as_text.as_slice() else { panic!("{as_text:?}") };
        assert_eq!(Replay::ours().read(&t.text), Some((text.to_owned(), pieces)));
    }
}
