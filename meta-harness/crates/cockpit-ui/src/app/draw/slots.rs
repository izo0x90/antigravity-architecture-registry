//! Each column's frame, its tabs, and the palette.

use super::super::*;

impl CockpitApp {
    pub fn render_slot(&mut self, slot_idx: usize, ui: &mut egui::Ui) {
        let is_focused = self.desk.columns.focused() == slot_idx;
        let border_color = if is_focused { colors().focus } else { colors().border };

        let slot_id = format!("slot_{}", slot_idx + 1);
        self.feedback.register_inspectable(ui, &slot_id, ui.available_rect_before_wrap());

        Frame::NONE.fill(colors().bg).stroke(Stroke::new(crate::theme::look().line_thin, border_color)).corner_radius(crate::theme::look().radius_m).inner_margin(crate::theme::margin(crate::theme::look().space_m)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(ui.available_height());

            ui.vertical(|ui| {
                let active_kind = self.desk.columns.kind(slot_idx);

                // Quiet chrome: the slot's number, then its controls. The row
                // wraps rather than widen the slot.
                let header_res = ui.horizontal_wrapped(|ui| {
                    let number_color = if is_focused { colors().focus } else { colors().faint };
                    ui.label(RichText::new((slot_idx + 1).to_string()).strong().monospace().size(crate::theme::look().text_body).color(number_color))
                        .on_hover_text(format!("Slot {}: {}", slot_idx + 1, active_kind.title().to_lowercase()));
                    // Its own controls from the one list (its host's layer
                    // where it is): a tab per kind of view, closing it.
                    let here = Here { slot: slot_idx, kind: active_kind, look: false };
                    let own: Vec<Bound> = self.shown_at(here).into_iter().filter(|b| b.layer == Layer::Host).collect();
                    self.host_controls(ui, &own, slot_idx);
                });
                self.feedback.register_inspectable(ui, &format!("slot_{}:header", slot_idx + 1), header_res.response.rect);
                ui.separator();

                self.render_view(Here { slot: slot_idx, kind: active_kind, look: false }, ui, is_focused);
            });
        });

        if let Some(rect) = self.slot_rects.get_mut(slot_idx) {
            *rect = ui.min_rect();
        }
        // Frame-level click-to-focus detection
        if ui.input(|i| i.pointer.primary_clicked())
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            && ui.min_rect().contains(pos)
        {
            self.desk.columns.focus(slot_idx);
        }
    }

    /// The palette overlay (opened by its key, or its control on the top
    /// bar).
    pub fn render_palette(&mut self, ctx: &egui::Context) {
        let bound = self.palette_actions();
        // A view asked for an action's picker (e.g. Resume once sessions loaded).
        for kind in BufferKind::ALL {
            if let Some(id) = self.desk.provider_mut(kind).take_palette_request()
                && let Some(action) = bound.iter().find(|b| b.action.id == id)
            {
                self.open_palette(Some(action));
            }
        }
        let picked = self.palette.render(ctx, &bound, &self.config.keymap);
        if let Some(ask) = self.palette.take_search()
            && let Origin::View(kind) = ask.origin
        {
            self.desk.provider_mut(kind).search(&ask.action, &ask.query);
        }
        if let Some(invocation) = picked {
            self.input_mode = InputMode::Normal;
            release_keyboard(ctx);
            self.invoke(invocation, ctx);
        }
        if self.input_mode == InputMode::Palette && !self.palette.is_open() {
            self.input_mode = InputMode::Normal;
            release_keyboard(ctx);
        }
    }
}
