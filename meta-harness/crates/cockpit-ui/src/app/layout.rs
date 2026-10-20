//! Each panel's own view: which view of a kind is in use, parking, new and closed views, slots.

use super::*;

impl CockpitApp {
    /// The cockpit uses `buffer` from now on: keys, links and go-tos act
    /// on it. A new one starts (next frame) where the one of its kind used
    /// before it was.
    pub(super) fn use_buffer(&mut self, buffer: BufferId) {
        self.make(buffer);
        if !buffer.kind.one_only() {
            self.desk.last_used.insert(buffer.kind, buffer.id);
        }
        self.desk.drawn();
    }

    /// Makes `buffer` if it isn't yet: it reads what the buffers share,
    /// and starts (next frame) where the one of its kind used before it was.
    fn make(&mut self, buffer: BufferId) {
        if self.desk.ensure(buffer) {
            self.desk.share();
            if buffer.id != crate::quick_look::PANE {
                let place = self.desk.view(buffer.kind).kept_place();
                self.desk.fresh.push((buffer, place));
            }
        }
    }

    /// Draws `buffer` (its panel's): it's the one of its kind in use until
    /// the panels are drawn.
    pub(super) fn draw_as(&mut self, buffer: BufferId) {
        self.make(buffer);
        self.desk.drawing(buffer);
    }

    /// The focused panel's buffer is the one in use (the quick look's card
    /// is, while it's open).
    pub(super) fn use_focused(&mut self) {
        if self.quick_look.is_none() {
            self.use_buffer(self.desk.panels.focused_buffer());
        }
    }

    /// New buffers start where the buffer of their kind used before them was.
    pub(super) fn start_fresh(&mut self) {
        for (buffer, place) in std::mem::take(&mut self.desk.fresh) {
            if let Some(view) = self.desk.buffer_mut(buffer) {
                view.restore(&place);
            }
        }
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

    /// Deletes the focused panel's buffer for good: the panel shows the
    /// buffer hidden last, else a new one of its kind. The queue (there's
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
            // A new one took its number: the buffer itself starts over.
            self.desk.renew(gone);
            self.desk.share();
        }
        self.use_buffer(now);
        self.desk.fresh.retain(|(p, _)| *p != now);
        // The buffer of its kind used last, if that was the one deleted.
        if self.desk.used_buffer(gone.kind) == gone && now != gone {
            let next = self.desk.panels.other_of(gone.kind, gone).unwrap_or_else(|| BufferId::new(gone.kind, 0));
            self.desk.last_used.insert(gone.kind, next.id);
        }
        self.drop_unheld();
    }

    /// Buffers no panel shows or hides any more are dropped.
    pub(super) fn drop_unheld(&mut self) {
        let (panels, quick) = (self.desk.panels.clone(), crate::quick_look::PANE);
        self.desk.keep_only(|b| b.id == quick || panels.has(b));
        self.desk.fresh.retain(|(p, _)| p.id == quick || panels.has(*p));
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
    pub(super) fn show_view(&mut self, buffer: BufferId) {
        let panel = self.desk.panels.place(self.desk.panels.focused(), buffer);
        self.desk.panels.focus(panel);
        self.use_buffer(buffer);
    }

    /// What a buffer shows, in words (the buffer list's lines).
    pub(super) fn describe(&self, buffer: BufferId) -> String {
        let what = self.desk.buffer(buffer).map_or_else(|| "new".to_owned(), |b| b.describe());
        format!("{} · {what}", buffer.kind.name())
    }

    /// Where `buffer` is.
    pub(super) fn place_of(&self, buffer: BufferId) -> Option<Place> {
        self.desk.buffer(buffer).map(|b| b.kept_place())
    }

    /// The view the keys go to: the quick look's, while it's open (its
    /// card's spec view, or the inbox); else the focused panel's.
    pub(super) fn focused_kind(&self) -> BufferKind {
        self.quick_look.as_ref().map_or_else(|| self.desk.panels.focused_kind(), |q| q.kind())
    }

    /// What the field of the buffer `at` holds takes while typed in.
    pub(super) fn field_at(&self, at: crate::keymap::Host) -> crate::keymap::Field {
        let buffer = match (at, &self.quick_look) {
            (crate::keymap::Host::Look, Some(look)) => look.buffer,
            (crate::keymap::Host::Panel(panel), _) if panel < self.desk.panels.len() => self.desk.panels.buffer(panel),
            _ => return crate::keymap::Field::Text,
        };
        self.desk.buffer(buffer).map_or(crate::keymap::Field::Text, |b| b.field())
    }

    /// What holds the buffer of `kind` in use: the quick look, while it
    /// holds it; else its panel.
    pub(super) fn host_of(&self, kind: BufferKind) -> crate::keymap::Host {
        match &self.quick_look {
            Some(look) if look.kind() == kind => crate::keymap::Host::Look,
            _ => crate::keymap::Host::Panel(self.panel_of(kind)),
        }
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
