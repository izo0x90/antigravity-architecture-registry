//! What a client holds: a tree it was sent, with the branches it opened
//! since merged in. The server keeps the trees it sends; opening a card
//! walks only that card's branch, merges it into the tree the client holds,
//! keeps to the card cap by closing the opened branches furthest from it,
//! and lays the whole out again: laying out runs only on the server. The
//! client gets the new cards, every card's place, and the lines.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{Card, Cut, Line, Opened, Rect, Tree, TreeRequest};

impl TreeRequest {
    /// The request once `card` is opened `levels` further (open again if it
    /// was closed).
    pub fn opening(&self, card: &str, levels: u32) -> TreeRequest {
        let mut next = self.clone();
        next.closed.retain(|c| c != card);
        next.opened.retain(|o| o.card != card);
        next.opened.push(Opened { card: card.to_owned(), levels });
        next
    }
}

/// What opening a card changed in the tree a client holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Expanded {
    /// What the tree now answers: the card opened, and any branch closed
    /// again to keep to the cap.
    pub request: TreeRequest,
    /// The cards it adds.
    pub cards: Vec<Card>,
    /// Every card held now, where it is and whether it's open (the tree is
    /// laid out again, so cards held before move). A card held before
    /// that's not here is gone (its branch was closed).
    pub places: Vec<Place>,
    /// Every line, laid out.
    pub lines: Vec<Line>,
    /// Branches closed again to keep to the cap, furthest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<String>,
    pub size: (f32, f32),
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut: Option<Cut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_ms: Option<u64>,
}

impl Expanded {
    /// The cards added, and the lines that reach them, as a tree of their
    /// own (for an agent's text).
    pub fn added(&self) -> Tree {
        let keys: BTreeSet<&str> = self.cards.iter().map(|c| c.key.as_str()).collect();
        Tree {
            request: self.request.clone(),
            roots: Vec::new(),
            cards: self.cards.clone(),
            lines: self.lines.iter().filter(|l| keys.contains(l.from.as_str()) || keys.contains(l.to.as_str())).cloned().collect(),
            cut: self.cut.clone(),
            path: Vec::new(),
            dropped: Vec::new(),
            size: self.size,
            slow_ms: self.slow_ms,
        }
    }
}

/// A held card's place and state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub key: String,
    pub rect: Rect,
    #[serde(default)]
    pub open: bool,
    #[serde(default)]
    pub more: u32,
}

/// Puts a branch's cards and lines (what opening `card` adds) into `tree`:
/// no card or line twice, `card` shown open, and the branch's cut (if it
/// didn't fit) is what the tree now says. Nothing is laid out.
pub fn merge(tree: &mut Tree, card: &str, branch: Tree) {
    if branch.cut.is_some() {
        tree.cut = branch.cut;
    }
    let held: BTreeSet<String> = tree.cards.iter().map(|c| c.key.clone()).collect();
    tree.cards.extend(branch.cards.into_iter().filter(|c| !held.contains(&c.key)));
    for line in branch.lines {
        if !tree.lines.iter().any(|l| l.from == line.from && l.to == line.to && l.relation == line.relation && l.kind == line.kind) {
            tree.lines.push(line);
        }
    }
    if let Some(opened) = tree.cards.iter_mut().find(|c| c.key == card) {
        opened.open = true;
    }
    // A branch counts its levels from its own top: each card's level is
    // one below the card it hangs from.
    let mut level: BTreeMap<String, u32> = tree.cards.iter().filter(|c| c.parent.is_none()).map(|c| (c.key.clone(), c.level)).collect();
    loop {
        let mut grew = false;
        for c in &tree.cards {
            if let Some(p) = &c.parent
                && !level.contains_key(&c.key)
                && let Some(&l) = level.get(p)
            {
                level.insert(c.key.clone(), l + 1);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    for c in &mut tree.cards {
        if let Some(&l) = level.get(&c.key) {
            c.level = l;
        }
    }
}

/// What `key` holds in `tree`, all the way down (not itself).
fn below(tree: &Tree, key: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut frontier = vec![key.to_owned()];
    while let Some(at) = frontier.pop() {
        for c in tree.cards.iter().filter(|c| c.parent.as_deref() == Some(at.as_str())) {
            if out.insert(c.key.clone()) {
                frontier.push(c.key.clone());
            }
        }
    }
    out
}

/// The cards from `key` up to its top, `key` first.
fn ancestry(parents: &BTreeMap<&str, Option<&str>>, key: &str) -> Vec<String> {
    let mut out = vec![key.to_owned()];
    let mut at = key;
    while let Some(Some(p)) = parents.get(at) {
        out.push((*p).to_owned());
        at = p;
    }
    out
}

/// How many steps apart two cards are in the tree (up from one to where
/// their paths meet, and down to the other); cards in different starting
/// points' trees are furthest.
fn distance(parents: &BTreeMap<&str, Option<&str>>, a: &str, b: &str) -> usize {
    let (up_a, up_b) = (ancestry(parents, a), ancestry(parents, b));
    match up_a.iter().position(|k| up_b.contains(k)) {
        Some(i) => i + up_b.iter().position(|k| *k == up_a[i]).unwrap_or(0),
        None => usize::MAX,
    }
}

/// At most `cap` cards: while over, the opened branch furthest from
/// `newest` (the one just opened, which stays) is closed again, the one
/// opened longest ago first among equals. Returns the cards closed.
pub fn hold(tree: &mut Tree, cap: usize, newest: &str) -> Vec<String> {
    let mut closed = Vec::new();
    while tree.cards.len() > cap {
        let parents: BTreeMap<&str, Option<&str>> = tree.cards.iter().map(|c| (c.key.as_str(), c.parent.as_deref())).collect();
        let furthest = tree
            .request
            .opened
            .iter()
            .enumerate()
            .filter(|(_, o)| o.card != newest && parents.contains_key(o.card.as_str()))
            .max_by_key(|(i, o)| (distance(&parents, &o.card, newest), std::cmp::Reverse(*i)))
            .map(|(_, o)| o.card.clone());
        let Some(furthest) = furthest else { break };
        let gone = below(tree, &furthest);
        let children = tree.cards.iter().filter(|c| c.parent.as_deref() == Some(furthest.as_str())).count() as u32;
        tree.cards.retain(|c| !gone.contains(&c.key));
        tree.lines.retain(|l| !gone.contains(&l.from) && !gone.contains(&l.to));
        if let Some(card) = tree.cards.iter_mut().find(|c| c.key == furthest) {
            card.open = false;
            card.more = card.more.max(children);
        }
        // What was opened inside it went with it.
        tree.request.opened.retain(|o| o.card != furthest && !gone.contains(&o.card));
        closed.push(furthest);
    }
    closed
}

/// Each card's count of what opening it would add, as a whole tree counts
/// it: an open card, nothing; a closed one, its children (`children`)
/// that aren't drawn anywhere.
pub fn recount(tree: &mut Tree, children: impl Fn(&str) -> Vec<String>) {
    let drawn: BTreeSet<String> = tree.cards.iter().map(|c| c.key.clone()).collect();
    for card in &mut tree.cards {
        card.more = if card.open {
            0
        } else {
            children(&card.key).into_iter().filter(|k| !drawn.contains(k)).collect::<BTreeSet<_>>().len() as u32
        };
    }
}

/// What a client holding `before` needs to hold `now`.
pub fn expanded(before: &Tree, now: &Tree, closed: Vec<String>) -> Expanded {
    let held: BTreeSet<&str> = before.cards.iter().map(|c| c.key.as_str()).collect();
    Expanded {
        request: now.request.clone(),
        cards: now.cards.iter().filter(|c| !held.contains(c.key.as_str())).cloned().collect(),
        places: now.cards.iter().map(|c| Place { key: c.key.clone(), rect: c.rect, open: c.open, more: c.more }).collect(),
        lines: now.lines.clone(),
        closed,
        size: now.size,
        cut: now.cut.clone(),
        slow_ms: now.slow_ms,
    }
}

/// The tree a client holds, once it takes in what opening a card changed
/// (a client's side: it places nothing itself).
pub fn apply(tree: &mut Tree, change: Expanded) {
    let places: BTreeMap<String, Place> = change.places.into_iter().map(|p| (p.key.clone(), p)).collect();
    tree.cards.retain(|c| places.contains_key(&c.key));
    tree.cards.extend(change.cards);
    for card in &mut tree.cards {
        if let Some(p) = places.get(&card.key) {
            card.rect = p.rect;
            card.open = p.open;
            card.more = p.more;
        }
    }
    tree.lines = change.lines;
    tree.size = change.size;
    tree.request = change.request;
    tree.roots.retain(|r| places.contains_key(r));
    tree.cut = change.cut;
    tree.slow_ms = change.slow_ms;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::LineKind;

    fn card(key: &str, parent: Option<&str>) -> Card {
        Card {
            key: key.into(),
            label: key.into(),
            kind: "function".into(),
            detail: String::new(),
            parent: parent.map(Into::into),
            level: 0,
            more: 0,
            open: false,
            links_left_out: 0,
            mark: Default::default(),
            other_plans: Vec::new(),
            callable: true,
            component: None,
            outside_plan: false,
            rect: Rect::default(),
        }
    }

    #[test]
    fn over_the_cap_the_opened_branch_furthest_from_the_new_one_closes_first() {
        // root ── a ── a1 ── a11 (opened a, then a1)
        //      └─ b ── b1        (opened b)
        //      └─ c ── c1        (opened c: the newest)
        let cards = [
            ("root", None),
            ("a", Some("root")),
            ("a1", Some("a")),
            ("a11", Some("a1")),
            ("b", Some("root")),
            ("b1", Some("b")),
            ("c", Some("root")),
            ("c1", Some("c")),
        ];
        let mut tree = Tree { cards: cards.iter().map(|(k, p)| card(k, *p)).collect(), ..Tree::default() };
        tree.lines.push(Line {
            from: "a11".into(),
            to: "b1".into(),
            kind: LineKind::Repeat,
            relation: arch_relation(),
            order: None,
            mark: Default::default(),
            pills: Vec::new(),
            path: Vec::new(),
            reach: Vec::new(),
            evidence: None,
        });
        tree.request.opened = ["a", "a1", "b", "c"].iter().map(|c| Opened { card: (*c).into(), levels: 1 }).collect();
        // Under the cap: nothing closes.
        assert!(hold(&mut tree, 8, "c").is_empty());
        // One over: a1 (three steps from c: a1, a, root, c... furthest) closes
        // before b (two steps), though b was opened later.
        assert_eq!(hold(&mut tree, 7, "c"), ["a1"]);
        assert!(tree.card("a11").is_none() && tree.card("a1").is_some_and(|c| !c.open && c.more == 1));
        assert!(tree.lines.is_empty(), "lines to what's gone go too");
        // More: a is next furthest from c... then b; the newest stays.
        assert_eq!(hold(&mut tree, 4, "c"), ["a", "b"]);
        assert_eq!(tree.request.opened.iter().map(|o| o.card.as_str()).collect::<Vec<_>>(), ["c"]);
        assert!(hold(&mut tree, 1, "c").is_empty(), "only the newest is left open: it stays");
    }

    #[test]
    fn a_branch_that_didnt_fit_says_so_and_a_later_answer_replaces_what_was_said() {
        let mut tree = Tree { cards: vec![card("root", None), card("a", Some("root"))], ..Tree::default() };
        let cut = Cut { reason: crate::tree::CutReason::Budget, starts_left_out: 0, shown: 1, asked: 2, hidden: 7 };
        merge(&mut tree, "a", Tree { cards: vec![card("a1", Some("a"))], cut: Some(cut.clone()), ..Tree::default() });
        assert_eq!(tree.cut, Some(cut), "the branch's cut is the answer's");
        // A client that held a cut tree takes an answer that isn't cut.
        let mut held = tree.clone();
        let whole = Tree { cards: tree.cards.clone(), ..Tree::default() };
        apply(&mut held, expanded(&tree, &whole, Vec::new()));
        assert_eq!(held.cut, None, "an old cut doesn't stay");
    }

    fn arch_relation() -> crate::RelationKind {
        crate::RelationKind::Calls
    }
}
