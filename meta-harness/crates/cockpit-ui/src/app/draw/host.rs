//! Hosts: a column and the quick look hold a view and draw it the same
//! way: the view's top line (its title, status and header controls, from
//! `Panel` and its actions), then the view's content. A view draws no
//! controls. A host's own controls (closing it, sending the quick look's
//! card to a column) are actions too, drawn by the same control, with
//! their keys. A control used asks for its action, and the app runs it
//! once the frame is drawn, the way its key runs it.
//!
//! Pointer and touch, the same in every view: a click or tap on an item
//! moves the cursor there (the column is focused by it); a double click or
//! tap opens it (Enter); a long press lists what can be done (the palette,
//! with the cursor on it). The view only says where its items are.

use super::super::*;
use crate::actions::At;

impl CockpitApp {
    /// Draws view `kind` (the one in use) for the host over column `slot`:
    /// its top line, then its content.
    pub(in crate::app) fn render_view(&mut self, kind: BufferKind, ui: &mut egui::Ui, slot: usize, is_focused: bool, shell: &[Action]) {
        let ctx = ui.ctx().clone();
        let hints = self.key_hints();
        let pan_held = ui.input(|i| self.config.keymap.leader.key_down(i));
        let actions = crate::panel::placed(self.desk.panel(kind), self.desk.provider(kind).actions(), shell);
        let mut cx = RunCx { net: &self.desk.net, ctx: &ctx, input_mode: &mut self.input_mode, slot, note: &mut self.note, shell, hints: &hints, asked: &mut self.asked };
        crate::panel::header(ui, self.desk.panel(kind), &actions, &mut cx);
        ui.separator();
        let content = ui.available_rect_before_wrap();
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
        self.pointer_on_content(kind, ui, content);
    }

    /// A click or double click on a view's content (not on something over
    /// it): on one of its items, the cursor goes there; a double opens it.
    fn pointer_on_content(&mut self, kind: BufferKind, ui: &egui::Ui, content: egui::Rect) {
        if self.desk.panel(kind).own_pointer() {
            return;
        }
        let (clicked, double, at) = ui.input(|i| (i.pointer.primary_clicked(), i.pointer.button_double_clicked(egui::PointerButton::Primary), i.pointer.interact_pos()));
        let Some(at) = at.filter(|at| (clicked || double) && content.contains(*at) && ui.ctx().layer_id_at(*at) == Some(ui.layer_id())) else { return };
        let Some(key) = self.desk.view(kind).spots().into_iter().find(|s| s.rect.contains(at)).map(|s| s.key) else { return };
        self.desk.view_mut(kind).move_to(&key);
        if double {
            self.asked = Some(("nav.enter".to_owned(), None));
        }
    }

    /// A host's own controls: those of `actions` that have one.
    pub(in crate::app) fn host_controls(&mut self, ui: &mut egui::Ui, actions: &[Action], slot: usize, shell: &[Action]) {
        let ctx = ui.ctx().clone();
        let hints = self.key_hints();
        let mut cx = RunCx { net: &self.desk.net, ctx: &ctx, input_mode: &mut self.input_mode, slot, note: &mut self.note, shell, hints: &hints, asked: &mut self.asked };
        crate::actions::controls(ui, actions, &mut cx);
    }

    /// Under where you are in the focused view (the cursor's item, or the
    /// box being typed in): that place's controls, from the view's actions
    /// (and the shell's it borrows). Below it, or above when there's no room.
    pub(in crate::app) fn render_item_controls(&mut self, ctx: &egui::Context, shell: &[Action]) {
        let kind = self.focused_kind();
        let (slot, bounds) = match &self.quick_look {
            Some(look) => (look.slot, ctx.memory(|m| m.area_rect(egui::Id::new("quick_look")))),
            None => (self.desk.columns.focused(), self.slot_rects.get(self.desk.columns.focused()).copied()),
        };
        let Some(bounds) = bounds else { return };
        let panel = self.desk.panel(kind);
        let (at, anchor) = match self.input_mode {
            InputMode::Insert { .. } => (At::Typing, panel.typing_rect()),
            _ => {
                let view = self.desk.view(kind);
                let here = panel.here().or_else(|| view.cursor_key());
                (At::Item, view.spots().into_iter().find(|s| Some(&s.key) == here.as_ref()).map(|s| s.rect))
            }
        };
        let Some(anchor) = anchor.map(|r| r.intersect(bounds)).filter(|r| r.is_positive()) else { return };
        let situation = self.situation();
        let actions = crate::panel::placed(panel, self.desk.provider(kind).actions(), shell);
        let here: Vec<Action> = actions.into_iter().filter(|a| a.at() == Some(at) && a.when.holds(&situation)).collect();
        if here.is_empty() {
            return;
        }
        let id = egui::Id::new("item_controls");
        let look = crate::theme::look();
        let size = ctx.memory(|m| m.area_rect(id)).map_or(egui::vec2(anchor.width(), look.text_title * 2.0), |r| r.size());
        let below = anchor.bottom() + look.space_s;
        let y = if below + size.y <= bounds.bottom() { below } else { (anchor.top() - look.space_s - size.y).max(bounds.top()) };
        let hints = self.key_hints();
        let mut cx = RunCx { net: &self.desk.net, ctx, input_mode: &mut self.input_mode, slot, note: &mut self.note, shell, hints: &hints, asked: &mut self.asked };
        egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(egui::pos2(anchor.left(), y)).show(ctx, |ui| {
            Frame::NONE.fill(colors().overlay_raised).stroke(Stroke::new(look.line_thin, colors().border)).corner_radius(look.radius_m).inner_margin(crate::theme::margin(look.space_s)).show(ui, |ui| {
                ui.set_max_width(anchor.width().max(look.quick_look_width / 3.0).min(bounds.width()));
                ui.horizontal_wrapped(|ui| crate::actions::controls(ui, &here, &mut cx));
            });
        });
    }

    /// Runs the action a control asked for this frame.
    pub(in crate::app) fn run_asked(&mut self, ctx: &egui::Context) {
        if let Some((id, value)) = self.asked.take() {
            self.run_by_id(&id, value, ctx);
        }
    }
}
