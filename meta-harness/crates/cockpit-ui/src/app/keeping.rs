//! The cockpit's state kept on the server: saving it and putting it back.

use super::*;

impl CockpitApp {
    /// The cockpit's state now, as it's saved.
    pub fn saved_ui(&self) -> SavedUi {
        // The panels' buffers, the one-only kinds', and the quick look's own.
        let buffers: Vec<BufferId> = self
            .desk
            .panels
            .buffers()
            .iter()
            .copied()
            .chain(self.desk.panels.put_away())
            .chain(BufferKind::ALL.iter().filter(|k| k.one_only()).map(|k| BufferId::new(*k, 0)))
            .chain(BufferKind::ALL.iter().map(|k| BufferId::new(*k, crate::quick_look::PANE)))
            .collect();
        let used = BufferKind::ALL.iter().filter(|k| !k.one_only()).map(|k| match &self.quick_look {
            Some(look) if look.kind() == *k => look.before,
            _ => self.desk.used_buffer(*k),
        });
        SavedUi {
            version: crate::saved::VERSION,
            panels: self.desk.panels.clone(),
            views: buffers.iter().filter_map(|p| self.place_of(*p).map(|place| (*p, place))).collect(),
            kept: buffers.iter().filter_map(|p| Some((*p, self.desk.buffer(*p)?.keep()?))).collect(),
            versions: BufferKind::ALL.iter().map(|k| (*k, k.saved_version())).collect(),
            used: used.collect(),
            marks: self.desk.marks.clone(),
            jumps: self.desk.jumps.clone(),
            agents: self.desk.remote.agents,
            comparison: self.desk.shared.changes.comparison().clone(),
            item_buttons: self.item_buttons,
            at_work: self.at_work,
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
        // The chats' unsent text: its own record (the chat views report
        // theirs as asks), saved a moment after each changed.
        for note in self.desk.shared.drafts.get_mut().tick(now, after, &self.desk.net, ctx) {
            self.note = Some(note);
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
        self.at_work = saved.at_work;
        // A kind saved in another version of itself starts new; the rest holds.
        let current = |kind: BufferKind| saved.versions.get(&kind).copied().unwrap_or(1) == kind.saved_version();
        let saved_kinds = saved.views.iter().map(|(b, _)| b.kind).chain(saved.kept.iter().map(|(b, _)| b.kind));
        let stale: std::collections::BTreeSet<BufferKind> = saved_kinds.filter(|k| !current(*k)).collect();
        if !stale.is_empty() {
            let names: Vec<&str> = stale.iter().map(BufferKind::name).collect();
            self.note = Some(format!(
                "The saved layout's {} {} from another version of the cockpit: starting {} new.",
                names.join(", "),
                if names.len() == 1 { "buffer is" } else { "buffers are" },
                if names.len() == 1 { "it" } else { "them" }
            ));
        }
        for (buffer, kept) in saved.kept.iter().filter(|(b, _)| current(b.kind)) {
            self.desk.ensure(*buffer);
            if let Some(view) = self.desk.buffer_mut(*buffer) {
                view.restore_kept(kept);
            }
        }
        for (buffer, place) in saved.views.iter().filter(|(b, _)| current(b.kind)) {
            self.desk.ensure(*buffer);
            if let Some(view) = self.desk.buffer_mut(*buffer) {
                view.restore(place);
            }
        }
        self.desk.share();
        // Put back as saved: not started from another buffer's place.
        self.desk.fresh.retain(|(buffer, _)| !saved.views.iter().any(|(p, _)| p == buffer && current(p.kind)));
        for buffer in &saved.used {
            self.desk.ensure(*buffer);
            self.desk.last_used.insert(buffer.kind, buffer.id);
        }
        self.use_focused();
        self.desk.keeper.restored(&saved);
    }
}
