//! What the buffers show in common, kept once on the desk: the source in
//! use, the plan's review (its discussions, what changed), the component types, the index, the user and
//! the UI settings buffers read. The shell keeps it up to date; every
//! buffer, shown or parked, reads what it needs from it each frame
//! (`Navigable::share`), skipping what it has (each part counts its
//! changes), and changes it only by asking.

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use arch_graph::{IndexStatus, Source, TypeSystem};

use crate::config::{ChangesUi, ChatUi, CodeUi, UiConfig};
use crate::graph_config::GraphUi;
use crate::review::ReviewState;

/// A shared value and how many times it changed: a reader keeps the count
/// it took the value at and takes it again only when that moved.
#[derive(Debug, Default)]
pub struct Kept<T> {
    value: T,
    changes: u64,
}

impl<T: PartialEq> Kept<T> {
    /// Keeps `value` (a change only when it differs).
    pub fn set(&mut self, value: T) {
        if self.value != value {
            self.value = value;
            self.changes += 1;
        }
    }

    pub fn get(&self) -> &T {
        &self.value
    }

    /// The value when it changed since `seen` (which moves to now).
    pub fn since(&self, seen: &mut u64) -> Option<&T> {
        (std::mem::replace(seen, self.changes) != self.changes).then_some(&self.value)
    }
}

/// The colour each link is drawn in: a tag's own (the shell says, from
/// the tags), anything else the accent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkColours(std::collections::BTreeMap<harness_contracts::TagId, egui::Color32>);

impl LinkColours {
    pub fn new(tags: std::collections::BTreeMap<harness_contracts::TagId, egui::Color32>) -> Self {
        Self(tags)
    }

    pub fn of(&self, to: &harness_contracts::Linked) -> egui::Color32 {
        match to {
            harness_contracts::Linked::Tag { tag } => self.0.get(tag).copied().unwrap_or(crate::theme::colors().accent),
            harness_contracts::Linked::Thing { .. } => crate::theme::colors().accent,
        }
    }
}

/// A service the desk keeps (the agents, the terminals), held by the
/// shell: it alone changes it; buffers get a `Reader`.
pub struct Held<T>(Rc<RefCell<T>>);

impl<T: Default> Default for Held<T> {
    fn default() -> Self {
        Self(Rc::default())
    }
}

impl<T> Held<T> {
    pub fn get(&self) -> Ref<'_, T> {
        self.0.borrow()
    }

    /// To change it: the shell's alone (it holds it mutably).
    pub fn get_mut(&mut self) -> RefMut<'_, T> {
        self.0.borrow_mut()
    }

    /// A reader for a buffer.
    pub fn reader(&self) -> Reader<T> {
        Reader(self.0.clone())
    }
}

/// A buffer's reader of a service the desk keeps (it changes it only by
/// asking).
pub struct Reader<T>(Rc<RefCell<T>>);

impl<T: Default> Default for Reader<T> {
    fn default() -> Self {
        Self(Rc::default())
    }
}

impl<T> Clone for Reader<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Reader<T> {
    pub fn get(&self) -> Ref<'_, T> {
        self.0.borrow()
    }

    /// Whether it reads `held`.
    pub fn reads(&self, held: &Held<T>) -> bool {
        Rc::ptr_eq(&self.0, &held.0)
    }

    /// Reads `held` from now on (when it reads another).
    pub fn follow(&mut self, held: &Held<T>) {
        if !self.reads(held) {
            *self = held.reader();
        }
    }
}

/// The settings buffers read, from the UI config.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub graph: GraphUi,
    pub code: CodeUi,
    pub changes: ChangesUi,
    pub chat: ChatUi,
}

impl Settings {
    pub fn of(config: &UiConfig) -> Self {
        Self { graph: config.graph.clone(), code: config.code.clone(), changes: config.changes.clone(), chat: config.chat.clone() }
    }
}

/// The changes of each shared part a buffer took last (`Kept::since`).
#[derive(Clone, Debug, Default)]
pub struct Seen {
    pub source: u64,
    pub review: u64,
    pub types: u64,
    pub index: u64,
    pub user: u64,
    pub settings: u64,
}

#[derive(Default)]
pub struct Shared {
    /// The code alone, or with one plan (what the graph used last shows).
    pub source: Kept<Source>,
    /// The plan's discussions and what changed since it was reviewed.
    pub review: ReviewState,
    /// The component types (each kind's look and its sections).
    pub types: Kept<Option<TypeSystem>>,
    /// Reading the code into the index: running, done, the files changed since.
    pub index: Kept<IndexStatus>,
    /// The cockpit's user (who approves and reviews).
    pub user: Kept<String>,
    pub settings: Kept<Settings>,
    /// The desk's comparison and what it changes (files, components, when).
    pub changes: crate::changes::Comparing,
    /// The server's agents: slots, sessions, harnesses, what waits on the user.
    pub agents: Held<crate::agents::Agents>,
    /// The server's terminals: which, and how each is.
    pub terminals: Held<crate::terminals::Terminals>,
    /// The server's tags: each one and what it's on.
    pub tags: Held<crate::tags::Tags>,
    /// The server's discussions, each whole.
    pub discussions: Held<crate::discussions::Discussions>,
    /// The chats' unsent text, by slot (its own record on the server).
    pub drafts: Held<crate::drafts::Drafts>,
    /// Code blocks' colours, as the server reads them.
    pub highlighter: Held<crate::highlighter::Highlighter>,
    /// Calls' changes, as the server compares them.
    pub comparer: Held<crate::comparer::Comparer>,
    /// Each link's colour (a tag's own), as the shell says from the tags.
    pub link_colours: Kept<LinkColours>,
    /// The desk's server (`here`: home's own).
    pub server: String,
    /// Every server's open notifications, in the order they're gone through.
    pub open: Kept<Vec<(String, harness_contracts::NotificationId)>>,
}

impl Shared {
    /// The review when it changed since `seen` (which moves to now).
    pub fn review_since(&self, seen: &mut u64) -> Option<&ReviewState> {
        (std::mem::replace(seen, self.review.generation) != self.review.generation).then_some(&self.review)
    }
}

/// Shared state showing plan `plan` (tests).
#[cfg(test)]
pub(crate) fn on_plan(plan: &str) -> Shared {
    let mut shared = Shared::default();
    shared.source.set(Source::Plan(arch_graph::PlanId::new(plan).unwrap()));
    shared
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_sees_what_the_shell_changes_and_follows_another_desks() {
        let mut held = Held::<Vec<u32>>::default();
        let mut reader = Reader::default();
        assert!(!reader.reads(&held));
        reader.follow(&held);
        held.get_mut().push(7);
        assert_eq!(*reader.get(), [7]);
        let other = Held::<Vec<u32>>::default();
        reader.follow(&other);
        assert!(reader.reads(&other) && reader.get().is_empty());
    }

    #[test]
    fn a_reader_takes_a_value_once_per_change() {
        let mut kept = Kept::<u32>::default();
        let mut seen = 0;
        assert_eq!(kept.since(&mut seen), None, "nothing new");
        kept.set(3);
        assert_eq!(kept.since(&mut seen), Some(&3));
        assert_eq!(kept.since(&mut seen), None, "taken");
        kept.set(3);
        assert_eq!(kept.since(&mut seen), None, "the same value isn't a change");
        let mut late = 0;
        assert_eq!(kept.since(&mut late), Some(&3), "another reader takes it too");
    }
}
