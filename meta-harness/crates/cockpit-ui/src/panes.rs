//! Every column's own view. The cockpit works on one view of each kind at a
//! time (`CockpitApp::graph`, `code`, `spec`, `tree`, the chat's `ui`): the
//! one used last.
//! The others wait here, by their pane's number, until their column is
//! used or drawn; then they change places with the one in use.

use std::collections::BTreeMap;

use crate::chat::ChatView;
use crate::code::CodeViewerState;
use crate::graph::GraphState;
use crate::spec::SpecState;
use crate::tree::ArchitectureTreeState;

#[derive(Default)]
pub struct Parked {
    pub graphs: BTreeMap<u32, GraphState>,
    pub codes: BTreeMap<u32, CodeViewerState>,
    pub specs: BTreeMap<u32, SpecState>,
    pub trees: BTreeMap<u32, ArchitectureTreeState>,
    pub chats: BTreeMap<u32, ChatView>,
}

impl Parked {
    /// Drops the views of panes no column has any more (and that aren't
    /// put away to come back).
    pub fn keep_only(&mut self, kept: impl Fn(crate::app::BufferKind, u32) -> bool) {
        use crate::app::BufferKind as K;
        self.graphs.retain(|id, _| kept(K::Graph, *id));
        self.codes.retain(|id, _| kept(K::Code, *id));
        self.specs.retain(|id, _| kept(K::Spec, *id));
        self.trees.retain(|id, _| kept(K::Tree, *id));
        self.chats.retain(|id, _| kept(K::Chat, *id));
    }
}

/// Puts view `id` in `live` (a new one if it has none yet) and parks the one
/// that was there as `current`. True when the view is new.
pub fn swap_view<T: Default>(live: &mut T, parked: &mut BTreeMap<u32, T>, current: u32, id: u32) -> bool {
    let (next, new) = match parked.remove(&id) {
        Some(view) => (view, false),
        None => (T::default(), true),
    };
    parked.insert(current, std::mem::replace(live, next));
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_change_places_and_a_new_one_says_so() {
        let mut live = String::from("first");
        let mut parked = BTreeMap::new();
        assert!(swap_view(&mut live, &mut parked, 0, 1), "pane 1 had no view yet");
        assert_eq!((live.as_str(), parked.get(&0).map(String::as_str)), ("", Some("first")));
        live.push_str("second");
        assert!(!swap_view(&mut live, &mut parked, 1, 0));
        assert_eq!((live.as_str(), parked.get(&1).map(String::as_str)), ("first", Some("second")));
    }
}
