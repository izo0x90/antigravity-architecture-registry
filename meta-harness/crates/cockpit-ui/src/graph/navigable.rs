//! The universal keys in the graph: moves between cards, go in (Enter) and
//! back (Backspace), opening and closing a card (the folds), search and
//! hints over the cards on screen, and the jump list's places.

use crate::nav::{Fold, Item, Motion, Navigable, Place, Spot, Step, Target};

use super::select::Dir;
use super::{GraphState, NavChange};

impl Navigable for GraphState {
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
            .filter(|(rect, _)| rect.intersects(self.open_area))
            // The part on screen: a label never sits over the details card.
            .map(|(rect, key)| (rect.center().distance(anchor), Spot { key, rect: rect.intersect(self.open_area) }))
            .collect();
        visible.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.key.cmp(&b.1.key)));
        visible.into_iter().map(|(_, spot)| spot).collect()
    }

    fn cursor_key(&self) -> Option<String> {
        self.selection.clone()
    }

    fn save_view(&mut self) {
        self.saved_view = Some((self.camera(), self.drawer_open));
    }

    fn restore_view(&mut self) {
        if let Some((camera, drawer_open)) = self.saved_view.take() {
            self.set_camera(camera);
            self.drawer_open = drawer_open;
        }
    }

    fn move_to(&mut self, key: &str) {
        if self.card(key).is_some() {
            self.select(Some(key.to_owned()));
            self.bring_into_view(key);
        }
    }
}
