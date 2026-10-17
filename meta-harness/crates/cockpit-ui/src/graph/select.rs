//! The selection: what's tied to it (it stays bright, the rest dims), and
//! moving between cards with the keys. The universal keys in the graph too:
//! moves between cards, go in (Enter) and back (Backspace), opening and
//! closing a card (the folds), search and hints over the cards on screen,
//! and the jump list's places.

use std::collections::BTreeSet;

use arch_graph::tree::{Card, Rect as WorldRect, Tree, ViewKind};

use arch_graph::ComponentId;

use egui::Vec2;

use super::draw::FEW;
use super::{GraphState, NavChange};
use crate::nav::{Fold, Item, Motion, Move, Navigable, Place, Spot, Step, Target};
use crate::viewport::Show;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Down,
    Up,
    Right,
}

/// Where a move goes: to a card, or opening one to go to its first card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Goes {
    To(String),
    OpenFirst(String),
}

/// Where going in from `key` leads, as drawn (top to bottom, then left to
/// right): the cards hanging from it; then, in the trees, the cards its
/// lines lead to the way the tree grows that are shown elsewhere (a call
/// to something already drawn in another branch).
pub(super) fn children(tree: &Tree, key: &str) -> Vec<String> {
    let order = |a: &&Card, b: &&Card| a.rect.y.total_cmp(&b.rect.y).then(a.rect.x.total_cmp(&b.rect.x));
    let mut hanging: Vec<&Card> = tree.cards.iter().filter(|c| c.parent.as_deref() == Some(key)).collect();
    hanging.sort_by(order);
    if tree.request.view != ViewKind::Contains {
        let parent = tree.card(key).and_then(|c| c.parent.as_deref());
        let mut elsewhere: Vec<&Card> = tree
            .lines
            .iter()
            // The way the tree grows: what it calls (where it goes), or flipped, what calls it.
            .filter_map(|l| match tree.request.way {
                arch_graph::tree::Way::Down => (l.from == key).then_some(&l.to),
                arch_graph::tree::Way::Up => (l.to == key).then_some(&l.from),
            })
            .filter(|other| Some(other.as_str()) != parent && !hanging.iter().any(|c| &c.key == *other))
            .filter_map(|other| tree.card(other))
            .collect();
        elsewhere.sort_by(order);
        elsewhere.dedup_by(|a, b| a.key == b.key);
        hanging.extend(elsewhere);
    }
    hanging.into_iter().map(|c| c.key.clone()).collect()
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


    /// Where a move `dir` goes from the selection, by the layout. In the
    /// trees (left to right, a column a level): left is the card it hangs
    /// from, right its first card, down and up the next and previous card
    /// in its column, across branches. In the boxes: the nearest box that
    /// way at the same depth, its siblings first. Nothing selected: the
    /// first card. Err: why nothing.
    pub(crate) fn goes(&self, dir: Dir) -> Result<Goes, String> {
        let tree = self.tree.as_ref().ok_or("Nothing shown yet.")?;
        let Some(current) = self.selected_card() else {
            return tree.roots.first().cloned().map(Goes::To).ok_or_else(|| "Nothing shown.".into());
        };
        if tree.request.view == ViewKind::Contains {
            let r = current.rect;
            let (cx, cy) = r.center();
            let gap = |a0: f32, a1: f32, b0: f32, b1: f32| if a1 < b0 { b0 - a1 } else if b1 < a0 { a0 - b1 } else { 0.0 };
            let ways = |siblings: bool| {
                tree.cards
                    .iter()
                    .filter(|c| c.key != current.key && c.level == current.level && (!siblings || c.parent == current.parent))
                    .filter_map(|c| {
                        let (x, y) = c.rect.center();
                        let (along, across) = match dir {
                            Dir::Right => (x - cx, gap(r.y, r.bottom(), c.rect.y, c.rect.bottom())),
                            Dir::Left => (cx - x, gap(r.y, r.bottom(), c.rect.y, c.rect.bottom())),
                            Dir::Down => (y - cy, gap(r.x, r.right(), c.rect.x, c.rect.right())),
                            Dir::Up => (cy - y, gap(r.x, r.right(), c.rect.x, c.rect.right())),
                        };
                        (along > 1.0 && across <= along * 2.0).then_some((across > 0.5, along + 2.0 * across, c.key.clone()))
                    })
                    // In the same row (or column) first, then the nearest.
                    .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)))
                    .map(|(_, _, key)| key)
            };
            return ways(true).or_else(|| ways(false)).map(Goes::To).ok_or_else(|| "Nothing that way at this depth.".into());
        }
        match dir {
            Dir::Left => current.parent.clone().map(Goes::To).ok_or_else(|| "It's where the tree starts.".into()),
            Dir::Right => self.goes_in(),
            Dir::Down | Dir::Up => {
                let mut column: Vec<&Card> = tree.cards.iter().filter(|c| c.level == current.level).collect();
                column.sort_by(|a, b| a.rect.y.total_cmp(&b.rect.y).then(a.rect.x.total_cmp(&b.rect.x)));
                let at = column.iter().position(|c| c.key == current.key).unwrap_or(0);
                let next = if dir == Dir::Down { column.get(at + 1) } else { at.checked_sub(1).and_then(|i| column.get(i)) };
                next.map(|c| Goes::To(c.key.clone())).ok_or_else(|| if dir == Dir::Down { "The last at this depth." } else { "The first at this depth." }.into())
            }
        }
    }

    /// In (Enter): the selected card's first card, opening it if it's
    /// closed: the first box inside a box; the first card from a card in
    /// the trees.
    pub(crate) fn goes_in(&self) -> Result<Goes, String> {
        let (tree, card) = (self.tree.as_ref().ok_or("Nothing shown yet.")?, self.selected_card().ok_or("Select a card first.")?);
        let boxes = tree.request.view == ViewKind::Contains;
        if let Some(first) = children(tree, &card.key).into_iter().next() {
            return Ok(Goes::To(first));
        }
        if card.more > 0 && !card.open {
            return Ok(Goes::OpenFirst(card.key.clone()));
        }
        Err(if boxes { format!("Nothing inside {}.", card.label) } else { format!("Nothing comes from {} here.", card.label) })
    }

    /// Out (Backspace): the card the selection hangs from.
    pub(crate) fn goes_out(&self) -> Option<&Card> {
        self.card(self.selected_card()?.parent.as_deref()?)
    }

    /// Takes a move: the card selected and kept in view; or opened, its
    /// first card selected once it shows.
    pub(crate) fn take(&mut self, goes: Goes) {
        match goes {
            Goes::To(key) => {
                self.select(Some(key.clone()));
                self.reveal_after_fit = Some((key, Show::InView));
            }
            Goes::OpenFirst(key) => {
                self.first_child_of = Some(key.clone());
                self.navigate(NavChange::Open(key));
            }
        }
    }

    /// In words, for its key.
    fn goes_label(&self, goes: &Goes) -> String {
        let name = |key: &str| self.card(key).map_or_else(|| key.to_owned(), |c| c.label.clone());
        match goes {
            Goes::To(key) => format!("To {}", name(key)),
            Goes::OpenFirst(key) => format!("Open {} and go to its first", name(key)),
        }
    }

    /// After moving by hand, a key starts from what's on the screen: the
    /// selection, when it's off it, comes to the nearest card that shows.
    fn start_from_what_shows(&mut self) {
        if !std::mem::take(&mut self.by_hand) {
            return;
        }
        let at = self.selected_card().map(|c| self.to_screen(&c.rect));
        if at.is_some_and(|r| self.canvas.intersects(r)) {
            return;
        }
        let from = at.map_or(self.canvas.center(), |r| r.center());
        let nearest = self.on_screen().into_iter().map(|c| (self.to_screen(&c.rect).center().distance(from), c.key.clone())).min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, key)) = nearest {
            self.select(Some(key));
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
        self.reveal_after_fit = Some((key, Show::InView));
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
            self.reveal_after_fit = Some((key, Show::InView));
        }
    }

    /// Half a screen `way` (down: +y), as far as the tree's edges let the
    /// view go; the selection moves the whole way, from where it is on the
    /// screen (else the middle), to the card there.
    pub(crate) fn half_page(&mut self, way: Vec2, count: usize) {
        let Some(tree) = &self.tree else { return };
        let camera = self.aimed();
        let by = way * self.canvas.size() / 2.0 * count as f32;
        let to = self.clamped(super::Camera { pan: camera.pan - by, zoom: camera.zoom });
        let on_screen = |r: &WorldRect| self.canvas.min + camera.pan + Vec2::new(r.x + r.w / 2.0, r.y + r.h / 2.0) * camera.zoom;
        let anchor = self.selected_card().map(|c| on_screen(&c.rect)).filter(|p| self.canvas.contains(*p)).unwrap_or(self.canvas.center());
        let world = (anchor - self.canvas.min - camera.pan + by) / camera.zoom;
        // Still inside the selected card (a box bigger than the screen):
        // moved through it. Else the nearest card there (in the boxes, at
        // the selection's depth).
        let current = self.selected_card();
        let inside = current.filter(|c| c.rect.x <= world.x && world.x <= c.rect.right() && c.rect.y <= world.y && world.y <= c.rect.bottom()).map(|c| c.key.clone());
        let same_depth = |c: &&Card| tree.request.view != ViewKind::Contains || current.is_none_or(|s| s.level == c.level);
        let nearest = inside.or_else(|| tree.cards.iter().filter(same_depth).map(|c| ((Vec2::from(c.rect.center()) - world).length(), c.key.clone())).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, k)| k));
        if nearest == self.selection && to == camera {
            return;
        }
        self.aim(to, true);
        if let Some(key) = nearest {
            self.take(Goes::To(key));
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
            self.reveal_after_fit = Some((key, Show::Middle));
            return;
        }
        if self.nav.view == ViewKind::DataFlow {
            self.navigate(NavChange::View { view: ViewKind::Calls, root: None });
        }
        self.navigate(NavChange::GoIn(key.clone()));
        self.pending_select = Some(key);
    }
}

impl GraphState {
    /// The shared moves where the cursor is, in the graph's words: between
    /// cards, into the selected card and back out, folding cards.
    pub(crate) fn moves(&self) -> Vec<crate::actions::Action> {
        // The four ways by the layout, each saying where it goes.
        let mut out: Vec<crate::actions::Action> = crate::nav::list_moves().into_iter().filter(|a| a.id != Move::Motion(Motion::Down).id() && a.id != Move::Motion(Motion::Up).id()).collect();
        for (motion, dir, plain) in [(Motion::Left, Dir::Left, "Left"), (Motion::Down, Dir::Down, "Down"), (Motion::Up, Dir::Up, "Up"), (Motion::Right, Dir::Right, "Right")] {
            out.push(match self.goes(dir) {
                Ok(goes) => Move::Motion(motion).offer(self.goes_label(&goes)),
                Err(_) => Move::Motion(motion).offer_if(false, plain),
            });
        }
        // The canvas moves every way.
        let shown = self.tree.is_some();
        out.push(Move::Motion(Motion::HalfLeft).offer_if(shown, "Half a screen left"));
        out.push(Move::Motion(Motion::HalfRight).offer_if(shown, "Half a screen right"));
        out.push(Move::Motion(Motion::FewLeft).offer_if(shown, "A little left"));
        out.push(Move::Motion(Motion::FewRight).offer_if(shown, "A little right"));
        out.push(Move::Motion(Motion::NextSibling).offer("Next sibling (same module)"));
        out.push(Move::Motion(Motion::PrevSibling).offer("Previous sibling"));
        let boxes = self.nav.view == ViewKind::Contains;
        out.push(match (self.goes_in(), self.selected_card()) {
            (Ok(_), Some(card)) if boxes => Move::Enter.offer(format!("Into {}", card.label)),
            (Ok(goes), _) => Move::Enter.offer(self.goes_label(&goes)),
            (Err(_), _) => Move::Enter.offer_if(false, if boxes { "Into it" } else { "To its first" }),
        });
        let leave = match self.goes_out() {
            Some(parent) if boxes => Some(format!("Out to {}", parent.label)),
            Some(parent) => Some(format!("To {}", parent.label)),
            None => self.leave_label(),
        };
        out.push(Move::Leave.offer_if(leave.is_some(), leave.unwrap_or_else(|| "Back to the whole view".into())));
        let card = self.selected_card().filter(|c| c.open || c.more > 0);
        out.extend(crate::nav::fold_here(card.map(|c| (c.label.as_str(), !c.open))));
        out.push(Move::Fold(Fold::OpenAll).offer_if(self.tree.is_some(), "Show one more level"));
        out.push(Move::Fold(Fold::CloseAll).offer_if(!self.nav.opened.is_empty() || !self.nav.closed.is_empty(), "Put back what was opened and closed by hand"));
        out
    }

    /// What Backspace does now, in words ("Back to crates"), when it does
    /// anything.
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
}

impl Navigable for GraphState {
    /// What it asked, and its selection when that changed: the user is on it.
    /// What it asked, its selection when that changed (the user is on
    /// it), and its source when that did (every buffer reads from it).
    fn take_asks(&mut self) -> Vec<crate::nav::Ask> {
        if let Some(on) = self.current()
            && self.said.as_ref() != Some(&on)
        {
            self.said = Some(on.clone());
            self.asks.push(crate::nav::Ask::Follow(on));
        }
        if self.said_source.as_ref() != Some(&self.nav.source) {
            self.said_source = Some(self.nav.source.clone());
            self.asks.push(crate::nav::Ask::Source(self.nav.source.clone()));
        }
        std::mem::take(&mut self.asks)
    }

    /// Its settings, the review's badges, the files changed since read.
    fn share(&mut self, shared: &crate::shared::Shared) {
        if let Some(settings) = shared.settings.since(&mut self.seen.settings) {
            self.ui = settings.graph.clone();
        }
        if let Some(review) = shared.review_since(&mut self.seen.review) {
            self.marks = review.marks();
        }
        if let Some(index) = shared.index.since(&mut self.seen.index) {
            self.stale_files.clone_from(&index.stale);
        }
    }

    /// Moves to the component the user is on, unless it's selected here.
    fn follow(&mut self, id: &ComponentId) {
        if self.current().as_ref() != Some(id) {
            self.reveal(id.clone());
        }
    }

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

    /// Any component (a line too: it's in one), not the one selected.
    fn can_show(&self, target: &Target) -> bool {
        target.in_component() && target.elsewhere(self.target().as_ref())
    }

    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        let id = target.component().ok_or("Not a component.")?;
        self.reveal(id.clone());
        Ok(())
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        self.start_from_what_shows();
        let dir = match motion {
            Motion::Left => Some(Dir::Left),
            Motion::Down => Some(Dir::Down),
            Motion::Up => Some(Dir::Up),
            Motion::Right => Some(Dir::Right),
            _ => None,
        };
        if let Some(dir) = dir {
            for _ in 0..count {
                match self.goes(dir) {
                    Ok(Goes::To(key)) => self.take(Goes::To(key)),
                    // Opening ends the move: its cards come with the next tree.
                    Ok(opens) => return self.take(opens),
                    Err(why) => return self.note = Some(why),
                }
            }
            return;
        }
        match motion {
            Motion::HalfDown => self.half_page(Vec2::Y, count),
            Motion::HalfUp => self.half_page(-Vec2::Y, count),
            Motion::HalfLeft => self.half_page(-Vec2::X, count),
            Motion::HalfRight => self.half_page(Vec2::X, count),
            Motion::FewLeft => self.pan_by(Vec2::new(FEW * count as f32, 0.0)),
            Motion::FewRight => self.pan_by(Vec2::new(-FEW * count as f32, 0.0)),
            Motion::First => self.select_index(Some(0)),
            Motion::Last => self.select_index(None),
            Motion::Line(n) => self.select_index(Some(n.saturating_sub(1))),
            Motion::NextSibling => self.step_sibling(true, count),
            Motion::PrevSibling => self.step_sibling(false, count),
            Motion::Left | Motion::Down | Motion::Up | Motion::Right => {}
        }
    }

    /// In: into the selected box, or to the selected card's first card,
    /// opening it if it's closed.
    fn enter(&mut self) -> Step {
        self.start_from_what_shows();
        match self.goes_in() {
            Ok(goes) => {
                self.take(goes);
                Step::Done
            }
            Err(why) => Step::Nothing(why),
        }
    }

    /// Out: to the box holding it, or the card it hangs from; from where
    /// the view starts, back where the last "make it the root" started,
    /// else out to what holds the root.
    fn leave(&mut self) -> Step {
        self.start_from_what_shows();
        if let Some(parent) = self.goes_out().map(|c| c.key.clone()) {
            self.take(Goes::To(parent));
            return Step::Done;
        }
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

    /// A jump (a hint, a mark, a search): in the middle.
    fn move_to(&mut self, key: &str) {
        if self.card(key).is_some() {
            self.select(Some(key.to_owned()));
            self.show_card(key, Show::Middle);
        }
    }
}
