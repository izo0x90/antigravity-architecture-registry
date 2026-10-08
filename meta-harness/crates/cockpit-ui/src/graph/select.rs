//! The selection: what's tied to it (it stays bright, the rest dims), and
//! moving between cards with the keys.

use std::collections::BTreeSet;

use arch_graph::tree::{Rect as WorldRect, ViewKind};

use super::GraphState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Down,
    Up,
    Right,
}

impl GraphState {
    /// With something selected: it, every card a line joins it to, what it
    /// hangs from, and (a box) what's inside it. Everything else dims.
    pub(crate) fn related(&self) -> Option<BTreeSet<String>> {
        let sel = self.selection.as_ref()?;
        let tree = self.tree.as_ref()?;
        let mut out: BTreeSet<String> = BTreeSet::from([sel.clone()]);
        for line in &tree.lines {
            if &line.from == sel {
                out.insert(line.to.clone());
            } else if &line.to == sel {
                out.insert(line.from.clone());
            }
        }
        let card = self.card(sel)?;
        if let Some(parent) = &card.parent {
            out.insert(parent.clone());
        }
        if tree.request.view == ViewKind::Contains {
            // What holds it, all the way out, and everything inside it.
            let mut up = card.parent.clone();
            while let Some(p) = up {
                up = self.card(&p).and_then(|c| c.parent.clone());
                out.insert(p);
            }
            let mut inside = vec![sel.clone()];
            while let Some(at) = inside.pop() {
                for c in tree.cards.iter().filter(|c| c.parent.as_ref() == Some(&at)) {
                    out.insert(c.key.clone());
                    inside.push(c.key.clone());
                }
            }
        } else {
            out.extend(tree.cards.iter().filter(|c| c.parent.as_ref() == Some(sel)).map(|c| c.key.clone()));
        }
        Some(out)
    }

    /// Whether a line touches the selection (its pills show, it stays bright).
    pub(crate) fn touches_selection(&self, from: &str, to: &str) -> bool {
        self.selection.as_deref().is_some_and(|s| s == from || s == to)
    }

    fn rect_of(&self, key: &str) -> Option<WorldRect> {
        self.card(key).map(|c| c.rect)
    }

    /// Moves the selection to the nearest card that way on the drawing.
    pub fn move_selection(&mut self, dir: Dir) {
        let Some(tree) = &self.tree else { return };
        let Some(current) = self.selection.as_deref().and_then(|s| self.rect_of(s)) else {
            if let Some(first) = tree.roots.first().cloned() {
                self.select(Some(first.clone()));
                self.reveal_after_fit = Some(first);
            }
            return;
        };
        let center = |r: &WorldRect| r.center();
        let (cx, cy) = center(&current);
        let gap = |a0: f32, a1: f32, b0: f32, b1: f32| if a1 < b0 { b0 - a1 } else if b1 < a0 { a0 - b1 } else { 0.0 };
        let best = tree
            .cards
            .iter()
            // Not what it sits in, nor what sits in it: moves go between neighbours.
            .filter(|c| Some(c.key.as_str()) != self.selection.as_deref() && !c.rect.contains(&current) && !current.contains(&c.rect))
            .filter_map(|c| {
                let r = &c.rect;
                let (x, y) = center(r);
                let (along, across) = match dir {
                    Dir::Right => (x - cx, gap(current.y, current.bottom(), r.y, r.bottom())),
                    Dir::Left => (cx - x, gap(current.y, current.bottom(), r.y, r.bottom())),
                    Dir::Down => (y - cy, gap(current.x, current.right(), r.x, r.right())),
                    Dir::Up => (cy - y, gap(current.x, current.right(), r.x, r.right())),
                };
                (along > 1.0 && across <= along * 2.0).then_some((across > 0.5, along + 2.0 * across, c.key.clone()))
            })
            // Cards in the same row (or column) first, then the nearest.
            .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)))
            .map(|(_, _, key)| key)
            // Nothing beside it: a box's moves go into it.
            .or_else(|| {
                tree.cards
                    .iter()
                    .filter(|c| current.contains(&c.rect) && c.rect != current)
                    .min_by(|a, b| (a.rect.y, a.rect.x).partial_cmp(&(b.rect.y, b.rect.x)).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|c| c.key.clone())
            });
        if let Some(best) = best {
            self.select(Some(best.clone()));
            self.reveal_after_fit = Some(best);
        }
    }

    /// Cards hanging from the same card as the selection, top to bottom.
    pub(crate) fn siblings(&self) -> Vec<String> {
        let (Some(tree), Some(sel)) = (&self.tree, self.selected_card()) else { return Vec::new() };
        let mut out: Vec<(&WorldRect, &String)> = tree.cards.iter().filter(|c| c.parent == sel.parent).map(|c| (&c.rect, &c.key)).collect();
        out.sort_by(|a, b| a.0.y.total_cmp(&b.0.y).then(a.0.x.total_cmp(&b.0.x)).then_with(|| a.1.cmp(b.1)));
        out.into_iter().map(|(_, k)| k.clone()).collect()
    }

    pub(crate) fn step_sibling(&mut self, forward: bool, count: usize) {
        let siblings = self.siblings();
        let Some(at) = self.selection.as_ref().and_then(|s| siblings.iter().position(|x| x == s)) else { return };
        let next = if forward { (at + count).min(siblings.len() - 1) } else { at.saturating_sub(count) };
        let key = siblings[next].clone();
        self.select(Some(key.clone()));
        self.reveal_after_fit = Some(key);
    }

    /// The `index`th card in tree order, or the last.
    pub(crate) fn select_index(&mut self, index: Option<usize>) {
        let Some(tree) = &self.tree else { return };
        let card = match index {
            Some(i) => tree.cards.get(i),
            None => tree.cards.last(),
        };
        if let Some(key) = card.map(|c| c.key.clone()) {
            self.select(Some(key.clone()));
            self.reveal_after_fit = Some(key);
        }
    }

    /// Half a screen down (or up): the camera moves and the card nearest to
    /// where the selection was on screen is selected.
    pub(crate) fn half_page(&mut self, down: bool, count: usize) {
        if self.tree.is_none() {
            return;
        }
        let anchor = self.selection.as_deref().and_then(|s| self.rect_of(s)).map(|r| self.to_screen(&r).center()).filter(|p| self.open_area.contains(*p)).unwrap_or(self.open_area.center());
        let dy = self.open_area.height() / 2.0 * count as f32;
        self.camera.pan.y += if down { -dy } else { dy };
        let nearest = self.tree.as_ref().and_then(|t| t.cards.iter().map(|c| (self.to_screen(&c.rect).center().distance(anchor), c.key.clone())).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, k)| k));
        if let Some(key) = nearest {
            self.select(Some(key));
        }
    }
}
