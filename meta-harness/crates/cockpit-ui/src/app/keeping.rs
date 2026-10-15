//! The cockpit's state kept on the server: saving it and putting it back.

use super::*;

impl CockpitApp {
    /// The cockpit's state now, as it's saved.
    pub fn saved_ui(&self) -> SavedUi {
        let panes: Vec<Pane> = self.columns.panes().iter().copied().chain(self.columns.put_away()).collect();
        let used = BufferKind::ALL.iter().filter(|k| !k.one_only()).map(|k| match (&self.quick_look, k) {
            (Some(look), BufferKind::Spec) => Pane::new(*k, look.previous_spec),
            _ => self.used_pane(*k),
        });
        SavedUi {
            version: crate::saved::VERSION,
            columns: self.columns.clone(),
            views: panes.iter().filter_map(|p| self.place_of(*p).map(|place| (*p, place))).collect(),
            pinned: panes.iter().filter(|p| p.kind == BufferKind::Spec && self.pinned(**p)).map(|p| p.id).collect(),
            used: used.collect(),
            marks: self.marks.clone(),
            jumps: self.jumps.clone(),
            drafts: self.drafts(),
            browser_alerts: self.alerts.on(),
            agents: self.remote.agents,
        }
    }

    /// Each chat view's unsent text.
    fn drafts(&self) -> BTreeMap<u32, String> {
        let used = self.active.get(&BufferKind::Chat).copied().unwrap_or(0);
        std::iter::once((used, &self.chat.ui))
            .chain(self.parked.chats.iter().map(|(id, v)| (*id, v)))
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
        for event in self.keeper.tick(&state, now, after, &self.net, ctx) {
            match event {
                crate::saved::Event::Restore(saved) => self.restore_saved(*saved, ctx),
                crate::saved::Event::Fresh => self.code.load_file("Cargo.toml", None, &self.net, ctx.clone()),
                crate::saved::Event::Note(note) => self.note = Some(note),
            }
        }
    }

    /// Puts the saved state back: the columns, each view where it was, the
    /// marks and jumps, and each chat view's unsent text.
    pub(super) fn restore_saved(&mut self, saved: SavedUi, ctx: &egui::Context) {
        self.close_quick_look();
        self.columns = saved.columns.clone().checked();
        self.marks = saved.marks.clone();
        self.jumps = saved.jumps.clone().checked();
        self.remote.agents = saved.agents;
        if saved.browser_alerts != self.alerts.on() {
            self.alerts.set(saved.browser_alerts);
        }
        for (pane, place) in &saved.views {
            self.swap_in(*pane);
            if pane.kind == BufferKind::Chat
                && self.chat.ui.input_buffer.is_empty()
                && let Some(draft) = saved.drafts.get(&pane.id)
            {
                self.chat.ui.input_buffer.clone_from(draft);
            }
            if pane.kind == BufferKind::Spec {
                self.spec.pinned = saved.pinned.contains(&pane.id);
                // One not pinned follows the graph's selection (put back with the graph).
                if !self.spec.pinned {
                    continue;
                }
            }
            self.restore_place(pane.kind, place, ctx);
        }
        // Put back as saved: not started from another view's place.
        self.fresh.retain(|(pane, _)| !saved.views.iter().any(|(p, _)| p == pane));
        for pane in &saved.used {
            self.last_used.insert(pane.kind, pane.id);
        }
        self.back_to_used();
        self.use_focused();
        self.keeper.restored(&saved);
    }
}
