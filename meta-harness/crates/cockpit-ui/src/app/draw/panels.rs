//! Each panel's frame, its tabs, and the palette.

use super::super::*;
use crate::keymap::Host;

impl CockpitApp {
    pub fn render_panel(&mut self, panel: usize, ui: &mut egui::Ui) {
        let is_focused = self.desk.panels.focused() == panel;
        let border_color = if is_focused { colors().focus } else { colors().border };

        let panel_id = format!("panel_{}", panel + 1);
        self.feedback.register_inspectable(ui, &panel_id, ui.available_rect_before_wrap());

        Frame::NONE.fill(colors().bg).stroke(Stroke::new(crate::theme::look().line_thin, border_color)).corner_radius(crate::theme::look().radius_m).inner_margin(crate::theme::margin(crate::theme::look().space_m)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(ui.available_height());

            ui.vertical(|ui| {
                let active_kind = self.desk.panels.kind(panel);

                // Quiet chrome: the panel's number, then its controls. The row
                // wraps rather than widen the panel.
                let header_res = ui.horizontal_wrapped(|ui| {
                    let number_color = if is_focused { colors().focus } else { colors().faint };
                    ui.label(RichText::new((panel + 1).to_string()).strong().monospace().size(crate::theme::look().text_body).color(number_color))
                        .on_hover_text(format!("Panel {}: {}", panel + 1, active_kind.title().to_lowercase()));
                    // Its own controls from the one list (its host's layer
                    // where it is): a tab per kind of view, closing it.
                    let here = Here { at: Host::Panel(panel), kind: active_kind };
                    let own: Vec<Bound> = self.shown_at(here).into_iter().filter(|b| b.layer == Layer::Host).collect();
                    self.host_controls(ui, &own, here.at);
                });
                self.feedback.register_inspectable(ui, &format!("panel_{}:header", panel + 1), header_res.response.rect);
                ui.separator();

                self.render_view(Here { at: Host::Panel(panel), kind: active_kind }, ui, is_focused);
            });
        });

        if let Some(rect) = self.panel_rects.get_mut(panel) {
            *rect = ui.min_rect();
        }
        // Frame-level click-to-focus detection
        if ui.input(|i| i.pointer.primary_clicked())
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            && ui.min_rect().contains(pos)
        {
            self.desk.panels.focus(panel);
        }
    }

    /// The palette overlay (opened by its key, or its control on the top
    /// bar).
    pub fn render_palette(&mut self, ctx: &egui::Context) {
        let bound = self.palette_actions();
        // A view asked for an action's picker (e.g. Resume once sessions loaded).
        for kind in BufferKind::ALL {
            // (Only one that's offered there now: none of them, nothing opens.)
            if let Some(id) = self.desk.view_mut(kind).take_palette_request()
                && let Some(action) = bound.iter().find(|b| b.action.id == id && b.action.when != crate::actions::When::Unavailable)
            {
                self.open_palette(Some(action));
            }
        }
        let picked = self.palette.render(ctx, &bound, &self.config.keymap);
        // A trigger in what's written there: its picker, what it picks put after it.
        if let Some(pick) = self.palette.take_trigger().and_then(|c| self.triggered(c)) {
            self.palette.hold_for(&pick);
        }
        if let Some(ask) = self.palette.take_search() {
            match ask.origin {
                Origin::View(kind) => self.desk.view_mut(kind).search(&ask.action, &ask.query),
                Origin::Shell => {
                    self.desk.shell_searches.insert(ask.action, ask.query);
                }
            }
        }
        if let Some(invocation) = picked {
            // Still open: the picker changed what it acts on (the cycle key).
            if !self.palette.is_open() {
                self.input_mode = InputMode::Normal;
                release_keyboard(ctx);
            }
            self.invoke(invocation, ctx);
        }
        // Closed without a pick: back to typing, when that's where it was;
        // a step-by-step start or edit left there is gone.
        if self.input_mode == InputMode::Palette && !self.palette.is_open() {
            (self.desk.discussing, self.desk.tag_editing) = (None, None);
            self.desk.discussions_here.clear();
            self.input_mode = match self.palette_from {
                typing @ InputMode::Insert { .. } => typing,
                _ => {
                    release_keyboard(ctx);
                    InputMode::Normal
                }
            };
        }
    }
}
