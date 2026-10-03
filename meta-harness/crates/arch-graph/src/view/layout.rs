//! Layouts for views: layered (left to right along the view's own edges) and
//! nested (containers as boxes holding their children).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{LayoutKind, View};
use crate::{ComponentId, RelationKind};

pub const NODE_W: f32 = 220.0;
pub const NODE_H: f32 = 64.0;
pub const GAP_X: f32 = 90.0;
pub const GAP_Y: f32 = 28.0;
pub const PAD: f32 = 14.0;
pub const HEADER: f32 = 30.0;
/// Most children per row inside a container (rows aim for a square box).
pub const MAX_ROW: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }

    pub fn contains(&self, o: &Rect) -> bool {
        self.x <= o.x && self.y <= o.y && o.x + o.w <= self.x + self.w && o.y + o.h <= self.y + self.h
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub rects: BTreeMap<ComponentId, Rect>,
    /// Nodes drawn as containers (their children are inside).
    pub containers: BTreeSet<ComponentId>,
}

pub fn layout(view: &View) -> Layout {
    match view.spec.layout {
        LayoutKind::Layered => layered(view),
        LayoutKind::Nested => nested(view),
    }
}

/// Longest-path layers on the view's own (non-overlay) edges, cycles broken
/// by ignoring back edges; within a layer, nodes are ordered by the average
/// position of their predecessors so edges cross less.
fn layered(view: &View) -> Layout {
    let ids: Vec<&ComponentId> = view.nodes.iter().map(|n| &n.id).collect();
    let index: BTreeMap<&ComponentId, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); ids.len()];
    for e in view.edges.iter().filter(|e| !e.overlay) {
        if let (Some(&a), Some(&b)) = (index.get(&e.source), index.get(&e.target))
            && a != b
        {
            succ[a].push(b);
        }
    }
    // Drop back edges (DFS from roots first, then everything else).
    let mut state = vec![0u8; ids.len()];
    let mut forward: Vec<Vec<usize>> = vec![Vec::new(); ids.len()];
    let starts: Vec<usize> = view.roots.iter().filter_map(|r| index.get(r).copied()).chain(0..ids.len()).collect();
    for start in starts {
        if state[start] != 0 {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        state[start] = 1;
        while let Some((node, next)) = stack.pop() {
            if next < succ[node].len() {
                stack.push((node, next + 1));
                let child = succ[node][next];
                match state[child] {
                    0 => {
                        forward[node].push(child);
                        state[child] = 1;
                        stack.push((child, 0));
                    }
                    2 => forward[node].push(child),
                    _ => {} // back edge: part of a cycle
                }
            } else {
                state[node] = 2;
            }
        }
    }
    // Longest path layering over the acyclic forward edges.
    let mut indegree = vec![0usize; ids.len()];
    for targets in &forward {
        for &t in targets {
            indegree[t] += 1;
        }
    }
    let mut layer = vec![0usize; ids.len()];
    let mut ready: Vec<usize> = (0..ids.len()).filter(|&i| indegree[i] == 0).collect();
    while let Some(n) = ready.pop() {
        for &t in &forward[n] {
            layer[t] = layer[t].max(layer[n] + 1);
            indegree[t] -= 1;
            if indegree[t] == 0 {
                ready.push(t);
            }
        }
    }
    let layers = layer.iter().copied().max().map_or(0, |m| m + 1);
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); layers];
    for (i, &l) in layer.iter().enumerate() {
        rows[l].push(i);
    }
    let mut position = vec![0f32; ids.len()];
    for row in rows.iter_mut() {
        row.sort_by(|&a, &b| {
            let bary = |n: usize| {
                let preds: Vec<f32> = (0..ids.len()).filter(|&p| forward[p].contains(&n)).map(|p| position[p]).collect();
                if preds.is_empty() { n as f32 } else { preds.iter().sum::<f32>() / preds.len() as f32 }
            };
            bary(a).total_cmp(&bary(b)).then(a.cmp(&b))
        });
        for (slot, &n) in row.iter().enumerate() {
            position[n] = slot as f32;
        }
    }
    let mut out = Layout::default();
    for (l, row) in rows.iter().enumerate() {
        for (slot, &n) in row.iter().enumerate() {
            out.rects.insert(
                ids[n].clone(),
                Rect { x: l as f32 * (NODE_W + GAP_X), y: slot as f32 * (NODE_H + GAP_Y), w: NODE_W, h: NODE_H },
            );
        }
    }
    out
}

/// Containers hold their shown children (via Contains edges) in roughly
/// square grids; top-level boxes are packed into rows toward a wide aspect.
fn nested(view: &View) -> Layout {
    let shown: BTreeSet<&ComponentId> = view.nodes.iter().map(|n| &n.id).collect();
    let mut children: BTreeMap<&ComponentId, Vec<&ComponentId>> = BTreeMap::new();
    let mut has_parent: BTreeSet<&ComponentId> = BTreeSet::new();
    for e in view.edges.iter().filter(|e| e.kind == RelationKind::Contains) {
        if shown.contains(&e.source) && shown.contains(&e.target) && !has_parent.contains(&e.target) && e.source != e.target {
            children.entry(&e.source).or_default().push(&e.target);
            has_parent.insert(&e.target);
        }
    }
    // Lay each top-level box out at the origin first to learn its size.
    let tops: Vec<&ComponentId> = view.nodes.iter().map(|n| &n.id).filter(|id| !has_parent.contains(id)).collect();
    let mut sized: Vec<(&ComponentId, (f32, f32))> = Vec::new();
    for top in &tops {
        let mut scratch = Layout::default();
        sized.push((top, place(top, 0.0, 0.0, &children, &mut scratch, &mut BTreeSet::new())));
    }
    let area: f32 = sized.iter().map(|(_, (w, h))| (w + GAP_X) * (h + GAP_Y)).sum();
    let widest = sized.iter().map(|(_, (w, _))| *w).fold(NODE_W, f32::max);
    let row_width = (area.sqrt() * 1.6).max(widest);
    let mut out = Layout::default();
    let (mut x, mut y, mut row_h) = (0.0f32, 0.0f32, 0.0f32);
    for (top, (w, h)) in sized {
        if x > 0.0 && x + w > row_width {
            x = 0.0;
            y += row_h + GAP_Y;
            row_h = 0.0;
        }
        place(top, x, y, &children, &mut out, &mut BTreeSet::new());
        x += w + GAP_X;
        row_h = row_h.max(h);
    }
    out
}

/// Places `id` at (x, y); returns its (w, h).
fn place<'a>(
    id: &'a ComponentId,
    x: f32,
    y: f32,
    children: &BTreeMap<&'a ComponentId, Vec<&'a ComponentId>>,
    out: &mut Layout,
    visiting: &mut BTreeSet<&'a ComponentId>,
) -> (f32, f32) {
    let kids: Vec<&ComponentId> = children.get(id).map(|k| k.iter().copied().filter(|k| !visiting.contains(k)).collect()).unwrap_or_default();
    if kids.is_empty() {
        out.rects.insert(id.clone(), Rect { x, y, w: NODE_W, h: NODE_H });
        return (NODE_W, NODE_H);
    }
    visiting.insert(id);
    out.containers.insert(id.clone());
    let per_row = ((kids.len() as f32).sqrt().ceil() as usize).clamp(1, MAX_ROW);
    let (mut cx, mut cy) = (x + PAD, y + HEADER);
    let (mut row_h, mut max_w) = (0f32, 0f32);
    for (i, kid) in kids.iter().enumerate() {
        if i > 0 && i % per_row == 0 {
            cx = x + PAD;
            cy += row_h + GAP_Y / 2.0;
            row_h = 0.0;
        }
        let (w, h) = place(kid, cx, cy, children, out, visiting);
        cx += w + PAD;
        row_h = row_h.max(h);
        max_w = max_w.max(cx - x);
    }
    visiting.remove(id);
    let size = (max_w.max(NODE_W), cy + row_h + PAD - y);
    out.rects.insert(id.clone(), Rect { x, y, w: size.0, h: size.1 });
    size
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::tests::{id, sample};
    use crate::view::{ViewCatalog, query};

    fn no_overlaps_except_nesting(l: &Layout) {
        let rects: Vec<(&ComponentId, &Rect)> = l.rects.iter().collect();
        for (i, (a, ra)) in rects.iter().enumerate() {
            for (b, rb) in rects.iter().skip(i + 1) {
                if ra.overlaps(rb) {
                    assert!(ra.contains(rb) || rb.contains(ra), "{a} and {b} overlap without nesting");
                }
            }
        }
    }

    // req: P7.3
    #[test]
    fn layered_layout_follows_edges_without_overlaps() {
        let g = sample();
        let mut calls = ViewCatalog::defaults().unwrap().get("calls").unwrap().clone();
        let v = query(&g, &calls);
        let l = layout(&v);
        no_overlaps_except_nesting(&l);
        let x = |s: &str| l.rects[&id(s)].x;
        assert!(x("app::run") < x("app::step") && x("app::step") < x("app::save"));
        assert!(x("app::run") < x("app::log"));
        assert_eq!(layout(&v), l, "stable for the same input");
        // A cycle still lays out.
        let mut g2 = g.clone();
        g2.upsert_relation(crate::Relation::new(RelationKind::Calls, id("app::save"), id("app::run"), crate::Provenance::OBSERVED));
        calls.roots = vec![id("app::run")];
        let l2 = layout(&query(&g2, &calls));
        assert_eq!(l2.rects.len(), 4);
        no_overlaps_except_nesting(&l2);
    }

    // req: P7.3
    #[test]
    fn nested_layout_puts_children_inside_their_containers() {
        let g = sample();
        let structure = ViewCatalog::defaults().unwrap().get("structure").unwrap().clone();
        let v = query(&g, &structure);
        let l = layout(&v);
        no_overlaps_except_nesting(&l);
        assert!(l.containers.contains(&id("app")));
        for child in ["app::run", "app::step", "app::save", "app::log"] {
            assert!(l.rects[&id("app")].contains(&l.rects[&id(child)]), "{child} inside app");
        }
        assert!(l.rects[&id("lib")].contains(&l.rects[&id("lib::Step")]));
        assert!(!l.rects[&id("app")].overlaps(&l.rects[&id("lib")]));
    }

    // req: P7.3
    #[test]
    fn nested_layout_packs_many_boxes_into_a_wide_block() {
        use crate::view::{ViewNode, LayoutKind};
        let nodes: Vec<ViewNode> = (0..30).map(|i| ViewNode { id: id(&format!("m{i:02}")), depth: Some(0), truncated: false, alignment: crate::Alignment::Observed }).collect();
        let mut spec = ViewCatalog::defaults().unwrap().get("structure").unwrap().clone();
        spec.layout = LayoutKind::Nested;
        let view = crate::View { spec, roots: vec![], nodes, edges: vec![] };
        let l = layout(&view);
        no_overlaps_except_nesting(&l);
        let (w, h) = l.rects.values().fold((0f32, 0f32), |(w, h), r| (w.max(r.x + r.w), h.max(r.y + r.h)));
        assert!(w / h > 0.8 && w / h < 4.0, "aspect {w}x{h}");
    }
}
