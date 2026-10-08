//! Walking a view into a tree: level by level from the roots, each card
//! once (a card met again is a line to it, a loop when it's above on the
//! same path), cut at a whole level when the next one wouldn't fit the
//! budget, with each card counting the children it doesn't show.

use std::collections::{BTreeMap, BTreeSet};

use super::lens::{Child, Lens};
use super::{Card, Cut, CutReason, GraphRead, Input, Line, LineKind, Rect, Tree, TreeRequest, ViewKind};

/// The tree as it grows.
struct Growing<'l, 'a, G: GraphRead> {
    lens: &'l Lens<'a, G>,
    cards: Vec<Card>,
    at: BTreeMap<String, usize>,
    lines: Vec<Line>,
    /// (from, to) of every line, so none is drawn twice.
    drawn: BTreeSet<(String, String)>,
    budget: usize,
    /// Lines to cards shown elsewhere that may still be drawn.
    links_left: usize,
    closed: BTreeSet<String>,
}

/// One level about to be added: new cards and the lines to every child.
struct Next {
    cards: Vec<(String, usize, u32)>,
    lines: Vec<(usize, Child)>,
}

impl<G: GraphRead> Growing<'_, '_, G> {
    fn card(&self, key: &str, parent: Option<usize>, level: u32) -> Card {
        let face = self.lens.face(key);
        Card {
            key: key.to_owned(),
            label: face.label,
            kind: face.kind,
            detail: face.detail,
            parent: parent.map(|p| self.cards[p].key.clone()),
            level,
            more: 0,
            open: false,
            links_left_out: 0,
            mark: face.mark,
            other_plans: face.other_plans,
            component: face.component,
            callable: face.callable,
            outside_plan: self.lens.outside_plan(key),
            rect: Rect::default(),
        }
    }

    fn add_root(&mut self, key: &str) {
        let card = self.card(key, None, 0);
        self.at.insert(key.to_owned(), self.cards.len());
        self.cards.push(card);
    }

    /// Whether `ancestor` is on the tree path above card `at`.
    fn above(&self, at: usize, ancestor: &str) -> bool {
        let mut current = Some(at);
        while let Some(i) = current {
            if self.cards[i].key == ancestor {
                return true;
            }
            current = self.cards[i].parent.as_ref().and_then(|p| self.at.get(p).copied());
        }
        false
    }

    /// The level under `frontier`, not yet added. `forced` cards are walked
    /// even where the plan focus wouldn't (opened by hand).
    fn next(&self, frontier: &[usize], forced: bool) -> Next {
        let mut next = Next { cards: Vec::new(), lines: Vec::new() };
        let mut new: BTreeSet<String> = BTreeSet::new();
        for &f in frontier {
            let key = &self.cards[f].key;
            if self.closed.contains(key) || !(forced || self.lens.walks(key)) {
                continue;
            }
            for child in self.lens.children(key) {
                if !self.at.contains_key(&child.key) && new.insert(child.key.clone()) {
                    next.cards.push((child.key.clone(), f, self.cards[f].level + 1));
                }
                next.lines.push((f, child));
            }
        }
        next
    }

    /// Adds a level; false (and nothing added) if it doesn't fit.
    fn grow(&mut self, next: Next) -> bool {
        if self.cards.len() + next.cards.len() > self.budget {
            return false;
        }
        for (key, parent, level) in &next.cards {
            let card = self.card(key, Some(*parent), *level);
            self.at.insert(key.clone(), self.cards.len());
            self.cards.push(card);
            self.cards[*parent].open = true;
        }
        for (from, child) in next.lines {
            let to = self.at[&child.key];
            let kind = if self.cards[to].parent.as_deref() == Some(self.cards[from].key.as_str()) {
                LineKind::Tree
            } else if self.above(from, &child.key) {
                LineKind::Loop
            } else {
                LineKind::Repeat
            };
            let pair = (self.cards[from].key.clone(), child.key.clone());
            if !self.drawn.insert(pair.clone()) {
                continue;
            }
            if kind != LineKind::Tree {
                if self.links_left == 0 {
                    self.cards[from].links_left_out += 1;
                    continue;
                }
                self.links_left -= 1;
            }
            self.cards[from].open = true;
            self.lines.push(Line { from: pair.0, to: pair.1, kind, relation: child.relation, order: child.order, mark: child.mark, pills: child.pills, path: Vec::new(), reach: Vec::new() });
        }
        true
    }

    /// Walks `levels` levels under `frontier`. Returns the cut (levels added
    /// before it stopped) if it couldn't go all the way.
    fn deepen(&mut self, mut frontier: Vec<usize>, levels: u32, forced_first: bool, out_of_time: &dyn Fn() -> bool) -> Option<(u32, CutReason)> {
        for level in 0..levels {
            if out_of_time() {
                return Some((level, CutReason::Time));
            }
            let next = self.next(&frontier, forced_first && level == 0);
            if next.lines.is_empty() {
                return None;
            }
            let added: Vec<String> = next.cards.iter().map(|(k, _, _)| k.clone()).collect();
            if !self.grow(next) {
                return Some((level, CutReason::Budget));
            }
            frontier = added.iter().map(|k| self.at[k]).collect();
        }
        None
    }

    /// Every card that isn't open counts the cards opening it would add
    /// (its children not shown anywhere yet).
    fn count_hidden(&mut self) -> u32 {
        let mut hidden = 0;
        for i in 0..self.cards.len() {
            if self.cards[i].open {
                continue;
            }
            let key = self.cards[i].key.clone();
            let more = self.lens.children(&key).iter().filter(|c| !self.at.contains_key(&c.key)).map(|c| c.key.clone()).collect::<BTreeSet<_>>().len() as u32;
            self.cards[i].more = more;
            hidden += more;
        }
        hidden
    }

    /// Cards under `card` (itself included) that aren't open: where opening
    /// it further continues.
    fn leaves_under(&self, card: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![card];
        while let Some(i) = stack.pop() {
            if !self.cards[i].open {
                out.push(i);
                continue;
            }
            let key = &self.cards[i].key;
            stack.extend(self.cards.iter().enumerate().filter(|(_, c)| c.parent.as_ref() == Some(key)).map(|(j, _)| j));
        }
        out.sort();
        out
    }
}

pub(super) fn walk<G: GraphRead>(lens: &Lens<G>, input: &Input<G>, request: &TreeRequest) -> Tree {
    let depth = request.depth.unwrap_or_else(|| lens.default_depth());
    let budget = input.budget.max(1);
    let mut grow = Growing { lens, cards: Vec::new(), at: BTreeMap::new(), lines: Vec::new(), drawn: BTreeSet::new(), budget, links_left: input.link_budget, closed: request.closed.iter().cloned().collect() };
    let mut request = request.clone();
    request.depth = Some(depth);
    let starts: Vec<String> = match &request.root {
        Some(root) if lens.exists(root) => vec![root.clone()],
        Some(_) => Vec::new(),
        None => lens.starts(),
    };
    let mut cut = None;
    // Without a root, a view shows its first few starting points.
    let shown_starts = if request.root.is_some() { budget } else { input.config.view(request.view).starts.clamp(1, budget) };
    for key in starts.iter().take(shown_starts) {
        grow.add_root(key);
    }
    let roots: Vec<String> = grow.cards.iter().map(|c| c.key.clone()).collect();
    let starts_left_out = starts.len().saturating_sub(shown_starts) as u32;
    if let Some((shown, reason)) = grow.deepen((0..grow.cards.len()).collect(), depth, false, input.out_of_time) {
        cut = Some(Cut { reason, starts_left_out, shown, asked: depth, hidden: 0 });
    } else if starts_left_out > 0 {
        cut = Some(Cut { reason: CutReason::Budget, starts_left_out, shown: depth, asked: depth, hidden: 0 });
    }
    // Cards opened by hand, the latest first (it's what was asked for last).
    let mut dropped = Vec::new();
    for opened in request.opened.iter().rev() {
        let Some(&card) = grow.at.get(&opened.card) else { continue };
        if grow.closed.contains(&opened.card) {
            continue;
        }
        let frontier = grow.leaves_under(card);
        // A branch is opened whole or not at all.
        let before = (grow.cards.len(), grow.lines.len(), grow.drawn.clone(), grow.at.clone(), grow.cards.iter().map(|c| (c.open, c.links_left_out)).collect::<Vec<_>>(), grow.links_left);
        if grow.deepen(frontier, opened.levels, true, input.out_of_time).is_some() {
            grow.cards.truncate(before.0);
            grow.lines.truncate(before.1);
            grow.drawn = before.2;
            grow.at = before.3;
            for (c, (open, left_out)) in grow.cards.iter_mut().zip(before.4) {
                c.open = open;
                c.links_left_out = left_out;
            }
            grow.links_left = before.5;
            dropped.push(opened.card.clone());
        }
    }
    let hidden = grow.count_hidden();
    // Boxes in boxes need no lines: what holds what is the nesting.
    if request.view == ViewKind::Contains {
        grow.lines.retain(|l| l.kind != LineKind::Tree);
    }
    if let Some(cut) = cut.as_mut() {
        cut.hidden += hidden;
    }
    let path = match (&request.root, request.view) {
        (Some(root), _) => lens.path(root),
        (None, _) => Vec::new(),
    };
    Tree { request, roots, cards: grow.cards, lines: grow.lines, cut, path, dropped, size: (0.0, 0.0), slow_ms: None }
}

/// Where a card sits: what holds it, what else is in there, and the card's
/// lines (of the asked view) to those.
pub(super) fn context<G: GraphRead>(lens: &Lens<G>, input: &Input<G>, request: &TreeRequest, card: &str) -> Tree {
    let contains = TreeRequest { view: ViewKind::Contains, way: super::Way::Down, depth: Some(1), root: None, opened: Vec::new(), closed: Vec::new(), ..request.clone() };
    let holder_lens = Lens::new(input, &contains);
    let id = crate::ComponentId::new(lens.face(card).component.map_or_else(|| card.to_owned(), |c| c.0));
    let holder = lens.ancestors(&id).into_iter().next();
    let mut tree = match &holder {
        Some(h) => walk(&holder_lens, input, &TreeRequest { root: Some(h.0.clone()), ..contains }),
        None => walk(&holder_lens, input, &contains),
    };
    // The card's own lines (calls, flows) to what's beside it.
    let beside: BTreeSet<String> = tree.cards.iter().map(|c| c.key.clone()).collect();
    if request.view != ViewKind::Contains {
        for child in lens.children(&id.0) {
            if beside.contains(&child.key) && child.key != id.0 {
                tree.lines.push(Line { from: id.0.clone(), to: child.key, kind: LineKind::Repeat, relation: child.relation, order: child.order, mark: child.mark, pills: child.pills, path: Vec::new(), reach: Vec::new() });
            }
        }
    }
    tree.request = request.clone();
    tree
}
