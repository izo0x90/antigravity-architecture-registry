//! The panels on screen and the buffers open behind them, as vim's windows
//! and buffers: a panel shows one buffer; closing a panel, or fewer panels,
//! only hides its buffer (a wider layout, or picking it, brings it back
//! where it was); a buffer is gone only when it's deleted.
//!
//! A buffer is its kind and which one of that kind it is. Every panel can
//! hold its own graph, code, spec or outline; there is one chat and one
//! queue, so showing either in another panel swaps the two.

use serde::{Deserialize, Serialize};

use crate::app::BufferKind;

/// One view: its kind, and which one of that kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Buffer {
    pub kind: BufferKind,
    pub id: u32,
}

impl Buffer {
    pub fn new(kind: BufferKind, id: u32) -> Self {
        Self { kind, id }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Panels {
    shown: Vec<Buffer>,
    focused: usize,
    /// Views taken off screen, and the panel each was in, the last one
    /// taken last.
    #[serde(default)]
    put_away: Vec<(usize, Buffer)>,
    /// The next new view's number.
    #[serde(default)]
    next_id: u32,
}

impl Default for Panels {
    fn default() -> Self {
        Self::new(vec![Buffer::new(BufferKind::Chat, 0), Buffer::new(BufferKind::Graph, 0)], 0).with_put_away(vec![(2, Buffer::new(BufferKind::Code, 0))])
    }
}

impl Panels {
    /// `shown` left to right (a buffer twice, or a second chat or queue,
    /// dropped; at least one panel), focus on `focused` (or the last).
    pub fn new(shown: Vec<Buffer>, focused: usize) -> Self {
        let mut panels = Self { shown: Vec::new(), focused: 0, put_away: Vec::new(), next_id: 0 };
        for buffer in shown {
            if !panels.taken(buffer) {
                panels.shown.push(buffer);
            }
        }
        if panels.shown.is_empty() {
            panels.shown.push(Buffer::new(BufferKind::ALL[0], 0));
        }
        panels.focused = focused.min(panels.shown.len() - 1);
        panels.next_id = panels.shown.iter().map(|p| p.id + 1).max().unwrap_or(0);
        panels
    }

    fn with_put_away(mut self, put_away: Vec<(usize, Buffer)>) -> Self {
        for (panel, buffer) in put_away {
            if !self.taken(buffer) {
                self.next_id = self.next_id.max(buffer.id + 1);
                self.put_away.push((panel, buffer));
            }
        }
        self
    }

    /// Every view here, shown or put away.
    fn all(&self) -> impl Iterator<Item = &Buffer> {
        self.shown.iter().chain(self.put_away.iter().map(|(_, p)| p))
    }

    /// Whether `buffer` can't be added: it's here already, or it's a second
    /// queue.
    fn taken(&self, buffer: Buffer) -> bool {
        self.all().any(|p| *p == buffer || (buffer.kind.one_only() && p.kind == buffer.kind))
    }

    /// The same panels again, read back from what was saved (so its rules
    /// hold whatever was written).
    pub fn checked(self) -> Self {
        let next = self.next_id;
        let mut panels = Self::new(self.shown, self.focused).with_put_away(self.put_away);
        panels.next_id = panels.next_id.max(next);
        panels
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
    pub fn buffers(&self) -> &[Buffer] {
        &self.shown
    }

    /// The views taken off screen (they keep where they were).
    pub fn put_away(&self) -> impl Iterator<Item = Buffer> + '_ {
        self.put_away.iter().map(|(_, p)| *p)
    }

    pub fn buffer(&self, panel: usize) -> Buffer {
        self.shown[panel.min(self.shown.len() - 1)]
    }

    pub fn kind(&self, panel: usize) -> BufferKind {
        self.buffer(panel).kind
    }

    pub fn focused(&self) -> usize {
        self.focused
    }

    pub fn focused_buffer(&self) -> Buffer {
        self.shown[self.focused]
    }

    pub fn focused_kind(&self) -> BufferKind {
        self.focused_buffer().kind
    }

    /// Focus on panel `panel`, if there is one.
    pub fn focus(&mut self, panel: usize) -> bool {
        let there = panel < self.shown.len();
        if there {
            self.focused = panel;
        }
        there
    }

    /// Focus on the next (or previous) panel, round the end.
    pub fn cycle(&mut self, step: i32) {
        self.focused = (self.focused as i32 + step).rem_euclid(self.shown.len() as i32) as usize;
    }

    /// The panel showing `buffer`.
    pub fn panel_of(&self, buffer: Buffer) -> Option<usize> {
        self.shown.iter().position(|p| *p == buffer)
    }

    /// The panel nearest the focused one showing a view of `kind` (the
    /// left one of two as near).
    pub fn panel_of_kind(&self, kind: BufferKind) -> Option<usize> {
        let focused = self.focused as i64;
        self.shown.iter().enumerate().filter(|(_, p)| p.kind == kind).min_by_key(|(panel, _)| ((*panel as i64 - focused).abs(), *panel)).map(|(panel, _)| panel)
    }

    /// A view of `kind` that isn't anywhere yet: the first of its kind is
    /// view 0 (the one the cockpit starts with), any more are new.
    pub fn new_buffer(&mut self, kind: BufferKind) -> Buffer {
        if !self.all().any(|p| p.kind == kind) {
            return Buffer::new(kind, 0);
        }
        let buffer = Buffer::new(kind, self.next_id.max(1));
        self.next_id = buffer.id + 1;
        buffer
    }

    /// Shows `buffer` in panel `panel`: one shown elsewhere swaps places with
    /// the one there; otherwise the one there is put away. Returns the panel.
    pub fn place(&mut self, panel: usize, buffer: Buffer) -> usize {
        let panel = panel.min(self.shown.len() - 1);
        if let Some(other) = self.panel_of(buffer) {
            self.shown.swap(panel, other);
            return panel;
        }
        self.put_away.retain(|(_, p)| *p != buffer);
        let replaced = std::mem::replace(&mut self.shown[panel], buffer);
        if replaced != buffer {
            self.put_away.push((panel, replaced));
        }
        panel
    }

    /// Shows a view of `kind` in panel `panel`: the one there if it's of
    /// that kind; the queue from where it is; else the last one
    /// of that kind put away, or a new one. Returns the view.
    pub fn show(&mut self, panel: usize, kind: BufferKind) -> Buffer {
        let panel = panel.min(self.shown.len() - 1);
        if self.shown[panel].kind == kind {
            return self.shown[panel];
        }
        let buffer = match self.panel_of_kind(kind).filter(|_| kind.one_only()) {
            Some(other) => self.shown[other],
            None => self.put_away.iter().rev().find(|(_, p)| p.kind == kind).map(|(_, p)| *p).unwrap_or_else(|| self.new_buffer(kind)),
        };
        self.place(panel, buffer);
        buffer
    }

    /// Shows `buffer` somewhere without moving focus: where it is; else a
    /// view of its kind takes the panel nearest the focused one that shows
    /// that kind, or the panel beside the focused one (the only one, with
    /// one). Returns the panel.
    pub fn ensure_visible(&mut self, buffer: Buffer) -> usize {
        if let Some(panel) = self.panel_of(buffer) {
            return panel;
        }
        let panel = self.panel_of_kind(buffer.kind).unwrap_or(if self.shown.len() == 1 { 0 } else { (self.focused + 1) % self.shown.len() });
        self.place(panel, buffer)
    }

    /// `count` panels, at most `max`. Fewer put views away from the right,
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
            let (panel, buffer) = match self.put_away.pop() {
                Some(back) => back,
                None => {
                    let kind = BufferKind::ALL.into_iter().find(|k| self.panel_of_kind(*k).is_none()).unwrap_or(BufferKind::Code);
                    (self.shown.len(), self.new_buffer(kind))
                }
            };
            let panel = panel.min(self.shown.len());
            self.shown.insert(panel, buffer);
            if panel <= self.focused && self.shown.len() > 1 {
                self.focused += 1;
            }
        }
    }

    /// Closes panel `panel`: its view is hidden, not lost. The last panel
    /// stays.
    pub fn close(&mut self, panel: usize) -> bool {
        if self.shown.len() == 1 || panel >= self.shown.len() {
            return false;
        }
        self.take_out(panel);
        true
    }

    /// Deletes the view in panel `panel` for good; the panel shows the view
    /// hidden last instead, else a new one of the same kind. Returns the
    /// view deleted and the one shown now (the same when a new one of its
    /// kind took its number).
    pub fn delete(&mut self, panel: usize) -> (Buffer, Buffer) {
        let panel = panel.min(self.shown.len() - 1);
        let gone = self.shown[panel];
        let next = match self.put_away.pop() {
            Some((_, buffer)) => buffer,
            None => {
                self.shown[panel] = Buffer::new(gone.kind, u32::MAX);
                let buffer = self.new_buffer(gone.kind);
                self.shown[panel] = gone;
                buffer
            }
        };
        self.shown[panel] = next;
        (gone, next)
    }

    /// How many views are hidden.
    pub fn hidden(&self) -> usize {
        self.put_away.len()
    }

    /// Whether `buffer` is anywhere: shown or put away.
    pub fn has(&self, buffer: Buffer) -> bool {
        self.all().any(|p| *p == buffer)
    }

    /// A view of `kind` here (shown first, then put away), other than `not`.
    pub fn other_of(&self, kind: BufferKind, not: Buffer) -> Option<Buffer> {
        self.all().find(|p| p.kind == kind && **p != not).copied()
    }

    /// Puts panel `panel`'s view away; focus stays on the view it was on
    /// (the one to the left, if it was this one).
    fn take_out(&mut self, panel: usize) {
        self.put_away.push((panel, self.shown.remove(panel)));
        if self.focused > panel || self.focused >= self.shown.len() {
            self.focused = self.focused.saturating_sub(1);
        }
    }

    /// Forgets a view put away (the quick look's card, once it's a panel's).
    pub fn forget(&mut self, buffer: Buffer) {
        self.put_away.retain(|(_, p)| *p != buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use BufferKind as K;

    #[test]
    fn every_column_can_hold_its_own_view_but_there_is_one_queue() {
        let mut c = Panels::new(vec![Buffer::new(K::Queue, 0), Buffer::new(K::Graph, 0), Buffer::new(K::Code, 0)], 0);
        // Another graph in the third panel: a new one, the code put away.
        let second = c.show(2, K::Graph);
        assert_eq!((c.kinds(), second == c.buffer(1)), (vec![K::Queue, K::Graph, K::Graph], false));
        // The code comes back where it's asked for, the same one.
        assert_eq!(c.show(1, K::Code), Buffer::new(K::Code, 0));
        // The queue moves rather than being doubled; a chat is another one.
        c.show(2, K::Queue);
        assert_eq!(c.kinds(), vec![K::Graph, K::Code, K::Queue]);
        assert_eq!(Panels::new(vec![Buffer::new(K::Queue, 0), Buffer::new(K::Queue, 1), Buffer::new(K::Tree, 0)], 5).buffers(), [Buffer::new(K::Queue, 0), Buffer::new(K::Tree, 0)]);
        assert_eq!(Panels::new(vec![Buffer::new(K::Chat, 0), Buffer::new(K::Chat, 1)], 0).buffers().len(), 2, "two chats");
    }

    #[test]
    fn fewer_columns_keep_the_focused_view_and_more_bring_back_what_went() {
        let buffers = vec![Buffer::new(K::Chat, 0), Buffer::new(K::Graph, 0), Buffer::new(K::Code, 0), Buffer::new(K::Spec, 0)];
        let mut c = Panels::new(buffers.clone(), 3);
        c.set_count(1, 12);
        assert_eq!((c.kinds(), c.focused_kind()), (vec![K::Spec], K::Spec));
        c.set_count(4, 12);
        assert_eq!((c.buffers(), c.focused()), (buffers.as_slice(), 3), "as they were, focus on the same view");
        c.set_count(9, 8);
        assert_eq!(c.len(), 8, "up to the most there may be");
        assert!(K::ALL.iter().all(|k| c.panel_of_kind(*k).is_some()), "every kind first, then more code views");
    }

    #[test]
    fn a_view_goes_to_the_nearest_column_of_its_kind_or_beside_the_focused_one() {
        let mut c = Panels::new(vec![Buffer::new(K::Graph, 0), Buffer::new(K::Chat, 0), Buffer::new(K::Code, 0), Buffer::new(K::Graph, 1)], 2);
        assert_eq!(c.panel_of_kind(K::Graph), Some(3), "the nearer graph");
        let third = c.new_buffer(K::Graph);
        assert_eq!(c.ensure_visible(third), 3, "a graph not shown takes the nearest graph's panel");
        let tree = c.new_buffer(K::Tree);
        assert_eq!(c.ensure_visible(tree), 3, "a kind not shown goes beside the focused panel");
        let tree = c.buffer(3);
        assert!(c.close(3));
        assert!(c.has(tree) && c.hidden() == 3, "a closed panel's view is hidden, not lost");
        c.set_count(4, 12);
        assert_eq!(c.buffer(3), tree, "and comes back");
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
        assert!(!c.close(0), "the last panel stays");
    }
}
