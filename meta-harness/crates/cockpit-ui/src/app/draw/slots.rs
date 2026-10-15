//! Each column's frame, its tabs, and the palette.

use super::super::*;

impl CockpitApp {
    /// Each view's tab key ("SPC b g"), in `BufferKind::ALL` order.
    pub(in crate::app) fn tab_keys(&self, shell: &[Action]) -> Vec<String> {
        let bound = self.config.keymap.bind(shell.iter().map(|a| (Origin::Shell, a.clone())).collect());
        BufferKind::ALL.iter().map(|kind| bound.iter().find(|b| b.action.id == format!("buffer.{}", kind.name())).map(Bound::key_hint).unwrap_or_default()).collect()
    }

    pub fn render_slot(&mut self, slot_idx: usize, ui: &mut egui::Ui, shell: &[Action], tab_keys: &[String]) {
        let is_focused = self.desk.columns.focused() == slot_idx;
        let border_color = if is_focused { colors().focus } else { colors().border };

        let slot_id = format!("slot_{}", slot_idx + 1);
        self.feedback.register_inspectable(ui, &slot_id, ui.available_rect_before_wrap());

        Frame::NONE.fill(colors().bg).stroke(Stroke::new(crate::theme::look().line_thin, border_color)).corner_radius(crate::theme::look().radius_m).inner_margin(crate::theme::margin(crate::theme::look().space_m)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(ui.available_height());

            ui.vertical(|ui| {
                let active_kind = self.desk.columns.kind(slot_idx);
                let is_compact = ui.available_width() < 340.0;

                // Quiet chrome: the slot's number, then a tab per view as an
                // icon (the active one named), each saying its name and key on
                // hover. The row wraps rather than widen the slot.
                let header_res = ui.horizontal_wrapped(|ui| {
                    let number_color = if is_focused { colors().focus } else { colors().faint };
                    let slot_label = ui.add(egui::Label::new(RichText::new((slot_idx + 1).to_string()).strong().monospace().size(crate::theme::look().text_body).color(number_color)).sense(egui::Sense::click()));
                    if slot_label.on_hover_text(format!("Slot {}: {}", slot_idx + 1, active_kind.title().to_lowercase())).clicked() {
                        self.desk.columns.focus(slot_idx);
                    }
                    for (n, kind) in BufferKind::ALL.into_iter().enumerate() {
                        let title = kind.title();
                        let name = format!("{}{}", &title[..1], title[1..].to_lowercase());
                        let label = if !is_compact && active_kind == kind { format!("{} {name}", kind.icon()) } else { kind.icon().to_owned() };
                        let tip = match tab_keys.get(n).filter(|k| !k.is_empty()) {
                            Some(key) => format!("{name} ({key})"),
                            None => name,
                        };
                        let tab = ui.selectable_label(active_kind == kind, label).on_hover_text(tip);
                        if tab.clicked() {
                            let pane = self.desk.columns.show(slot_idx, kind);
                            self.desk.columns.focus(slot_idx);
                            self.swap_in(pane);
                        }
                        self.feedback.register_inspectable(ui, &format!("slot_{}:tab:{}", slot_idx + 1, kind.name()), tab.rect);
                    }
                    // Its own controls (a click focuses it first, so they act on it).
                    let own = self.column_actions();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.host_controls(ui, &own, slot_idx, shell));
                });
                self.feedback.register_inspectable(ui, &format!("slot_{}:header", slot_idx + 1), header_res.response.rect);
                ui.separator();

                self.render_view(active_kind, ui, slot_idx, is_focused, shell);
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

    /// The palette overlay plus a floating button that opens it (touch entry point).
    pub fn render_palette(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("palette_touch_button")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -48.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            let button = egui::Button::new(RichText::new(format!("{} Actions", egui_phosphor::regular::COMMAND)).size(crate::theme::look().text_title)).fill(colors().overlay_raised).corner_radius(crate::theme::look().radius_pill);
            if ui.add(button).on_hover_text("Command palette").clicked() {
                self.open_palette(None);
            }
        });

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
