//! The cards the graph holds: the tree the server sent, and what opening a
//! card changed since. The server walks the branch, keeps the whole to its
//! card cap (closing the opened branches furthest away) and lays it out;
//! the cockpit takes the new cards and every card's place, and places
//! nothing itself.

use arch_graph::tree::{Expanded, Tree};

use super::request::Nav;

/// Takes `change` into the tree held and the request it answers (which
/// cards are open, which were closed to keep to the cap). Returns the names
/// of the cards closed again.
pub fn take(tree: &mut Tree, nav: &mut Nav, change: Expanded) -> Vec<String> {
    let closed: Vec<String> = change.closed.iter().map(|k| tree.card(k).map_or_else(|| k.clone(), |c| c.label.clone())).collect();
    nav.opened = change.request.opened.clone();
    nav.closed = change.request.closed.clone();
    nav.depth = change.request.depth;
    arch_graph::tree::apply(tree, change);
    closed
}

impl super::GraphState {
    /// Takes in what opening a card changed (the server merged it in, kept
    /// to its cap and laid it out), saying what was closed to keep to it.
    pub(super) fn take_change(&mut self, change: arch_graph::tree::Expanded) {
        let Some(tree) = &mut self.tree else {
            self.stale = true;
            return;
        };
        let closed = take(tree, &mut self.nav, change);
        if !closed.is_empty() {
            self.note = Some(format!("Closed {} again to keep the graph to its size.", closed.join(", ")));
        }
        self.at = tree.cards.iter().enumerate().map(|(i, c)| (c.key.clone(), i)).collect();
        self.land_on_first();
    }

    /// Opened to go to its first card: there, once the cards are in (a
    /// whole tree or just the opened branch); none came: it says so.
    pub(super) fn land_on_first(&mut self) {
        let (Some(tree), Some(parent)) = (&self.tree, self.first_child_of.take()) else { return };
        match super::select::children(tree, &parent).into_iter().next() {
            Some(first) => {
                self.selection = Some(first.clone());
                self.reveal_after_fit = Some((first, crate::viewport::Show::InView));
            }
            None => self.note = Some(format!("Opened {}: nothing more came to show.", tree.card(&parent).map_or(parent.as_str(), |c| c.label.as_str()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use arch_graph::tree::{TreeRequest, ViewKind};

    use super::*;
    use crate::graph::tests::{change_of, sample_graph, tree_of};

    #[test]
    fn opening_takes_the_servers_cards_and_places_and_lays_out_nothing_here() {
        let request = TreeRequest { view: ViewKind::Contains, depth: Some(0), ..TreeRequest::default() };
        let mut tree = tree_of(&sample_graph(), None, request.clone());
        let mut nav = Nav { view: request.view, depth: request.depth, ..Nav::default() };
        let (a, b) = ("rust:a", "rust:b");
        // Under the cap: a's functions come in, placed by the server.
        let change = change_of(&sample_graph(), &tree, a, 100);
        let server_places: Vec<(String, arch_graph::tree::Rect)> = change.places.iter().map(|p| (p.key.clone(), p.rect)).collect();
        assert!(take(&mut tree, &mut nav, change).is_empty());
        assert_eq!(tree.cards.iter().filter(|c| c.parent.as_deref() == Some(a)).count(), 4, "a's functions are in");
        assert!(tree.cards.iter().all(|c| server_places.contains(&(c.key.clone(), c.rect))), "every card where the server put it");
        assert_eq!(nav.opened.iter().map(|o| o.card.as_str()).collect::<Vec<_>>(), [a]);
        // Over it: the server closes a's branch (opened first, furthest from b).
        let held = tree.cards.len();
        let change = change_of(&sample_graph(), &tree, b, held);
        assert_eq!(change.closed, [a]);
        let closed = take(&mut tree, &mut nav, change);
        assert_eq!(closed, ["a"], "by name");
        assert!(tree.card("rust:a::run").is_none() && tree.card("rust:b::util").is_some());
        let card = tree.card(a).unwrap();
        assert!(!card.open && card.more == 4, "it shows what it holds again");
        assert_eq!(nav.opened.iter().map(|o| o.card.as_str()).collect::<Vec<_>>(), [b]);
    }
}
