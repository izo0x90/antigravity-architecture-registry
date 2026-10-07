//! Scenes: how a view is drawn at one level. Cards are sized to their text;
//! things are arranged by meaning (callers before what they depend on,
//! coupled parts near each other) on an aligned grid; modules show shut
//! unless opened, and a focused module is the level everything sits in;
//! relations between hidden parts are lifted onto what's drawn and merged
//! into counted lines, routed in straight segments around the cards. What a
//! focused level reaches outside it shows as blocks beside it; a level with
//! too many parts shows the selection's neighbourhood and a "+N more" block.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use serde::{Deserialize, Serialize};

use super::{LayoutKind, View};
use crate::{Alignment, ComponentId, RelationKind};

/// A rectangle in world units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
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

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Whether a point is strictly inside.
    pub fn holds(&self, (x, y): (f32, f32)) -> bool {
        x > self.x && x < self.right() && y > self.y && y < self.bottom()
    }

    fn moved(&self, dx: f32, dy: f32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..*self }
    }
}

/// Spacing and sizing, from the UI config.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// Positions and sizes snap to half of this.
    pub grid: f32,
    /// Between columns, and between cards in a column.
    pub gap_x: f32,
    pub gap_y: f32,
    /// Inside a card or an open module, around its contents.
    pub pad: f32,
    /// An open module's title band.
    pub header: f32,
    /// Most cards in one column before it wraps into another.
    pub column: usize,
    /// How far apart lines run, and the step they're routed on.
    pub lane: f32,
    /// What a bend costs a route, in lanes.
    pub bend: f32,
    pub card_min_w: f32,
    pub card_max_w: f32,
    /// Average glyph width as a share of the font size.
    pub char_width: f32,
    /// Line height as a share of the font size.
    pub line_height: f32,
}

impl Metrics {
    fn step(&self) -> f32 {
        (self.grid / 2.0).max(1.0)
    }

    fn snap(&self, v: f32) -> f32 {
        (v / self.step()).round() * self.step()
    }

    fn snap_up(&self, v: f32) -> f32 {
        (v / self.step() - 1e-3).ceil() * self.step()
    }

    /// How wide `text` is at font `size` (an estimate from the glyph width).
    pub fn text_width(&self, text: &str, size: f32) -> f32 {
        text.chars().count() as f32 * size * self.char_width
    }

    /// A card holding `lines` (text and font size): as wide as its widest
    /// line within the card limits, as tall as its lines, on the grid.
    pub fn card(&self, lines: &[(&str, f32)]) -> (f32, f32) {
        let text_w = lines.iter().map(|(t, size)| self.text_width(t, *size)).fold(0.0, f32::max);
        let w = (text_w + self.pad * 2.0).clamp(self.card_min_w, self.card_max_w);
        let h: f32 = lines.iter().map(|(_, size)| size * self.line_height).sum::<f32>() + self.pad * 1.5;
        (self.snap_up(w), self.snap_up(h))
    }
}

/// Sizes of what a scene draws, from the cockpit (which knows the text).
pub trait Measure {
    /// A component's card; `closed` for a module shown shut.
    fn card(&self, id: &ComponentId, closed: bool) -> (f32, f32);
    /// The block for what's reached in `group`, outside the focus.
    fn outside(&self, group: &ComponentId, reached: u32) -> (f32, f32);
    /// The block holding `hidden` parts of a crowded level.
    fn more(&self, hidden: usize) -> (f32, f32);
}

/// One end of a line.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum End {
    Node(ComponentId),
    /// A block outside the focus.
    Outside(ComponentId),
    /// The "+N more" block.
    More,
}

/// Relations between two drawn things, merged, with their route.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub from: End,
    pub to: End,
    pub kind: RelationKind,
    /// How many relations it stands for.
    pub count: u32,
    pub alignment: Alignment,
    pub overlay: bool,
    /// Corners in order, from the edge of `from` to the edge of `to`.
    pub points: Vec<(f32, f32)>,
    /// Where it leaves the level's frame (lines to what's outside).
    pub exit: Option<(f32, f32)>,
    /// Data flows: what flows along it (each distinct thing once).
    pub carries: Vec<String>,
    /// Seen in the code, but the types can't confirm it.
    pub unconfirmed: bool,
}

/// A view as drawn at one level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    /// Drawn nodes: cards, and open modules around their parts.
    pub rects: BTreeMap<ComponentId, Rect>,
    /// Open modules (drawn as boxes around their parts).
    pub containers: BTreeSet<ComponentId>,
    /// What contains what in the view (drawn or not).
    pub parent: BTreeMap<ComponentId, ComponentId>,
    /// The module the level is: everything else sits in it.
    pub frame: Option<ComponentId>,
    /// Blocks for what's reached outside the focus: where, and how many.
    pub outside: BTreeMap<ComponentId, (Rect, u32)>,
    /// The "+N more" block and the parts it holds.
    pub more: Option<(Rect, Vec<ComponentId>)>,
    pub lines: Vec<Line>,
    /// Stories: what's said beside a step (reached from elsewhere too,
    /// loops, a collapsed branch's size).
    pub notes: BTreeMap<ComponentId, Note>,
    /// Stories: the module each run of steps is in.
    pub bands: Vec<Band>,
}

/// Beside a step in a story.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    /// Reached from this many other places (told once, here).
    pub also: u32,
    /// Calls back into the steps above it.
    pub loops: Vec<ComponentId>,
    /// Collapsed: how many steps are inside.
    pub inside: u32,
}

/// A run of story steps in one module.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub module: ComponentId,
    pub rect: Rect,
}

impl Scene {
    pub fn rect_of(&self, end: &End) -> Option<Rect> {
        match end {
            End::Node(id) => self.rects.get(id).copied(),
            End::Outside(group) => self.outside.get(group).map(|(r, _)| *r),
            End::More => self.more.as_ref().map(|(r, _)| *r),
        }
    }

    /// Cards: drawn nodes that aren't open modules.
    pub fn cards(&self) -> impl Iterator<Item = (&ComponentId, &Rect)> {
        self.rects.iter().filter(|(id, _)| !self.containers.contains(*id))
    }

    /// The modules `id` is in (drawn or not), outermost first.
    pub fn ancestors(&self, id: &ComponentId) -> Vec<ComponentId> {
        let mut out = chain(&self.parent, id);
        out.reverse();
        out
    }

    /// Everything a line can't run through: cards and blocks.
    fn obstacles(&self) -> Vec<Rect> {
        let mut out: Vec<Rect> = self.cards().map(|(_, r)| *r).collect();
        out.extend(self.outside.values().map(|(r, _)| *r));
        out.extend(self.more.iter().map(|(r, _)| *r));
        out
    }

    /// The whole drawing's bounds.
    pub fn bounds(&self) -> Option<Rect> {
        let all: Vec<Rect> = self.rects.values().copied().chain(self.outside.values().map(|(r, _)| *r)).chain(self.more.iter().map(|(r, _)| *r)).collect();
        let first = all.first()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.right(), first.bottom());
        for r in &all {
            x0 = x0.min(r.x);
            y0 = y0.min(r.y);
            x1 = x1.max(r.right());
            y1 = y1.max(r.bottom());
        }
        Some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

/// The containers `id` is in, innermost first (loops cut).
fn chain(parent: &BTreeMap<ComponentId, ComponentId>, id: &ComponentId) -> Vec<ComponentId> {
    let mut out: Vec<ComponentId> = Vec::new();
    let mut at = id;
    while let Some(p) = parent.get(at) {
        if p == id || out.contains(p) {
            break;
        }
        out.push(p.clone());
        at = p;
    }
    out
}

/// What the cockpit decides about a level.
pub struct Options<'a> {
    /// Modules opened in place.
    pub open: &'a BTreeSet<ComponentId>,
    pub selection: Option<&'a ComponentId>,
    /// Most parts a level shows before the rest go into "+N more".
    pub max_cards: usize,
    /// Show every part however many there are.
    pub show_all: bool,
    /// Story steps collapsed (their branch summed up).
    pub shut: &'a BTreeSet<ComponentId>,
}

/// Builds the scene for `view` at its level.
pub fn build(view: &View, options: &Options, measure: &dyn Measure, metrics: &Metrics, previous: Option<&Scene>) -> Scene {
    if view.spec.layout == LayoutKind::Story {
        return story(view, options, measure, metrics);
    }
    let nested = view.spec.layout == LayoutKind::Nested;
    let ids: BTreeSet<&ComponentId> = view.nodes.iter().map(|n| &n.id).collect();
    // What contains what: the first container wins, loops are cut where they close.
    let mut parent: BTreeMap<ComponentId, ComponentId> = BTreeMap::new();
    if nested {
        let mut contains: Vec<(&ComponentId, &ComponentId)> =
            view.edges.iter().filter(|e| e.kind == RelationKind::Contains && e.source != e.target && ids.contains(&e.source) && ids.contains(&e.target)).map(|e| (&e.source, &e.target)).collect();
        contains.sort();
        for (source, target) in contains {
            if parent.contains_key(target) || chain(&parent, source).contains(target) || source == target {
                continue;
            }
            parent.insert(target.clone(), source.clone());
        }
    }
    let mut children: BTreeMap<ComponentId, Vec<ComponentId>> = BTreeMap::new();
    for node in &view.nodes {
        if let Some(p) = parent.get(&node.id) {
            children.entry(p.clone()).or_default().push(node.id.clone());
        }
    }
    let tops: Vec<ComponentId> = view.nodes.iter().filter(|n| !parent.contains_key(&n.id)).map(|n| n.id.clone()).collect();
    let frame = match tops.as_slice() {
        [only] if children.contains_key(only) => Some(only.clone()),
        _ => None,
    };
    let open: BTreeSet<ComponentId> = options.open.iter().chain(frame.iter()).filter(|c| children.contains_key(*c)).cloned().collect();

    // The level's own parts; a crowded level shows the selection's neighbourhood.
    let level: Vec<ComponentId> = match &frame {
        Some(f) => children.get(f).cloned().unwrap_or_default(),
        None => tops.clone(),
    };
    let up_to_level = |id: &ComponentId| -> Option<ComponentId> { std::iter::once(id.clone()).chain(chain(&parent, id)).find(|a| level.contains(a)) };
    let mut hidden: BTreeSet<ComponentId> = BTreeSet::new();
    if !options.show_all && level.len() > options.max_cards.max(1) {
        let mut weight: BTreeMap<(ComponentId, ComponentId), u32> = BTreeMap::new();
        let mut degree: BTreeMap<ComponentId, u32> = BTreeMap::new();
        let pairs = view.edges.iter().filter(|e| !(nested && e.kind == RelationKind::Contains)).map(|e| (&e.source, &e.target, 1)).chain(view.links.iter().map(|l| (&l.source, &l.target, l.count)));
        for (a, b, n) in pairs {
            if let (Some(a), Some(b)) = (up_to_level(a), up_to_level(b))
                && a != b
            {
                *weight.entry((a.clone(), b.clone())).or_default() += n;
                *weight.entry((b.clone(), a.clone())).or_default() += n;
                *degree.entry(a).or_default() += n;
                *degree.entry(b).or_default() += n;
            }
        }
        let mut kept: Vec<ComponentId> = Vec::new();
        if let Some(sel) = options.selection.and_then(up_to_level) {
            let mut near: Vec<(u32, ComponentId)> = weight.iter().filter(|((a, _), _)| *a == sel).map(|((_, b), w)| (*w, b.clone())).collect();
            near.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
            kept.push(sel);
            kept.extend(near.into_iter().map(|(_, b)| b));
        }
        let mut rest: Vec<&ComponentId> = level.iter().collect();
        rest.sort_by(|a, b| degree.get(*b).cmp(&degree.get(*a)).then_with(|| a.cmp(b)));
        kept.extend(rest.into_iter().cloned());
        let mut seen = BTreeSet::new();
        let kept: BTreeSet<ComponentId> = kept.into_iter().filter(|k| seen.insert(k.clone())).take(options.max_cards.max(1)).collect();
        hidden = level.iter().filter(|id| !kept.contains(*id)).cloned().collect();
    }

    // What's drawn: tops, and the parts of open modules, minus the hidden.
    let mut drawn: Vec<ComponentId> = Vec::new();
    let mut stack: Vec<ComponentId> = tops.iter().rev().cloned().collect();
    while let Some(id) = stack.pop() {
        if hidden.contains(&id) {
            continue;
        }
        if open.contains(&id) {
            stack.extend(children.get(&id).into_iter().flatten().rev().cloned());
        }
        drawn.push(id);
    }
    let drawn_set: BTreeSet<&ComponentId> = drawn.iter().collect();
    let end_of = |id: &ComponentId| -> Option<End> {
        for a in std::iter::once(id.clone()).chain(chain(&parent, id)) {
            if hidden.contains(&a) {
                return Some(End::More);
            }
            if drawn_set.contains(&a) {
                return Some(End::Node(a));
            }
        }
        None
    };
    // A line between a module and its own part says nothing.
    let nested_ends = |a: &End, b: &End| match (a, b) {
        (End::Node(a), End::Node(b)) => chain(&parent, a).contains(b) || chain(&parent, b).contains(a),
        _ => false,
    };

    // Relations lifted onto what's drawn, merged.
    let mut merged: BTreeMap<(End, End, RelationKind, bool, bool), (u32, Alignment, Vec<String>)> = BTreeMap::new();
    let mut add = |from: End, to: End, kind: RelationKind, overlay: bool, count: u32, alignment: Alignment, carries: Option<&String>, unconfirmed: bool| {
        let entry = merged.entry((from, to, kind, overlay, unconfirmed)).or_insert((0, alignment, Vec::new()));
        entry.0 += count;
        entry.1 = worst(entry.1, alignment);
        if let Some(c) = carries
            && !entry.2.contains(c)
        {
            entry.2.push(c.clone());
        }
    };
    for e in &view.edges {
        if nested && e.kind == RelationKind::Contains {
            continue;
        }
        if let (Some(a), Some(b)) = (end_of(&e.source), end_of(&e.target))
            && a != b
            && !nested_ends(&a, &b)
        {
            add(a, b, e.kind, e.overlay, 1, e.alignment, e.carries.as_ref(), e.unconfirmed.is_some());
        }
    }
    for l in &view.links {
        if let (Some(a), Some(b)) = (end_of(&l.source), end_of(&l.target))
            && a != b
            && !nested_ends(&a, &b)
        {
            add(a, b, l.kind, false, l.count, Alignment::Observed, None, false);
        }
    }
    let mut outside_sides: BTreeMap<ComponentId, bool> = BTreeMap::new();
    for o in &view.outside {
        for link in &o.links {
            let Some(inside) = end_of(&link.inside) else { continue };
            let group = End::Outside(o.group.clone());
            let (from, to) = if link.outgoing { (inside, group) } else { (group, inside) };
            add(from, to, link.kind, false, link.count, Alignment::Observed, None, false);
            *outside_sides.entry(o.group.clone()).or_default() |= link.outgoing;
        }
    }

    // Arrange: cards sized to their text, by what depends on what.
    let mut sizes: BTreeMap<ComponentId, (f32, f32)> = BTreeMap::new();
    for id in &drawn {
        sizes.insert(id.clone(), measure.card(id, children.contains_key(id) && !open.contains(id)));
    }
    let mut weights: BTreeMap<(ComponentId, ComponentId), u32> = BTreeMap::new();
    for ((from, to, _, overlay, _), (count, _, _)) in &merged {
        if let (End::Node(a), End::Node(b), false) = (from, to, overlay) {
            *weights.entry((a.clone(), b.clone())).or_default() += count;
        }
    }
    // Ties are broken by call order (then the order the view lists relations).
    let mut hints: BTreeMap<ComponentId, (u32, usize)> = BTreeMap::new();
    for (i, e) in view.edges.iter().enumerate().filter(|(_, e)| !(nested && e.kind == RelationKind::Contains)) {
        let hint = (e.order.unwrap_or(u32::MAX), i);
        let entry = hints.entry(e.target.clone()).or_insert(hint);
        *entry = (*entry).min(hint);
    }
    let arranger = Arranger { metrics, children: &children, open: &open, sizes: &sizes, weights: &weights, previous, hidden: &hidden, measure, hints: &hints };
    let mut scene = Scene { parent: parent.clone(), frame: frame.clone(), containers: open.iter().filter(|c| drawn_set.contains(c)).cloned().collect(), ..Scene::default() };
    let tops_drawn: Vec<ComponentId> = tops.iter().filter(|t| !hidden.contains(*t)).cloned().collect();
    let (local, passes, w, h) = arranger.group(&tops_drawn);
    for (id, r) in local {
        scene.rects.insert(id, r);
    }

    // Blocks around it: what's reached outside (left when it only comes in),
    // and what a crowded level holds back.
    let mut left: Vec<(ComponentId, (f32, f32))> = Vec::new();
    let mut right: Vec<(ComponentId, (f32, f32))> = Vec::new();
    for o in view.outside.iter().filter(|o| outside_sides.contains_key(&o.group)) {
        let size = measure.outside(&o.group, o.reached);
        if outside_sides[&o.group] { right.push((o.group.clone(), size)) } else { left.push((o.group.clone(), size)) }
    }
    let reach = |blocks: &[(ComponentId, (f32, f32))]| -> f32 { blocks.iter().map(|(_, (_, bh))| bh + metrics.gap_y).sum::<f32>() - metrics.gap_y };
    for (blocks, on_right) in [(&left, false), (&right, true)] {
        let widest = blocks.iter().map(|(_, (bw, _))| *bw).fold(0.0, f32::max);
        let x = if on_right { metrics.snap_up(w + metrics.gap_x * 2.0) } else { metrics.snap(-metrics.gap_x * 2.0 - widest) };
        let mut y = metrics.snap(((h - reach(blocks)) / 2.0).max(0.0));
        for (group, (bw, bh)) in blocks.iter() {
            let reached = view.outside.iter().find(|o| &o.group == group).map_or(0, |o| o.reached);
            let bx = if on_right { x } else { x + widest - bw };
            scene.outside.insert(group.clone(), (Rect { x: metrics.snap(bx), y, w: *bw, h: *bh }, reached));
            y = metrics.snap_up(y + bh + metrics.gap_y);
        }
    }
    if !hidden.is_empty() {
        let (bw, bh) = measure.more(hidden.len());
        let at = match frame.as_ref().and_then(|f| scene.rects.get(f)).copied() {
            // Inside the frame, after its parts.
            Some(f) => {
                let bottom = scene.rects.iter().filter(|(id, _)| Some(*id) != frame.as_ref()).map(|(_, r)| r.bottom()).fold(f.y + metrics.header, f32::max);
                let grown = Rect { h: metrics.snap_up(bottom + metrics.gap_y + bh + metrics.pad - f.y), w: f.w.max(bw + metrics.pad * 2.0), ..f };
                if let Some(frame_id) = &frame {
                    scene.rects.insert(frame_id.clone(), grown);
                }
                (f.x + metrics.pad, metrics.snap_up(bottom + metrics.gap_y))
            }
            None => (0.0, metrics.snap_up(h + metrics.gap_y * 2.0)),
        };
        scene.more = Some((Rect { x: at.0, y: at.1, w: bw, h: bh }, hidden.iter().cloned().collect()));
    }

    // Lines: heaviest first, so the main ones run straightest.
    let mut lines: Vec<Line> = merged
        .into_iter()
        .map(|((from, to, kind, overlay, unconfirmed), (count, alignment, carries))| Line { from, to, kind, count, alignment, overlay, points: Vec::new(), exit: None, carries, unconfirmed })
        .collect();
    let mut order: Vec<usize> = (0..lines.len()).collect();
    let span = |l: &Line| match (scene.rect_of(&l.from), scene.rect_of(&l.to)) {
        (Some(a), Some(b)) => (a.center().0 - b.center().0).abs() + (a.center().1 - b.center().1).abs(),
        _ => f32::MAX,
    };
    order.sort_by(|a, b| lines[*b].count.cmp(&lines[*a].count).then(span(&lines[*a]).total_cmp(&span(&lines[*b]))).then(a.cmp(b)));
    // Lines between siblings far apart run through their slots.
    let through_slots = |from: &End, to: &End| -> Vec<(f32, f32)> {
        let (End::Node(a), End::Node(b)) = (from, to) else { return Vec::new() };
        let up_a: Vec<ComponentId> = std::iter::once(a.clone()).chain(chain(&parent, a)).collect();
        let up_b: Vec<ComponentId> = std::iter::once(b.clone()).chain(chain(&parent, b)).collect();
        // The siblings they're in: just below what holds both (or the top).
        let common = up_b.iter().position(|x| up_a.contains(x));
        let (oa, ob) = match common {
            Some(k) => {
                let c = &up_b[k];
                let ka = up_a.iter().position(|x| x == c).unwrap_or(0);
                if k == 0 || ka == 0 {
                    return Vec::new();
                }
                (up_a[ka - 1].clone(), up_b[k - 1].clone())
            }
            None => (up_a[up_a.len() - 1].clone(), up_b[up_b.len() - 1].clone()),
        };
        match (passes.get(&(oa.clone(), ob.clone())), passes.get(&(ob, oa))) {
            (Some(points), _) => points.clone(),
            (None, Some(points)) => points.iter().rev().copied().collect(),
            _ => Vec::new(),
        }
    };
    let ends: Vec<(Rect, Rect, Vec<(f32, f32)>)> =
        order.iter().filter_map(|i| Some((scene.rect_of(&lines[*i].from)?, scene.rect_of(&lines[*i].to)?, through_slots(&lines[*i].from, &lines[*i].to)))).collect();
    let routes = route(&scene.obstacles(), &ends, scene.bounds().unwrap_or_default(), metrics);
    for (i, points) in order.into_iter().zip(routes) {
        lines[i].points = points;
    }
    let frame_rect = frame.as_ref().and_then(|f| scene.rects.get(f)).copied();
    for line in &mut lines {
        if let Some(f) = frame_rect
            && matches!((&line.from, &line.to), (End::Outside(_), _) | (_, End::Outside(_)))
        {
            line.exit = crossing(&line.points, &f);
        }
    }
    scene.lines = lines;
    scene
}

/// A story: each step a row, deeper steps further right, joined to the
/// step they're told under by a line down the gutter on the left (so lines
/// never cross a card); runs of steps in one module share a band.
fn story(view: &View, options: &Options, measure: &dyn Measure, metrics: &Metrics) -> Scene {
    let told = super::story::tell(view, options.shut);
    let indent = metrics.snap_up(metrics.gap_x / 2.0);
    let mut scene = Scene::default();
    let mut y = 0.0f32;
    for row in &told.rows {
        let (w, h) = measure.card(&row.id, false);
        scene.rects.insert(row.id.clone(), Rect { x: row.depth as f32 * indent, y, w, h });
        y = metrics.snap_up(y + h + metrics.gap_y / 2.0);
    }
    let outward = view.spec.direction != super::Direction::In;
    for row in &told.rows {
        let (Some(under), Some(child)) = (row.under.as_ref(), scene.rects.get(&row.id).copied()) else { continue };
        let Some(parent) = scene.rects.get(under).copied() else { continue };
        let gutter = parent.x + indent / 2.0;
        let mut points = vec![(gutter, parent.bottom()), (gutter, child.center().1), (child.x, child.center().1)];
        let edge = view.edges.iter().find(|e| e.kind == RelationKind::Calls && ((&e.source, &e.target) == (under, &row.id) || (&e.source, &e.target) == (&row.id, under)));
        let (mut from, mut to) = (End::Node(under.clone()), End::Node(row.id.clone()));
        // Backwards, the caller is the deeper one: the arrow points up the story.
        if !outward {
            points.reverse();
            std::mem::swap(&mut from, &mut to);
        }
        scene.lines.push(Line { from, to, kind: RelationKind::Calls, count: 1, alignment: edge.map_or(Alignment::Observed, |e| e.alignment), overlay: false, points, exit: None, carries: Vec::new(), unconfirmed: false });
    }
    for row in &told.rows {
        let note = Note { also: told.also.get(&row.id).copied().unwrap_or(0), loops: told.loops.get(&row.id).cloned().unwrap_or_default(), inside: told.inside.get(&row.id).copied().unwrap_or(0) };
        if note != Note::default() {
            scene.notes.insert(row.id.clone(), note);
        }
    }
    // Bands: consecutive steps in the same module, across the whole width
    // (with room for what's said beside the steps).
    let right = scene.rects.values().map(Rect::right).fold(0.0, f32::max) + metrics.gap_x * 2.0;
    let module_of = |id: &ComponentId| view.node(id).and_then(|n| n.container.clone());
    let mut start = 0;
    while start < told.rows.len() {
        let module = module_of(&told.rows[start].id);
        let mut end = start;
        while end + 1 < told.rows.len() && module_of(&told.rows[end + 1].id) == module {
            end += 1;
        }
        if let (Some(module), Some(top), Some(bottom)) = (module, scene.rects.get(&told.rows[start].id), scene.rects.get(&told.rows[end].id)) {
            let margin = metrics.gap_y / 4.0;
            scene.bands.push(Band { module, rect: Rect { x: -metrics.pad, y: top.y - margin, w: right + metrics.pad, h: bottom.bottom() - top.y + margin * 2.0 } });
        }
        start = end + 1;
    }
    scene
}

/// The least settled of two alignments (a merged line shows the worst).
fn worst(a: Alignment, b: Alignment) -> Alignment {
    let rank = |x: Alignment| match x {
        Alignment::Missing => 0,
        Alignment::Planned => 1,
        Alignment::Present => 2,
        Alignment::Observed => 3,
    };
    if rank(b) < rank(a) { b } else { a }
}

/// Where a polyline crosses a rectangle's border (the first time).
fn crossing(points: &[(f32, f32)], r: &Rect) -> Option<(f32, f32)> {
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let inside = |p: (f32, f32)| p.0 >= r.x && p.0 <= r.right() && p.1 >= r.y && p.1 <= r.bottom();
        if inside(a) != inside(b) {
            // Segments are straight: step along to the border.
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            for _ in 0..24 {
                let mid = (lo + hi) / 2.0;
                let p = (a.0 + (b.0 - a.0) * mid, a.1 + (b.1 - a.1) * mid);
                if inside(p) == inside(a) { lo = mid } else { hi = mid }
            }
            return Some((a.0 + (b.0 - a.0) * lo, a.1 + (b.1 - a.1) * lo));
        }
    }
    None
}

// ---- arrangement ----

struct Arranger<'a> {
    metrics: &'a Metrics,
    children: &'a BTreeMap<ComponentId, Vec<ComponentId>>,
    open: &'a BTreeSet<ComponentId>,
    sizes: &'a BTreeMap<ComponentId, (f32, f32)>,
    weights: &'a BTreeMap<(ComponentId, ComponentId), u32>,
    previous: Option<&'a Scene>,
    hidden: &'a BTreeSet<ComponentId>,
    measure: &'a dyn Measure,
    hints: &'a BTreeMap<ComponentId, (u32, usize)>,
}

/// Where long lines pass between siblings: for a pair (the one in the
/// earlier column first), the points its slots run through, left to right.
type Passes = BTreeMap<(ComponentId, ComponentId), Vec<(f32, f32)>>;

impl Arranger<'_> {
    /// Lays out `items` (siblings) and everything inside the open ones, from
    /// (0, 0); returns every rect, where long lines pass, and the group's size.
    fn group(&self, items: &[ComponentId]) -> (Vec<(ComponentId, Rect)>, Passes, f32, f32) {
        let m = self.metrics;
        // Each item's own size: a card, or an open module around its arranged parts.
        let mut inner: Vec<(Vec<(ComponentId, Rect)>, Passes)> = Vec::new();
        let mut sizes: Vec<(f32, f32)> = Vec::new();
        for id in items {
            if self.open.contains(id) {
                let kids: Vec<ComponentId> = self.children.get(id).into_iter().flatten().filter(|k| !self.hidden.contains(*k)).cloned().collect();
                let (rects, passes, w, h) = self.group(&kids);
                let title = self.measure.card(id, true).0;
                sizes.push((m.snap_up((w + m.pad * 2.0).max(title)), m.snap_up(h + m.header + m.pad)));
                let shift = |p: &(f32, f32)| (p.0 + m.pad, p.1 + m.header);
                inner.push((rects.into_iter().map(|(k, r)| (k, r.moved(m.pad, m.header))).collect(), passes.into_iter().map(|(k, pts)| (k, pts.iter().map(shift).collect())).collect()));
            } else {
                sizes.push(self.sizes.get(id).copied().unwrap_or((m.card_min_w, m.grid)));
                inner.push((Vec::new(), Passes::new()));
            }
        }
        // How strongly siblings are tied: relations between anything inside them.
        let n = items.len();
        let mut owner: BTreeMap<&ComponentId, usize> = BTreeMap::new();
        for (i, (rects, _)) in inner.iter().enumerate() {
            for (id, _) in rects {
                owner.insert(id, i);
            }
            owner.insert(&items[i], i);
        }
        let mut w = vec![vec![0u32; n]; n];
        for ((a, b), count) in self.weights {
            if let (Some(&i), Some(&j)) = (owner.get(a), owner.get(b))
                && i != j
            {
                w[i][j] += count;
            }
        }
        let previous: Vec<Option<Rect>> = items.iter().map(|id| self.previous.and_then(|p| p.rects.get(id)).copied()).collect();
        let keys: Vec<Key> = items.iter().map(|id| (self.hints.get(id).copied().unwrap_or((u32::MAX, usize::MAX)), id.clone())).collect();
        let placed = place(&sizes, &w, &previous, &keys, m);
        let mut out: Vec<(ComponentId, Rect)> = Vec::new();
        let mut passes = Passes::new();
        let (mut gw, mut gh) = (0.0f32, 0.0f32);
        for (i, id) in items.iter().enumerate() {
            let (x, y) = placed.pos[i];
            let (iw, ih) = sizes[i];
            out.push((id.clone(), Rect { x, y, w: iw, h: ih }));
            out.extend(inner[i].0.iter().map(|(k, r)| (k.clone(), r.moved(x, y))));
            passes.extend(inner[i].1.iter().map(|(k, pts)| (k.clone(), pts.iter().map(|p| (p.0 + x, p.1 + y)).collect())));
            gw = gw.max(x + iw);
            gh = gh.max(y + ih);
        }
        for ((lo, hi), pts) in placed.passes {
            gh = pts.iter().map(|p| p.1).fold(gh, f32::max);
            passes.insert((items[lo].clone(), items[hi].clone()), pts);
        }
        (out, passes, gw, gh)
    }
}

/// Siblings placed: their positions, and where lines between siblings more
/// than a column apart pass through the columns between.
struct Placed {
    pos: Vec<(f32, f32)>,
    passes: BTreeMap<(usize, usize), Vec<(f32, f32)>>,
}

/// Positions for siblings of the given sizes, tied by weights `w[i][j]`
/// (i depends on j): tied ones in columns, what depends before what it
/// depends on, each near what it's tied to; untied ones in rows below.
/// A line spanning columns gets a slot in each column it passes, shared by
/// every line going to the same place (so they travel together). Where an
/// earlier scene placed them, their order is kept.
/// How siblings tie-break: call order, then the view's order, then the id.
type Key = ((u32, usize), ComponentId);

fn place(sizes: &[(f32, f32)], w: &[Vec<u32>], previous: &[Option<Rect>], ids: &[Key], m: &Metrics) -> Placed {
    let n = sizes.len();
    let tie = |i: usize, j: usize| w[i][j] + w[j][i];
    let tied: Vec<usize> = (0..n).filter(|&i| (0..n).any(|j| j != i && tie(i, j) > 0)).collect();
    let loose: Vec<usize> = (0..n).filter(|i| !tied.contains(i)).collect();
    let settled = previous.iter().filter(|p| p.is_some()).count() * 2 >= n.max(1);
    let mut pos = vec![(0.0f32, 0.0f32); n];

    // Cycles broken by a greedy order (sources first, sinks last, else the
    // one that leads most); then the longest path puts each in its column.
    let mut rank = vec![0usize; n];
    {
        let mut left: BTreeSet<usize> = tied.iter().copied().collect();
        let (mut head, mut tail): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
        while !left.is_empty() {
            let out_of = |i: usize, left: &BTreeSet<usize>| left.iter().filter(|&&j| j != i).map(|&j| w[i][j]).sum::<u32>();
            let into = |i: usize, left: &BTreeSet<usize>| left.iter().filter(|&&j| j != i).map(|&j| w[j][i]).sum::<u32>();
            if let Some(&sink) = left.iter().find(|&&i| out_of(i, &left) == 0) {
                tail.push(sink);
                left.remove(&sink);
                continue;
            }
            if let Some(&source) = left.iter().find(|&&i| into(i, &left) == 0) {
                head.push(source);
                left.remove(&source);
                continue;
            }
            let best = left.iter().copied().max_by_key(|&i| (out_of(i, &left) as i64 - into(i, &left) as i64, Reverse(i))).unwrap_or(0);
            head.push(best);
            left.remove(&best);
        }
        tail.reverse();
        for (r, i) in head.into_iter().chain(tail).enumerate() {
            rank[i] = r;
        }
    }
    let mut by_rank = tied.clone();
    by_rank.sort_by_key(|&i| rank[i]);
    let mut layer = vec![0usize; n];
    for &j in &by_rank {
        for &i in &tied {
            if rank[i] < rank[j] && tie(i, j) > 0 {
                layer[j] = layer[j].max(layer[i] + 1);
            }
        }
    }
    let layers = tied.iter().map(|&i| layer[i] + 1).max().unwrap_or(0);

    // Elements in columns: the tied siblings, and slots for long lines (one
    // per column and far end).
    let mut elem_layer: Vec<usize> = layer.clone();
    let mut slot_end: Vec<Option<usize>> = vec![None; n];
    let mut slot_of: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut adj: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    let mut chains: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for (a, &i) in tied.iter().enumerate() {
        for &j in tied.iter().skip(a + 1) {
            let weight = tie(i, j);
            if weight == 0 {
                continue;
            }
            let (lo, hi) = if layer[i] <= layer[j] { (i, j) } else { (j, i) };
            let mut prev = lo;
            let mut chain = Vec::new();
            for l in layer[lo] + 1..layer[hi] {
                let slot = *slot_of.entry((hi, l)).or_insert_with(|| {
                    elem_layer.push(l);
                    slot_end.push(Some(hi));
                    elem_layer.len() - 1
                });
                *adj.entry((prev, slot)).or_default() += weight;
                *adj.entry((slot, prev)).or_default() += weight;
                chain.push(slot);
                prev = slot;
            }
            *adj.entry((prev, hi)).or_default() += weight;
            *adj.entry((hi, prev)).or_default() += weight;
            chains.insert((lo, hi), chain);
        }
    }
    let elems = elem_layer.len();
    let mut neighbours: Vec<Vec<(usize, u32)>> = vec![Vec::new(); elems];
    for ((a, b), wt) in &adj {
        neighbours[*a].push((*b, *wt));
    }
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); layers];
    for &i in &tied {
        columns[layer[i]].push(i);
    }
    for e in n..elems {
        columns[elem_layer[e]].push(e);
    }
    // Order inside columns: where they were before, else near what they're tied to.
    let prev_center = |e: usize| -> Option<f32> {
        let real = if e < n { e } else { slot_end[e]? };
        previous[real].map(|r| r.y + r.h / 2.0)
    };
    let name = |e: usize| if e < n { (ids[e].clone(), 0) } else { (slot_end[e].map_or_else(|| ids[0].clone(), |h| ids[h].clone()), 1) };
    for col in columns.iter_mut() {
        col.sort_by(|&a, &b| match (prev_center(a), prev_center(b)) {
            (Some(x), Some(y)) => x.total_cmp(&y).then(name(a).cmp(&name(b))),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => name(a).cmp(&name(b)),
        });
    }
    if !settled {
        let mut slot = vec![0f32; elems];
        let index = |columns: &Vec<Vec<usize>>, slot: &mut Vec<f32>| {
            for col in columns {
                for (k, &e) in col.iter().enumerate() {
                    slot[e] = k as f32;
                }
            }
        };
        index(&columns, &mut slot);
        let bary = |e: usize, other: &BTreeSet<usize>, slot: &[f32]| -> f32 {
            let (sum, total) = neighbours[e].iter().filter(|(o, _)| other.contains(o)).fold((0.0f32, 0.0f32), |(s, t), (o, wt)| (s + slot[*o] * *wt as f32, t + *wt as f32));
            if total > 0.0 { sum / total } else { slot[e] }
        };
        for _ in 0..6 {
            for l in 1..layers {
                let other: BTreeSet<usize> = columns[l - 1].iter().copied().collect();
                let key: BTreeMap<usize, f32> = columns[l].iter().map(|&e| (e, bary(e, &other, &slot))).collect();
                columns[l].sort_by(|a, b| key[a].total_cmp(&key[b]).then(name(*a).cmp(&name(*b))));
                index(&columns, &mut slot);
            }
            for l in (0..layers.saturating_sub(1)).rev() {
                let other: BTreeSet<usize> = columns[l + 1].iter().copied().collect();
                let key: BTreeMap<usize, f32> = columns[l].iter().map(|&e| (e, bary(e, &other, &slot))).collect();
                columns[l].sort_by(|a, b| key[a].total_cmp(&key[b]).then(name(*a).cmp(&name(*b))));
                index(&columns, &mut slot);
            }
        }
    }
    // Tall columns wrap into neighbours (not where lines pass through).
    let columns: Vec<Vec<usize>> = columns
        .into_iter()
        .flat_map(|col| if col.iter().any(|&e| e >= n) || col.len() <= m.column.max(1) { vec![col] } else { col.chunks(m.column.max(1)).map(<[usize]>::to_vec).collect() })
        .collect();
    // Columns side by side; in each, every element near the middle of what
    // it's tied to on its left, in order, without overlap.
    let mut at: Vec<Option<(f32, f32)>> = vec![None; elems];
    let mut spans: Vec<(f32, f32)> = vec![(0.0, 0.0); elems];
    let mut x = 0.0f32;
    let mut tied_h = 0.0f32;
    for col in &columns {
        let col_w = col.iter().filter(|&&e| e < n).map(|&e| sizes[e].0).fold(0.0, f32::max);
        let mut bottom: Option<(f32, bool)> = None;
        for &e in col {
            let real = e < n;
            let h = if real { sizes[e].1 } else { m.lane };
            let (sum, weight) = neighbours[e].iter().filter_map(|(o, wt)| at[*o].map(|(_, cy)| (cy, *wt as f32))).fold((0.0f32, 0.0f32), |(s, t), (cy, wt)| (s + cy * wt, t + wt));
            let floor = match bottom {
                Some((b, prev_real)) => b + if prev_real && real { m.gap_y } else { m.lane },
                None => 0.0,
            };
            let want = if weight > 0.0 { sum / weight - h / 2.0 } else { floor };
            let y = m.snap_up(want.max(floor));
            at[e] = Some((x, y + h / 2.0));
            spans[e] = (x, x + col_w);
            if real {
                pos[e] = (x, y);
            }
            bottom = Some((y + h, real));
            tied_h = tied_h.max(y + h);
        }
        x = m.snap_up(x + col_w + m.gap_x);
    }
    let tied_w = (x - m.gap_x).max(0.0);
    let passes: BTreeMap<(usize, usize), Vec<(f32, f32)>> = chains
        .into_iter()
        .filter(|(_, chain)| !chain.is_empty())
        .map(|(pair, chain)| {
            let points = chain.iter().flat_map(|&e| {
                let (_, cy) = at[e].unwrap_or((0.0, 0.0));
                let (x0, x1) = spans[e];
                [(x0, cy), (x1, cy)]
            });
            (pair, points.collect())
        })
        .collect();

    // Untied ones in rows below, kept in their old order.
    let mut loose = loose;
    loose.sort_by(|&a, &b| match (previous[a], previous[b]) {
        (Some(p), Some(q)) => p.y.total_cmp(&q.y).then(p.x.total_cmp(&q.x)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => ids[a].cmp(&ids[b]),
    });
    let area: f32 = loose.iter().map(|&i| (sizes[i].0 + m.gap_x) * (sizes[i].1 + m.gap_y)).sum();
    let widest = loose.iter().map(|&i| sizes[i].0).fold(0.0, f32::max);
    // Rows as wide as before when they were placed before (so they break
    // in the same places), else roughly square.
    let before = loose.iter().filter_map(|&i| previous[i].map(|r| r.right())).fold(0.0f32, f32::max);
    let row_w = if settled && before > 0.0 { before.max(widest) } else { tied_w.max(area.sqrt() * 1.6).max(widest) };
    let top = if tied.is_empty() { 0.0 } else { m.snap_up(tied_h + m.gap_y * 2.0) };
    let (mut lx, mut ly, mut row_h) = (0.0f32, top, 0.0f32);
    for i in loose {
        if lx > 0.0 && lx + sizes[i].0 > row_w {
            lx = 0.0;
            ly = m.snap_up(ly + row_h + m.gap_y);
            row_h = 0.0;
        }
        pos[i] = (lx, ly);
        lx = m.snap_up(lx + sizes[i].0 + m.gap_x / 2.0);
        row_h = row_h.max(sizes[i].1);
    }
    Placed { pos, passes }
}

// ---- routing ----

/// Directions a route runs in.
const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

struct Grid {
    x0: f32,
    y0: f32,
    cell: f32,
    cols: i32,
    rows: i32,
    blocked: Vec<bool>,
    /// How many routes already run through each cell, across and along.
    across: Vec<u16>,
    along: Vec<u16>,
}

impl Grid {
    fn index(&self, c: i32, r: i32) -> Option<usize> {
        (c >= 0 && r >= 0 && c < self.cols && r < self.rows).then(|| (r * self.cols + c) as usize)
    }

    fn cell_of(&self, (x, y): (f32, f32)) -> (i32, i32) {
        (((x - self.x0) / self.cell).floor() as i32, ((y - self.y0) / self.cell).floor() as i32)
    }

    fn center(&self, (c, r): (i32, i32)) -> (f32, f32) {
        (self.x0 + (c as f32 + 0.5) * self.cell, self.y0 + (r as f32 + 0.5) * self.cell)
    }

    fn free(&self, cell: (i32, i32)) -> bool {
        self.index(cell.0, cell.1).is_some_and(|i| !self.blocked[i])
    }
}

/// Which side of a card a route leaves (or enters) by, as an outward step.
fn side(from: &Rect, to: &Rect, lane: f32) -> (i32, i32) {
    if to.x >= from.right() + lane {
        (1, 0)
    } else if to.right() <= from.x - lane {
        (-1, 0)
    } else if to.y >= from.bottom() {
        (0, 1)
    } else {
        (0, -1)
    }
}

/// Routes each (from, to) pair in straight segments around `obstacles`:
/// out of the side facing the other end, through the gaps, turning as
/// little as it can, running along routes going the same way and crossing
/// others as little as it can. Returns corner points, edge to edge.
pub fn route(obstacles: &[Rect], ends: &[(Rect, Rect, Vec<(f32, f32)>)], bounds: Rect, m: &Metrics) -> Vec<Vec<(f32, f32)>> {
    let cell = m.lane.max(1.0);
    let margin = cell * 6.0;
    let x0 = bounds.x - margin;
    let y0 = bounds.y - margin;
    let cols = ((bounds.w + margin * 2.0) / cell).ceil() as i32 + 1;
    let rows = ((bounds.h + margin * 2.0) / cell).ceil() as i32 + 1;
    let mut grid = Grid { x0, y0, cell, cols, rows, blocked: vec![false; (cols * rows) as usize], across: vec![0; (cols * rows) as usize], along: vec![0; (cols * rows) as usize] };
    for r in obstacles {
        let grown = Rect { x: r.x - cell / 2.0, y: r.y - cell / 2.0, w: r.w + cell, h: r.h + cell };
        let (c0, r0) = grid.cell_of((grown.x, grown.y));
        let (c1, r1) = grid.cell_of((grown.right(), grown.bottom()));
        for row in r0..=r1 {
            for col in c0..=c1 {
                if grown.holds(grid.center((col, row)))
                    && let Some(i) = grid.index(col, row)
                {
                    grid.blocked[i] = true;
                }
            }
        }
    }
    // Ports: routes leaving one side of a card spread along it, ordered by
    // where their other ends are.
    let mut ports: BTreeMap<(i64, i64, i32, i32), Vec<(usize, bool, f32)>> = BTreeMap::new();
    let key = |r: &Rect, d: (i32, i32)| ((r.x * 4.0) as i64, (r.y * 4.0) as i64, d.0, d.1);
    for (i, (a, b, _)) in ends.iter().enumerate() {
        let (da, db) = (side(a, b, cell), side(b, a, cell));
        ports.entry(key(a, da)).or_default().push((i, true, if da.0 != 0 { b.center().1 } else { b.center().0 }));
        ports.entry(key(b, db)).or_default().push((i, false, if db.0 != 0 { a.center().1 } else { a.center().0 }));
    }
    let mut offset: BTreeMap<(usize, bool), f32> = BTreeMap::new();
    for ((_, _, dx, _), list) in ports.iter_mut() {
        list.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
        let k = list.len() as f32;
        for (n, (i, start, _)) in list.iter().enumerate() {
            let r = if *start { ends[*i].0 } else { ends[*i].1 };
            let room = if *dx != 0 { r.h } else { r.w } - cell * 2.0;
            let step = if k > 1.0 { (room / (k - 1.0)).clamp(0.0, cell) } else { 0.0 };
            offset.insert((*i, *start), (n as f32 - (k - 1.0) / 2.0) * step);
        }
    }
    let mut buffers = Search::new((cols * rows * 4) as usize);
    ends.iter()
        .enumerate()
        .map(|(i, (a, b, slots))| {
            let (da, db) = (side(a, b, cell), side(b, a, cell));
            let (pa, ca) = port(&grid, a, da, offset.get(&(i, true)).copied().unwrap_or(0.0));
            let (pb, cb) = port(&grid, b, db, offset.get(&(i, false)).copied().unwrap_or(0.0));
            // Through each slot in turn, then to the far end.
            let mut stops: Vec<(i32, i32)> = slots.iter().map(|p| grid.cell_of(*p)).collect();
            stops.dedup();
            stops.push(cb);
            let (mut here, mut dir) = (ca, da);
            let mut cells: Vec<(i32, i32)> = vec![ca];
            for stop in stops {
                let leg = search(&grid, &mut buffers, here, dir, stop, m.bend).unwrap_or_else(|| fallback(here, stop));
                if leg.len() >= 2 {
                    let (p, q) = (leg[leg.len() - 2], leg[leg.len() - 1]);
                    dir = (q.0 - p.0, q.1 - p.1);
                }
                cells.extend(leg.into_iter().skip(1));
                here = stop;
            }
            for pair in cells.windows(2) {
                let horizontal = pair[0].1 == pair[1].1;
                for c in [pair[0], pair[1]] {
                    if let Some(k) = grid.index(c.0, c.1) {
                        if horizontal { grid.along[k] = grid.along[k].saturating_add(1) } else { grid.across[k] = grid.across[k].saturating_add(1) }
                    }
                }
            }
            let mut points = vec![pa];
            points.extend(cells.iter().map(|c| grid.center(*c)));
            points.push(pb);
            corners(points)
        })
        .collect()
}

/// Where a route meets a card's side (offset along it) and the free cell
/// just outside.
fn port(grid: &Grid, r: &Rect, d: (i32, i32), offset: f32) -> ((f32, f32), (i32, i32)) {
    let (cx, cy) = r.center();
    let edge = match d {
        (1, 0) => (r.right(), cy + offset),
        (-1, 0) => (r.x, cy + offset),
        (0, 1) => (cx + offset, r.bottom()),
        _ => (cx + offset, r.y),
    };
    let mut cell = grid.cell_of((edge.0 + d.0 as f32 * grid.cell, edge.1 + d.1 as f32 * grid.cell));
    for _ in 0..4 {
        if grid.free(cell) {
            break;
        }
        cell = (cell.0 + d.0, cell.1 + d.1);
    }
    // The point on the side, in line with the cell.
    let center = grid.center(cell);
    let point = if d.0 != 0 { (edge.0, center.1.clamp(r.y, r.bottom())) } else { (center.0.clamp(r.x, r.right()), edge.1) };
    (point, cell)
}

/// Reused A* buffers (one stamp per search instead of clearing).
struct Search {
    best: Vec<u32>,
    came: Vec<u32>,
    stamp: Vec<u32>,
    round: u32,
}

impl Search {
    fn new(states: usize) -> Self {
        Self { best: vec![0; states], came: vec![u32::MAX; states], stamp: vec![0; states], round: 0 }
    }

    fn best(&self, s: usize) -> Option<u32> {
        (self.stamp[s] == self.round).then_some(self.best[s])
    }

    fn set(&mut self, s: usize, cost: u32, from: u32) {
        self.stamp[s] = self.round;
        self.best[s] = cost;
        self.came[s] = from;
    }
}

/// A* over the grid: steps cost, bends cost more, running along other routes
/// costs less and crossing them more.
fn search(grid: &Grid, buffers: &mut Search, start: (i32, i32), start_dir: (i32, i32), goal: (i32, i32), bend: f32) -> Option<Vec<(i32, i32)>> {
    buffers.round += 1;
    let dir_index = |d: (i32, i32)| DIRS.iter().position(|x| *x == d).unwrap_or(0);
    let state = |c: (i32, i32), d: usize| -> Option<usize> { grid.index(c.0, c.1).map(|i| i * 4 + d) };
    let heuristic = |c: (i32, i32)| (((c.0 - goal.0).abs() + (c.1 - goal.1).abs()) * 6) as u32;
    let bend_cost = (bend * 10.0) as u32;
    let mut open: BinaryHeap<Reverse<(u32, u32, usize)>> = BinaryHeap::new();
    let first = state(start, dir_index(start_dir))?;
    buffers.set(first, 0, u32::MAX);
    open.push(Reverse((heuristic(start), 0, first)));
    let limit = (grid.cols * grid.rows * 4) as usize;
    let mut popped = 0usize;
    while let Some(Reverse((_, cost, s))) = open.pop() {
        popped += 1;
        if popped > limit {
            break;
        }
        if buffers.best(s).is_some_and(|b| b < cost) {
            continue;
        }
        let (cell_index, d) = (s / 4, s % 4);
        let cell = ((cell_index as i32) % grid.cols, (cell_index as i32) / grid.cols);
        if cell == goal {
            let mut path = vec![cell];
            let mut at = s;
            while buffers.came[at] != u32::MAX {
                at = buffers.came[at] as usize;
                let i = at / 4;
                path.push(((i as i32) % grid.cols, (i as i32) / grid.cols));
            }
            path.reverse();
            path.dedup();
            return Some(path);
        }
        for (nd, step) in DIRS.iter().enumerate() {
            // No turning straight back.
            if DIRS[d].0 == -step.0 && DIRS[d].1 == -step.1 {
                continue;
            }
            let next = (cell.0 + step.0, cell.1 + step.1);
            if !grid.free(next) && next != goal {
                continue;
            }
            let Some(k) = grid.index(next.0, next.1) else { continue };
            let horizontal = step.1 == 0;
            let (same, other) = if horizontal { (grid.along[k], grid.across[k]) } else { (grid.across[k], grid.along[k]) };
            let mut c = if same > 0 { 6 } else { 10 };
            if other > 0 {
                c += 20;
            }
            if nd != d {
                c += bend_cost;
            }
            let Some(ns) = state(next, nd) else { continue };
            let total = cost + c;
            if buffers.best(ns).is_none_or(|b| total < b) {
                buffers.set(ns, total, s as u32);
                open.push(Reverse((total + heuristic(next), total, ns)));
            }
        }
    }
    None
}

/// When no way round is found: across, then down, then across.
fn fallback(a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
    let mid = (a.0 + b.0) / 2;
    let mut out = Vec::new();
    let step = |from: i32, to: i32| if to >= from { 1 } else { -1 };
    let mut c = a;
    out.push(c);
    while c.0 != mid {
        c.0 += step(c.0, mid);
        out.push(c);
    }
    while c.1 != b.1 {
        c.1 += step(c.1, b.1);
        out.push(c);
    }
    while c.0 != b.0 {
        c.0 += step(c.0, b.0);
        out.push(c);
    }
    out
}

/// Only the points where a route turns (and its two ends).
fn corners(points: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = Vec::new();
    for p in points {
        if out.last().is_some_and(|l: &(f32, f32)| (l.0 - p.0).abs() < 0.01 && (l.1 - p.1).abs() < 0.01) {
            continue;
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let straight = ((a.0 - b.0).abs() < 0.01 && (b.0 - p.0).abs() < 0.01) || ((a.1 - b.1).abs() < 0.01 && (b.1 - p.1).abs() < 0.01);
            if straight {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// How many places routes cross each other (lines running together cross
/// as one).
pub fn crossings(lines: &[Line]) -> usize {
    let segments: Vec<Vec<((f32, f32), (f32, f32))>> = lines.iter().map(|l| l.points.windows(2).map(|p| (p[0], p[1])).collect()).collect();
    let mut places: BTreeSet<(i64, i64)> = BTreeSet::new();
    for (i, a) in segments.iter().enumerate() {
        for b in segments.iter().skip(i + 1) {
            for sa in a {
                for sb in b {
                    if let Some((x, y)) = cross(*sa, *sb) {
                        places.insert((x.round() as i64, y.round() as i64));
                    }
                }
            }
        }
    }
    places.len()
}

/// Where a horizontal and a vertical segment cross (inside both).
fn cross(a: ((f32, f32), (f32, f32)), b: ((f32, f32), (f32, f32))) -> Option<(f32, f32)> {
    let horizontal = |s: ((f32, f32), (f32, f32))| (s.0.1 - s.1.1).abs() < 0.01;
    let (h, v) = match (horizontal(a), horizontal(b)) {
        (true, false) => (a, b),
        (false, true) => (b, a),
        _ => return None,
    };
    let (hx0, hx1) = (h.0.0.min(h.1.0), h.0.0.max(h.1.0));
    let (vy0, vy1) = (v.0.1.min(v.1.1), v.0.1.max(v.1.1));
    let (x, y) = (v.0.0, h.0.1);
    (x > hx0 + 0.01 && x < hx1 - 0.01 && y > vy0 + 0.01 && y < vy1 - 0.01).then_some((x, y))
}

/// Whether a route's segment runs through a card (other than its ends).
pub fn through(points: &[(f32, f32)], card: &Rect) -> bool {
    points.windows(2).any(|p| {
        let (a, b) = (p[0], p[1]);
        let n = ((a.0 - b.0).abs() + (a.1 - b.1).abs()).ceil().max(1.0) as usize;
        (0..=n).any(|k| {
            let t = k as f32 / n as f32;
            card.holds((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t))
        })
    })
}

/// What breaks the layout rules, in words: cards that overlap (other than
/// nesting), parts outside their open module, lines through a card or not
/// in straight segments.
pub fn problems(scene: &Scene) -> Vec<String> {
    let mut out = Vec::new();
    let cards: Vec<(String, Rect)> = scene
        .cards()
        .map(|(id, r)| (id.to_string(), *r))
        .chain(scene.outside.iter().map(|(g, (r, _))| (format!("outside {g}"), *r)))
        .chain(scene.more.iter().map(|(r, _)| ("more".to_owned(), *r)))
        .collect();
    for (i, (a, ra)) in cards.iter().enumerate() {
        for (b, rb) in cards.iter().skip(i + 1) {
            if ra.overlaps(rb) {
                out.push(format!("{a} and {b} overlap"));
            }
        }
    }
    for (id, r) in &scene.rects {
        if let Some(p) = scene.parent.get(id).and_then(|p| scene.rects.get(p))
            && !p.contains(r)
        {
            out.push(format!("{id} sticks out of its module"));
        }
    }
    for c in &scene.containers {
        let r = scene.rects[c];
        let around = scene.ancestors(c);
        for (id, other) in scene.rects.iter().filter(|(id, _)| *id != c && !scene.ancestors(id).contains(c) && !around.contains(id)) {
            if r.overlaps(other) {
                out.push(format!("{id} overlaps the open {c}"));
            }
        }
        for (name, block) in cards.iter().filter(|(n, _)| n.starts_with("outside") || n == "more") {
            if r.overlaps(block) && !r.contains(block) {
                out.push(format!("{name} overlaps the open {c}"));
            }
        }
    }
    for line in &scene.lines {
        let (Some(a), Some(b)) = (scene.rect_of(&line.from), scene.rect_of(&line.to)) else {
            out.push(format!("{:?} -> {:?} has an end that isn't drawn", line.from, line.to));
            continue;
        };
        for (name, card) in &cards {
            if *card != a && *card != b && through(&line.points, card) {
                out.push(format!("{:?} -> {:?} runs through {name}", line.from, line.to));
            }
        }
        if line.points.windows(2).any(|p| (p[0].0 - p[1].0).abs() > 0.01 && (p[0].1 - p[1].1).abs() > 0.01) {
            out.push(format!("{:?} -> {:?} has a slanted segment", line.from, line.to));
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::view::tests::{id, sample};
    use crate::view::{ViewCatalog, query};

    pub(crate) fn metrics() -> Metrics {
        Metrics { grid: 32.0, gap_x: 96.0, gap_y: 32.0, pad: 16.0, header: 32.0, column: 8, lane: 16.0, bend: 3.0, card_min_w: 96.0, card_max_w: 320.0, char_width: 0.6, line_height: 1.4 }
    }

    /// Cards sized from the name (and a second line for modules).
    pub(crate) struct Names;

    impl Measure for Names {
        fn card(&self, id: &ComponentId, closed: bool) -> (f32, f32) {
            let name = id.as_str().rsplit([':', '.']).next().unwrap_or_default().to_owned();
            if closed { metrics().card(&[(&name, 14.0), ("module · 99", 11.0)]) } else { metrics().card(&[(&name, 14.0), ("function", 11.0)]) }
        }
        fn outside(&self, group: &ComponentId, reached: u32) -> (f32, f32) {
            metrics().card(&[(&format!("{reached} more in {group}"), 12.0)])
        }
        fn more(&self, hidden: usize) -> (f32, f32) {
            metrics().card(&[(&format!("+{hidden} more"), 12.0)])
        }
    }

    pub(crate) fn scene_of(view: &View, open: &BTreeSet<ComponentId>, previous: Option<&Scene>) -> Scene {
        build(view, &Options { open, selection: None, max_cards: 60, show_all: false, shut: &BTreeSet::new() }, &Names, &metrics(), previous)
    }

    pub(crate) fn sound(scene: &Scene) {
        let found = problems(scene);
        assert!(found.is_empty(), "{found:#?}");
    }

    // req: G.4, G.7, P7.3
    #[test]
    fn callers_come_before_what_they_call_on_a_grid_without_overlaps() {
        let g = sample();
        let mut calls = ViewCatalog::defaults().unwrap().get("calls").unwrap().clone();
        let v = query(&g, &calls);
        let s = scene_of(&v, &BTreeSet::new(), None);
        sound(&s);
        let x = |n: &str| s.rects[&id(n)].x;
        assert!(x("app::run") < x("app::step") && x("app::step") < x("app::save") && x("app::run") < x("app::log"));
        for r in s.rects.values() {
            assert_eq!((r.x % 16.0, r.y % 16.0, r.w % 16.0, r.h % 16.0), (0.0, 0.0, 0.0, 0.0), "on the grid: {r:?}");
        }
        assert_eq!(scene_of(&v, &BTreeSet::new(), None), s, "the same view, the same scene");
        // A cycle still lays out.
        let mut g2 = g.clone();
        g2.upsert_relation(crate::Relation::new(RelationKind::Calls, id("app::save"), id("app::run"), crate::Provenance::OBSERVED));
        calls.roots = vec![id("app::run")];
        let s2 = scene_of(&query(&g2, &calls), &BTreeSet::new(), None);
        assert_eq!(s2.rects.len(), 4);
        sound(&s2);
    }

    // req: G.1, G.6, P7.3
    #[test]
    fn modules_show_shut_until_opened_and_relations_inside_them_merge_onto_them() {
        let mut g = sample();
        // lib::util calls app::step twice over (two call sites, one relation) and app::run once.
        g.upsert_relation(crate::Relation::new(RelationKind::Calls, id("lib::util"), id("app::step"), crate::Provenance::OBSERVED));
        g.upsert_relation(crate::Relation::new(RelationKind::Calls, id("lib::util"), id("app::run"), crate::Provenance::OBSERVED));
        let structure = ViewCatalog::defaults().unwrap().get("structure").unwrap().clone();
        let v = query(&g, &structure);
        assert!(v.links.iter().any(|l| l.source == id("lib::util") && l.target == id("app::step") && l.kind == RelationKind::Calls));
        // Shut: one card each, and one line lib -> app counting both calls.
        let shut = scene_of(&v, &BTreeSet::new(), None);
        sound(&shut);
        assert!(shut.containers.is_empty() && shut.rects.contains_key(&id("app")) && !shut.rects.contains_key(&id("app::run")));
        let merged: Vec<&Line> = shut.lines.iter().filter(|l| l.kind == RelationKind::Calls).collect();
        assert_eq!(merged.len(), 1, "{:?}", shut.lines);
        assert_eq!((&merged[0].from, &merged[0].to, merged[0].count), (&End::Node(id("lib")), &End::Node(id("app")), 2));
        // Opened in place: its parts inside it, the line splits onto them.
        let open: BTreeSet<ComponentId> = [id("app")].into();
        let opened = scene_of(&v, &open, Some(&shut));
        sound(&opened);
        assert!(opened.containers.contains(&id("app")));
        for part in ["app::run", "app::step", "app::save", "app::log"] {
            assert!(opened.rects[&id("app")].contains(&opened.rects[&id(part)]));
        }
        let to: BTreeSet<End> = opened.lines.iter().filter(|l| l.kind == RelationKind::Calls && l.from == End::Node(id("lib"))).map(|l| l.to.clone()).collect();
        assert_eq!(to, [End::Node(id("app::run")), End::Node(id("app::step"))].into());
        // lib stays where it was.
        assert_eq!(opened.rects[&id("lib")].x < opened.rects[&id("app")].x, shut.rects[&id("lib")].x < shut.rects[&id("app")].x);
    }

    // req: G.5, G.6
    #[test]
    fn a_focused_module_is_the_level_and_what_it_reaches_outside_shows_beside_it() {
        let mut g = sample();
        g.upsert_relation(crate::Relation::new(RelationKind::Calls, id("app::step"), id("lib::util"), crate::Provenance::OBSERVED));
        let mut structure = ViewCatalog::defaults().unwrap().get("structure").unwrap().clone();
        structure.roots = vec![id("app")];
        let v = query(&g, &structure);
        assert_eq!(v.trail, Vec::<ComponentId>::new(), "app is top-level");
        assert_eq!(v.outside.len(), 1, "{:?}", v.outside);
        assert_eq!((&v.outside[0].group, v.outside[0].reached), (&id("lib"), 2), "util, and the Step interface step implements");
        let s = scene_of(&v, &BTreeSet::new(), None);
        sound(&s);
        assert_eq!(s.frame, Some(id("app")), "the level is app, open");
        let (block, reached) = s.outside[&id("lib")];
        assert_eq!(reached, 2);
        assert!(block.x > s.rects[&id("app")].right(), "outgoing: on the right");
        let out: Vec<&Line> = s.lines.iter().filter(|l| l.to == End::Outside(id("lib"))).collect();
        assert!(!out.is_empty() && out.iter().all(|l| l.exit.is_some_and(|(x, _)| (x - s.rects[&id("app")].right()).abs() < 1.0)), "lines leave through the frame's edge: {out:?}");
        // Deeper: a member's trail names what holds it.
        structure.roots = vec![id("app::step")];
        assert_eq!(query(&g, &structure).trail, vec![id("app")]);
    }

    // req: G.5
    #[test]
    fn a_crowded_level_shows_the_neighbourhood_and_holds_the_rest_in_more() {
        use crate::view::{ViewEdge, ViewNode};
        let node = |s: &str| ViewNode { id: id(s), depth: Some(0), truncated: false, alignment: Alignment::Observed, members: 0, container: None };
        let call = |a: &str, b: &str| ViewEdge { id: crate::RelationId(format!("calls:{a}->{b}")), kind: RelationKind::Calls, source: id(a), target: id(b), alignment: Alignment::Observed, overlay: false, order: None, carries: None, unconfirmed: None };
        let names: Vec<String> = (0..12).map(|i| format!("n{i:02}")).collect();
        let spec = ViewCatalog::defaults().unwrap().get("calls").unwrap().clone();
        let view = View { spec, roots: vec![], nodes: names.iter().map(|n| node(n)).collect(), edges: vec![call("n05", "n09"), call("n05", "n10"), call("n01", "n02")], links: vec![], trail: vec![], outside: vec![] };
        let open = BTreeSet::new();
        let selected = id("n05");
        let s = build(&view, &Options { open: &open, selection: Some(&selected), max_cards: 4, show_all: false, shut: &BTreeSet::new() }, &Names, &metrics(), None);
        sound(&s);
        let shown: BTreeSet<&ComponentId> = s.rects.keys().collect();
        assert_eq!(shown.len(), 4);
        for n in ["n05", "n09", "n10"] {
            assert!(shown.contains(&id(n)), "{n}: the selection and what it's tied to");
        }
        let (_, held) = s.more.as_ref().unwrap();
        assert_eq!(held.len(), 8);
        assert!(s.lines.iter().all(|l| l.from != End::More || l.to != End::More));
        let all = build(&view, &Options { open: &open, selection: Some(&selected), max_cards: 4, show_all: true, shut: &BTreeSet::new() }, &Names, &metrics(), None);
        assert_eq!((all.rects.len(), all.more.is_none()), (12, true), "opened on demand");
    }

    // req: G.8, P7.3
    #[test]
    fn things_keep_their_places_when_something_is_added() {
        let mut g = sample();
        let calls = ViewCatalog::defaults().unwrap().get("calls").unwrap().clone();
        let before = scene_of(&query(&g, &calls), &BTreeSet::new(), None);
        let mut extra = g.component(&id("app::log")).unwrap().clone();
        extra.id = id("app::audit");
        extra.name = "audit".into();
        g.upsert_component(extra, crate::Provenance::OBSERVED);
        g.upsert_relation(crate::Relation::new(RelationKind::Calls, id("app::run"), id("app::audit"), crate::Provenance::OBSERVED));
        let after = scene_of(&query(&g, &calls), &BTreeSet::new(), Some(&before));
        sound(&after);
        for (k, r) in &before.rects {
            assert_eq!(after.rects.get(k), Some(r), "{k} stays put");
        }
        assert!(after.rects.contains_key(&id("app::audit")));
    }

    // req: G.7
    #[test]
    fn routes_go_round_cards_and_travel_together() {
        let m = metrics();
        let a = Rect { x: 0.0, y: 0.0, w: 96.0, h: 48.0 };
        let wall = Rect { x: 192.0, y: -64.0, w: 96.0, h: 176.0 };
        let b = Rect { x: 384.0, y: 0.0, w: 96.0, h: 48.0 };
        let bounds = Rect { x: 0.0, y: -64.0, w: 480.0, h: 176.0 };
        let routes = route(&[a, wall, b], &[(a, b, vec![]), (a, b, vec![])], bounds, &m);
        for points in &routes {
            assert!(!through(points, &wall), "{points:?}");
            assert_eq!(points.first().unwrap().0, a.right());
            assert_eq!(points.last().unwrap().0, b.x);
        }
        assert!(routes[0].len() > 2, "it bends round the wall");
    }

    // req: C.1, C.2, C.3
    #[test]
    fn a_story_is_rows_in_call_order_with_lines_down_the_gutter_and_module_bands() {
        let mut g = sample();
        g.upsert_relation(crate::Relation::new(RelationKind::Calls, id("app::step"), id("lib::util"), crate::Provenance::OBSERVED));
        g.upsert_relation(crate::Relation::new(RelationKind::Calls, id("app::save"), id("app::run"), crate::Provenance::OBSERVED));
        let mut spec = ViewCatalog::defaults().unwrap().get("story").unwrap().clone();
        spec.roots = vec![id("app::run")];
        let v = query(&g, &spec);
        let s = scene_of(&v, &BTreeSet::new(), None);
        sound(&s);
        let r = |n: &str| s.rects[&id(n)];
        assert!(r("app::step").y > r("app::run").y && r("app::step").x > r("app::run").x, "under it, to the right");
        assert!(r("app::save").x > r("app::step").x, "deeper, further right");
        assert_eq!(s.notes[&id("app::save")].loops, vec![id("app::run")], "the loop is said beside save");
        // Bands: run, step, save, log are in app; util in lib.
        let modules: Vec<ComponentId> = s.bands.iter().map(|b| b.module.clone()).collect();
        assert!(modules.contains(&id("app")) && modules.contains(&id("lib")), "{modules:?}");
        for b in &s.bands {
            for (step, rect) in &s.rects {
                if v.node(step).and_then(|n| n.container.as_ref()) == Some(&b.module) {
                    assert!(b.rect.contains(rect), "{step} in its band");
                }
            }
        }
        // Collapsed in place: the branch goes, its size is said.
        let shut: BTreeSet<ComponentId> = [id("app::step")].into();
        let folded = build(&v, &Options { open: &BTreeSet::new(), selection: None, max_cards: 60, show_all: false, shut: &shut }, &Names, &metrics(), None);
        assert!(!folded.rects.contains_key(&id("app::save")) && folded.notes[&id("app::step")].inside > 0);
        sound(&folded);
    }
}
