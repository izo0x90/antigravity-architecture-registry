//! Laying trees out. Calls and Data flow are tidy trees left to right: each
//! level a column, each card centred on its children, and each branch
//! moved up as close to the one above as their outlines allow, level by
//! level (Reingold-Tilford), so a small branch tucks in beside a big one and
//! nothing overlaps. Contains is a treemap: boxes in boxes, each row of
//! boxes stretched to fill its parent's width, a box as big as what it
//! holds needs. Tree lines are curves across the gap between two columns; a
//! line to a card drawn further away is a stub in the gap (its whole way is
//! drawn while one of its ends is selected), so no line runs through a
//! card. Time grows with the number of cards; the layout has its own time
//! limit, and what it couldn't place in time is left out and said so.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{Card, Cut, CutReason, LayoutConfig, Line, LineKind, Tree, ViewKind};

pub type Point = (f32, f32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn center(&self) -> Point {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    #[cfg(any(test, feature = "testing"))]
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
    pub fn contains(&self, o: &Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }
}

/// A card's lines of text, each with its font size: the name, then what
/// it is (and how much it hides), then where it is. The cockpit draws
/// exactly these.
pub fn card_lines(card: &Card, view: ViewKind, config: &LayoutConfig) -> Vec<(String, f32)> {
    let mut kind = card.kind.clone();
    if card.more > 0 {
        let what = match view {
            ViewKind::Contains => "inside",
            ViewKind::Calls | ViewKind::DataFlow => "more",
        };
        kind = format!("{kind} · {} {what}", card.more);
    }
    let mut lines = vec![(card.label.clone(), config.name_size), (kind, config.small_size)];
    if !card.detail.is_empty() {
        lines.push((card.detail.clone(), config.small_size));
    }
    lines
}

/// The clickable count of what a card hides ("+6"), drawn inside the
/// card at its right edge; none when it hides nothing.
pub fn count_text(card: &Card) -> Option<String> {
    (card.more > 0).then(|| format!("+{}", card.more))
}

/// A card's size from its text (and room for its count).
fn card_size(card: &Card, view: ViewKind, config: &LayoutConfig) -> (f32, f32) {
    let lines = card_lines(card, view, config);
    let widest = lines.iter().map(|(t, s)| config.text_width(t, *s)).fold(0.0, f32::max);
    let count = count_text(card).map_or(0.0, |c| config.text_width(&c, config.small_size) + config.pad);
    let w = (widest + count + config.pad * 2.0).clamp(config.card_min_w, config.card_max_w);
    let h = config.pad * 2.0 + lines.iter().map(|(_, s)| s * config.line_height).sum::<f32>();
    (w, h)
}

pub(super) fn lay_out(tree: &mut Tree, view: ViewKind, config: &LayoutConfig, out_of_time: &dyn Fn() -> bool) {
    // The first ask starts the limit's clock: it counts from here.
    let _ = out_of_time();
    match view {
        ViewKind::Contains => treemap(tree, config, out_of_time),
        ViewKind::Calls | ViewKind::DataFlow => tidy(tree, view, config, out_of_time),
    }
}

/// Every card under `i` (itself included).
fn branch(i: usize, kids: &BTreeMap<usize, Vec<usize>>) -> Vec<usize> {
    let mut out = vec![i];
    let mut at = 0;
    while at < out.len() {
        out.extend(kids.get(&out[at]).into_iter().flatten().copied());
        at += 1;
    }
    out
}

/// Takes out the branches (and all under them) the layout had no time
/// for, and says so: a card that loses some of what it holds counts it as
/// more, and the tree says it was cut.
fn leave_out(tree: &mut Tree, late: &[usize], kids: &BTreeMap<usize, Vec<usize>>) {
    if late.is_empty() {
        return;
    }
    let gone: BTreeSet<usize> = late.iter().flat_map(|&r| branch(r, kids)).collect();
    let keys: BTreeSet<String> = gone.iter().map(|&i| tree.cards[i].key.clone()).collect();
    let starts = late.iter().filter(|&&i| tree.cards[i].parent.is_none()).count() as u32;
    let mut lost: BTreeMap<String, u32> = BTreeMap::new();
    for &i in late {
        if let Some(p) = &tree.cards[i].parent {
            *lost.entry(p.clone()).or_default() += 1;
        }
    }
    let mut index = 0;
    tree.cards.retain(|_| {
        index += 1;
        !gone.contains(&(index - 1))
    });
    for card in &mut tree.cards {
        if let Some(n) = lost.get(&card.key) {
            card.more += n;
        }
    }
    tree.lines.retain(|l| !keys.contains(&l.from) && !keys.contains(&l.to));
    tree.roots.retain(|r| !keys.contains(r));
    let cut = tree.cut.get_or_insert(Cut { reason: CutReason::Time, starts_left_out: 0, shown: 0, asked: 0, hidden: 0 });
    cut.reason = CutReason::Time;
    cut.starts_left_out += starts;
    cut.hidden += gone.len() as u32 - starts;
}

/// Children of each card in the tree, in card order.
fn children_of(tree: &Tree) -> BTreeMap<usize, Vec<usize>> {
    let index: BTreeMap<&str, usize> = tree.cards.iter().enumerate().map(|(i, c)| (c.key.as_str(), i)).collect();
    let mut kids: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, c) in tree.cards.iter().enumerate() {
        if let Some(p) = c.parent.as_deref().and_then(|p| index.get(p)) {
            kids.entry(*p).or_default().push(i);
        }
    }
    kids
}

/// One branch's outline: for each level from its top card down, how far
/// up and down it reaches (from the branch's top card's centre).
type Outline = Vec<(f32, f32)>;

/// Places `children` (each with its outline) one under the other, each as
/// close to those above as their outlines allow with `gap` between, the
/// first at 0. Returns each one's centre and the outline of them all.
fn stack(children: &[(usize, Outline)], gap: f32) -> (Vec<(usize, f32)>, Outline) {
    let mut placed = Vec::with_capacity(children.len());
    let mut all: Outline = Vec::new();
    for (c, outline) in children {
        let shift = if all.is_empty() {
            0.0
        } else {
            outline.iter().zip(&all).map(|((top, _), (_, bottom))| bottom + gap - top).fold(f32::MIN, f32::max)
        };
        for (level, (top, bottom)) in outline.iter().enumerate() {
            match all.get_mut(level) {
                Some((t, b)) => {
                    *t = t.min(top + shift);
                    *b = b.max(bottom + shift);
                }
                None => all.push((top + shift, bottom + shift)),
            }
        }
        placed.push((*c, shift));
    }
    (placed, all)
}

/// Lays one branch out as a tidy tree, children before their card.
struct Shape<'a> {
    sizes: &'a [(f32, f32)],
    kids: &'a BTreeMap<usize, Vec<usize>>,
    gap: f32,
    out_of_time: &'a dyn Fn() -> bool,
    /// A child's centre below its card's.
    offset: &'a mut [f32],
    /// What there was no time for (left out after).
    late: &'a mut Vec<usize>,
}

impl Shape<'_> {
    /// The branch under `i`: its children stacked, it centred on them.
    /// Time is checked before each child but the first, at every level, so
    /// even one big branch is cut where time ran out.
    fn outline(&mut self, i: usize) -> Outline {
        let half = self.sizes[i].1 / 2.0;
        let mut outline = vec![(-half, half)];
        let Some(k) = self.kids.get(&i) else { return outline };
        let mut children: Vec<(usize, Outline)> = Vec::with_capacity(k.len());
        for &c in k {
            if !children.is_empty() && (self.out_of_time)() {
                self.late.push(c);
                continue;
            }
            let below = self.outline(c);
            children.push((c, below));
        }
        let (placed, below) = stack(&children, self.gap);
        let middle = (placed.first().map_or(0.0, |p| p.1) + placed.last().map_or(0.0, |p| p.1)) / 2.0;
        for (c, y) in placed {
            self.offset[c] = y - middle;
        }
        outline.extend(below.into_iter().map(|(t, b)| (t - middle, b - middle)));
        outline
    }
}

/// What each box in a treemap needs: its own text, or its parts' rows.
struct Sizing<'a> {
    tree: &'a Tree,
    kids: &'a BTreeMap<usize, Vec<usize>>,
    config: &'a LayoutConfig,
    out_of_time: &'a dyn Fn() -> bool,
    need: &'a mut [(f32, f32)],
    /// The parts each box keeps (what there was time for).
    kept: &'a mut BTreeMap<usize, Vec<usize>>,
    late: &'a mut Vec<usize>,
}

impl Sizing<'_> {
    /// Its parts first, time checked before each but the first, at every
    /// level.
    fn need(&mut self, i: usize) {
        let own = card_size(&self.tree.cards[i], ViewKind::Contains, self.config);
        let mut parts = Vec::new();
        for &c in self.kids.get(&i).into_iter().flatten() {
            if !parts.is_empty() && (self.out_of_time)() {
                self.late.push(c);
                continue;
            }
            self.need(c);
            parts.push(c);
        }
        self.need[i] = if parts.is_empty() {
            own
        } else {
            let (w, h) = rows_size(&parts, self.need, self.config);
            ((w + self.config.pad * 2.0).max(own.0), self.config.header + h + self.config.pad)
        };
        self.kept.insert(i, parts);
    }
}

/// Calls and Data flow: a tidy tree, left to right.
fn tidy(tree: &mut Tree, view: ViewKind, config: &LayoutConfig, out_of_time: &dyn Fn() -> bool) {
    let sizes: Vec<(f32, f32)> = tree.cards.iter().map(|c| card_size(c, view, config)).collect();
    let kids = children_of(tree);
    let roots: Vec<usize> = (0..tree.cards.len()).filter(|&i| tree.cards[i].parent.is_none()).collect();
    // Each branch, deepest first: its children stacked, it centred on them.
    // `offset[c]`: a child's centre below its parent's.
    let mut offset = vec![0.0f32; tree.cards.len()];
    let mut outlines: BTreeMap<usize, Outline> = BTreeMap::new();
    let mut done: Vec<usize> = Vec::new();
    let mut late: Vec<usize> = Vec::new();
    for &root in &roots {
        if !done.is_empty() && out_of_time() {
            late.push(root);
            continue;
        }
        let mut shape = Shape { sizes: &sizes, kids: &kids, gap: config.gap_y, out_of_time, offset: &mut offset, late: &mut late };
        outlines.insert(root, shape.outline(root));
        done.push(root);
    }
    // The starting points one under the other, two gaps apart.
    let starts: Vec<(usize, Outline)> = done.iter().map(|r| (*r, outlines.remove(r).unwrap_or_default())).collect();
    let (placed, all) = stack(&starts, config.gap_y * 2.0);
    let top = all.iter().map(|(t, _)| *t).fold(0.0f32, f32::min);
    let bottom = all.iter().map(|(_, b)| *b).fold(0.0f32, f32::max);
    // Each level a column as wide as its widest card.
    let levels = tree.cards.iter().map(|c| c.level).max().map_or(0, |m| m + 1) as usize;
    let mut widths = vec![0.0f32; levels];
    for (c, (w, _)) in tree.cards.iter().zip(&sizes) {
        widths[c.level as usize] = widths[c.level as usize].max(*w);
    }
    let mut column_x = vec![0.0f32; levels];
    for l in 1..levels {
        column_x[l] = column_x[l - 1] + widths[l - 1] + config.gap_x;
    }
    let mut stack_: Vec<(usize, f32)> = placed.into_iter().map(|(r, y)| (r, y - top)).collect();
    while let Some((i, centre)) = stack_.pop() {
        let (w, h) = sizes[i];
        tree.cards[i].rect = Rect { x: column_x[tree.cards[i].level as usize], y: centre - h / 2.0, w, h };
        for &c in kids.get(&i).into_iter().flatten() {
            stack_.push((c, centre + offset[c]));
        }
    }
    let width = column_x.last().copied().unwrap_or(0.0) + widths.last().copied().unwrap_or(0.0) + config.gap_x / 4.0;
    route(tree, &column_x, &widths, config);
    tree.size = (width, (bottom - top).max(0.0));
    leave_out(tree, &late, &kids);
}

fn curve(a: Point, b: Point, steps: usize) -> Vec<Point> {
    let mid = (a.0 + b.0) / 2.0;
    let (c1, c2) = ((mid, a.1), (mid, b.1));
    (0..=steps.max(2))
        .map(|s| {
            let t = s as f32 / steps.max(2) as f32;
            let u = 1.0 - t;
            let x = u * u * u * a.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * b.0;
            let y = u * u * u * a.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * b.1;
            (x, y)
        })
        .collect()
}

/// A cubic from `a` to `b` leaving and arriving horizontally, `reach` out
/// from each end first (so a line going back loops round).
fn reach_curve(a: Point, b: Point, reach: f32, steps: usize) -> Vec<Point> {
    let (c1, c2) = ((a.0 + reach, a.1), (b.0 - reach, b.1));
    (0..=steps.max(2))
        .map(|s| {
            let t = s as f32 / steps.max(2) as f32;
            let u = 1.0 - t;
            let x = u * u * u * a.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * b.0;
            let y = u * u * u * a.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * b.1;
            (x, y)
        })
        .collect()
}

/// Gives every line its path.
///
/// A line to the next column (a child, or a card reached again from the
/// level before it) is a curve across the gap. A line to a card drawn
/// further away (reached again, or back up its own path) is a short stub
/// out of its card's lower right, in the gap: drawn always, it never
/// crosses a card. Its whole way to the other card (`reach`) is drawn only
/// while either end is selected.
fn route(tree: &mut Tree, column_x: &[f32], widths: &[f32], config: &LayoutConfig) {
    let index: BTreeMap<String, usize> = tree.cards.iter().enumerate().map(|(i, c)| (c.key.clone(), i)).collect();
    let levels: Vec<u32> = tree.cards.iter().map(|c| c.level).collect();
    for line in &mut tree.lines {
        let (Some(&a), Some(&b)) = (index.get(&line.from), index.get(&line.to)) else { continue };
        let (from, to) = (tree.cards[a].rect, tree.cards[b].rect);
        let level = levels[a] as usize;
        let edge = column_x[level] + widths[level];
        let end = (to.x, to.center().1);
        if line.kind == LineKind::Tree || levels[b] == levels[a] + 1 {
            // Straight to the edge of the start's column (past the wider
            // cards in it), then a curve across the gap.
            let start = (from.right(), from.center().1);
            let mut path = vec![start];
            path.extend(curve((edge, start.1), end, config.curve_steps));
            line.path = path;
        } else {
            let start = (from.right(), from.bottom() - config.pad);
            line.path = vec![start, (edge + config.gap_x / 4.0, start.1)];
            line.reach = reach_curve(start, end, config.gap_x / 2.0, config.curve_steps * 2);
        }
        place_pills(line, end, config);
    }
}

/// A line's pills, stacked just before where it arrives (`end`), each as
/// wide as its text needs.
fn place_pills(line: &mut Line, end: Point, config: &LayoutConfig) {
    if line.pills.is_empty() {
        return;
    }
    let h = config.small_size * config.line_height + config.pill_pad / 2.0;
    let step = h + config.pill_pad / 2.0;
    let mut y = end.1 - step * line.pills.len() as f32 / 2.0;
    for pill in &mut line.pills {
        let w = config.text_width(&pill.text, config.small_size) + config.pill_pad * 2.0;
        pill.rect = Some(Rect { x: end.0 - config.pill_gap - w, y, w, h });
        y += step;
    }
}

/// Contains: a treemap. Each box holds its parts in rows, every row
/// stretched to the box's full width (a part as wide as its share of the
/// row); a box is as big as its parts need, its name in a header on top.
fn treemap(tree: &mut Tree, config: &LayoutConfig, out_of_time: &dyn Fn() -> bool) {
    let kids = children_of(tree);
    let roots: Vec<usize> = (0..tree.cards.len()).filter(|&i| tree.cards[i].parent.is_none()).collect();
    // What each card needs, deepest first: its own text, or its parts' rows.
    let mut need = vec![(0.0f32, 0.0f32); tree.cards.len()];
    let mut done: Vec<usize> = Vec::new();
    let mut late: Vec<usize> = Vec::new();
    // What each box keeps of what it holds (what there was time for).
    let mut kept: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &root in &roots {
        if !done.is_empty() && out_of_time() {
            late.push(root);
            continue;
        }
        let mut sizing = Sizing { tree: &*tree, kids: &kids, config, out_of_time, need: &mut need, kept: &mut kept, late: &mut late };
        sizing.need(root);
        done.push(root);
    }
    let placed_kids = kept;
    // Top down: each box in the place its row gives it, its parts filling it.
    let (w, h) = rows_size(&done, &need, config);
    let mut todo: Vec<(usize, Rect)> = fill_rows(&done, &need, Rect { x: 0.0, y: 0.0, w, h }, config);
    while let Some((i, rect)) = todo.pop() {
        tree.cards[i].rect = rect;
        if let Some(k) = placed_kids.get(&i).filter(|k| !k.is_empty()) {
            let inside = Rect {
                x: rect.x + config.pad,
                y: rect.y + config.header,
                w: rect.w - config.pad * 2.0,
                h: rect.h - config.header - config.pad,
            };
            todo.extend(fill_rows(k, &need, inside, config));
        }
    }
    // Lines (only a card's context has them): straight, centre to centre.
    let index: BTreeMap<String, Rect> = tree.cards.iter().map(|c| (c.key.clone(), c.rect)).collect();
    for line in &mut tree.lines {
        if let (Some(a), Some(b)) = (index.get(&line.from), index.get(&line.to)) {
            line.path = vec![a.center(), b.center()];
        }
    }
    tree.size = (w, h);
    leave_out(tree, &late, &kids);
}

/// `items` broken into rows of a box about `row_aspect` times as wide as
/// it is tall, squarified (Bruls, Huizing and van Wijk): the biggest part
/// first, and a row takes the next part only while that keeps its worst
/// aspect ratio from getting worse (and the row within the box's width:
/// each part keeps at least the size it needs).
fn rows(items: &[usize], need: &[(f32, f32)], config: &LayoutConfig) -> Vec<Vec<usize>> {
    let area_of = |i: usize| (need[i].0 + config.box_gap) * (need[i].1 + config.box_gap);
    let area: f32 = items.iter().map(|&i| area_of(i)).sum();
    let widest = items.iter().map(|&i| need[i].0).fold(0.0, f32::max);
    let row_w = (area * config.row_aspect).sqrt().max(widest);
    let mut order: Vec<usize> = items.to_vec();
    // (A stable sort: parts the same size stay in the order they're written.)
    order.sort_by(|a, b| area_of(*b).total_cmp(&area_of(*a)));
    // A row's worst aspect ratio, its parts sharing the box's width by area.
    let worst = |row: &[usize]| {
        let height = row.iter().map(|&i| area_of(i)).sum::<f32>() / row_w;
        row.iter().map(|&i| area_of(i) / height).map(|w| (w / height).max(height / w)).fold(0.0, f32::max)
    };
    let width = |row: &[usize]| row.iter().map(|&i| need[i].0 + config.box_gap).sum::<f32>();
    let mut out: Vec<Vec<usize>> = Vec::new();
    for i in order {
        let joins = out.last().is_some_and(|row| {
            let mut with = row.clone();
            with.push(i);
            width(row) + need[i].0 <= row_w && worst(&with) <= worst(row)
        });
        match out.last_mut() {
            Some(row) if joins => row.push(i),
            _ => out.push(vec![i]),
        }
    }
    out
}

/// How big `items`' rows are: the widest row, and the rows' heights.
fn rows_size(items: &[usize], need: &[(f32, f32)], config: &LayoutConfig) -> (f32, f32) {
    let rows = rows(items, need, config);
    let width = rows.iter().map(|r| r.iter().map(|&i| need[i].0).sum::<f32>() + config.box_gap * (r.len() - 1) as f32).fold(0.0, f32::max);
    let height = rows.iter().map(|r| r.iter().map(|&i| need[i].1).fold(0.0, f32::max)).sum::<f32>()
        + config.box_gap * rows.len().saturating_sub(1) as f32;
    (width, height)
}

/// `items` in `area`: each row as tall as its tallest, the rows sharing the
/// height left over, each part's width its share of the row (at least what
/// it needs), filling the area's width.
fn fill_rows(items: &[usize], need: &[(f32, f32)], area: Rect, config: &LayoutConfig) -> Vec<(usize, Rect)> {
    let rows = rows(items, need, config);
    let heights: Vec<f32> = rows.iter().map(|r| r.iter().map(|&i| need[i].1).fold(0.0, f32::max)).collect();
    let gaps_y = config.box_gap * rows.len().saturating_sub(1) as f32;
    let spare_y = (area.h - gaps_y - heights.iter().sum::<f32>()).max(0.0) / rows.len().max(1) as f32;
    let mut out = Vec::with_capacity(items.len());
    let mut y = area.y;
    for (row, h) in rows.iter().zip(&heights) {
        let h = h + spare_y;
        let natural: f32 = row.iter().map(|&i| need[i].0).sum();
        let spare_x = (area.w - config.box_gap * (row.len() - 1) as f32 - natural).max(0.0);
        let mut x = area.x;
        for &i in row {
            // The spare width shared by need: a bigger part gets more.
            let w = need[i].0 + spare_x * need[i].0 / natural.max(1.0);
            out.push((i, Rect { x, y, w, h }));
            x += w + config.box_gap;
        }
        y += h + config.box_gap;
    }
    out
}

/// What's wrong with a laid-out tree: cards overlapping (other than a box
/// holding its parts), parts outside their box, cards closer than the gap
/// in a column, lines through cards. Empty when it's clean. (The layout
/// tests' check, built only for them.)
#[cfg(any(test, feature = "testing"))]
pub fn problems(tree: &Tree, config: &LayoutConfig) -> Vec<String> {
    let mut found = Vec::new();
    let holds = |outer: &Card, inner: &Card| {
        let mut p = inner.parent.as_deref();
        while let Some(k) = p {
            if k == outer.key {
                return true;
            }
            p = tree.card(k).and_then(|c| c.parent.as_deref());
        }
        false
    };
    let boxes = tree.request.view == ViewKind::Contains;
    for (i, a) in tree.cards.iter().enumerate() {
        for b in &tree.cards[i + 1..] {
            let nested = boxes && (holds(a, b) || holds(b, a));
            if nested {
                let (outer, inner) = if holds(a, b) { (a, b) } else { (b, a) };
                if !outer.rect.contains(&inner.rect) {
                    found.push(format!("{} sticks out of {}", inner.key, outer.key));
                }
            } else if a.rect.overlaps(&b.rect) {
                found.push(format!("{} overlaps {}", a.key, b.key));
            } else if !boxes && a.level == b.level {
                let gap = (a.rect.y - b.rect.bottom()).max(b.rect.y - a.rect.bottom());
                if gap + 0.5 < config.gap_y {
                    found.push(format!("{} and {} are {gap:.0} apart (less than the gap)", a.key, b.key));
                }
            }
        }
    }
    if !boxes {
        for line in &tree.lines {
            for card in tree.cards.iter().filter(|c| c.key != line.from && c.key != line.to) {
                if line.path.windows(2).any(|w| crosses(w[0], w[1], &card.rect)) {
                    found.push(format!("the line {} -> {} runs through {}", line.from, line.to, card.key));
                }
            }
        }
    }
    found
}

/// Whether the segment a-b passes through the inside of `r` (clipped
/// exactly; touching an edge doesn't count).
#[cfg(any(test, feature = "testing"))]
fn crosses(a: Point, b: Point, r: &Rect) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let inset = 0.5;
    for (p, q) in
        [(-dx, a.0 - (r.x + inset)), (dx, (r.right() - inset) - a.0), (-dy, a.1 - (r.y + inset)), (dy, (r.bottom() - inset) - a.1)]
    {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_treemaps_rows_are_squarified_biggest_first() {
        let config = crate::tree::TreeConfig::defaults().unwrap().layout;
        // Parts as written: small ones first, then big ones.
        let need = [(60.0, 30.0), (60.0, 30.0), (300.0, 200.0), (200.0, 150.0), (60.0, 30.0)];
        let rows = rows(&[0, 1, 2, 3, 4], &need, &config);
        assert_eq!(rows[0][0], 2, "the biggest leads: {rows:?}");
        assert_eq!(rows.iter().flatten().copied().collect::<Vec<_>>().len(), 5, "every part once");
        // Each row is the squarified one: adding its next part would have made
        // its worst aspect ratio worse, or overflowed the box.
        let area = |i: usize| (need[i].0 + config.box_gap) * (need[i].1 + config.box_gap);
        let total: f32 = (0..5).map(area).sum();
        let row_w = (total * config.row_aspect).sqrt().max(300.0);
        let worst = |row: &[usize]| {
            let h = row.iter().map(|&i| area(i)).sum::<f32>() / row_w;
            row.iter().map(|&i| area(i) / h).map(|w| (w / h).max(h / w)).fold(0.0, f32::max)
        };
        for pair in rows.windows(2) {
            let mut with = pair[0].clone();
            with.push(pair[1][0]);
            let width: f32 = with.iter().map(|&i| need[i].0 + config.box_gap).sum::<f32>() - config.box_gap;
            assert!(worst(&with) > worst(&pair[0]) || width > row_w, "{:?} could have taken {}", pair[0], pair[1][0]);
        }
    }
}
