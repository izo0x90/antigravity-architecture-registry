//! Each column's own view: which view of a kind is in use, parking, new and closed views, slots.

use super::*;

impl CockpitApp {
    /// The view of `kind` in use: the one used last.
    pub(super) fn used_pane(&self, kind: BufferKind) -> Pane {
        Pane::new(kind, self.last_used.get(&kind).copied().unwrap_or(0))
    }

    /// Puts `pane`'s view where the cockpit works on it, parking the one
    /// there. A new view starts (next frame) where the one before it was.
    pub(super) fn swap_in(&mut self, pane: Pane) {
        let current = self.active.get(&pane.kind).copied().unwrap_or(0);
        if pane.kind.one_only() || current == pane.id {
            return;
        }
        let place = self.view(pane.kind).place();
        let new = match pane.kind {
            BufferKind::Graph => swap_view(&mut self.graph, &mut self.parked.graphs, current, pane.id),
            BufferKind::Code => swap_view(&mut self.code, &mut self.parked.codes, current, pane.id),
            BufferKind::Spec => swap_view(&mut self.spec, &mut self.parked.specs, current, pane.id),
            BufferKind::Tree => swap_view(&mut self.tree, &mut self.parked.trees, current, pane.id),
            BufferKind::Chat | BufferKind::Queue => false,
        };
        self.active.insert(pane.kind, pane.id);
        if new {
            self.set_up(pane.kind);
            if pane.id != crate::quick_look::PANE {
                self.fresh.push((pane, place));
            }
        }
    }

    /// The cockpit uses `pane` from now on: keys, links and go-tos act on it.
    pub(super) fn use_pane(&mut self, pane: Pane) {
        self.swap_in(pane);
        if !pane.kind.one_only() {
            self.last_used.insert(pane.kind, pane.id);
        }
    }

    /// The focused column's view is the one in use (the quick look's card
    /// is, while it's open).
    pub(super) fn use_focused(&mut self) {
        if self.quick_look.is_none() {
            self.use_pane(self.columns.focused_pane());
        }
    }

    /// The views used last back in use (after drawing the others).
    pub(super) fn back_to_used(&mut self) {
        for kind in BufferKind::ALL.into_iter().filter(|k| !k.one_only()) {
            self.swap_in(self.used_pane(kind));
        }
    }

    /// A new view gets what every view of its kind has.
    pub(super) fn set_up(&mut self, kind: BufferKind) {
        match kind {
            BufferKind::Graph => self.graph.ui = self.config.graph.clone(),
            BufferKind::Code => self.code.ui = self.config.code.clone(),
            BufferKind::Spec => {
                self.spec.types.clone_from(&self.types);
                self.spec.author.clone_from(&self.config.user.name);
                self.spec.comments.clone_from(&self.review.comments);
                self.spec.diff.clone_from(&self.review.diff);
            }
            BufferKind::Tree | BufferKind::Chat | BufferKind::Queue => {}
        }
    }

    /// New views start where the view of their kind used before them was.
    pub(super) fn start_fresh(&mut self, ctx: &egui::Context) {
        for (pane, place) in std::mem::take(&mut self.fresh) {
            self.swap_in(pane);
            self.restore_place(pane.kind, &place, ctx);
        }
        self.back_to_used();
    }

    /// The graph the agent moves: column `column` (made a graph if it isn't
    /// one), else the one used last.
    pub(super) fn agents_graph(&mut self, column: Option<u32>) {
        let Some(slot) = column.and_then(|c| (c as usize).checked_sub(1)) else { return };
        if slot >= self.columns.len() {
            self.note = Some(format!("The agent asked for column {}; there are {}: it moved the graph used last.", slot + 1, self.columns.len()));
            return;
        }
        let pane = self.columns.show(slot, BufferKind::Graph);
        self.use_pane(pane);
    }

    /// Closes column `slot`: its view is hidden, not lost.
    pub(super) fn close_column(&mut self, slot: usize) {
        if !self.columns.close(slot) {
            self.note = Some("The last column stays.".into());
        }
        self.use_focused();
    }

    /// Deletes the focused column's view for good: the column shows the
    /// view hidden last, else a new one of its kind. The chat and the queue
    /// (there's one of each) aren't deleted.
    pub(super) fn delete_view(&mut self) {
        let slot = self.columns.focused();
        let pane = self.columns.pane(slot);
        if pane.kind.one_only() {
            self.note = Some(format!("There's one {}: close its column to hide it.", pane.kind.name()));
            return;
        }
        let (gone, now) = self.columns.delete(slot);
        if now == gone {
            // A new one took its number: the view itself starts over.
            self.swap_in(gone);
            self.reset(gone.kind);
        }
        self.use_pane(now);
        self.fresh.retain(|(p, _)| *p != now);
        // The view of its kind used last, if that was the one deleted.
        if self.used_pane(gone.kind) == gone && now != gone {
            let next = self.columns.other_of(gone.kind, gone).unwrap_or_else(|| Pane::new(gone.kind, 0));
            self.last_used.insert(gone.kind, next.id);
        }
        self.drop_unheld();
    }

    /// Views no column shows or hides any more are dropped.
    pub(super) fn drop_unheld(&mut self) {
        let (columns, quick) = (&self.columns, crate::quick_look::PANE);
        self.parked.keep_only(|kind, id| id == quick || columns.has(Pane::new(kind, id)));
        self.fresh.retain(|(p, _)| p.id == quick || self.columns.has(*p));
    }

    /// A new view of `kind` in the focused column (the one there is hidden).
    pub(super) fn new_view(&mut self, kind: BufferKind) {
        let pane = self.columns.new_pane(kind);
        let slot = self.columns.place(self.columns.focused(), pane);
        self.columns.focus(slot);
        self.use_pane(pane);
        // New, not a copy of the last one.
        self.fresh.retain(|(p, _)| *p != pane);
    }

    /// Shows `pane` (shown elsewhere, or hidden) in the focused column; the
    /// view there is hidden (or swaps places with it).
    pub(super) fn show_view(&mut self, pane: Pane) {
        let slot = self.columns.place(self.columns.focused(), pane);
        self.columns.focus(slot);
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
                if self.pinned(pane) { format!("{card} (pinned)") } else { card }
            }
            Some(Place::Tree { cursor }) => cursor.as_ref().map_or_else(|| "the outline".to_owned(), |id| format!("at {}", name(id))),
            Some(Place::Chat { .. }) => "the agent's session".to_owned(),
            Some(Place::Queue { .. }) => "the plan's jobs".to_owned(),
            None => "new".to_owned(),
        };
        format!("{} · {what}", pane.kind.name())
    }

    /// The view of `kind` in use, new again.
    pub(super) fn reset(&mut self, kind: BufferKind) {
        match kind {
            BufferKind::Graph => self.graph = GraphState::default(),
            BufferKind::Code => self.code = CodeViewerState::default(),
            BufferKind::Spec => self.spec = SpecState::default(),
            BufferKind::Tree => self.tree = ArchitectureTreeState::default(),
            BufferKind::Chat | BufferKind::Queue => return,
        }
        self.set_up(kind);
    }

    /// Every graph (the one in use first).
    pub(super) fn graphs(&mut self) -> impl Iterator<Item = &mut GraphState> {
        std::iter::once(&mut self.graph).chain(self.parked.graphs.values_mut())
    }

    pub(super) fn codes(&mut self) -> impl Iterator<Item = &mut CodeViewerState> {
        std::iter::once(&mut self.code).chain(self.parked.codes.values_mut())
    }

    pub(super) fn specs(&mut self) -> impl Iterator<Item = &mut SpecState> {
        std::iter::once(&mut self.spec).chain(self.parked.specs.values_mut())
    }

    pub(super) fn trees(&mut self) -> impl Iterator<Item = &mut ArchitectureTreeState> {
        std::iter::once(&mut self.tree).chain(self.parked.trees.values_mut())
    }

    /// Where `pane`'s view is, wherever it's kept.
    pub(super) fn place_of(&self, pane: Pane) -> Option<Place> {
        if pane.kind.one_only() || self.active.get(&pane.kind).copied().unwrap_or(0) == pane.id {
            return Some(self.view(pane.kind).place());
        }
        match pane.kind {
            BufferKind::Graph => self.parked.graphs.get(&pane.id).map(|v| v.place()),
            BufferKind::Code => self.parked.codes.get(&pane.id).map(|v| v.place()),
            BufferKind::Spec => self.parked.specs.get(&pane.id).map(|v| v.place()),
            BufferKind::Tree => self.parked.trees.get(&pane.id).map(|v| v.place()),
            BufferKind::Chat | BufferKind::Queue => None,
        }
    }

    /// Whether `pane`'s spec view is pinned.
    pub(super) fn pinned(&self, pane: Pane) -> bool {
        if self.active.get(&BufferKind::Spec).copied().unwrap_or(0) == pane.id { self.spec.pinned } else { self.parked.specs.get(&pane.id).is_some_and(|s| s.pinned) }
    }

    /// The view the keys go to: the spec, while the quick look shows it;
    /// else the focused column's.
    pub(super) fn focused_kind(&self) -> BufferKind {
        if self.quick_look.is_some() { BufferKind::Spec } else { self.columns.focused_kind() }
    }

    /// The column showing the view of `kind` in use (else the nearest of
    /// its kind, else the focused one).
    pub(super) fn slot_of(&self, kind: BufferKind) -> usize {
        self.columns.slot_of(self.used_pane(kind)).or_else(|| self.columns.slot_of_kind(kind)).unwrap_or(self.columns.focused())
    }

    /// Shows a view of `kind` without moving focus, and uses it: in the
    /// column a go-to's count names; else the one used last, where it is;
    /// else the nearest column of that kind; else beside the focused column.
    pub(super) fn ensure_visible(&mut self, kind: BufferKind) -> usize {
        let slot = match self.landing.take() {
            Some(slot) => {
                self.columns.show(slot, kind);
                slot
            }
            None => match self.columns.slot_of(self.used_pane(kind)).or_else(|| self.columns.slot_of_kind(kind)) {
                Some(slot) => slot,
                None => self.columns.ensure_visible(self.used_pane(kind)),
            },
        };
        self.use_pane(self.columns.pane(slot));
        slot
    }

    /// Shows a view of `kind` (as `ensure_visible`) and moves focus to it.
    pub(super) fn focus_kind(&mut self, kind: BufferKind) {
        let slot = self.ensure_visible(kind);
        self.columns.focus(slot);
    }
}
