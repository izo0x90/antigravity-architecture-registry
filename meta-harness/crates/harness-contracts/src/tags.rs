//! Tags: names the user (or an agent) gives things in the project, to find
//! them again and point agents at them. A tag is on subjects: durable
//! references to one thing each (some lines of a terminal, a command it
//! ran...), each kept with what it said when tagged. What a subject says
//! now is its owner's to tell. Tags are a project server's.

use serde::{Deserialize, Serialize};

use crate::ids::{NativeId, TagId, TerminalId};

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

/// Where a thing's text is in the tags' logs: `len` bytes of a file from
/// byte `start`; to the file's end for a thing that was growing when tagged
/// (a command running: its file is its own).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    pub file: String,
    pub start: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<u64>,
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
    /// other lines), and the command they're in (kept when they're noted:
    /// it's named with them after its shell is gone).
    TerminalLines {
        terminal: TerminalId,
        shell: u64,
        first: u64,
        last: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<u64>,
    },
    /// A command a terminal's shell ran (`TerminalCommand::id`), in one
    /// start of its shell, and what was typed (its first line: kept when
    /// it's noted, to name it after its shell is gone).
    TerminalCommand {
        terminal: TerminalId,
        shell: u64,
        command: u64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        typed: String,
    },
    /// A turn of a chat (by its number in the session: the harness's
    /// session id, which outlives our threads): your message, the answer,
    /// its tool calls.
    ChatTurn { session: NativeId, turn: u32 },
    /// A message of a chat's turn (what you said, or an answer: numbered
    /// from 1 in its turn, `message_number`).
    ChatMessage { session: NativeId, turn: u32, message: u32 },
    /// A component of the code (a symbol: a function, a type...): the
    /// index follows it through edits; where it was when it was noted is
    /// kept (`was`: its lines at a snapshot), to read it once it's gone.
    Component {
        id: arch_graph::ComponentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        was: Option<CodeAt>,
    },
    /// Lines of a file (`first` to `last`, from 1) as the files were at a
    /// snapshot (the changes' timeline; none yet: the files now, kept as
    /// one when it's tagged); kept where they are as the files change.
    CodeLines { path: String, first: u32, last: u32, #[serde(default)] snapshot: String },
    /// A plan, as a whole.
    Plan { plan: arch_graph::PlanId },
    /// A component of a plan (its card).
    PlanComponent { plan: arch_graph::PlanId, id: arch_graph::ComponentId },
    /// One section of a plan's component's card (`overview`, `signature`,
    /// `logic`, `invariants`, `relations`, `code`, `changes`,
    /// `placeholders`, `findings`).
    PlanSection { plan: arch_graph::PlanId, id: arch_graph::ComponentId, section: String },
    /// A relation of a plan (`relation`: its kind).
    PlanRelation { plan: arch_graph::PlanId, relation: arch_graph::RelationKind, source: arch_graph::ComponentId, target: arch_graph::ComponentId },
    /// An item of a plan (a finding, a decision, a to-do), by its id.
    PlanItem { plan: arch_graph::PlanId, item: String },
}

/// Lines of a file as the files were at a snapshot (the changes' timeline).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAt {
    pub path: String,
    pub first: u32,
    pub last: u32,
    pub snapshot: String,
}

/// What names a thing beyond the thing itself: what its owner (or a view
/// showing it) knows of it now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    /// A terminal's thing: the start of its shell that isn't the current
    /// one (named with it).
    pub start: Option<u64>,
    /// A chat's thing: whose chat (`chat_who`).
    pub chat: String,
}

/// Whose chat a session is, in a name: its agent slot's with the start of
/// its id (a slot has several sessions over time: the name stays the
/// session's), or the session's own.
pub fn chat_who(slot: Option<&str>, session: &NativeId) -> String {
    let short: String = session.as_str().chars().take(8).collect();
    match slot {
        Some(slot) => format!("{slot}@{short}"),
        None => format!("session {short}"),
    }
}

/// What was typed for a command, as a noted command keeps it: its first
/// line, at most 80 characters.
pub fn typed_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().chars().take(80).collect()
}

/// Lines `first` to `last` in a name (`line 4`, `lines 4-9`).
fn lines_words<N: std::fmt::Display + PartialEq>(first: N, last: N) -> String {
    if first == last { format!("line {first}") } else { format!("lines {first}-{last}") }
}

/// A line of detail in what a thing says (a tool call's output, under
/// its line): reads leave runs of them out unless asked for in full.
pub const DETAIL: &str = "\u{2007}\u{2502} ";

/// Whether an item is a message (what you said, or an answer): what a tag
/// on a message can be on.
pub fn is_message(body: &crate::ItemBody) -> bool {
    matches!(body, crate::ItemBody::UserMessage { .. } | crate::ItemBody::AssistantMessage { .. })
}

/// Item `item`'s number among the messages of its turn (`items`: the
/// turn's, in any order), from 1; none when it isn't a message.
pub fn message_number(items: &[&crate::TurnItem], item: &crate::ItemId) -> Option<u32> {
    let mut messages: Vec<&&crate::TurnItem> = items.iter().filter(|i| is_message(&i.body)).collect();
    messages.sort_by_key(|i| i.ordinal);
    messages.iter().position(|i| i.id == *item).map(|n| n as u32 + 1)
}

/// How many of a terminal's lines a tag is on at most, at once.
pub const TAGGED_LINES: u64 = 200;

impl Subject {
    /// Its name: the one name a thing has, wherever it's shown (the
    /// cockpit's picker, a tag, an agent's tools) and named back by
    /// (`terminal t1 lines 4-9 (of command 3)`, `src/a.rs line 12`, `chat
    /// lead@4f2a91c0 turn 3 message 2`, `component rust:app::save`). What's in
    /// brackets says more; naming it back doesn't need it.
    pub fn name(&self, known: &Known) -> String {
        let terminal = |terminal: &TerminalId| match known.start {
            Some(start) => format!("terminal {terminal}@{start}"),
            None => format!("terminal {terminal}"),
        };
        let more = |text: &str| if text.is_empty() { String::new() } else { format!(" ({text})") };
        match self {
            Self::TerminalLines { terminal: t, first, last, command, .. } => format!("{} {}{}", terminal(t), lines_words(first, last), command.map(|c| format!(" (of command {c})")).unwrap_or_default()),
            Self::TerminalCommand { terminal: t, command, typed, .. } => format!("{} command {command}{}", terminal(t), more(typed.lines().next().unwrap_or_default())),
            Self::ChatTurn { turn, .. } => format!("chat {} turn {turn}", known.chat),
            Self::ChatMessage { turn, message, .. } => format!("chat {} turn {turn} message {message}", known.chat),
            Self::Component { id, .. } => format!("component {id}"),
            Self::CodeLines { path, first, last, .. } => format!("{path} {}", lines_words(first, last)),
            Self::Plan { plan } => format!("plan {plan}"),
            Self::PlanComponent { plan, id } => format!("plan {plan} component {id}"),
            Self::PlanSection { plan, id, section } => format!("plan {plan} component {id} section {section}"),
            Self::PlanRelation { plan, relation, source, target } => format!("plan {plan} relation {source} {} {target}", relation.name()),
            Self::PlanItem { plan, item } => format!("plan {plan} item {item}"),
        }
    }

    /// Why it can't be tagged as it is (lines the wrong way round, too
    /// many of them): none when it can.
    pub fn unfit(&self) -> Option<String> {
        match self {
            Self::TerminalLines { first, last, .. } if last < first => Some("its last line is before its first".into()),
            Self::TerminalLines { first, last, .. } if last - first >= TAGGED_LINES => Some(format!("at most {TAGGED_LINES} lines at once")),
            Self::CodeLines { first, last, .. } if *first == 0 || last < first => Some("its lines are from 1, its last not before its first".into()),
            _ => None,
        }
    }

    /// Whether it's the same thing as `other`: equal, but for what's kept
    /// of it when it's noted (lines of a file: the snapshot they're kept
    /// at, or none yet, as they're kept where they are now; a terminal's
    /// lines: the command they're in; a command: what was typed; a
    /// component: where it was).
    pub fn same_thing(&self, other: &Subject) -> bool {
        self.bare() == other.bare()
    }

    /// It without what's kept of it when it's noted.
    fn bare(&self) -> Subject {
        match self.clone() {
            Self::CodeLines { path, first, last, .. } => Self::CodeLines { path, first, last, snapshot: String::new() },
            Self::TerminalLines { terminal, shell, first, last, .. } => Self::TerminalLines { terminal, shell, first, last, command: None },
            Self::TerminalCommand { terminal, shell, command, .. } => Self::TerminalCommand { terminal, shell, command, typed: String::new() },
            Self::Component { id, .. } => Self::Component { id, was: None },
            other => other,
        }
    }

    /// Whether it is `other`, or has it in it (lines of a terminal have
    /// each of their lines in them).
    pub fn contains(&self, other: &Subject) -> bool {
        match (self, other) {
            (Self::TerminalLines { terminal, shell, first, last, .. }, Self::TerminalLines { terminal: t, shell: s, first: f, last: l, .. }) => terminal == t && shell == s && first <= f && l <= last,
            // Lines of a file: where they are now (one side's snapshot not
            // taken yet: the files now, as the other's are kept).
            (Self::CodeLines { path, first, last, snapshot }, Self::CodeLines { path: p, first: f, last: l, snapshot: s }) => path == p && first <= f && l <= last && (snapshot == s || snapshot.is_empty() || s.is_empty()),
            // A plan has its parts in it; a component, its sections.
            (Self::Plan { plan }, Self::PlanComponent { plan: p, .. } | Self::PlanSection { plan: p, .. } | Self::PlanRelation { plan: p, .. } | Self::PlanItem { plan: p, .. }) => plan == p,
            (Self::PlanComponent { plan, id }, Self::PlanSection { plan: p, id: i, .. }) => plan == p && id == i,
            _ => self.same_thing(other),
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
    /// As it was when noted: its source is gone (scrolled away, a shell
    /// ended) or changed it (lines of code edited), and what it said then
    /// is kept (the logs, a snapshot of the code).
    Kept,
    /// Nowhere: it's gone, and nothing was kept (or no owner here).
    Gone,
}

impl Held {
    /// Where it is, in words (a read says it beside what it says).
    pub fn words(self) -> &'static str {
        match self {
            Held::Source => "as it is now",
            Held::Kept => "as it was when tagged: changed or gone since",
            Held::Gone => "gone",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_contain_their_lines_and_names_are_checked() {
        let lines = |first, last| Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first, last, command: None };
        assert!(lines(10, 12).contains(&lines(11, 11)) && lines(10, 12).contains(&lines(10, 12)));
        assert!(!lines(10, 12).contains(&lines(12, 13)) && !lines(11, 11).contains(&lines(10, 12)));
        assert!(!lines(10, 12).contains(&Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 2, first: 11, last: 11, command: None }), "another start's");
        assert!(lines(10, 12).contains(&Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first: 11, last: 11, command: Some(3) }), "what's kept when noted isn't what it is");
        let command = Subject::TerminalCommand { terminal: TerminalId::new("t1"), shell: 1, command: 3, typed: String::new() };
        assert!(command.contains(&command.clone()) && !lines(0, 99).contains(&command));
        assert_eq!((lines(1, 200).unfit(), lines(1, 201).unfit().is_some(), lines(5, 4).unfit().is_some()), (None, true, true), "200 lines at most, the right way round");
        // One name for a thing: what the picker shows is what's kept.
        let known = |start| Known { start, chat: "lead@4f2a91c0".into() };
        let of_command = Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first: 4, last: 9, command: Some(3) };
        assert_eq!(of_command.name(&known(None)), "terminal t1 lines 4-9 (of command 3)");
        let typed = Subject::TerminalCommand { terminal: TerminalId::new("t1"), shell: 1, command: 3, typed: "make\nmore".into() };
        assert_eq!(typed.name(&known(Some(1))), "terminal t1@1 command 3 (make)", "another start's; the first line of what was typed");
        assert!(typed.same_thing(&command), "what's kept when noted isn't what it is");
        let code = Subject::CodeLines { path: "src/a.rs".into(), first: 12, last: 12, snapshot: "s1".into() };
        assert_eq!(code.name(&Known::default()), "src/a.rs line 12");
        let plan = arch_graph::PlanId::new("quick-look").unwrap();
        let (a, b) = (arch_graph::ComponentId::new("rust:app::open"), arch_graph::ComponentId::new("rust:app::show"));
        let section = Subject::PlanSection { plan: plan.clone(), id: a.clone(), section: "logic".into() };
        assert_eq!(section.name(&Known::default()), "plan quick-look component rust:app::open section logic");
        assert_eq!(Subject::PlanRelation { plan: plan.clone(), relation: arch_graph::RelationKind::Calls, source: a.clone(), target: b }.name(&Known::default()), "plan quick-look relation rust:app::open calls rust:app::show");
        assert!(Subject::Plan { plan: plan.clone() }.contains(&section) && Subject::PlanComponent { plan: plan.clone(), id: a.clone() }.contains(&section), "a plan has its parts in it");
        assert!(!Subject::PlanComponent { plan: arch_graph::PlanId::new("other").unwrap(), id: a }.contains(&section));
        let message = Subject::ChatMessage { session: NativeId::new("4f2a91c0aa"), turn: 3, message: 2 };
        assert_eq!(message.name(&known(None)), "chat lead@4f2a91c0 turn 3 message 2");
        assert_eq!(chat_who(Some("lead"), &NativeId::new("4f2a91c0aa")), "lead@4f2a91c0");
        for ok in ["flaky-build", "a_1", "X"] {
            assert!(tag_name_ok(ok), "{ok}");
        }
        for bad in ["", "two words", "dots.too", "a/b", "café", "wip_", "build-", "0b8f8a9e-3c1d-4f6e-9a2b-7c5d4e3f2a1b", &"x".repeat(41)] {
            assert!(!tag_name_ok(bad), "{bad}");
        }
    }
}
