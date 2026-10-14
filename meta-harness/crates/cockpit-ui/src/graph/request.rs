//! Where the graph looks, and the one reducer that changes it. Every change
//! comes from something the user (or the agent) did: picking a view or a
//! source, going into a card or back, more or fewer levels, flipping the
//! way, opening or closing a card. Nothing here changes on its own.

use arch_graph::tree::{Opened, TreeRequest, ViewKind, Way};
use arch_graph::{Source, ViewRequest};

use crate::nav::Navigable;

/// What the graph shows: one view, over the code or one plan, from a root
/// (or the view's starting points), one way, to a depth, with the cards
/// opened or closed by hand. Two equal `Nav`s ask for the same tree.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Nav {
    pub view: ViewKind,
    /// The code alone, or the code with one plan laid over it.
    pub source: Source,
    pub root: Option<String>,
    pub way: Way,
    /// None: the view's own depth.
    pub depth: Option<u32>,
    /// Cards opened further than the depth, oldest first.
    pub opened: Vec<Opened>,
    pub closed: Vec<String>,
    /// With a plan: shown in the whole codebase.
    pub full_code: bool,
}

impl Nav {
    pub fn request(&self) -> TreeRequest {
        TreeRequest {
            view: self.view,
            source: self.source.clone(),
            root: self.root.clone(),
            way: self.way,
            depth: self.depth,
            opened: self.opened.clone(),
            closed: self.closed.clone(),
            full_code: self.full_code,
        }
    }

    /// Hand-opened and hand-closed cards start over (they belonged to the
    /// tree as it was).
    fn reset_folds(&mut self) {
        self.opened.clear();
        self.closed.clear();
    }
}

/// Every change to where the graph looks.
#[derive(Clone, Debug, PartialEq)]
pub enum NavChange {
    /// Another view; `root` is the card it starts from there (if any).
    View { view: ViewKind, root: Option<String> },
    /// The code, or the code with a plan.
    Source(Source),
    /// With a plan: in the whole codebase, or only the plan and what it
    /// plugs into.
    FullCode(bool),
    /// Make a card the root.
    GoIn(String),
    /// More (+) or fewer (-) levels from `base` (the depth shown now).
    Depth { delta: i32, base: u32 },
    /// Down becomes up and up becomes down.
    Flip,
    /// Show one card's children (one level further).
    Open(String),
    /// Hide one card's children.
    Close(String),
    /// The view's starting points again (no root).
    Top,
    /// What the agent asked to show.
    Request(Box<ViewRequest>),
    /// Back to a recorded place (the trail, the jump list, the agent).
    Restore(Box<Nav>),
}

/// Applies `change` to `nav`.
pub fn apply(nav: &mut Nav, change: NavChange) {
    match change {
        NavChange::View { view, root } => {
            nav.view = view;
            nav.root = root;
            nav.reset_folds();
        }
        NavChange::Source(source) => {
            nav.source = source;
            nav.full_code = false;
            nav.reset_folds();
        }
        NavChange::FullCode(full) => {
            nav.full_code = full;
            nav.reset_folds();
        }
        NavChange::GoIn(card) => {
            nav.root = Some(card);
            nav.reset_folds();
        }
        NavChange::Depth { delta, base } => {
            nav.depth = Some((base as i32 + delta).max(0) as u32);
            nav.reset_folds();
        }
        NavChange::Flip => {
            nav.way = nav.way.flipped();
            nav.reset_folds();
        }
        NavChange::Open(card) => {
            nav.closed.retain(|c| c != &card);
            nav.opened.retain(|o| o.card != card);
            nav.opened.push(Opened { card, levels: 1 });
        }
        NavChange::Close(card) => {
            nav.opened.retain(|o| o.card != card);
            if !nav.closed.contains(&card) {
                nav.closed.push(card);
            }
        }
        NavChange::Top => {
            nav.root = None;
            nav.reset_folds();
        }
        NavChange::Request(request) => {
            if let Some(view) = request.view {
                nav.view = view;
            }
            if let Some(source) = request.source {
                nav.source = source;
                nav.full_code = false;
            }
            if request.root.is_some() {
                nav.root = request.root;
            }
            if let Some(way) = request.way {
                nav.way = way;
            }
            if request.depth.is_some() {
                nav.depth = request.depth;
            }
            nav.reset_folds();
        }
        NavChange::Restore(to) => *nav = *to,
    }
}

/// Drops the opened cards the server couldn't fit (it says which).
pub fn forget_dropped(nav: &mut Nav, dropped: &[String]) {
    nav.opened.retain(|o| !dropped.contains(&o.card));
}

/// Where the user was before going into a card: back returns there.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Step {
    pub nav: Nav,
    pub selection: Option<String>,
}

impl super::GraphState {
    /// The one way where the graph looks changes: applies the change and
    /// fetches the tree again if it changed. A user's move ends the agent's.
    pub fn navigate(&mut self, change: NavChange) {
        let user = !matches!(change, NavChange::Request(_));
        let before = self.nav.clone();
        if let NavChange::GoIn(card) = &change
            && self.nav.root.as_ref() != Some(card)
        {
            self.trail.push(Step { nav: self.nav.clone(), selection: self.selection.clone() });
            self.pending_select = Some(card.clone());
        }
        // Opening a card that has more under it: just its branch, merged in.
        let expand = match &change {
            NavChange::Open(card) => self.tree.as_ref().and_then(|t| t.card(card)).filter(|c| c.more > 0 && !c.open).map(|c| c.key.clone()),
            _ => None,
        };
        apply(&mut self.nav, change);
        if self.nav != before {
            match expand {
                Some(card) if !self.stale => self.expanding = Some((card, before.request())),
                _ => {
                    self.stale = true;
                    self.refit = true;
                }
            }
            self.note = None;
        }
        if user {
            self.agent = None;
        }
    }

    /// Back: to where the last "go in" started, else out to what holds the
    /// root (or the view's starting points). False when there's nowhere back.
    pub fn back(&mut self) -> bool {
        if let Some(step) = self.trail.pop() {
            self.nav = step.nav;
            self.pending_select = step.selection;
            self.stale = true;
            self.refit = true;
            self.agent = None;
            return true;
        }
        let Some(root) = self.nav.root.clone() else { return false };
        let holder = self.tree.as_ref().and_then(|t| t.path.last()).map(|c| c.key.clone());
        let change = match (self.nav.view, holder) {
            (ViewKind::Contains, Some(holder)) => NavChange::GoIn(holder),
            _ => NavChange::Top,
        };
        self.navigate(change);
        self.trail.clear();
        self.pending_select = Some(root);
        true
    }

    /// The roots gone through to get here, oldest first (each one a way back).
    pub fn trail_roots(&self) -> Vec<Option<String>> {
        self.trail.iter().map(|s| s.nav.root.clone()).collect()
    }

    /// Back to the `index`th step of the trail.
    pub fn back_to(&mut self, index: usize) {
        if index >= self.trail.len() {
            return;
        }
        let step = self.trail[index].clone();
        self.trail.truncate(index);
        self.nav = step.nav;
        self.pending_select = step.selection;
        self.stale = true;
        self.refit = true;
        self.agent = None;
    }

    /// A jump made with the pointer or a finger: remembers where it left
    /// from for the jump list, then navigates.
    pub(super) fn pointer_jump(&mut self, change: NavChange) {
        if self.jumped.is_none() {
            self.jumped = Some(self.place());
        }
        self.navigate(change);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arch_graph::PlanId;

    fn nav() -> Nav {
        Nav { view: ViewKind::Calls, root: Some("rust:a::run".into()), depth: Some(3), ..Nav::default() }
    }

    #[test]
    fn every_change_is_one_the_user_made_and_folds_belong_to_the_tree_they_were_made_in() {
        let mut n = nav();
        apply(&mut n, NavChange::Open("rust:a::step".into()));
        apply(&mut n, NavChange::Close("rust:a::log".into()));
        assert_eq!((n.opened.len(), n.closed.len()), (1, 1));
        assert_eq!(n.request().opened, vec![Opened { card: "rust:a::step".into(), levels: 1 }]);
        // Opening a closed card un-closes it; opening twice keeps one.
        apply(&mut n, NavChange::Open("rust:a::log".into()));
        apply(&mut n, NavChange::Open("rust:a::log".into()));
        assert_eq!((n.opened.len(), n.closed.len()), (2, 0));
        // Depth, flipping, going in, the source and the view start the folds over.
        for change in [
            NavChange::Depth { delta: 1, base: 3 },
            NavChange::Flip,
            NavChange::GoIn("rust:a::step".into()),
            NavChange::Source(Source::Plan(PlanId::new("p").unwrap())),
            NavChange::FullCode(true),
            NavChange::View { view: ViewKind::Contains, root: None },
        ] {
            apply(&mut n, NavChange::Open("x".into()));
            apply(&mut n, change);
            assert!(n.opened.is_empty() && n.closed.is_empty());
        }
        assert_eq!((n.view, n.root.as_deref(), n.way, n.depth), (ViewKind::Contains, None, Way::Up, Some(4)));
        // Full code stays on (another view, same plan) until the plan changes.
        assert!(n.full_code);
        apply(&mut n, NavChange::Source(Source::Code));
        assert_eq!(n.request().source, Source::Code);
        // Depth never goes below the root alone.
        apply(&mut n, NavChange::Depth { delta: -9, base: 2 });
        assert_eq!(n.depth, Some(0));
        // The server closed what didn't fit: forgotten here too.
        apply(&mut n, NavChange::Open("a".into()));
        apply(&mut n, NavChange::Open("b".into()));
        forget_dropped(&mut n, &["a".into()]);
        assert_eq!(n.opened, vec![Opened { card: "b".into(), levels: 1 }]);
    }

    #[test]
    fn the_agent_can_send_the_user_to_the_code_a_plan_a_view_or_a_root() {
        let mut n = Nav { source: Source::Plan(PlanId::new("p").unwrap()), ..nav() };
        let request: ViewRequest = serde_json::from_value(serde_json::json!({ "source": "code", "view": "data_flow", "root": "type:Plan", "way": "up" })).unwrap();
        apply(&mut n, NavChange::Request(Box::new(request)));
        assert_eq!((&n.source, n.view, n.root.as_deref(), n.way), (&Source::Code, ViewKind::DataFlow, Some("type:Plan"), Way::Up));
        // Nothing asked: nothing changes.
        let before = n.clone();
        apply(&mut n, NavChange::Request(Box::default()));
        assert_eq!(n, before);
    }
}
