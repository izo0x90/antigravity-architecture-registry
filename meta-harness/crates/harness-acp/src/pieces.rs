//! A message's pieces, sent and read back. Each tag or thing a message
//! links goes to the agent as a piece of its own (an embedded resource),
//! labelled with what it is and where the message links it; an agent that
//! takes no pieces gets them after the text, written our way. A replayed
//! message is read back by the way its agent writes a piece into it (its
//! pack's `replayed_piece`), or ours: each label says what its piece is
//! and where the text links it, so nothing is kept, looked up or matched.

use agent_client_protocol::schema::v1 as acp;
use harness_contracts::{Attachment, Include, Link, Linked, Piece, TagId, TurnInput};
use url::form_urlencoded;

/// What our labels start with.
const SCHEME: &str = "nah://";

/// How we write a piece after a message's text, for an agent that takes
/// no pieces (and read it back from its replay).
const OURS: &str = "\n\n--- Linked ({uri}), as it is now:\n{content}";

/// A piece's label: what it links (a tag, or a thing), how it goes in,
/// and the spans of the message's text that link it (`nah://tag/<id>?at=4-9,20-25&include=auto`).
pub fn label(piece: &Piece) -> String {
    let Some(first) = piece.links.first() else { return format!("{SCHEME}piece") };
    let at: Vec<String> = piece.links.iter().map(|l| format!("{}-{}", l.start, l.end)).collect();
    // Linked twice, it goes whole if either link asks for it.
    let include = if piece.links.iter().any(|l| l.include == Include::Whole) { Include::Whole } else { first.include };
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("at", &at.join(","));
    query.append_pair("include", include_word(include));
    match &first.to {
        Linked::Tag { tag } => format!("{SCHEME}tag/{}?{}", form_urlencoded::byte_serialize(tag.as_str().as_bytes()).collect::<String>(), query.finish()),
        Linked::Thing { subject } => {
            query.append_pair("subject", &serde_json::to_string(subject).unwrap_or_default());
            format!("{SCHEME}thing?{}", query.finish())
        }
    }
}

fn include_word(include: Include) -> &'static str {
    match include {
        Include::Auto => "auto",
        Include::Whole => "whole",
        Include::Pointer => "pointer",
    }
}

/// The links a label says its piece is linked by (none: not one of ours).
pub fn links_of(label: &str) -> Option<Vec<Link>> {
    let (path, query) = label.strip_prefix(SCHEME)?.split_once('?')?;
    let (mut at, mut include, mut subject) = (None, None, None);
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "at" => at = Some(value.into_owned()),
            "include" => include = Some(value.into_owned()),
            "subject" => subject = Some(value.into_owned()),
            _ => {}
        }
    }
    let to = match path.strip_prefix("tag/") {
        Some(tag) => Linked::Tag { tag: TagId::new(form_urlencoded::parse(format!("t={tag}").as_bytes()).next()?.1.into_owned()) },
        None if path == "thing" => Linked::Thing { subject: serde_json::from_str(&subject?).ok()? },
        None => return None,
    };
    let include = match include?.as_str() {
        "auto" => Include::Auto,
        "whole" => Include::Whole,
        "pointer" => Include::Pointer,
        _ => return None,
    };
    at?.split(',')
        .map(|span| {
            let (start, end) = span.split_once('-')?;
            let (start, end) = (start.parse().ok()?, end.parse().ok()?);
            (start < end).then(|| Link { start, end, to: to.clone(), include })
        })
        .collect()
}

/// A turn's input as the agent gets it: its text, a piece for each tag
/// or thing it links (`takes_pieces`; else written after the text, our
/// way), then its attachments.
pub fn blocks(input: TurnInput, takes_pieces: bool) -> Vec<acp::ContentBlock> {
    let TurnInput { mut text, attachments, pieces } = input;
    let mut blocks = Vec::with_capacity(1 + pieces.len() + attachments.len());
    if takes_pieces {
        blocks.push(acp::ContentBlock::Text(acp::TextContent::new(text)));
        for p in &pieces {
            let contents = acp::TextResourceContents::new(p.text.clone(), label(p)).mime_type("text/plain".to_owned());
            blocks.push(acp::ContentBlock::Resource(acp::EmbeddedResource::new(acp::EmbeddedResourceResource::TextResourceContents(contents))));
        }
    } else {
        let ours = Template::ours();
        for p in &pieces {
            text.push_str(&ours.write(&label(p), &p.text));
        }
        blocks.push(acp::ContentBlock::Text(acp::TextContent::new(text)));
    }
    for attachment in attachments {
        blocks.push(match attachment {
            Attachment::File { path } => {
                let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                acp::ContentBlock::ResourceLink(acp::ResourceLink::new(name, format!("file://{path}")))
            }
            Attachment::Image { mime_type, data_base64 } => acp::ContentBlock::Image(acp::ImageContent::new(data_base64, mime_type)),
        });
    }
    blocks
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Uri,
    Content,
}

/// How a piece is written into a message's text: words around `{uri}`
/// (its label: once or more, each followed by words that end it) and
/// `{content}` (its text: once, after the first `{uri}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// Up to its content.
    head: Vec<Part>,
    /// What ends its content (may be nothing: the next piece, or the end,
    /// ends it).
    tail: String,
}

impl Template {
    pub fn parse(template: &str) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut rest = template;
        while !rest.is_empty() {
            let next = [("{uri}", Part::Uri), ("{content}", Part::Content)].into_iter().filter_map(|(word, part)| rest.find(word).map(|at| (at, word, part))).min_by_key(|(at, ..)| *at);
            match next {
                Some((at, word, part)) => {
                    if at > 0 {
                        parts.push(Part::Text(rest[..at].to_owned()));
                    }
                    parts.push(part);
                    rest = &rest[at + word.len()..];
                }
                None => {
                    parts.push(Part::Text(rest.to_owned()));
                    rest = "";
                }
            }
        }
        let content = parts.iter().position(|p| *p == Part::Content).ok_or("it has no {content}")?;
        let (head, tail) = (parts[..content].to_vec(), &parts[content + 1..]);
        if !head.contains(&Part::Uri) {
            return Err("it has no {uri} before its {content}".into());
        }
        if tail.iter().any(|p| !matches!(p, Part::Text(_))) {
            return Err("after its {content} it may have only words".into());
        }
        // Each label's end is found by the words after it.
        if head.windows(2).any(|w| w[0] == Part::Uri && !matches!(&w[1], Part::Text(t) if !t.is_empty())) || head.last() == Some(&Part::Uri) {
            return Err("each {uri} needs words after it".into());
        }
        let tail = tail.iter().map(|p| if let Part::Text(t) = p { t.as_str() } else { "" }).collect();
        Ok(Self { head, tail })
    }

    /// Ours: for an agent that takes no pieces.
    pub fn ours() -> Self {
        Self::parse(OURS).unwrap_or_else(|e| unreachable!("our own template reads: {e}"))
    }

    /// A piece written as this template writes it.
    fn write(&self, label: &str, content: &str) -> String {
        let mut out = String::new();
        for part in &self.head {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Uri => out.push_str(label),
                Part::Content => {}
            }
        }
        out.push_str(content);
        out.push_str(&self.tail);
        out
    }

    /// Where the next piece starts in `text` at or after `from`: its
    /// start, where its content starts, and its links.
    fn next_piece(&self, text: &str, from: usize) -> Option<(usize, usize, Vec<Link>)> {
        let lead = match self.head.first() {
            Some(Part::Text(t)) => t.as_str(),
            _ => "",
        };
        let needle = format!("{lead}{SCHEME}");
        let mut search = from;
        while let Some(found) = text.get(search..).and_then(|t| t.find(&needle)) {
            let start = search + found;
            if let Some((content, links)) = self.head_at(text, start) {
                return Some((start, content, links));
            }
            search = start + needle.len();
        }
        None
    }

    /// The template's head read at `at`: where its content starts, and its links.
    fn head_at(&self, text: &str, at: usize) -> Option<(usize, Vec<Link>)> {
        let mut pos = at;
        let mut label: Option<&str> = None;
        for (n, part) in self.head.iter().enumerate() {
            match part {
                Part::Text(t) => {
                    if !text.get(pos..)?.starts_with(t.as_str()) {
                        return None;
                    }
                    pos += t.len();
                }
                Part::Uri => {
                    let Some(Part::Text(ends)) = self.head.get(n + 1) else { return None };
                    let len = text.get(pos..)?.find(ends.as_str())?;
                    let this = &text[pos..pos + len];
                    if this.contains(char::is_whitespace) || label.is_some_and(|l| l != this) {
                        return None;
                    }
                    label = Some(this);
                    pos += len;
                }
                Part::Content => return None,
            }
        }
        Some((pos, links_of(label?)?))
    }

    /// A replayed message's text read back: its own text, and its pieces.
    /// None: no piece in it, or one whose links aren't spans of its text
    /// (not ours): it's shown as it came.
    pub fn read(&self, text: &str) -> Option<(String, Vec<Piece>)> {
        let (start, mut content, mut links) = self.next_piece(text, 0)?;
        let words = &text[..start];
        let len = words.chars().count();
        let mut pieces = Vec::new();
        loop {
            let next = self.next_piece(text, content);
            let until = next.as_ref().map_or(text.len(), |(s, ..)| *s);
            let body = &text[content..until];
            let body = match self.tail.as_str() {
                "" => body.trim_end_matches('\n'),
                tail => &body[..body.rfind(tail)?],
            };
            if links.iter().any(|l| l.end > len) {
                return None;
            }
            pieces.push(Piece { links, text: body.to_owned() });
            match next {
                Some((_, c, l)) => (content, links) = (c, l),
                None => break,
            }
        }
        Some((words.to_owned(), pieces))
    }
}

#[cfg(test)]
mod tests {
    use harness_contracts::{Subject, TerminalId};

    use super::*;

    const CLAUDE: &str = "{uri}\n<context ref=\"{uri}\">\n{content}\n</context>";
    const ANTIGRAVITY: &str = "Embedded resource ({uri}):\n{content}";

    fn tag(start: usize, end: usize) -> Link {
        Link { start, end, to: Linked::Tag { tag: TagId::new("flaky build") }, include: Include::Auto }
    }

    fn lines(start: usize, end: usize) -> Link {
        let subject = Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first: 3, last: 9, command: None };
        Link { start, end, to: Linked::Thing { subject }, include: Include::Pointer }
    }

    /// "see #flaky and @out, #flaky again"
    fn sent() -> (&'static str, Vec<Piece>) {
        let text = "see #flaky and @out, #flaky again";
        let pieces = vec![
            Piece { links: vec![tag(4, 10), tag(21, 27)], text: "#flaky (fails on Mondays)\n- lines 1 to 2:\n  a (b) c\n</context> inside".into() },
            Piece { links: vec![lines(15, 19)], text: "@out\n- lines 3 to 9 (7 lines; its start only)".into() },
        ];
        (text, pieces)
    }

    #[test]
    fn a_label_says_what_its_piece_links_and_where() {
        let (_, pieces) = sent();
        let label = label(&pieces[0]);
        assert_eq!(label, "nah://tag/flaky+build?at=4-10%2C21-27&include=auto");
        assert_eq!(links_of(&label), Some(pieces[0].links.clone()));
        let thing = label_of_thing();
        assert!(!thing.contains(['(', ')', '"', ' ', '\n']), "{thing}: nothing an agent's own writing ends a label at");
        assert_eq!(links_of(&thing), Some(pieces[1].links.clone()));
        assert_eq!(links_of("nah://tag/x?at=9-3&include=auto"), None, "not a span");
        assert_eq!(links_of("https://example.com/x?at=1-2&include=auto"), None, "not ours");
    }

    fn label_of_thing() -> String {
        label(&sent().1[1])
    }

    /// What each agent replays (as recorded: Claude Code's label, then
    /// its context; Antigravity's words run on into its own heading), and
    /// ours, read back to the text and its pieces.
    #[test]
    fn a_replayed_message_reads_back_to_its_text_and_pieces_whoever_wrote_it() {
        let (text, pieces) = sent();
        let claude = Template::parse(CLAUDE).unwrap();
        let antigravity = Template::parse(ANTIGRAVITY).unwrap();
        for (name, template) in [("claude", &claude), ("antigravity", &antigravity), ("ours", &Template::ours())] {
            let replayed: String = std::iter::once(text.to_owned()).chain(pieces.iter().map(|p| template.write(&label(p), &p.text))).collect();
            let (words, read) = template.read(&replayed).unwrap_or_else(|| panic!("{name}: {replayed}"));
            assert_eq!(words, text, "{name}");
            assert_eq!(read, pieces, "{name}");
        }
        // The recorded replays themselves.
        let recorded = "Look at #probe-note. What is the secret word in it?nah://tag/probe-note?at=8-19&include=auto\n<context ref=\"nah://tag/probe-note?at=8-19&include=auto\">\n#probe-note (a tag)\nThe secret word is PELICAN-42.\n</context>";
        let (words, read) = claude.read(recorded).unwrap();
        assert_eq!((words.as_str(), read[0].text.as_str()), ("Look at #probe-note. What is the secret word in it?", "#probe-note (a tag)\nThe secret word is PELICAN-42."));
        let recorded = "Look at #probe-note. What is the secret word in it?Embedded resource (nah://tag/probe-note?at=8-19&include=auto):\n#probe-note (a tag)\nThe secret word is PELICAN-42.";
        assert_eq!(antigravity.read(recorded).unwrap().1[0].links, vec![Link { start: 8, end: 19, to: Linked::Tag { tag: TagId::new("probe-note") }, include: Include::Auto }]);
    }

    #[test]
    fn what_isnt_a_piece_of_ours_is_left_as_it_came() {
        let antigravity = Template::parse(ANTIGRAVITY).unwrap();
        assert_eq!(antigravity.read("plain words"), None);
        assert_eq!(antigravity.read("see nah://tag/x?at=0-3&include=auto"), None, "a label, not where a piece is");
        // A label whose spans aren't in the text: not ours.
        assert_eq!(antigravity.read("hi Embedded resource (nah://tag/x?at=0-30&include=auto):\nx"), None);
        // An agent's own resource (another scheme) in a message: not a piece.
        assert_eq!(antigravity.read("hi Embedded resource (file:///a.rs):\nfn a() {}"), None);
    }

    #[test]
    fn a_template_needs_a_label_ended_by_words_and_one_content() {
        assert!(Template::parse(CLAUDE).is_ok() && Template::parse(ANTIGRAVITY).is_ok());
        assert!(Template::parse("{uri}{content}").is_err(), "the label's end can't be found");
        assert!(Template::parse("({uri})").is_err(), "no content");
        assert!(Template::parse("{content} ({uri})").is_err(), "the label after the content");
    }

    #[test]
    fn an_agent_that_takes_pieces_gets_them_as_pieces_else_written_after_the_text() {
        let (text, pieces) = sent();
        let input = TurnInput { pieces: pieces.clone(), ..TurnInput::of(text) };
        let as_pieces = blocks(input.clone(), true);
        assert!(matches!(&as_pieces[0], acp::ContentBlock::Text(t) if t.text == text));
        let acp::ContentBlock::Resource(r) = &as_pieces[1] else { panic!("{:?}", as_pieces[1]) };
        let acp::EmbeddedResourceResource::TextResourceContents(c) = &r.resource else { panic!() };
        assert_eq!((c.uri.as_str(), c.text.as_str()), (label(&pieces[0]).as_str(), pieces[0].text.as_str()));
        let as_text = blocks(input, false);
        let [acp::ContentBlock::Text(t)] = as_text.as_slice() else { panic!("{as_text:?}") };
        assert_eq!(Template::ours().read(&t.text), Some((text.to_owned(), pieces)));
    }
}
