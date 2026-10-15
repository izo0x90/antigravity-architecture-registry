//! Navigation every view shares: what a cursor is on (`Target`), where a view
//! is (`Place`, so it can be returned to), the jump list behind ctrl+o /
//! ctrl+i, the motions and folds the universal keys ask for, and the items a
//! view offers to search (`Item`) and to hints (`Spot`). Views implement
//! `Navigable`; the shell owns the keys, the history, marks, search and hints.

use arch_graph::{CommentId, ComponentId, RelationKind};

use crate::app::BufferKind;
use crate::graph::Nav;
use crate::spec::Section;

/// What a view's cursor is on.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A component (a graph node, an outline row).
    Component(ComponentId),
    /// A line of code: the component it's in, and the components named on
    /// it (what it calls, the types it uses), the one under the cursor first.
    Line { file: String, line: usize, within: Option<ComponentId>, refs: Vec<ComponentId> },
    /// An item in a component's spec.
    Spec { component: ComponentId, section: Section, item: SpecItem },
    /// A job in the work queue.
    Job { component: ComponentId },
    /// A message in the chat, and the file it points to (if any).
    Message { thread: Option<String>, index: usize, link: Option<(String, Option<usize>)> },
}

/// One navigable item in the spec view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecItem {
    /// The section's title row.
    Header,
    Relation { kind: RelationKind, other: ComponentId, outgoing: bool },
    Step(usize),
    Invariant(usize),
    Placeholder(usize),
    Finding(usize),
    /// The component's source (the code section's body).
    Source,
    Comment(CommentId),
    Reply(CommentId, usize),
}

impl Target {
    /// The component the target is about: for a relation in the spec, the
    /// other end (what the cursor points at).
    pub fn component(&self) -> Option<&ComponentId> {
        match self {
            Self::Component(id) | Self::Job { component: id } => Some(id),
            Self::Line { within, .. } => within.as_ref(),
            Self::Spec { item: SpecItem::Relation { other, .. }, .. } => Some(other),
            Self::Spec { component, .. } => Some(component),
            Self::Message { .. } => None,
        }
    }

    /// A place in the code the target names directly (a line, a file link).
    pub fn code(&self) -> Option<(String, Option<usize>)> {
        match self {
            Self::Line { file, line, .. } => Some((file.clone(), Some(*line))),
            Self::Message { link, .. } => link.clone(),
            _ => None,
        }
    }
}

/// Where a view is, enough to come back to it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    /// The chat: which slot it shows, and the message.
    Chat {
        message: Option<usize>,
        #[serde(default)]
        slot: Option<harness_contracts::SlotId>,
    },
    Code { file: String, line: usize },
    /// The graph: where it looks, and the selected card's key.
    Graph { nav: Nav, selection: Option<String> },
    Tree { cursor: Option<ComponentId> },
    /// The item by its key (indexes shift as the spec loads).
    Spec { component: Option<ComponentId>, item: Option<String> },
    Queue { component: Option<ComponentId> },
}

/// A place in a slot: what the jump list and marks hold.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Jump {
    pub slot: usize,
    pub kind: BufferKind,
    pub place: Place,
}

/// The jump list: where big moves started, to go back (ctrl+o) and forward
/// (ctrl+i) across views and slots. Jumping somewhere new drops what was
/// ahead, like a browser.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JumpList {
    entries: Vec<Jump>,
    /// Where we are: `entries.len()` means past the newest entry.
    at: usize,
}

const JUMPS: usize = 100;

impl JumpList {
    /// The same list, its rules holding whatever it was read from.
    pub fn checked(mut self) -> Self {
        let over = self.entries.len().saturating_sub(JUMPS);
        self.entries.drain(..over);
        self.at = self.at.saturating_sub(over).min(self.entries.len());
        self
    }

    /// Records the place a jump left from.
    pub fn record(&mut self, from: Jump) {
        self.entries.truncate(self.at);
        if self.entries.last() != Some(&from) {
            self.entries.push(from);
        }
        if self.entries.len() > JUMPS {
            self.entries.remove(0);
        }
        self.at = self.entries.len();
    }

    pub fn can_back(&self) -> bool {
        self.at > 0
    }

    pub fn can_forward(&self) -> bool {
        self.at + 1 < self.entries.len()
    }

    /// Steps back from `here`; returns where to go.
    pub fn back(&mut self, here: Jump) -> Option<Jump> {
        while self.at > 0 {
            if self.at == self.entries.len() {
                self.entries.push(here.clone());
            } else {
                self.entries[self.at] = here.clone();
            }
            self.at -= 1;
            // The same place twice in a row isn't a step.
            if self.entries[self.at] != here {
                return Some(self.entries[self.at].clone());
            }
        }
        None
    }

    /// Steps forward from `here` (after going back); returns where to go.
    pub fn forward(&mut self, here: Jump) -> Option<Jump> {
        while self.at + 1 < self.entries.len() {
            self.entries[self.at] = here.clone();
            self.at += 1;
            if self.entries[self.at] != here {
                return Some(self.entries[self.at].clone());
            }
        }
        None
    }
}

/// What the movement keys ask of a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Down,
    Up,
    Left,
    Right,
    HalfDown,
    HalfUp,
    First,
    Last,
    NextSibling,
    PrevSibling,
    /// Row or line `n` (1-based).
    Line(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    Open,
    Close,
    Toggle,
    OpenAll,
    CloseAll,
}

/// Where a go-to key sends the cursor's target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoTo {
    Code,
    Spec,
    Graph,
    Tree,
    Queue,
    /// Every use of it in the code (calls, types named, imports, paths).
    Uses,
    Callers,
    Callees,
    Parent,
    Members,
    Implements,
    Flows,
}

impl GoTo {
    /// The relation a relational go-to follows, and in which direction.
    pub fn relation(self) -> Option<(RelationKind, bool)> {
        match self {
            Self::Callers => Some((RelationKind::Calls, false)),
            Self::Callees => Some((RelationKind::Calls, true)),
            Self::Parent => Some((RelationKind::Contains, false)),
            Self::Members => Some((RelationKind::Contains, true)),
            Self::Implements => Some((RelationKind::Implements, true)),
            Self::Flows => Some((RelationKind::DataFlow, true)),
            _ => None,
        }
    }

    /// One of what it goes to, for pickers ("Go to which caller?").
    pub fn one(self) -> &'static str {
        match self {
            Self::Uses => "use",
            Self::Callers => "caller",
            Self::Callees => "callee",
            Self::Parent => "container",
            Self::Members => "member",
            Self::Implements => "interface",
            Self::Flows => "data flow target",
            _ => self.noun(),
        }
    }

    /// In words, for "no …" notes and pickers.
    pub fn noun(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Spec => "spec",
            Self::Graph => "graph node",
            Self::Tree => "outline row",
            Self::Queue => "queue job",
            Self::Uses => "uses",
            Self::Callers => "callers",
            Self::Callees => "callees",
            Self::Parent => "container",
            Self::Members => "members",
            Self::Implements => "implemented interface",
            Self::Flows => "data flow",
        }
    }
}

/// What a view did with Enter or Backspace.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Done,
    /// The shell should run this go-to on the cursor's target.
    GoTo(GoTo),
    /// Nothing to do, and why.
    Nothing(String),
}

/// Something in a view that search can find, in document order.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub key: String,
    /// Matched first, fuzzily (letters in order).
    pub name: String,
    /// Also searched: every word as written.
    pub text: String,
}

impl Item {
    pub fn new(key: impl Into<String>, name: impl Into<String>, text: impl Into<String>) -> Self {
        Self { key: key.into(), name: name.into(), text: text.into() }
    }
}

/// A drawn item hints can label, and where it is on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    pub key: String,
    pub rect: egui::Rect,
}

/// Indices of the items matching `query`, the way search matches everywhere
/// (`arch_search`): names first, best first (letters in order, the whole
/// name before part of it); then items whose text has every word as
/// written, best first.
pub fn rank_matches(items: &[Item], query: &str) -> Vec<usize> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let matching = arch_search::Matching::shared();
    let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
    let texts: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
    let mut out: Vec<usize> = arch_search::rank(query, &names, matching).into_iter().filter(|r| !names[r.index].is_empty()).map(|r| r.index).collect();
    let named: std::collections::HashSet<usize> = out.iter().copied().collect();
    out.extend(arch_search::rank_text(query, &texts).into_iter().map(|r| r.index).filter(|i| !named.contains(i)));
    out
}

/// The match to step to from the cursor. On a match: the next (or previous)
/// one in ranked order. Elsewhere: the first match after (or last before) the
/// cursor in document order. Wraps.
pub fn step_match(matches: &[usize], cursor: Option<usize>, forward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let n = matches.len();
    if let Some(at) = cursor.and_then(|c| matches.iter().position(|m| *m == c)) {
        return Some(matches[if forward { (at + 1) % n } else { (at + n - 1) % n }]);
    }
    let mut in_order = matches.to_vec();
    in_order.sort_unstable();
    Some(match (cursor, forward) {
        (Some(c), true) => in_order.iter().copied().find(|m| *m > c).unwrap_or(in_order[0]),
        (Some(c), false) => in_order.iter().rev().copied().find(|m| *m < c).unwrap_or(in_order[n - 1]),
        (None, true) => matches[0],
        (None, false) => matches[n - 1],
    })
}

/// Minimal, unique, prefix-free labels for `n` targets: all the same length,
/// the shortest that the alphabet allows.
pub fn hint_labels(n: usize, alphabet: &[char]) -> Vec<String> {
    let k = alphabet.len().max(1);
    if n == 0 || alphabet.is_empty() {
        return Vec::new();
    }
    let mut len = 1;
    while k.pow(len as u32) < n {
        len += 1;
    }
    (0..n)
        .map(|mut i| {
            let mut label = vec![alphabet[0]; len];
            for slot in (0..len).rev() {
                label[slot] = alphabet[i % k];
                i /= k;
            }
            label.into_iter().collect()
        })
        .collect()
}

/// How a list view scrolls to its cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    /// Bring the row to the middle (a jump, opening a target).
    Center,
    /// Scroll only as far as needed to keep the row on screen (moves).
    KeepVisible,
}

/// The scroll offset that shows row `row` (0-based) the way `how` asks,
/// given the row pitch, the current offset and the viewport height.
pub fn scroll_offset(how: Scroll, row: usize, pitch: f32, current: f32, view_h: f32) -> f32 {
    let top = row as f32 * pitch;
    // A couple of rows of context around a cursor kept in view.
    let margin = (2.0 * pitch).min((view_h - pitch).max(0.0) / 2.0);
    match how {
        Scroll::Center => (top - (view_h - pitch) / 2.0).max(0.0),
        Scroll::KeepVisible if top < current + margin => (top - margin).max(0.0),
        Scroll::KeepVisible if top + pitch > current + view_h - margin => (top + pitch + margin - view_h).max(0.0),
        Scroll::KeepVisible => current,
    }
}

/// What a search is doing, for views that narrow what they show while it's
/// typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchEvent<'a> {
    Typing(&'a str),
    Confirmed,
    Cancelled,
}

/// A view the universal keys drive.
pub trait Navigable {
    /// Where the view is now.
    fn place(&self) -> Place;

    /// Goes back to a place it reported (a jump, a mark).
    fn restore(&mut self, place: &Place);

    /// What the cursor is on.
    fn target(&self) -> Option<Target>;

    /// Moves the cursor (`count` times where that means something).
    fn motion(&mut self, motion: Motion, count: usize);

    /// Enter: go in, open, follow.
    fn enter(&mut self) -> Step;

    /// Whether Enter (or Backspace) would do something now (what the key
    /// bar offers).
    fn can_enter(&self) -> bool;

    /// What Enter does to what the cursor is on, in words ("Follow to
    /// save"); the view's general wording when `None`.
    fn enter_label(&self) -> Option<String> {
        None
    }

    fn can_leave(&self) -> bool;

    /// What Backspace does now, in words ("Up to crates"); the view's
    /// general wording when `None`.
    fn leave_label(&self) -> Option<String> {
        None
    }

    /// Backspace: go out, up.
    fn leave(&mut self) -> Step;

    fn fold(&mut self, fold: Fold);

    /// Everything search can find, in document order.
    fn items(&self) -> Vec<Item>;

    /// What is drawn now that hints can label (from the last frame).
    fn spots(&self) -> Vec<Spot>;

    /// The item the cursor is on (an `Item::key`).
    fn cursor_key(&self) -> Option<String>;

    /// Moves the cursor to an item and brings it into view.
    fn move_to(&mut self, key: &str);

    /// Opens or goes into an item (`F` on a hint).
    fn open_item(&mut self, key: &str) -> Step {
        self.move_to(key);
        self.enter()
    }

    /// A search in this view changed (views that narrow what they show).
    fn on_search(&mut self, _event: SearchEvent<'_>) {}

    /// Remembers how the view looks (zoom, folds) before a search previews
    /// matches, so giving up can put it back.
    fn save_view(&mut self) {}

    fn restore_view(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: usize) -> Jump {
        Jump { slot: 0, kind: BufferKind::Code, place: Place::Code { file: "a.rs".into(), line } }
    }

    #[test]
    fn the_jump_list_goes_back_and_forward_and_drops_what_was_ahead() {
        let mut list = JumpList::default();
        assert_eq!(list.back(at(1)), None, "nothing recorded");
        list.record(at(1));
        list.record(at(5));
        list.record(at(5));
        assert_eq!(list.back(at(9)), Some(at(5)), "back to where the last jump left");
        assert_eq!(list.back(at(5)), Some(at(1)));
        assert_eq!(list.back(at(1)), None, "the oldest");
        assert_eq!(list.forward(at(1)), Some(at(5)));
        assert_eq!(list.forward(at(5)), Some(at(9)), "forward returns to where back started");
        assert!(!list.can_forward());
        list.back(at(9));
        list.record(at(5));
        assert!(!list.can_forward(), "a new jump drops what was ahead");
        assert_eq!(list.back(at(20)), Some(at(5)));
        // Another view in another slot is just another entry.
        let graph = Jump { slot: 1, kind: BufferKind::Graph, place: Place::Tree { cursor: None } };
        list.record(graph.clone());
        assert_eq!(list.back(at(3)), Some(graph));
    }

    #[test]
    fn matches_rank_names_then_text_and_step_from_the_cursor() {
        let items = vec![Item::new("0", "stepper", ""), Item::new("1", "run", "calls step"), Item::new("2", "step", ""), Item::new("3", "log", "")];
        assert_eq!(rank_matches(&items, "step"), vec![2, 0, 1], "the whole name, then names, then text");
        assert!(rank_matches(&items, "STEP").is_empty(), "capitals ask for case (as everywhere)");
        assert_eq!(rank_matches(&items, "spr"), vec![0], "names: letters in order");
        assert!(rank_matches(&items, "cls").is_empty(), "text: words as written");
        assert!(rank_matches(&items, "").is_empty());
        let found = rank_matches(&items, "step");
        assert_eq!(step_match(&found, Some(2), true), Some(0), "on a match: next in rank order");
        assert_eq!(step_match(&found, Some(1), true), Some(2), "wraps");
        assert_eq!(step_match(&found, Some(3), true), Some(0), "off a match: first after the cursor, wrapping");
        let lines = vec![Item::new("1", "", "a"), Item::new("2", "", "xa"), Item::new("3", "", "b"), Item::new("4", "", "a")];
        let found = rank_matches(&lines, "a");
        assert_eq!(step_match(&found, Some(2), true), Some(3), "lines: the next one below");
        assert_eq!(step_match(&found, Some(2), false), Some(1), "and the one above");
    }

    #[test]
    fn hint_labels_are_unique_minimal_and_prefix_free() {
        let alphabet: Vec<char> = "asdf".chars().collect();
        assert_eq!(hint_labels(3, &alphabet), vec!["a", "s", "d"]);
        let many = hint_labels(10, &alphabet);
        assert!(many.iter().all(|l| l.len() == 2));
        let unique: std::collections::BTreeSet<&String> = many.iter().collect();
        assert_eq!(unique.len(), 10);
        assert!(hint_labels(0, &alphabet).is_empty());
    }

    #[test]
    fn scroll_offsets_center_targets_and_keep_the_cursor_visible() {
        assert_eq!(scroll_offset(Scroll::Center, 0, 20.0, 0.0, 400.0), 0.0);
        assert_eq!(scroll_offset(Scroll::Center, 100, 20.0, 0.0, 420.0), 2000.0 - 200.0);
        assert_eq!(scroll_offset(Scroll::KeepVisible, 9, 20.0, 0.0, 400.0), 0.0, "already visible");
        assert_eq!(scroll_offset(Scroll::KeepVisible, 29, 20.0, 0.0, 400.0), 600.0 + 40.0 - 400.0, "just below: scroll by what's needed, with context");
        assert_eq!(scroll_offset(Scroll::KeepVisible, 4, 20.0, 500.0, 400.0), 40.0, "above: bring near the top, with context");
    }

    #[test]
    fn targets_resolve_to_components_and_code() {
        let a = ComponentId::new("rust:a::run");
        let b = ComponentId::new("rust:a::step");
        assert_eq!(Target::Component(a.clone()).component(), Some(&a));
        let line = Target::Line { file: "src/a.rs".into(), line: 4, within: Some(a.clone()), refs: vec![b.clone()] };
        assert_eq!((line.component(), line.code()), (Some(&a), Some(("src/a.rs".into(), Some(4)))));
        let relation = Target::Spec { component: a.clone(), section: Section::Relations, item: SpecItem::Relation { kind: RelationKind::Calls, other: b.clone(), outgoing: true } };
        assert_eq!(relation.component(), Some(&b), "a relation points at its other end");
        let step = Target::Spec { component: a.clone(), section: Section::Logic, item: SpecItem::Step(0) };
        assert_eq!(step.component(), Some(&a));
        assert_eq!(Target::Job { component: b.clone() }.component(), Some(&b));
        let message = Target::Message { thread: None, index: 2, link: Some(("src/b.rs".into(), Some(7))) };
        assert_eq!((message.component(), message.code()), (None, Some(("src/b.rs".into(), Some(7)))));
    }
}
