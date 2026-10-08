//! Laying trees out. Calls and Data flow are tidy trees, left to right: each
//! level a column, each card centred on the block its children fill, so
//! cards never overlap and siblings are evenly spaced. Contains is boxes in
//! boxes, packed in rows. Tree lines are curves across the gap between two
//! columns; lines back to a card shown elsewhere travel in lanes above the
//! drawing and down the gaps, so no line runs through a card. Time grows
//! with the number of cards.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Card, LayoutConfig, LineKind, Tree, ViewKind};

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
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
    pub fn contains(&self, o: &Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }
    pub fn holds(&self, (x, y): Point) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
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

/// A card's size from its text.
fn card_size(card: &Card, view: ViewKind, config: &LayoutConfig) -> (f32, f32) {
    let lines = card_lines(card, view, config);
    let widest = lines.iter().map(|(t, s)| config.text_width(t, *s)).fold(0.0, f32::max);
    let w = (widest + config.pad * 2.0).clamp(config.card_min_w, config.card_max_w);
    let h = config.pad * 2.0 + lines.iter().map(|(_, s)| s * config.line_height).sum::<f32>();
    (w, h)
}

pub(super) fn lay_out(tree: &mut Tree, view: ViewKind, config: &LayoutConfig) {
    match view {
        ViewKind::Contains => boxes(tree, config),
        ViewKind::Calls | ViewKind::DataFlow => columns(tree, view, config),
    }
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

fn columns(tree: &mut Tree, view: ViewKind, config: &LayoutConfig) {
    let sizes: Vec<(f32, f32)> = tree.cards.iter().map(|c| card_size(c, view, config)).collect();
    let kids = children_of(tree);
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
    // Each card's block: tall enough for it and for its children's blocks.
    let mut block = vec![0.0f32; tree.cards.len()];
    for i in (0..tree.cards.len()).rev() {
        let inner: f32 = kids.get(&i).map_or(0.0, |k| k.iter().map(|&c| block[c]).sum::<f32>() + config.gap_y * (k.len().saturating_sub(1)) as f32);
        block[i] = sizes[i].1.max(inner);
    }
    let roots: Vec<usize> = (0..tree.cards.len()).filter(|&i| tree.cards[i].parent.is_none()).collect();
    let mut top = 0.0;
    let mut stack: Vec<(usize, f32)> = Vec::new();
    for r in roots {
        stack.push((r, top));
        top += block[r] + config.gap_y * 2.0;
    }
    while let Some((i, block_top)) = stack.pop() {
        let (w, h) = sizes[i];
        let level = tree.cards[i].level as usize;
        tree.cards[i].rect = Rect { x: column_x[level], y: block_top + (block[i] - h) / 2.0, w, h };
        if let Some(k) = kids.get(&i) {
            let inner: f32 = k.iter().map(|&c| block[c]).sum::<f32>() + config.gap_y * (k.len().saturating_sub(1)) as f32;
            let mut y = block_top + (block[i] - inner) / 2.0;
            for &c in k {
                stack.push((c, y));
                y += block[c] + config.gap_y;
            }
        }
    }
    let width = column_x.last().copied().unwrap_or(0.0) + widths.last().copied().unwrap_or(0.0);
    let height = (top - config.gap_y * 2.0).max(0.0);
    let lanes = route(tree, &column_x, &widths, config);
    tree.size = (width, height + lanes);
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

/// Gives every line its path, and returns how far the lanes for lines that
/// skip columns or go back reach above the drawing (cards move down by it).
///
/// A line to the next column (a child, or a card reached again from the
/// level before it) is a curve across the gap. Any other line goes down the
/// gap after its start's column, along a lane above every card, and down
/// the gap before its end's column; lines whose stretches don't overlap
/// share a lane.
fn route(tree: &mut Tree, column_x: &[f32], widths: &[f32], config: &LayoutConfig) -> f32 {
    let index: BTreeMap<String, usize> = tree.cards.iter().enumerate().map(|(i, c)| (c.key.clone(), i)).collect();
    let ends: Vec<Option<(usize, usize)>> = tree.lines.iter().map(|l| Some((*index.get(&l.from)?, *index.get(&l.to)?))).collect();
    let levels: Vec<u32> = tree.cards.iter().map(|c| c.level).collect();
    let next_column = |a: usize, b: usize| levels[b] == levels[a] + 1;
    // Lanes: the stretch each lane-going line covers, packed greedily.
    let gap_in = |level: usize| column_x[level] + widths[level] + config.lane;
    let gap_before = |level: usize| column_x[level] - config.lane;
    let mut lane_of: Vec<usize> = vec![0; tree.lines.len()];
    let mut wanting: Vec<(f32, f32, usize)> = Vec::new();
    for (i, e) in ends.iter().enumerate() {
        let Some((a, b)) = *e else { continue };
        if tree.lines[i].kind == LineKind::Tree || next_column(a, b) {
            continue;
        }
        let (x1, x2) = (gap_in(tree.cards[a].level as usize), gap_before(tree.cards[b].level as usize));
        wanting.push((x1.min(x2), x1.max(x2), i));
    }
    wanting.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut lane_ends: Vec<f32> = Vec::new();
    for (from, to, i) in wanting {
        let free = lane_ends.iter().position(|end| *end + config.lane < from);
        let lane = match free {
            Some(l) => l,
            None => {
                lane_ends.push(f32::MIN);
                lane_ends.len() - 1
            }
        };
        lane_ends[lane] = to;
        lane_of[i] = lane;
    }
    let lanes = if lane_ends.is_empty() { 0.0 } else { config.lane * (lane_ends.len() as f32 + 1.0) };
    for c in &mut tree.cards {
        c.rect.y += lanes;
    }
    for (i, e) in ends.iter().enumerate() {
        let Some((a, b)) = *e else { continue };
        let (from, to) = (tree.cards[a].rect, tree.cards[b].rect);
        let start = (from.right(), from.center().1);
        let end = (to.x, to.center().1);
        let line = &mut tree.lines[i];
        line.path = if line.kind == LineKind::Tree || next_column(a, b) {
            // Straight to the edge of the start's column (past the wider
            // cards in it), then a curve across the gap.
            let level = tree.cards[a].level as usize;
            let edge = (column_x[level] + widths[level], start.1);
            let mut path = vec![start];
            path.extend(curve(edge, end, config.curve_steps));
            path
        } else {
            let y = lanes - config.lane * (lane_of[i] as f32 + 1.0);
            let (out_x, in_x) = (gap_in(tree.cards[a].level as usize), gap_before(tree.cards[b].level as usize));
            vec![start, (out_x, start.1), (out_x, y), (in_x, y), (in_x, end.1), end]
        };
    }
    lanes
}

/// Boxes in boxes: each open card holds its children packed in rows; the
/// roots are packed the same way.
fn boxes(tree: &mut Tree, config: &LayoutConfig) {
    let kids = children_of(tree);
    let mut size = vec![(0.0f32, 0.0f32); tree.cards.len()];
    // Inner placement of each card's children, relative to its content area.
    let mut placed: BTreeMap<usize, Vec<(usize, f32, f32)>> = BTreeMap::new();
    for i in (0..tree.cards.len()).rev() {
        let own = card_size(&tree.cards[i], ViewKind::Contains, config);
        match kids.get(&i) {
            Some(k) if !k.is_empty() => {
                let (spots, w, h) = pack(k, &size, config);
                let width = (w + config.pad * 2.0).max(own.0);
                size[i] = (width, config.header + h + config.pad);
                placed.insert(i, spots);
            }
            _ => size[i] = own,
        }
    }
    let roots: Vec<usize> = (0..tree.cards.len()).filter(|&i| tree.cards[i].parent.is_none()).collect();
    let (spots, w, h) = pack(&roots, &size, config);
    let mut stack: Vec<(usize, f32, f32)> = spots;
    while let Some((i, x, y)) = stack.pop() {
        tree.cards[i].rect = Rect { x, y, w: size[i].0, h: size[i].1 };
        if let Some(inner) = placed.get(&i) {
            for &(c, cx, cy) in inner {
                stack.push((c, x + config.pad + cx, y + config.header + cy));
            }
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
}

/// Packs `items` (with their sizes) in rows about `row_aspect` times as
/// wide as they are tall. Returns each item's offset and the total size.
fn pack(items: &[usize], size: &[(f32, f32)], config: &LayoutConfig) -> (Vec<(usize, f32, f32)>, f32, f32) {
    let area: f32 = items.iter().map(|&i| (size[i].0 + config.box_gap) * (size[i].1 + config.box_gap)).sum();
    let widest = items.iter().map(|&i| size[i].0).fold(0.0, f32::max);
    let row_w = (area * config.row_aspect).sqrt().max(widest);
    let (mut x, mut y, mut row_h, mut width) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut spots = Vec::with_capacity(items.len());
    for &i in items {
        let (w, h) = size[i];
        if x > 0.0 && x + w > row_w {
            y += row_h + config.box_gap;
            x = 0.0;
            row_h = 0.0;
        }
        spots.push((i, x, y));
        x += w + config.box_gap;
        row_h = row_h.max(h);
        width = width.max(x - config.box_gap);
    }
    (spots, width, y + row_h)
}

/// What's wrong with a laid-out tree: cards overlapping (other than a box
/// holding its parts), parts outside their box, cards closer than the gap
/// in a column, lines through cards. Empty when it's clean.
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
fn crosses(a: Point, b: Point, r: &Rect) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let inset = 0.5;
    for (p, q) in [(-dx, a.0 - (r.x + inset)), (dx, (r.right() - inset) - a.0), (-dy, a.1 - (r.y + inset)), (dy, (r.bottom() - inset) - a.1)] {
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
