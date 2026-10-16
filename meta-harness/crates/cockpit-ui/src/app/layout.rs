//! Each panel's own view: which view of a kind is in use, parking, new and closed views, slots.

use super::*;

impl CockpitApp {
    /// Puts `buffer`'s view where the cockpit works on it, parking the one
    /// there. A new view starts (next frame) where the one before it was.
    pub(super) fn swap_in(&mut self, buffer: Buffer) {
        let current = self.desk.active.get(&buffer.kind).copied().unwrap_or(0);
        if buffer.kind.one_only() || current == buffer.id {
            return;
        }
        let place = self.desk.view(buffer.kind).place();
        let new = match buffer.kind {
            BufferKind::Graph => swap_view(&mut self.desk.graph, &mut self.desk.parked.graphs, current, buffer.id),
            BufferKind::Code => swap_view(&mut self.desk.code, &mut self.desk.parked.codes, current, buffer.id),
            BufferKind::Spec => swap_view(&mut self.desk.spec, &mut self.desk.parked.specs, current, buffer.id),
            BufferKind::Tree => swap_view(&mut self.desk.tree, &mut self.desk.parked.trees, current, buffer.id),
            BufferKind::Chat => swap_view(&mut self.desk.chat.ui, &mut self.desk.parked.chats, current, buffer.id),
            BufferKind::Queue | BufferKind::Inbox | BufferKind::Changes => false,
        };
        self.desk.active.insert(buffer.kind, buffer.id);
        if new {
            self.set_up(buffer.kind);
            if buffer.id != crate::quick_look::PANE {
                self.desk.fresh.push((buffer, place));
            }
        }
    }

    /// The cockpit uses `buffer` from now on: keys, links and go-tos act on it.
    pub(super) fn use_buffer(&mut self, buffer: Buffer) {
        self.swap_in(buffer);
        if !buffer.kind.one_only() {
            self.desk.last_used.insert(buffer.kind, buffer.id);
        }
    }

    /// The focused panel's view is the one in use (the quick look's card
    /// is, while it's open).
    pub(super) fn use_focused(&mut self) {
        if self.quick_look.is_none() {
            self.use_buffer(self.desk.panels.focused_buffer());
        }
    }

    /// The views used last back in use (after drawing the others).
    pub(super) fn back_to_used(&mut self) {
        for kind in BufferKind::ALL.into_iter().filter(|k| !k.one_only()) {
            self.swap_in(self.desk.used_buffer(kind));
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
        for (buffer, place) in std::mem::take(&mut self.desk.fresh) {
            self.swap_in(buffer);
            self.restore_place(buffer.kind, &place, ctx);
        }
        self.back_to_used();
    }

    /// The graph the agent moves: panel `number` (made a graph if it isn't
    /// one), else the one used last.
    pub(super) fn agents_graph(&mut self, number: Option<u32>) {
        let Some(panel) = number.and_then(|c| (c as usize).checked_sub(1)) else { return };
        if panel >= self.desk.panels.len() {
            self.note = Some(format!("The agent asked for panel {}; there are {}: it moved the graph used last.", panel + 1, self.desk.panels.len()));
            return;
        }
        let buffer = self.desk.panels.show(panel, BufferKind::Graph);
        self.use_buffer(buffer);
    }

    /// Closes panel `panel`: its view is hidden, not lost.
    pub(super) fn close_panel(&mut self, panel: usize) {
        if !self.desk.panels.close(panel) {
            self.note = Some("The last panel stays.".into());
        }
        self.use_focused();
    }

    /// Deletes the focused panel's view for good: the panel shows the
    /// view hidden last, else a new one of its kind. The queue (there's
    /// one) isn't deleted.
    pub(super) fn delete_view(&mut self) {
        let panel = self.desk.panels.focused();
        let buffer = self.desk.panels.buffer(panel);
        if buffer.kind.one_only() {
            self.note = Some(format!("There's one {}: close its panel to hide it.", buffer.kind.name()));
            return;
        }
        let (gone, now) = self.desk.panels.delete(panel);
        if now == gone {
            // A new one took its number: the view itself starts over.
            self.swap_in(gone);
            self.reset(gone.kind);
        }
        self.use_buffer(now);
        self.desk.fresh.retain(|(p, _)| *p != now);
        // The view of its kind used last, if that was the one deleted.
        if self.desk.used_buffer(gone.kind) == gone && now != gone {
            let next = self.desk.panels.other_of(gone.kind, gone).unwrap_or_else(|| Buffer::new(gone.kind, 0));
            self.desk.last_used.insert(gone.kind, next.id);
        }
        self.drop_unheld();
    }

    /// Views no panel shows or hides any more are dropped.
    pub(super) fn drop_unheld(&mut self) {
        let (panels, quick) = (&self.desk.panels, crate::quick_look::PANE);
        self.desk.parked.keep_only(|kind, id| id == quick || panels.has(Buffer::new(kind, id)));
        self.desk.fresh.retain(|(p, _)| p.id == quick || self.desk.panels.has(*p));
    }

    /// A new view of `kind` in the focused panel (the one there is hidden).
    pub(super) fn new_view(&mut self, kind: BufferKind) {
        let buffer = self.desk.panels.new_buffer(kind);
        let panel = self.desk.panels.place(self.desk.panels.focused(), buffer);
        self.desk.panels.focus(panel);
        self.use_buffer(buffer);
        // New, not a copy of the last one.
        self.desk.fresh.retain(|(p, _)| *p != buffer);
    }

    /// Shows `buffer` (shown elsewhere, or hidden) in the focused panel; the
    /// view there is hidden (or swaps places with it).
    pub(super) fn show_view(&mut self, buffer: Buffer) {
        let panel = self.desk.panels.place(self.desk.panels.focused(), buffer);
        self.desk.panels.focus(panel);
        self.use_buffer(buffer);
    }

    /// What a view shows, in words (the buffer list's lines).
    pub(super) fn describe(&self, buffer: Buffer) -> String {
        let name = |id: &ComponentId| self.component_name(id);
        let what = match self.place_of(buffer) {
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
                if self.desk.pinned(buffer) { format!("{card} (pinned)") } else { card }
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
        format!("{} · {what}", buffer.kind.name())
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

    /// Where `buffer`'s view is, wherever it's kept.
    pub(super) fn place_of(&self, buffer: Buffer) -> Option<Place> {
        if buffer.kind.one_only() || self.desk.active.get(&buffer.kind).copied().unwrap_or(0) == buffer.id {
            return Some(self.desk.view(buffer.kind).place());
        }
        match buffer.kind {
            BufferKind::Graph => self.desk.parked.graphs.get(&buffer.id).map(|v| v.place()),
            BufferKind::Code => self.desk.parked.codes.get(&buffer.id).map(|v| v.place()),
            BufferKind::Spec => self.desk.parked.specs.get(&buffer.id).map(|v| v.place()),
            BufferKind::Tree => self.desk.parked.trees.get(&buffer.id).map(|v| v.place()),
            BufferKind::Chat => self.desk.parked.chats.get(&buffer.id).map(|v| v.place()),
            BufferKind::Queue | BufferKind::Inbox | BufferKind::Changes => None,
        }
    }

    /// The view the keys go to: the quick look's, while it's open (its
    /// card's spec view, or the inbox); else the focused panel's.
    pub(super) fn focused_kind(&self) -> BufferKind {
        self.quick_look.as_ref().map_or_else(|| self.desk.panels.focused_kind(), |q| q.kind)
    }

    /// The panel showing the view of `kind` in use (else the nearest of
    /// its kind, else the focused one).
    pub(super) fn panel_of(&self, kind: BufferKind) -> usize {
        self.desk.panels.panel_of(self.desk.used_buffer(kind)).or_else(|| self.desk.panels.panel_of_kind(kind)).unwrap_or(self.desk.panels.focused())
    }

    /// Shows a view of `kind` without moving focus, and uses it: in the
    /// panel a go-to's count names; else the one used last, where it is;
    /// else the nearest panel of that kind; else beside the focused panel.
    pub(super) fn ensure_visible(&mut self, kind: BufferKind) -> usize {
        let panel = match self.landing.take() {
            Some(panel) => {
                self.desk.panels.show(panel, kind);
                panel
            }
            None => match self.desk.panels.panel_of(self.desk.used_buffer(kind)).or_else(|| self.desk.panels.panel_of_kind(kind)) {
                Some(panel) => panel,
                None => self.desk.panels.ensure_visible(self.desk.used_buffer(kind)),
            },
        };
        self.use_buffer(self.desk.panels.buffer(panel));
        panel
    }

    /// Shows a view of `kind` (as `ensure_visible`) and moves focus to it.
    pub(super) fn focus_kind(&mut self, kind: BufferKind) {
        let panel = self.ensure_visible(kind);
        self.desk.panels.focus(panel);
    }
}
