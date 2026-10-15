//! The which-key menu.

use super::super::*;

impl CockpitApp {
    /// A leader submenu's name: the focused view's own, or from config.
    pub(in crate::app) fn group_label(&self, path: &[char]) -> Option<String> {
        if path == [self.config.keymap.view_menu] {
            return Some(format!("All {} actions", self.focused_kind().name()));
        }
        self.config.keymap.group_label(path).map(str::to_owned)
    }

    /// "LEADER > SESSION" style title for the current leader path.
    pub(in crate::app) fn leader_title(&self, root: &str) -> String {
        let mut title = root.to_owned();
        for depth in 1..=self.leader_path.len() {
            let label = match self.group_label(&self.leader_path[..depth]) {
                Some(label) => label.to_uppercase(),
                None => self.leader_path[depth - 1].to_string(),
            };
            title.push_str(" > ");
            title.push_str(&label);
        }
        title
    }

    /// Which-key HUD, generated from the actions available at the current
    /// path: the focused view's first (marked), then the rest. Every entry is
    /// also tappable.
    pub fn render_which_key_hud(&mut self, ctx: &egui::Context) {
        if self.input_mode != InputMode::Leader {
            return;
        }
        let bound = self.bound_actions();
        let focused = Origin::View(self.focused_kind());
        let entries = leader_entries(&bound, &self.leader_path, focused);
        let screen_rect = ctx.content_rect();
        let hud_w = (screen_rect.width() - 32.0).min(760.0);
        let mut tapped: Option<char> = None;
        let mut back = false;

        egui::Area::new(egui::Id::new("which_key_floating_hud")).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0)).order(egui::Order::Foreground).constrain(true).show(ctx, |ui| {
            ui.set_max_width(hud_w);
            Frame::NONE.fill(colors().overlay).stroke(Stroke::new(crate::theme::look().line_thin, colors().focus)).corner_radius(crate::theme::look().radius_xl).inner_margin(crate::theme::margin2(crate::theme::look().space_xl, crate::theme::look().space_sm)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(colors().info, RichText::new(self.leader_title(&self.config.keymap.leader.label().to_uppercase())).strong().monospace().size(crate::theme::look().text_body));
                    let esc = egui::Button::new(RichText::new(self.config.keymap.back.label()).color(colors().key).monospace().size(crate::theme::look().text_body)).corner_radius(crate::theme::look().radius_m);
                    if ui.add(esc).clicked() {
                        back = true;
                    }
                });
                ui.separator();
                // The focused view's entries first, under its name; then the rest.
                let (own, rest): (Vec<&LeaderEntry>, Vec<&LeaderEntry>) = entries.iter().partition(|e| is_primary(e, &bound, &self.leader_path, focused));
                // Explicit rows sized to the screen (wrapping layouts ignore Area widths).
                let per_row = ((hud_w - 28.0) / 210.0).floor().max(1.0) as usize;
                let view_name = self.focused_kind().title();
                let blocks = [(Some(view_name), own), (None, rest)];
                for (heading, block) in blocks.iter().filter(|(_, b)| !b.is_empty()) {
                    match heading {
                        Some(name) => {
                            ui.label(RichText::new(*name).monospace().strong().size(crate::theme::look().text_small).color(colors().focus));
                        }
                        None if blocks[0].1.is_empty() => {}
                        None => {
                            ui.separator();
                        }
                    }
                for row in block.chunks(per_row) {
                    ui.horizontal(|ui| {
                        for entry in row {
                            let primary = heading.is_some();
                            let (key, label) = match entry {
                                LeaderEntry::Group { key } => {
                                    let mut path = self.leader_path.clone();
                                    path.push(*key);
                                    (*key, format!("{}…", self.group_label(&path).unwrap_or_else(|| "more".to_owned())))
                                }
                                LeaderEntry::Action { key, index } => (*key, bound[*index].action.label.clone()),
                            };
                            ui.allocate_ui(egui::vec2(200.0, 22.0), |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    // A bright pill takes the text meant for bright fills.
                                    let (fill, ink) = if primary { (colors().focus, colors().on_bright) } else { (colors().overlay_raised, colors().key) };
                                    let btn = egui::Button::new(RichText::new(key.to_string()).color(ink).monospace().strong().size(crate::theme::look().text_body)).fill(fill).corner_radius(crate::theme::look().radius_m);
                                    if ui.add(btn).clicked() {
                                        tapped = Some(key);
                                    }
                                    let color = if primary { colors().text_strong } else { colors().text };
                                    ui.add(egui::Label::new(RichText::new(label).size(crate::theme::look().text_body).color(color)).truncate());
                                });
                            });
                        }
                    });
                }
                }
            });
        });

        if let Some(key) = tapped {
            self.leader_key(key, ctx);
        } else if back && self.leader_path.pop().is_none() {
            self.input_mode = InputMode::Normal;
        }
    }
}
