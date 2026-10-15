//! Hosts: a column and the quick look hold a view and draw it the same
//! way: the view's top line (its title, status and header controls, from
//! `Panel` and its actions), then the view's content. A view draws no
//! controls. A host's own controls (closing it, sending the quick look's
//! card to a column) are actions too, drawn by the same control, with
//! their keys. A control used asks for its action, and the app runs it
//! once the frame is drawn, the way its key runs it.

use super::super::*;

impl CockpitApp {
    /// Draws view `kind` (the one in use) for the host over column `slot`:
    /// its top line, then its content.
    pub(in crate::app) fn render_view(&mut self, kind: BufferKind, ui: &mut egui::Ui, slot: usize, is_focused: bool, shell: &[Action]) {
        let ctx = ui.ctx().clone();
        let hints = self.key_hints();
        let pan_held = ui.input(|i| self.config.keymap.leader.key_down(i));
        let panel = self.desk.panel(kind);
        let mut actions = self.desk.provider(kind).actions();
        actions.extend(crate::panel::borrowed(panel, shell));
        let mut cx = RunCx { net: &self.desk.net, ctx: &ctx, input_mode: &mut self.input_mode, slot, note: &mut self.note, shell, hints: &hints, asked: &mut self.asked };
        crate::panel::header(ui, self.desk.panel(kind), &actions, &mut cx);
        ui.separator();
        match kind {
            BufferKind::Chat => self.desk.chat.render(ui, &mut self.feedback, is_focused, &mut cx),
            BufferKind::Code => self.desk.code.render(ui, &mut self.feedback, &mut cx),
            BufferKind::Graph => self.desk.graph.render(ui, &mut self.feedback, slot, pan_held, &mut cx),
            BufferKind::Tree => self.desk.tree.render(ui, &mut self.feedback, &mut cx),
            BufferKind::Spec => {
                // A card over a column: what the graph says about it first.
                if self.desk.spec.floating
                    && let Some(id) = self.desk.spec.target.clone()
                {
                    self.desk.graph.render_card_context(ui, &id);
                    ui.separator();
                }
                self.desk.spec.render(ui, &mut cx);
            }
            BufferKind::Queue => self.desk.queue.render(ui, &mut cx),
            BufferKind::Inbox => self.desk.inbox.render(ui, &mut cx, &mut self.desk.chat),
        }
    }

    /// A host's own controls: those of `actions` that have one.
    pub(in crate::app) fn host_controls(&mut self, ui: &mut egui::Ui, actions: &[Action], slot: usize, shell: &[Action]) {
        let ctx = ui.ctx().clone();
        let hints = self.key_hints();
        let mut cx = RunCx { net: &self.desk.net, ctx: &ctx, input_mode: &mut self.input_mode, slot, note: &mut self.note, shell, hints: &hints, asked: &mut self.asked };
        crate::actions::controls(ui, actions, &mut cx);
    }

    /// Runs the action a control asked for this frame.
    pub(in crate::app) fn run_asked(&mut self, ctx: &egui::Context) {
        if let Some((id, value)) = self.asked.take() {
            self.run_by_id(&id, value, ctx);
        }
    }
}
