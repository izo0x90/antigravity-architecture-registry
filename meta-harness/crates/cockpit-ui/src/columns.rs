//! The columns on screen and the views open behind them, as vim's windows
//! and buffers: a column shows one view; closing a column, or fewer columns,
//! only hides its view (a wider layout, or picking it, brings it back where
//! it was); a view is gone only when it's deleted.
//!
//! A view is a pane: its kind and which one of that kind it is. Every
//! column can hold its own graph, code, spec or outline; there is one chat
//! and one queue, so showing either in another column swaps the two.

use serde::{Deserialize, Serialize};

use crate::app::BufferKind;

/// One view: its kind, and which one of that kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Pane {
    pub kind: BufferKind,
    pub id: u32,
}

impl Pane {
    pub fn new(kind: BufferKind, id: u32) -> Self {
        Self { kind, id }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Columns {
    shown: Vec<Pane>,
    focused: usize,
    /// Views taken off screen, and the column each was in, the last one
    /// taken last.
    #[serde(default)]
    put_away: Vec<(usize, Pane)>,
    /// The next new view's number.
    #[serde(default)]
    next_id: u32,
}

impl Default for Columns {
    fn default() -> Self {
        Self::new(vec![Pane::new(BufferKind::Chat, 0), Pane::new(BufferKind::Graph, 0)], 0).with_put_away(vec![(2, Pane::new(BufferKind::Code, 0))])
    }
}

impl Columns {
    /// `shown` left to right (a pane twice, or a second chat or queue,
    /// dropped; at least one column), focus on `focused` (or the last).
    pub fn new(shown: Vec<Pane>, focused: usize) -> Self {
        let mut columns = Self { shown: Vec::new(), focused: 0, put_away: Vec::new(), next_id: 0 };
        for pane in shown {
            if !columns.taken(pane) {
                columns.shown.push(pane);
            }
        }
        if columns.shown.is_empty() {
            columns.shown.push(Pane::new(BufferKind::ALL[0], 0));
        }
        columns.focused = focused.min(columns.shown.len() - 1);
        columns.next_id = columns.shown.iter().map(|p| p.id + 1).max().unwrap_or(0);
        columns
    }

    fn with_put_away(mut self, put_away: Vec<(usize, Pane)>) -> Self {
        for (slot, pane) in put_away {
            if !self.taken(pane) {
                self.next_id = self.next_id.max(pane.id + 1);
                self.put_away.push((slot, pane));
            }
        }
        self
    }

    /// Every view here, shown or put away.
    fn all(&self) -> impl Iterator<Item = &Pane> {
        self.shown.iter().chain(self.put_away.iter().map(|(_, p)| p))
    }

    /// Whether `pane` can't be added: it's here already, or it's a second
    /// queue.
    fn taken(&self, pane: Pane) -> bool {
        self.all().any(|p| *p == pane || (pane.kind.one_only() && p.kind == pane.kind))
    }

    /// The same columns again, read back from what was saved (so its rules
    /// hold whatever was written).
    pub fn checked(self) -> Self {
        let next = self.next_id;
        let mut columns = Self::new(self.shown, self.focused).with_put_away(self.put_away);
        columns.next_id = columns.next_id.max(next);
        columns
    }

    pub fn len(&self) -> usize {
        self.shown.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    /// The kinds of view shown, left to right.
    pub fn kinds(&self) -> Vec<BufferKind> {
        self.shown.iter().map(|p| p.kind).collect()
    }

    /// The views shown, left to right.
    pub fn panes(&self) -> &[Pane] {
        &self.shown
    }

    /// The views taken off screen (they keep where they were).
    pub fn put_away(&self) -> impl Iterator<Item = Pane> + '_ {
        self.put_away.iter().map(|(_, p)| *p)
    }

    pub fn pane(&self, slot: usize) -> Pane {
        self.shown[slot.min(self.shown.len() - 1)]
    }

    pub fn kind(&self, slot: usize) -> BufferKind {
        self.pane(slot).kind
    }

    pub fn focused(&self) -> usize {
        self.focused
    }

    pub fn focused_pane(&self) -> Pane {
        self.shown[self.focused]
    }

    pub fn focused_kind(&self) -> BufferKind {
        self.focused_pane().kind
    }

    /// Focus on column `slot`, if there is one.
    pub fn focus(&mut self, slot: usize) -> bool {
        let there = slot < self.shown.len();
        if there {
            self.focused = slot;
        }
        there
    }

    /// Focus on the next (or previous) column, round the end.
    pub fn cycle(&mut self, step: i32) {
        self.focused = (self.focused as i32 + step).rem_euclid(self.shown.len() as i32) as usize;
    }

    /// The column showing `pane`.
    pub fn slot_of(&self, pane: Pane) -> Option<usize> {
        self.shown.iter().position(|p| *p == pane)
    }

    /// The column nearest the focused one showing a view of `kind` (the
    /// left one of two as near).
    pub fn slot_of_kind(&self, kind: BufferKind) -> Option<usize> {
        let focused = self.focused as i64;
        self.shown.iter().enumerate().filter(|(_, p)| p.kind == kind).min_by_key(|(slot, _)| ((*slot as i64 - focused).abs(), *slot)).map(|(slot, _)| slot)
    }

    /// A view of `kind` that isn't anywhere yet: the first of its kind is
    /// view 0 (the one the cockpit starts with), any more are new.
    pub fn new_pane(&mut self, kind: BufferKind) -> Pane {
        if !self.all().any(|p| p.kind == kind) {
            return Pane::new(kind, 0);
        }
        let pane = Pane::new(kind, self.next_id.max(1));
        self.next_id = pane.id + 1;
        pane
    }

    /// Shows `pane` in column `slot`: one shown elsewhere swaps places with
    /// the one there; otherwise the one there is put away. Returns the slot.
    pub fn place(&mut self, slot: usize, pane: Pane) -> usize {
        let slot = slot.min(self.shown.len() - 1);
        if let Some(other) = self.slot_of(pane) {
            self.shown.swap(slot, other);
            return slot;
        }
        self.put_away.retain(|(_, p)| *p != pane);
        let replaced = std::mem::replace(&mut self.shown[slot], pane);
        if replaced != pane {
            self.put_away.push((slot, replaced));
        }
        slot
    }

    /// Shows a view of `kind` in column `slot`: the one there if it's of
    /// that kind; the queue from where it is; else the last one
    /// of that kind put away, or a new one. Returns the view.
    pub fn show(&mut self, slot: usize, kind: BufferKind) -> Pane {
        let slot = slot.min(self.shown.len() - 1);
        if self.shown[slot].kind == kind {
            return self.shown[slot];
        }
        let pane = match self.slot_of_kind(kind).filter(|_| kind.one_only()) {
            Some(other) => self.shown[other],
            None => self.put_away.iter().rev().find(|(_, p)| p.kind == kind).map(|(_, p)| *p).unwrap_or_else(|| self.new_pane(kind)),
        };
        self.place(slot, pane);
        pane
    }

    /// Shows `pane` somewhere without moving focus: where it is; else a
    /// view of its kind takes the column nearest the focused one that shows
    /// that kind, or the column beside the focused one (the only one, with
    /// one). Returns the column.
    pub fn ensure_visible(&mut self, pane: Pane) -> usize {
        if let Some(slot) = self.slot_of(pane) {
            return slot;
        }
        let slot = self.slot_of_kind(pane.kind).unwrap_or(if self.shown.len() == 1 { 0 } else { (self.focused + 1) % self.shown.len() });
        self.place(slot, pane)
    }

    /// `count` columns, at most `max`. Fewer put views away from the right,
    /// never the focused one; more bring back what was put away last, where
    /// it was, then a kind of view not shown yet, then another code view.
    /// Focus stays on the same view.
    pub fn set_count(&mut self, count: usize, max: usize) {
        let count = count.clamp(1, max.max(1));
        while self.shown.len() > count {
            let last = self.shown.len() - 1;
            self.take_out(if self.focused == last { last - 1 } else { last });
        }
        while self.shown.len() < count {
            let (slot, pane) = match self.put_away.pop() {
                Some(back) => back,
                None => {
                    let kind = BufferKind::ALL.into_iter().find(|k| self.slot_of_kind(*k).is_none()).unwrap_or(BufferKind::Code);
                    (self.shown.len(), self.new_pane(kind))
                }
            };
            let slot = slot.min(self.shown.len());
            self.shown.insert(slot, pane);
            if slot <= self.focused && self.shown.len() > 1 {
                self.focused += 1;
            }
        }
    }

    /// Closes column `slot`: its view is hidden, not lost. The last column
    /// stays.
    pub fn close(&mut self, slot: usize) -> bool {
        if self.shown.len() == 1 || slot >= self.shown.len() {
            return false;
        }
        self.take_out(slot);
        true
    }

    /// Deletes the view in column `slot` for good; the column shows the view
    /// hidden last instead, else a new one of the same kind. Returns the
    /// view deleted and the one shown now (the same when a new one of its
    /// kind took its number).
    pub fn delete(&mut self, slot: usize) -> (Pane, Pane) {
        let slot = slot.min(self.shown.len() - 1);
        let gone = self.shown[slot];
        let next = match self.put_away.pop() {
            Some((_, pane)) => pane,
            None => {
                self.shown[slot] = Pane::new(gone.kind, u32::MAX);
                let pane = self.new_pane(gone.kind);
                self.shown[slot] = gone;
                pane
            }
        };
        self.shown[slot] = next;
        (gone, next)
    }

    /// How many views are hidden.
    pub fn hidden(&self) -> usize {
        self.put_away.len()
    }

    /// Whether `pane` is anywhere: shown or put away.
    pub fn has(&self, pane: Pane) -> bool {
        self.all().any(|p| *p == pane)
    }

    /// A view of `kind` here (shown first, then put away), other than `not`.
    pub fn other_of(&self, kind: BufferKind, not: Pane) -> Option<Pane> {
        self.all().find(|p| p.kind == kind && **p != not).copied()
    }

    /// Puts column `slot`'s view away; focus stays on the view it was on
    /// (the one to the left, if it was this one).
    fn take_out(&mut self, slot: usize) {
        self.put_away.push((slot, self.shown.remove(slot)));
        if self.focused > slot || self.focused >= self.shown.len() {
            self.focused = self.focused.saturating_sub(1);
        }
    }

    /// Forgets a view put away (the quick look's card, once it's a column's).
    pub fn forget(&mut self, pane: Pane) {
        self.put_away.retain(|(_, p)| *p != pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use BufferKind as K;

    #[test]
    fn every_column_can_hold_its_own_view_but_there_is_one_queue() {
        let mut c = Columns::new(vec![Pane::new(K::Queue, 0), Pane::new(K::Graph, 0), Pane::new(K::Code, 0)], 0);
        // Another graph in the third column: a new one, the code put away.
        let second = c.show(2, K::Graph);
        assert_eq!((c.kinds(), second == c.pane(1)), (vec![K::Queue, K::Graph, K::Graph], false));
        // The code comes back where it's asked for, the same one.
        assert_eq!(c.show(1, K::Code), Pane::new(K::Code, 0));
        // The queue moves rather than being doubled; a chat is another one.
        c.show(2, K::Queue);
        assert_eq!(c.kinds(), vec![K::Graph, K::Code, K::Queue]);
        assert_eq!(Columns::new(vec![Pane::new(K::Queue, 0), Pane::new(K::Queue, 1), Pane::new(K::Tree, 0)], 5).panes(), [Pane::new(K::Queue, 0), Pane::new(K::Tree, 0)]);
        assert_eq!(Columns::new(vec![Pane::new(K::Chat, 0), Pane::new(K::Chat, 1)], 0).panes().len(), 2, "two chats");
    }

    #[test]
    fn fewer_columns_keep_the_focused_view_and_more_bring_back_what_went() {
        let panes = vec![Pane::new(K::Chat, 0), Pane::new(K::Graph, 0), Pane::new(K::Code, 0), Pane::new(K::Spec, 0)];
        let mut c = Columns::new(panes.clone(), 3);
        c.set_count(1, 12);
        assert_eq!((c.kinds(), c.focused_kind()), (vec![K::Spec], K::Spec));
        c.set_count(4, 12);
        assert_eq!((c.panes(), c.focused()), (panes.as_slice(), 3), "as they were, focus on the same view");
        c.set_count(9, 8);
        assert_eq!(c.len(), 8, "up to the most there may be");
        assert!(K::ALL.iter().all(|k| c.slot_of_kind(*k).is_some()), "every kind first, then more code views");
    }

    #[test]
    fn a_view_goes_to_the_nearest_column_of_its_kind_or_beside_the_focused_one() {
        let mut c = Columns::new(vec![Pane::new(K::Graph, 0), Pane::new(K::Chat, 0), Pane::new(K::Code, 0), Pane::new(K::Graph, 1)], 2);
        assert_eq!(c.slot_of_kind(K::Graph), Some(3), "the nearer graph");
        let third = c.new_pane(K::Graph);
        assert_eq!(c.ensure_visible(third), 3, "a graph not shown takes the nearest graph's column");
        let tree = c.new_pane(K::Tree);
        assert_eq!(c.ensure_visible(tree), 3, "a kind not shown goes beside the focused column");
        let tree = c.pane(3);
        assert!(c.close(3));
        assert!(c.has(tree) && c.hidden() == 3, "a closed column's view is hidden, not lost");
        c.set_count(4, 12);
        assert_eq!(c.pane(3), tree, "and comes back");
        // Deleting: gone, and the view hidden last shows instead.
        let (gone, now) = c.delete(3);
        assert_eq!((gone, c.has(tree), now.kind), (tree, false, K::Graph));
        // Nothing hidden: a new one of its kind.
        c.set_count(1, 12);
        while c.hidden() > 0 {
            c.put_away.pop();
        }
        let (gone, now) = c.delete(0);
        assert_eq!((gone.kind, now.kind), (now.kind, gone.kind));
        assert!(!c.close(0), "the last column stays");
    }
}
