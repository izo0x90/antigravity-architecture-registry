//! The cockpit's state kept on the server: saving it and putting it back.

use super::*;

impl CockpitApp {
    /// The cockpit's state now, as it's saved.
    pub fn saved_ui(&self) -> SavedUi {
        let buffers: Vec<BufferId> = self.desk.panels.buffers().iter().copied().chain(self.desk.panels.put_away()).collect();
        let used = BufferKind::ALL.iter().filter(|k| !k.one_only()).map(|k| match &self.quick_look {
            Some(look) if look.kind() == *k => look.before,
            _ => self.desk.used_buffer(*k),
        });
        SavedUi {
            version: crate::saved::VERSION,
            panels: self.desk.panels.clone(),
            views: buffers.iter().filter_map(|p| self.place_of(*p).map(|place| (*p, place))).collect(),
            // The panels' buffers, the one-only kinds', and the quick look's own.
            kept: buffers
                .iter()
                .copied()
                .chain(BufferKind::ALL.iter().filter(|k| k.one_only()).map(|k| BufferId::new(*k, 0)))
                .chain(BufferKind::ALL.iter().map(|k| BufferId::new(*k, crate::quick_look::PANE)))
                .filter_map(|p| Some((p, self.desk.buffer(p)?.keep()?)))
                .collect(),
            used: used.collect(),
            marks: self.desk.marks.clone(),
            jumps: self.desk.jumps.clone(),
            agents: self.desk.remote.agents,
            comparison: self.desk.shared.changes.comparison().clone(),
            item_buttons: self.item_buttons,
        }
    }

    /// Saves the state a moment after it changed, and puts back what was
    /// saved once it's read.
    pub(super) fn keep_state(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let after = self.config.net.save_after_ms as f64 / 1000.0;
        let state = self.saved_ui();
        for event in self.desk.keeper.tick(&state, now, after, &self.desk.net, ctx) {
            match event {
                crate::saved::Event::Restore(saved) => self.restore_saved(*saved),
                crate::saved::Event::Fresh => {
                    let _ = self.desk.view_mut(BufferKind::Code).show_target(&Target::Span { file: "Cargo.toml".into(), start: 1, end: 1 });
                }
                crate::saved::Event::Note(note) => self.note = Some(note),
            }
        }
    }

    /// Puts the saved state back: the panels, each buffer where it was and
    /// what it keeps, the marks and jumps.
    pub(super) fn restore_saved(&mut self, saved: SavedUi) {
        self.close_quick_look();
        self.desk.panels = saved.panels.clone().checked();
        self.desk.marks = saved.marks.clone();
        self.desk.jumps = saved.jumps.clone().checked();
        self.desk.remote.agents = saved.agents;
        self.desk.shared.changes.compare(saved.comparison.clone());
        self.item_buttons = saved.item_buttons;
        for (buffer, kept) in &saved.kept {
            self.desk.ensure(*buffer);
            if let Some(view) = self.desk.buffer_mut(*buffer) {
                view.restore_kept(kept);
            }
        }
        for (buffer, place) in &saved.views {
            self.desk.ensure(*buffer);
            if let Some(view) = self.desk.buffer_mut(*buffer) {
                view.restore(place);
            }
        }
        self.desk.share();
        // Put back as saved: not started from another buffer's place.
        self.desk.fresh.retain(|(buffer, _)| !saved.views.iter().any(|(p, _)| p == buffer));
        for buffer in &saved.used {
            self.desk.ensure(*buffer);
            self.desk.last_used.insert(buffer.kind, buffer.id);
        }
        self.use_focused();
        self.desk.keeper.restored(&saved);
    }
}
