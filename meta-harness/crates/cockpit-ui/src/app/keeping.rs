//! The cockpit's state kept on the server: saving it and putting it back.

use super::*;

impl CockpitApp {
    /// The cockpit's state now, as it's saved.
    pub fn saved_ui(&self) -> SavedUi {
        let buffers: Vec<Buffer> = self.desk.panels.buffers().iter().copied().chain(self.desk.panels.put_away()).collect();
        let used = BufferKind::ALL.iter().filter(|k| !k.one_only()).map(|k| match (&self.quick_look, k) {
            (Some(look), BufferKind::Spec) => Buffer::new(*k, look.previous_spec),
            _ => self.desk.used_buffer(*k),
        });
        SavedUi {
            version: crate::saved::VERSION,
            panels: self.desk.panels.clone(),
            views: buffers.iter().filter_map(|p| self.place_of(*p).map(|place| (*p, place))).collect(),
            pinned: buffers.iter().filter(|p| p.kind == BufferKind::Spec && self.desk.pinned(**p)).map(|p| p.id).collect(),
            used: used.collect(),
            marks: self.desk.marks.clone(),
            jumps: self.desk.jumps.clone(),
            drafts: self.drafts(),
            agents: self.desk.remote.agents,
            comparison: self.desk.changes.comparison.clone(),
            diff_layout: self.desk.changes.layout,
            item_buttons: self.item_buttons,
        }
    }

    /// Each chat view's unsent text.
    fn drafts(&self) -> BTreeMap<u32, String> {
        let used = self.desk.active.get(&BufferKind::Chat).copied().unwrap_or(0);
        std::iter::once((used, &self.desk.chat.ui))
            .chain(self.desk.parked.chats.iter().map(|(id, v)| (*id, v)))
            .filter(|(_, v)| !v.input_buffer.is_empty())
            .map(|(id, v)| (id, v.input_buffer.clone()))
            .collect()
    }

    /// Saves the state a moment after it changed, and puts back what was
    /// saved once it's read.
    pub(super) fn keep_state(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let after = self.config.net.save_after_ms as f64 / 1000.0;
        let state = self.saved_ui();
        for event in self.desk.keeper.tick(&state, now, after, &self.desk.net, ctx) {
            match event {
                crate::saved::Event::Restore(saved) => self.restore_saved(*saved, ctx),
                crate::saved::Event::Fresh => self.desk.code.load_file("Cargo.toml", None, &self.desk.net, ctx.clone()),
                crate::saved::Event::Note(note) => self.note = Some(note),
            }
        }
    }

    /// Puts the saved state back: the panels, each view where it was, the
    /// marks and jumps, and each chat view's unsent text.
    pub(super) fn restore_saved(&mut self, saved: SavedUi, ctx: &egui::Context) {
        self.close_quick_look();
        self.desk.panels = saved.panels.clone().checked();
        self.desk.marks = saved.marks.clone();
        self.desk.jumps = saved.jumps.clone().checked();
        self.desk.remote.agents = saved.agents;
        self.desk.changes.compare(saved.comparison.clone());
        self.desk.changes.layout = saved.diff_layout;
        self.item_buttons = saved.item_buttons;
        for (buffer, place) in &saved.views {
            self.swap_in(*buffer);
            if buffer.kind == BufferKind::Chat
                && self.desk.chat.ui.input_buffer.is_empty()
                && let Some(draft) = saved.drafts.get(&buffer.id)
            {
                self.desk.chat.ui.input_buffer.clone_from(draft);
            }
            if buffer.kind == BufferKind::Spec {
                self.desk.spec.pinned = saved.pinned.contains(&buffer.id);
                // One not pinned follows the graph's selection (put back with the graph).
                if !self.desk.spec.pinned {
                    continue;
                }
            }
            self.restore_place(buffer.kind, place, ctx);
        }
        // Put back as saved: not started from another view's place.
        self.desk.fresh.retain(|(buffer, _)| !saved.views.iter().any(|(p, _)| p == buffer));
        for buffer in &saved.used {
            self.desk.last_used.insert(buffer.kind, buffer.id);
        }
        self.back_to_used();
        self.use_focused();
        self.desk.keeper.restored(&saved);
    }
}
