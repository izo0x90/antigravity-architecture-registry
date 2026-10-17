//! What the buffers show in common, kept once on the desk: the source in
//! use, the plan's review, the component types, the index, the user and
//! the UI settings buffers read. The shell keeps it up to date; every
//! buffer, shown or parked, reads what it needs from it each frame
//! (`Navigable::share`), skipping what it has (each part counts its
//! changes), and changes it only by asking.

use arch_graph::{IndexStatus, Source, TypeSystem};

use crate::config::{ChangesUi, CodeUi, UiConfig};
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

/// The settings buffers read, from the UI config.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub graph: GraphUi,
    pub code: CodeUi,
    pub changes: ChangesUi,
}

impl Settings {
    pub fn of(config: &UiConfig) -> Self {
        Self { graph: config.graph.clone(), code: config.code.clone(), changes: config.changes.clone() }
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
    /// The plan's comments and what changed since it was reviewed.
    pub review: ReviewState,
    /// The component types (each kind's look and its sections).
    pub types: Kept<Option<TypeSystem>>,
    /// Reading the code into the index: running, done, the files changed since.
    pub index: Kept<IndexStatus>,
    /// The cockpit's user (who comments, approves, reviews).
    pub user: Kept<String>,
    pub settings: Kept<Settings>,
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
