//! Calls as a story: from an entry, what it calls in call order, each
//! called function told once (later callers just count), a call back into
//! the path marked as a loop, and a collapsed branch summed up. Walking the
//! other way (a view with direction `in`) tells what reaches a function.

use std::collections::{BTreeMap, BTreeSet};

use super::{Direction, View};
use crate::{ComponentId, RelationKind};

/// One step of the story, at its depth (0: an entry).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: ComponentId,
    pub depth: usize,
    /// The step it's told under (None: an entry).
    pub under: Option<ComponentId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Story {
    pub rows: Vec<Row>,
    /// Steps reached from more than one place: how many other places.
    pub also: BTreeMap<ComponentId, u32>,
    /// Calls back into the path above (recursion): from, to.
    pub loops: BTreeMap<ComponentId, Vec<ComponentId>>,
    /// Collapsed steps: how many distinct steps are inside them.
    pub inside: BTreeMap<ComponentId, u32>,
}

/// The view's calls as a story, with the `shut` steps collapsed. Forwards,
/// a function is told where it's first called (the order things happen);
/// backwards, each caller at its nearest distance.
pub fn tell(view: &View, shut: &BTreeSet<ComponentId>) -> Story {
    let outward = view.spec.direction != Direction::In;
    // Each step's next steps, in call order (callers by name, going backwards).
    let mut next: BTreeMap<&ComponentId, Vec<(u32, usize, &ComponentId)>> = BTreeMap::new();
    for (i, e) in view.edges.iter().enumerate().filter(|(_, e)| e.kind == RelationKind::Calls && !e.overlay) {
        let (from, to) = if outward { (&e.source, &e.target) } else { (&e.target, &e.source) };
        let order = if outward { e.order.unwrap_or(u32::MAX) } else { u32::MAX };
        next.entry(from).or_default().push((order, i, to));
    }
    for list in next.values_mut() {
        list.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| if outward { a.1.cmp(&b.1) } else { a.2.cmp(b.2) }));
        let mut seen = BTreeSet::new();
        list.retain(|(_, _, to)| seen.insert(*to));
    }
    let after = |id: &ComponentId| -> Vec<&ComponentId> { next.get(id).map(|l| l.iter().map(|(_, _, to)| *to).collect()).unwrap_or_default() };
    // Entries: the view's roots, then anything not reached from them.
    let mut entries: Vec<&ComponentId> = view.roots.iter().filter(|r| view.node(r).is_some()).collect();
    entries.extend(view.nodes.iter().map(|n| &n.id));
    // How far each step is from the nearest entry (for telling callers nearest first).
    let mut distance: BTreeMap<&ComponentId, usize> = BTreeMap::new();
    for entry in &entries {
        if distance.contains_key(entry) {
            continue;
        }
        distance.insert(entry, 0);
        let mut queue = std::collections::VecDeque::from([*entry]);
        while let Some(at) = queue.pop_front() {
            let d = distance[at];
            for step in after(at) {
                if !distance.contains_key(step) {
                    distance.insert(step, d + 1);
                    queue.push_back(step);
                }
            }
        }
    }
    struct Walk<'a, 'v> {
        outward: bool,
        shut: &'a BTreeSet<ComponentId>,
        next: &'a BTreeMap<&'v ComponentId, Vec<(u32, usize, &'v ComponentId)>>,
        distance: &'a BTreeMap<&'v ComponentId, usize>,
        told: BTreeSet<ComponentId>,
        covered: BTreeSet<ComponentId>,
        path: Vec<ComponentId>,
        story: Story,
    }
    impl<'v> Walk<'_, 'v> {
        fn after(&self, id: &ComponentId) -> Vec<&'v ComponentId> {
            self.next.get(id).map(|l| l.iter().map(|(_, _, to)| *to).collect()).unwrap_or_default()
        }

        fn walk(&mut self, id: &'v ComponentId, depth: usize, under: Option<&ComponentId>) {
            if self.path.contains(id) {
                if let Some(from) = under {
                    self.story.loops.entry(from.clone()).or_default().push(id.clone());
                }
                return;
            }
            if self.told.contains(id) || (!self.outward && self.distance.get(id).is_some_and(|d| *d != depth)) {
                return;
            }
            self.told.insert(id.clone());
            self.story.rows.push(Row { id: id.clone(), depth, under: under.cloned() });
            if self.shut.contains(id) {
                // Everything reachable inside, counted once.
                let mut inside: BTreeSet<&ComponentId> = BTreeSet::new();
                let mut stack = self.after(id);
                while let Some(step) = stack.pop() {
                    if step != id && inside.insert(step) {
                        stack.extend(self.after(step));
                    }
                }
                if !inside.is_empty() {
                    self.story.inside.insert(id.clone(), inside.len() as u32);
                }
                self.covered.extend(inside.into_iter().cloned());
                return;
            }
            self.path.push(id.clone());
            for step in self.after(id) {
                self.walk(step, depth + 1, Some(id));
            }
            self.path.pop();
        }
    }
    let mut w = Walk { outward, shut, next: &next, distance: &distance, told: BTreeSet::new(), covered: BTreeSet::new(), path: Vec::new(), story: Story::default() };
    for entry in entries {
        if !w.told.contains(entry) && !w.covered.contains(entry) {
            w.walk(entry, 0, None);
        }
    }
    let mut story = w.story;
    // Reached from elsewhere too: other told steps that lead to it, not
    // counting the one it's told under or calls back up its own line.
    let under: BTreeMap<ComponentId, Option<ComponentId>> = story.rows.iter().map(|r| (r.id.clone(), r.under.clone())).collect();
    let line_above = |id: &ComponentId| -> BTreeSet<ComponentId> {
        let mut out = BTreeSet::new();
        let mut at = under.get(id).cloned().flatten();
        while let Some(a) = at {
            if !out.insert(a.clone()) {
                break;
            }
            at = under.get(&a).cloned().flatten();
        }
        out
    };
    for row in &story.rows {
        let others = story
            .rows
            .iter()
            .filter(|p| Some(&p.id) != row.under.as_ref() && after(&p.id).contains(&&row.id) && !line_above(&p.id).contains(&row.id) && p.id != row.id)
            .count() as u32;
        if others > 0 {
            story.also.insert(row.id.clone(), others);
        }
    }
    story
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::tests::{id, sample};
    use crate::view::{ViewCatalog, query};

    /// run calls step then log; step calls save and log; save calls run back.
    fn knotted() -> View {
        let mut g = sample();
        let call = |a: &str, b: &str, order: u32| {
            let mut r = crate::Relation::new(RelationKind::Calls, id(a), id(b), crate::Provenance::OBSERVED);
            r.order = Some(order);
            r
        };
        for r in [call("app::run", "app::step", 0), call("app::run", "app::log", 1), call("app::step", "app::save", 0), call("app::step", "app::log", 1), call("app::save", "app::run", 0)] {
            g.upsert_relation(r);
        }
        let mut story = ViewCatalog::defaults().unwrap().get("story").unwrap().clone();
        story.roots = vec![id("app::run")];
        query(&g, &story)
    }

    // req: C.1, C.2
    #[test]
    fn a_story_tells_calls_in_order_each_once_with_loops_marked() {
        let s = tell(&knotted(), &BTreeSet::new());
        let rows: Vec<(String, usize)> = s.rows.iter().map(|r| (r.id.to_string(), r.depth)).collect();
        assert_eq!(rows, [("rust:app::run".to_owned(), 0), ("rust:app::step".to_owned(), 1), ("rust:app::save".to_owned(), 2), ("rust:app::log".to_owned(), 2)], "call order, depth to the right, log told once");
        assert_eq!(s.also.get(&id("app::log")), Some(&1), "log: also called from one more place");
        assert_eq!(s.loops.get(&id("app::save")), Some(&vec![id("app::run")]), "save calls back into run: a loop");
        // Collapsed: the branch is summed up.
        let s = tell(&knotted(), &[id("app::step")].into());
        assert_eq!(s.rows.len(), 3, "run, step (shut), log");
        assert_eq!(s.inside.get(&id("app::step")), Some(&3), "step: save, log and run inside");
    }

    // req: C.3
    #[test]
    fn the_story_backwards_tells_what_reaches_a_function() {
        let mut view = knotted();
        view.spec.direction = Direction::In;
        view.roots = vec![id("app::log")];
        let s = tell(&view, &BTreeSet::new());
        assert_eq!(s.rows[0].id, id("app::log"));
        let reach: BTreeSet<ComponentId> = s.rows.iter().filter(|r| r.depth == 1).map(|r| r.id.clone()).collect();
        assert_eq!(reach, [id("app::run"), id("app::step")].into(), "both callers, one step out");
    }
}
