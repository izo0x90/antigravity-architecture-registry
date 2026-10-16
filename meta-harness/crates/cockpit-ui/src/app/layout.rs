//! Each column's own view: which view of a kind is in use, parking, new and closed views, slots.

use super::*;

impl CockpitApp {
    /// Puts `pane`'s view where the cockpit works on it, parking the one
    /// there. A new view starts (next frame) where the one before it was.
    pub(super) fn swap_in(&mut self, pane: Pane) {
        let current = self.desk.active.get(&pane.kind).copied().unwrap_or(0);
        if pane.kind.one_only() || current == pane.id {
            return;
        }
        let place = self.desk.view(pane.kind).place();
        let new = match pane.kind {
            BufferKind::Graph => swap_view(&mut self.desk.graph, &mut self.desk.parked.graphs, current, pane.id),
            BufferKind::Code => swap_view(&mut self.desk.code, &mut self.desk.parked.codes, current, pane.id),
            BufferKind::Spec => swap_view(&mut self.desk.spec, &mut self.desk.parked.specs, current, pane.id),
            BufferKind::Tree => swap_view(&mut self.desk.tree, &mut self.desk.parked.trees, current, pane.id),
            BufferKind::Chat => swap_view(&mut self.desk.chat.ui, &mut self.desk.parked.chats, current, pane.id),
            BufferKind::Queue | BufferKind::Inbox | BufferKind::Changes => false,
        };
        self.desk.active.insert(pane.kind, pane.id);
        if new {
            self.set_up(pane.kind);
            if pane.id != crate::quick_look::PANE {
                self.desk.fresh.push((pane, place));
            }
        }
    }

    /// The cockpit uses `pane` from now on: keys, links and go-tos act on it.
    pub(super) fn use_pane(&mut self, pane: Pane) {
        self.swap_in(pane);
        if !pane.kind.one_only() {
            self.desk.last_used.insert(pane.kind, pane.id);
        }
    }

    /// The focused column's view is the one in use (the quick look's card
    /// is, while it's open).
    pub(super) fn use_focused(&mut self) {
        if self.quick_look.is_none() {
            self.use_pane(self.desk.columns.focused_pane());
        }
    }

    /// The views used last back in use (after drawing the others).
    pub(super) fn back_to_used(&mut self) {
        for kind in BufferKind::ALL.into_iter().filter(|k| !k.one_only()) {
            self.swap_in(self.desk.used_pane(kind));
        }
    }

    /// A new view gets what every view of its kind has.
    pub(super) fn set_up(&mut self, kind: BufferKind) {
        match kind {
            BufferKind::Graph => self.desk.graph.ui = self.config.graph.clone(),
            BufferKind::Code => self.desk.code.ui = self.config.code.clone(),
            BufferKind::Spec => {
                self.desk.spec.types.clone_from(&self.desk.types);
                self.desk.spec.author.clone_from(&self.config.user.name);
                self.desk.spec.comments.clone_from(&self.desk.review.comments);
                self.desk.spec.diff.clone_from(&self.desk.review.diff);
            }
            BufferKind::Tree | BufferKind::Chat | BufferKind::Queue | BufferKind::Inbox | BufferKind::Changes => {}
        }
    }

    /// New views start where the view of their kind used before them was.
    pub(super) fn start_fresh(&mut self, ctx: &egui::Context) {
        for (pane, place) in std::mem::take(&mut self.desk.fresh) {
            self.swap_in(pane);
            self.restore_place(pane.kind, &place, ctx);
        }
        self.back_to_used();
    }

    /// The graph the agent moves: column `column` (made a graph if it isn't
    /// one), else the one used last.
    pub(super) fn agents_graph(&mut self, column: Option<u32>) {
        let Some(slot) = column.and_then(|c| (c as usize).checked_sub(1)) else { return };
        if slot >= self.desk.columns.len() {
            self.note = Some(format!("The agent asked for column {}; there are {}: it moved the graph used last.", slot + 1, self.desk.columns.len()));
            return;
        }
        let pane = self.desk.columns.show(slot, BufferKind::Graph);
        self.use_pane(pane);
    }

    /// Closes column `slot`: its view is hidden, not lost.
    pub(super) fn close_column(&mut self, slot: usize) {
        if !self.desk.columns.close(slot) {
            self.note = Some("The last column stays.".into());
        }
        self.use_focused();
    }

    /// Deletes the focused column's view for good: the column shows the
    /// view hidden last, else a new one of its kind. The queue (there's
    /// one) isn't deleted.
    pub(super) fn delete_view(&mut self) {
        let slot = self.desk.columns.focused();
        let pane = self.desk.columns.pane(slot);
        if pane.kind.one_only() {
            self.note = Some(format!("There's one {}: close its column to hide it.", pane.kind.name()));
            return;
        }
        let (gone, now) = self.desk.columns.delete(slot);
        if now == gone {
            // A new one took its number: the view itself starts over.
            self.swap_in(gone);
            self.reset(gone.kind);
        }
        self.use_pane(now);
        self.desk.fresh.retain(|(p, _)| *p != now);
        // The view of its kind used last, if that was the one deleted.
        if self.desk.used_pane(gone.kind) == gone && now != gone {
            let next = self.desk.columns.other_of(gone.kind, gone).unwrap_or_else(|| Pane::new(gone.kind, 0));
            self.desk.last_used.insert(gone.kind, next.id);
        }
        self.drop_unheld();
    }

    /// Views no column shows or hides any more are dropped.
    pub(super) fn drop_unheld(&mut self) {
        let (columns, quick) = (&self.desk.columns, crate::quick_look::PANE);
        self.desk.parked.keep_only(|kind, id| id == quick || columns.has(Pane::new(kind, id)));
        self.desk.fresh.retain(|(p, _)| p.id == quick || self.desk.columns.has(*p));
    }

    /// A new view of `kind` in the focused column (the one there is hidden).
    pub(super) fn new_view(&mut self, kind: BufferKind) {
        let pane = self.desk.columns.new_pane(kind);
        let slot = self.desk.columns.place(self.desk.columns.focused(), pane);
        self.desk.columns.focus(slot);
        self.use_pane(pane);
        // New, not a copy of the last one.
        self.desk.fresh.retain(|(p, _)| *p != pane);
    }

    /// Shows `pane` (shown elsewhere, or hidden) in the focused column; the
    /// view there is hidden (or swaps places with it).
    pub(super) fn show_view(&mut self, pane: Pane) {
        let slot = self.desk.columns.place(self.desk.columns.focused(), pane);
        self.desk.columns.focus(slot);
        self.use_pane(pane);
    }

    /// What a view shows, in words (the buffer list's lines).
    pub(super) fn describe(&self, pane: Pane) -> String {
        let name = |id: &ComponentId| self.component_name(id);
        let what = match self.place_of(pane) {
            Some(Place::Graph { nav, selection }) => {
                let mut out = format!("{} view", nav.view.name().replace('_', " "));
                if nav.source != arch_graph::Source::Code {
                    out.push_str(&format!(", {}", nav.source));
                }
                if let Some(root) = &nav.root {
                    out.push_str(&format!(", from {}", name(&ComponentId::new(root.clone()))));
                }
                if let Some(at) = &selection {
                    out.push_str(&format!(", at {}", name(&ComponentId::new(at.clone()))));
                }
                out
            }
            Some(Place::Code { file, line }) => format!("{file}:{line}"),
            Some(Place::Spec { component, .. }) => {
                let card = component.as_ref().map_or_else(|| "following the graph".to_owned(), &name);
                if self.desk.pinned(pane) { format!("{card} (pinned)") } else { card }
            }
            Some(Place::Tree { cursor }) => cursor.as_ref().map_or_else(|| "the outline".to_owned(), |id| format!("at {}", name(id))),
            Some(Place::Chat { slot, .. }) => {
                let slot = slot.as_ref().and_then(|s| self.desk.chat.store.slots.get(s));
                slot.map_or_else(|| "the agent's session".to_owned(), |s| format!("agent slot {}", s.name))
            }
            Some(Place::Queue { .. }) => "the plan's jobs".to_owned(),
            Some(Place::Inbox { .. }) => "what needs you".to_owned(),
            Some(Place::Changes { .. }) => self.desk.changes.comparison.to_string(),
            None => "new".to_owned(),
        };
        format!("{} · {what}", pane.kind.name())
    }

    /// The view of `kind` in use, new again.
    pub(super) fn reset(&mut self, kind: BufferKind) {
        match kind {
            BufferKind::Graph => self.desk.graph = GraphState::default(),
            BufferKind::Code => self.desk.code = CodeViewerState::default(),
            BufferKind::Spec => self.desk.spec = SpecState::default(),
            BufferKind::Tree => self.desk.tree = ArchitectureTreeState::default(),
            BufferKind::Chat | BufferKind::Queue | BufferKind::Inbox | BufferKind::Changes => return,
        }
        self.set_up(kind);
    }

    /// Where `pane`'s view is, wherever it's kept.
    pub(super) fn place_of(&self, pane: Pane) -> Option<Place> {
        if pane.kind.one_only() || self.desk.active.get(&pane.kind).copied().unwrap_or(0) == pane.id {
            return Some(self.desk.view(pane.kind).place());
        }
        match pane.kind {
            BufferKind::Graph => self.desk.parked.graphs.get(&pane.id).map(|v| v.place()),
            BufferKind::Code => self.desk.parked.codes.get(&pane.id).map(|v| v.place()),
            BufferKind::Spec => self.desk.parked.specs.get(&pane.id).map(|v| v.place()),
            BufferKind::Tree => self.desk.parked.trees.get(&pane.id).map(|v| v.place()),
            BufferKind::Chat => self.desk.parked.chats.get(&pane.id).map(|v| v.place()),
            BufferKind::Queue | BufferKind::Inbox | BufferKind::Changes => None,
        }
    }

    /// The view the keys go to: the quick look's, while it's open (its
    /// card's spec view, or the inbox); else the focused column's.
    pub(super) fn focused_kind(&self) -> BufferKind {
        self.quick_look.as_ref().map_or_else(|| self.desk.columns.focused_kind(), |q| q.kind)
    }

    /// The column showing the view of `kind` in use (else the nearest of
    /// its kind, else the focused one).
    pub(super) fn slot_of(&self, kind: BufferKind) -> usize {
        self.desk.columns.slot_of(self.desk.used_pane(kind)).or_else(|| self.desk.columns.slot_of_kind(kind)).unwrap_or(self.desk.columns.focused())
    }

    /// Shows a view of `kind` without moving focus, and uses it: in the
    /// column a go-to's count names; else the one used last, where it is;
    /// else the nearest column of that kind; else beside the focused column.
    pub(super) fn ensure_visible(&mut self, kind: BufferKind) -> usize {
        let slot = match self.landing.take() {
            Some(slot) => {
                self.desk.columns.show(slot, kind);
                slot
            }
            None => match self.desk.columns.slot_of(self.desk.used_pane(kind)).or_else(|| self.desk.columns.slot_of_kind(kind)) {
                Some(slot) => slot,
                None => self.desk.columns.ensure_visible(self.desk.used_pane(kind)),
            },
        };
        self.use_pane(self.desk.columns.pane(slot));
        slot
    }

    /// Shows a view of `kind` (as `ensure_visible`) and moves focus to it.
    pub(super) fn focus_kind(&mut self, kind: BufferKind) {
        let slot = self.ensure_visible(kind);
        self.desk.columns.focus(slot);
    }
}
