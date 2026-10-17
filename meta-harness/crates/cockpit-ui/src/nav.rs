//! Navigation every view shares: what a cursor is on (`Target`), where a view
//! is (`Place`, so it can be returned to), the jump list behind ctrl+o /
//! ctrl+i, the motions and folds the universal keys ask for, and the items a
//! view offers to search (`Item`) and to hints (`Spot`). Views implement
//! `Navigable`; the shell owns the keys, the history, marks, search and hints.

use arch_graph::{CommentId, ComponentId, RelationKind};
use harness_contracts::{NotificationId, SlotId, ThreadId};

use crate::app::BufferKind;
use crate::graph::Nav;
use crate::spec::Section;

/// A component's name from its id alone (`rust:a::b::run` → `run`), for
/// when nothing here knows its card.
pub fn short_name(id: &ComponentId) -> String {
    let symbol = id.as_str().split_once(':').map_or(id.as_str(), |(_, s)| s);
    symbol.rsplit([':', '.']).next().unwrap_or(symbol).to_owned()
}

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
    /// A message in the chat: its turn, and the file it points to (if any).
    Message { thread: Option<String>, turn: Option<String>, index: usize, link: Option<(String, Option<usize>)> },
    /// A change (git): the file, at its first changed line.
    Change { file: String, line: usize },
    /// Lines `start..=end` of a file (where a component's code is).
    Span { file: String, start: usize, end: usize },
    /// An agent's chat: its slot, or the slot running a session.
    Agent { slot: Option<SlotId>, thread: Option<ThreadId> },
}

/// What a buffer asks of the rest of the cockpit: the one way it reaches
/// past itself. The shell takes every buffer's asks (`take_asks`) and does
/// each the same way, whichever buffer asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// Show `target` in a buffer of kind `in_`: the go-to path (whether it
    /// can, which panel, focus, the jump list).
    Show { target: Target, in_: BufferKind },
    /// The component the user is on now: buffers that follow it move to it.
    Follow(ComponentId),
    /// Read this file into the index again.
    Reread(String),
    /// A typing box opened in it: the keyboard goes there.
    Type,
    /// Something shared changed where it's kept: read it again.
    Refresh(Refresh),
    /// The desk compares this from now on (what the changes show).
    Compare(harness_contracts::changes::Comparison),
    /// What waits on the user, across servers (the inbox's).
    Waiting(Waiting),
    /// The source the user picked (the graph's): every buffer reads from it.
    Source(arch_graph::Source),
    /// Where the server says a component's code is now: buffers showing it
    /// may be out of date.
    Located { id: ComponentId, anchor: Option<arch_graph::CodeAnchor> },
    /// Something for the agents to do, for agent slot `slot` (where it
    /// shows; its errors show there).
    Agents { slot: SlotId, act: crate::agents::Act },
}

/// What's read again on a buffer's word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    /// The plan's review (a comment or a review went through).
    Review,
    /// The desk's comparison (files were staged).
    Changes,
}

/// What the inbox asks about what waits on the user.
#[derive(Clone, Debug, PartialEq)]
pub enum Waiting {
    /// The next (or previous) item, on whichever server it is.
    Step(i32),
    /// Put it away.
    Dismiss(NotificationId),

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
    /// One of the changes in it (the changes section).
    Hunk(usize),
    Comment(CommentId),
    Reply(CommentId, usize),
}

impl Target {
    /// What it is, in a few words ("run", "a.rs:12", "message 3").
    pub fn in_words(&self) -> String {
        let file = |path: &str| path.rsplit('/').next().unwrap_or(path).to_owned();
        match self {
            Self::Component(id) | Self::Job { component: id } | Self::Spec { component: id, .. } => short_name(id),
            Self::Line { file: path, line, .. } | Self::Change { file: path, line } | Self::Span { file: path, start: line, .. } => format!("{}:{line}", file(path)),
            Self::Message { index, .. } => format!("message {}", index + 1),
            Self::Agent { slot: Some(slot), .. } => format!("agent slot {slot}"),
            Self::Agent { thread, .. } => format!("session {}", thread.as_ref().map_or("?", |t| t.as_str())),
        }
    }

    /// The component the target is about: for a relation in the spec, the
    /// other end (what the cursor points at).
    pub fn component(&self) -> Option<&ComponentId> {
        match self {
            Self::Component(id) | Self::Job { component: id } => Some(id),
            Self::Line { within, .. } => within.as_ref(),
            Self::Spec { item: SpecItem::Relation { other, .. }, .. } => Some(other),
            Self::Spec { component, .. } => Some(component),
            Self::Message { .. } | Self::Change { .. } | Self::Span { .. } | Self::Agent { .. } => None,
        }
    }

    /// Whether it is (or is in) a component: a line of code is in some
    /// component, even before the index says which.
    pub fn in_component(&self) -> bool {
        self.component().is_some() || matches!(self, Self::Line { .. })
    }

    /// A place in the code (a line, a change, lines): the index can say
    /// which component it's in.
    pub fn in_code(&self) -> bool {
        matches!(self, Self::Line { .. } | Self::Change { .. } | Self::Span { .. })
    }

    /// Whether it's about another component than `at` (a view's cursor),
    /// or about no component at all.
    pub fn elsewhere(&self, at: Option<&Target>) -> bool {
        self.component().is_none() || at.and_then(Target::component) != self.component()
    }

    /// A place in the code the target names directly (a line, a file link).
    pub fn code(&self) -> Option<(String, Option<usize>)> {
        match self {
            Self::Line { file, line, .. } | Self::Change { file, line } | Self::Span { file, start: line, .. } => Some((file.clone(), Some(*line))),
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
    /// The item shown, by its id.
    Inbox { item: Option<String> },
    /// The cursor's file or change, by its key.
    Changes { item: Option<String> },
}

/// A place in a panel: what the jump list and marks hold.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Jump {
    pub panel: usize,
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
    /// Half the window sideways (views wider than their window).
    HalfLeft,
    HalfRight,
    /// A few columns sideways.
    FewLeft,
    FewRight,
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

/// The moves every view shares: one vocabulary, each with its fixed id
/// (what the keymap binds), run the same way for every view (the app's
/// `run_move`: counts, the jump list, then the view's `Navigable`). A view
/// offers the ones that apply where its cursor is, in its own words
/// (`Move::offer`), with its own actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Motion(Motion),
    Enter,
    Leave,
    Fold(Fold),
}

impl Move {
    /// Its action's id.
    pub fn id(self) -> &'static str {
        match self {
            Self::Motion(Motion::Down) => "nav.down",
            Self::Motion(Motion::Up) => "nav.up",
            Self::Motion(Motion::Left) => "nav.left",
            Self::Motion(Motion::Right) => "nav.right",
            Self::Motion(Motion::HalfDown) => "nav.half_down",
            Self::Motion(Motion::HalfUp) => "nav.half_up",
            Self::Motion(Motion::HalfLeft) => "nav.half_left",
            Self::Motion(Motion::HalfRight) => "nav.half_right",
            Self::Motion(Motion::FewLeft) => "nav.few_left",
            Self::Motion(Motion::FewRight) => "nav.few_right",
            Self::Motion(Motion::First) => "nav.first",
            Self::Motion(Motion::Last) => "nav.last",
            Self::Motion(Motion::NextSibling) => "nav.next_sibling",
            Self::Motion(Motion::PrevSibling) => "nav.prev_sibling",
            Self::Motion(Motion::Line(_)) => "nav.line",
            Self::Enter => "nav.enter",
            Self::Leave => "nav.leave",
            Self::Fold(Fold::Open) => "nav.fold_open",
            Self::Fold(Fold::Close) => "nav.fold_close",
            Self::Fold(Fold::Toggle) => "nav.fold_toggle",
            Self::Fold(Fold::OpenAll) => "nav.fold_open_all",
            Self::Fold(Fold::CloseAll) => "nav.fold_close_all",
        }
    }

    /// How much it matters where it's offered (the key bar shows the most
    /// important first); a view can say otherwise.
    fn importance(self) -> u8 {
        use crate::actions::{MINOR, PRIMARY, SECONDARY};
        match self {
            Self::Enter => PRIMARY,
            Self::Leave | Self::Motion(Motion::Down | Motion::Up) | Self::Fold(Fold::Toggle) => SECONDARY,
            Self::Motion(_) | Self::Fold(_) => MINOR,
        }
    }

    /// The move as an action of the view offering it, in its words.
    pub fn offer(self, label: impl Into<String>) -> crate::actions::Action {
        use crate::actions::{Action, Group};
        let group = if matches!(self, Self::Fold(_)) { Group::View } else { Group::Navigate };
        Action::new(self.id(), label, self).group(group).importance(self.importance()).focused()
    }

    /// Offered whether or not it applies where the cursor is: when it
    /// doesn't, it isn't shown, and its key says why (the view's `Step` or
    /// note).
    pub fn offer_if(self, applies: bool, label: impl Into<String>) -> crate::actions::Action {
        let offer = self.offer(label);
        if applies { offer } else { offer.when(crate::actions::When::Unavailable) }
    }
}

/// The moves through a list every list view offers: down and up, half a
/// page, first and last.
pub fn list_moves() -> Vec<crate::actions::Action> {
    let m = |motion: Motion, label: &str| Move::Motion(motion).offer(label);
    vec![m(Motion::Down, "Down"), m(Motion::Up, "Up"), m(Motion::HalfDown, "Half a page down"), m(Motion::HalfUp, "Half a page up"), m(Motion::First, "First"), m(Motion::Last, "Last")]
}

/// Folding what the cursor is on: `here` is what it is, in words, and
/// whether it's folded, when it folds. Then its toggle (on the item too)
/// and the way it can go now apply; otherwise they're offered but don't.
pub fn fold_here(here: Option<(&str, bool)>) -> Vec<crate::actions::Action> {
    use crate::actions::Style;
    let Some((what, folded)) = here else {
        return vec![Move::Fold(Fold::Toggle).offer_if(false, "Fold or unfold"), Move::Fold(Fold::Open).offer_if(false, "Unfold"), Move::Fold(Fold::Close).offer_if(false, "Fold")];
    };
    let (word, icon) = if folded { ("Unfold", egui_phosphor::regular::CARET_RIGHT) } else { ("Fold", egui_phosphor::regular::CARET_DOWN) };
    vec![
        Move::Fold(Fold::Toggle).offer(format!("{word} {what}")).on_item(Style::Button, word, Some(icon)),
        Move::Fold(Fold::Open).offer_if(folded, format!("Unfold {what}")),
        Move::Fold(Fold::Close).offer_if(!folded, format!("Fold {what}")),
    ]
}

/// Unfolding everything (when something's folded) and folding everything
/// (when something isn't), each in the view's words.
pub fn fold_all((any_folded, unfold): (bool, &str), (any_open, fold): (bool, &str)) -> Vec<crate::actions::Action> {
    vec![Move::Fold(Fold::OpenAll).offer_if(any_folded, unfold), Move::Fold(Fold::CloseAll).offer_if(any_open, fold)]
}

/// Where a go-to key sends the cursor's target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoTo {
    Code,
    Spec,
    Graph,
    Tree,
    Queue,
    /// What the desk's comparison changed in it (the Changes view).
    Changes,
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
    pub const ALL: [GoTo; 13] = [Self::Code, Self::Spec, Self::Graph, Self::Tree, Self::Queue, Self::Changes, Self::Uses, Self::Callers, Self::Callees, Self::Parent, Self::Members, Self::Implements, Self::Flows];

    /// Its action's id (what the keymap binds).
    pub fn id(self) -> &'static str {
        match self {
            Self::Code => "goto.code",
            Self::Spec => "goto.spec",
            Self::Graph => "goto.graph",
            Self::Tree => "goto.tree",
            Self::Queue => "goto.queue",
            Self::Changes => "goto.changes",
            Self::Uses => "goto.uses",
            Self::Callers => "goto.callers",
            Self::Callees => "goto.callees",
            Self::Parent => "goto.parent",
            Self::Members => "goto.members",
            Self::Implements => "goto.implements",
            Self::Flows => "goto.flows",
        }
    }

    /// What it does, in words.
    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Go to its code",
            Self::Spec => "Go to its spec",
            Self::Graph => "Show it in the graph",
            Self::Tree => "Show it in the outline",
            Self::Queue => "Go to its queue job",
            Self::Changes => "Go to its changes",
            Self::Uses => "Go to a use",
            Self::Callers => "Go to a caller",
            Self::Callees => "Go to a callee",
            Self::Parent => "Go to what holds it",
            Self::Members => "Go to a member",
            Self::Implements => "Go to what it implements",
            Self::Flows => "Follow its data flow",
        }
    }

    pub fn importance(self) -> u8 {
        use crate::actions::{MINOR, PRIMARY, SECONDARY};
        match self {
            Self::Code => PRIMARY,
            Self::Spec | Self::Graph | Self::Uses | Self::Callers | Self::Callees => SECONDARY,
            Self::Tree | Self::Queue | Self::Changes | Self::Parent | Self::Members | Self::Implements | Self::Flows => MINOR,
        }
    }

    /// The view it shows the target in, when it's one view's (the
    /// relational ones pick among what's related, and land where they
    /// started or in the code).
    pub fn view(self) -> Option<BufferKind> {
        match self {
            Self::Code => Some(BufferKind::Code),
            Self::Spec => Some(BufferKind::Spec),
            Self::Graph => Some(BufferKind::Graph),
            Self::Tree => Some(BufferKind::Tree),
            Self::Queue => Some(BufferKind::Queue),
            Self::Changes => Some(BufferKind::Changes),
            Self::Uses | Self::Callers | Self::Callees | Self::Parent | Self::Members | Self::Implements | Self::Flows => None,
        }
    }

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
            Self::Changes => "changes",
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

    /// Enter: go in, open, follow (offered, in its words, with its actions
    /// when it applies: `Move::Enter`).
    fn enter(&mut self) -> Step;

    /// Backspace: go out, up (`Move::Leave`).
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

    /// As a go-to's destination: whether it can show `target`, and isn't
    /// showing it at its cursor already.
    fn can_show(&self, _target: &Target) -> bool {
        false
    }

    /// Shows `target` at its cursor (a go-to arrived); why not, when it
    /// can't.
    fn show_target(&mut self, _target: &Target) -> Result<(), String> {
        Err("This buffer doesn't show things gone to.".into())
    }

    /// What it asked of the rest of the cockpit since it was last asked
    /// (the shell takes them every frame and after every action).
    fn take_asks(&mut self) -> Vec<Ask> {
        Vec::new()
    }

    /// The component the user is on now (`Ask::Follow`, from any buffer):
    /// a buffer that follows it moves to it.
    fn follow(&mut self, _id: &ComponentId) {}

    /// What the buffers show in common (every frame, shown or parked): it
    /// takes what it needs, when that changed (`Kept::since`).
    fn share(&mut self, _shared: &crate::shared::Shared) {}

    /// Opens or goes into an item (`F` on a hint).
    fn open_item(&mut self, key: &str) -> Step {
        self.move_to(key);
        self.enter()
    }

    /// A finger gesture on its content, for a view that reads its canvas
    /// itself (`Panel::own_pointer`): taps, long presses, two fingers.
    fn gesture(&mut self, _gesture: crate::touch::Gesture) {}

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
        Jump { panel: 0, kind: BufferKind::Code, place: Place::Code { file: "a.rs".into(), line } }
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
        let graph = Jump { panel: 1, kind: BufferKind::Graph, place: Place::Tree { cursor: None } };
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
        let message = Target::Message { thread: None, turn: None, index: 2, link: Some(("src/b.rs".into(), Some(7))) };
        assert_eq!((message.component(), message.code()), (None, Some(("src/b.rs".into(), Some(7)))));
    }
}
