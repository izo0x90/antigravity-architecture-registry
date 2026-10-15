//! The selection: what's tied to it (it stays bright, the rest dims), and
//! moving between cards with the keys. The universal keys in the graph too:
//! moves between cards, go in (Enter) and back (Backspace), opening and
//! closing a card (the folds), search and hints over the cards on screen,
//! and the jump list's places.

use std::collections::BTreeSet;

use arch_graph::tree::{Rect as WorldRect, ViewKind};

use arch_graph::ComponentId;

use super::{GraphState, NavChange};
use crate::nav::{Fold, Item, Motion, Navigable, Place, Spot, Step, Target};

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
        let anchor = self.selection.as_deref().and_then(|s| self.rect_of(s)).map(|r| self.to_screen(&r).center()).filter(|p| self.canvas.contains(*p)).unwrap_or(self.canvas.center());
        let dy = self.canvas.height() / 2.0 * count as f32;
        self.camera.pan.y += if down { -dy } else { dy };
        let nearest = self.tree.as_ref().and_then(|t| t.cards.iter().map(|c| (self.to_screen(&c.rect).center().distance(anchor), c.key.clone())).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, k)| k));
        if let Some(key) = nearest {
            self.select(Some(key));
        }
    }
}

impl GraphState {
    pub fn select(&mut self, key: Option<String>) {
        self.selection = key;
        self.remembered = None;
    }

    /// Selects a component: here if the tree has it; otherwise the view
    /// goes into it (data flow shows calls for it).
    pub fn reveal(&mut self, id: ComponentId) {
        let key = id.0.clone();
        if self.card(&key).is_some() {
            self.select(Some(key.clone()));
            self.reveal_after_fit = Some(key);
            return;
        }
        if self.nav.view == ViewKind::DataFlow {
            self.navigate(NavChange::View { view: ViewKind::Calls, root: None });
        }
        self.navigate(NavChange::GoIn(key.clone()));
        self.pending_select = Some(key);
    }
}

impl Navigable for GraphState {
    fn gesture(&mut self, gesture: crate::touch::Gesture) {
        self.on_gesture(gesture);
    }

    fn place(&self) -> Place {
        Place::Graph { nav: self.nav.clone(), selection: self.selection.clone() }
    }

    fn restore(&mut self, place: &Place) {
        if let Place::Graph { nav, selection } = place {
            self.navigate(NavChange::Restore(Box::new(nav.clone())));
            match selection {
                Some(key) if self.card(key).is_some() && !self.stale => self.select(Some(key.clone())),
                Some(key) => self.pending_select = Some(key.clone()),
                // Back to nothing selected means nothing selected.
                None => {
                    self.selection = None;
                    self.pending_select = None;
                }
            }
        }
    }

    fn target(&self) -> Option<Target> {
        self.selected_card().and_then(|c| c.component.clone()).map(Target::Component)
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let dir = match motion {
            Motion::Left => Some(Dir::Left),
            Motion::Down => Some(Dir::Down),
            Motion::Up => Some(Dir::Up),
            Motion::Right => Some(Dir::Right),
            _ => None,
        };
        if let Some(dir) = dir {
            for _ in 0..count {
                self.move_selection(dir);
            }
            return;
        }
        match motion {
            Motion::HalfDown => self.half_page(true, count),
            Motion::HalfUp => self.half_page(false, count),
            Motion::First => self.select_index(Some(0)),
            Motion::Last => self.select_index(None),
            Motion::Line(n) => self.select_index(Some(n.saturating_sub(1))),
            Motion::NextSibling => self.step_sibling(true, count),
            Motion::PrevSibling => self.step_sibling(false, count),
            Motion::Left | Motion::Down | Motion::Up | Motion::Right => {}
        }
    }

    /// Go in: the selected card becomes the root.
    fn enter(&mut self) -> Step {
        match self.selection.clone() {
            Some(key) if self.nav.root.as_ref() != Some(&key) => {
                self.navigate(NavChange::GoIn(key));
                Step::Done
            }
            Some(_) => Step::Nothing("It's the root already.".into()),
            None => Step::Nothing("Select a card first.".into()),
        }
    }

    fn can_enter(&self) -> bool {
        self.selection.as_ref().is_some_and(|s| self.nav.root.as_ref() != Some(s))
    }

    fn enter_label(&self) -> Option<String> {
        self.selected_card().map(|c| format!("Into {}", c.label))
    }

    fn can_leave(&self) -> bool {
        !self.trail_roots().is_empty() || self.nav.root.is_some()
    }

    fn leave_label(&self) -> Option<String> {
        if let Some(back) = self.trail_roots().last() {
            return Some(match back.as_deref().and_then(|k| self.card(k).map(|c| c.label.clone()).or(Some(k.rsplit([':', '.']).next().unwrap_or(k).to_owned()))) {
                Some(name) => format!("Back to {name}"),
                None => "Back to the starting points".to_owned(),
            });
        }
        self.nav.root.as_ref()?;
        Some(match self.tree.as_ref().and_then(|t| t.path.last()) {
            Some(holder) if self.nav.view == arch_graph::tree::ViewKind::Contains => format!("Out to {}", holder.label),
            _ => "Out to the starting points".to_owned(),
        })
    }

    /// Back: where the last "go in" started, else out to what holds the root.
    fn leave(&mut self) -> Step {
        if self.back() { Step::Done } else { Step::Nothing("Already at the view's starting points.".into()) }
    }

    /// Opening a card shows its children (one level further); closing hides
    /// them. Open all is one more level; close all, the depth's own cards.
    fn fold(&mut self, fold: Fold) {
        match fold {
            Fold::OpenAll => {
                let base = self.depth();
                self.navigate(NavChange::Depth { delta: 1, base });
            }
            Fold::CloseAll => {
                let mut nav = self.nav.clone();
                nav.opened.clear();
                nav.closed.clear();
                self.navigate(NavChange::Restore(Box::new(nav)));
            }
            Fold::Open | Fold::Close | Fold::Toggle => {
                let Some(card) = self.selected_card().cloned() else { return };
                let open = match fold {
                    Fold::Open => true,
                    Fold::Close => false,
                    _ => !card.open,
                };
                match (open, card.open, card.more) {
                    (true, false, more) if more > 0 => self.navigate(NavChange::Open(card.key)),
                    (true, _, _) => self.note = Some(format!("{} has nothing more to show.", card.label)),
                    (false, true, _) => self.navigate(NavChange::Close(card.key)),
                    (false, false, _) => {}
                }
            }
        }
    }

    fn items(&self) -> Vec<Item> {
        let Some(tree) = &self.tree else { return Vec::new() };
        tree.cards
            .iter()
            .map(|c| {
                let symbol = c.key.split_once(':').map_or(c.key.as_str(), |(_, symbol)| symbol);
                Item::new(&c.key, c.label.clone(), symbol)
            })
            .collect()
    }

    fn spots(&self) -> Vec<Spot> {
        let anchor = self.selected_card().map(|c| self.to_screen(&c.rect).center()).unwrap_or(self.canvas.center());
        let mut visible: Vec<(f32, Spot)> = self
            .on_screen()
            .into_iter()
            .map(|c| (self.to_screen(&c.rect), c.key.clone()))
            .filter(|(rect, _)| rect.intersects(self.canvas))
            // The part on screen: a label never sits over the details card.
            .map(|(rect, key)| (rect.center().distance(anchor), Spot { key, rect: rect.intersect(self.canvas) }))
            .collect();
        visible.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.key.cmp(&b.1.key)));
        visible.into_iter().map(|(_, spot)| spot).collect()
    }

    fn cursor_key(&self) -> Option<String> {
        self.selection.clone()
    }

    fn save_view(&mut self) {
        self.saved_view = Some(self.camera());
    }

    fn restore_view(&mut self) {
        if let Some(camera) = self.saved_view.take() {
            self.set_camera(camera);
        }
    }

    fn move_to(&mut self, key: &str) {
        if self.card(key).is_some() {
            self.select(Some(key.to_owned()));
            self.bring_into_view(key);
        }
    }
}
