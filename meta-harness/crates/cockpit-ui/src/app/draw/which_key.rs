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

    /// The groups a host's top line offers where `here` is (`bound`: what's
    /// shown there): the leader's top-level groups about where it is (not
    /// only everywhere's), with an action that applies there now; each its
    /// key, its name (the view's own menu: "Graph actions"), what's in it.
    pub(in crate::app) fn groups_of(&self, bound: &[Bound], here: Here) -> Vec<crate::hosted::GroupButton> {
        let situation = self.situation_of(here.kind);
        leader_entries(bound, &[])
            .into_iter()
            .filter(|e| is_primary(e, bound, &[]))
            .filter_map(|e| match e {
                LeaderEntry::Group { key } => Some(key),
                LeaderEntry::Action { .. } => None,
            })
            .filter(|key| bound.iter().any(|b| b.leader.as_ref().is_some_and(|l| l.first() == Some(key)) && b.action.when.holds(&situation)))
            .map(|key| {
                let name = if key == self.config.keymap.view_menu {
                    let title = here.kind.title();
                    format!("{}{} actions", &title[..1], title[1..].to_lowercase())
                } else {
                    self.group_label(&[key]).unwrap_or_else(|| key.to_string())
                };
                let tip = format!("{name}: its actions ({})", crate::keymap::local().leader_hint(&[key]));
                crate::hosted::GroupButton { key, name, tip }
            })
            .collect()
    }

    /// The which-key menu, open at `path` (a group's button tapped: as its
    /// keys after the leader would).
    pub(in crate::app) fn open_leader_at(&mut self, path: Vec<char>) {
        self.leader_path = path;
        self.input_mode = InputMode::Leader;
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
        let entries = leader_entries(&bound, &self.leader_path);
        let screen_rect = ctx.content_rect();
        let hud_w = (screen_rect.width() - 32.0).min(760.0);
        let mut tapped: Option<char> = None;
        let mut back = false;
        // The path so far, each part tapped to go back to it.
        let mut path = vec![(0, self.config.keymap.leader.label())];
        path.extend((1..=self.leader_path.len()).map(|depth| (depth, self.group_label(&self.leader_path[..depth]).unwrap_or_else(|| self.leader_path[depth - 1].to_string()))));
        let mut back_to: Option<usize> = None;

        egui::Area::new(egui::Id::new("which_key_floating_hud")).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0)).order(egui::Order::Foreground).constrain(true).show(ctx, |ui| {
            ui.set_max_width(hud_w);
            Frame::NONE.fill(colors().overlay).stroke(Stroke::new(crate::theme::look().line_thin, colors().focus)).corner_radius(crate::theme::look().radius_xl).inner_margin(crate::theme::margin2(crate::theme::look().space_xl, crate::theme::look().space_sm)).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (depth, part) in &path {
                        let here = *depth == self.leader_path.len();
                        let text = RichText::new(format!("{part} \u{203a}")).strong().size(crate::theme::look().text_body).color(if here { colors().info } else { colors().muted });
                        if ui.add(egui::Button::new(text).frame(false)).clicked() && !here {
                            back_to = Some(*depth);
                        }
                    }
                    let esc = egui::Button::new(RichText::new(self.config.keymap.back.label()).color(colors().key).monospace().size(crate::theme::look().text_body)).corner_radius(crate::theme::look().radius_m);
                    if ui.add(esc).clicked() {
                        back = true;
                    }
                });
                ui.separator();
                // The focused view's entries first, under its name; then the rest.
                let (own, rest): (Vec<&LeaderEntry>, Vec<&LeaderEntry>) = entries.iter().partition(|e| is_primary(e, &bound, &self.leader_path));
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
                            let (key, label, detail) = match entry {
                                LeaderEntry::Group { key } => {
                                    let mut path = self.leader_path.clone();
                                    path.push(*key);
                                    (*key, format!("{}…", self.group_label(&path).unwrap_or_else(|| "more".to_owned())), None)
                                }
                                LeaderEntry::Action { key, index } => (*key, bound[*index].action.label.clone(), Some(bound[*index].action.detail.as_str())),
                            };
                            ui.allocate_ui(egui::vec2(200.0, 22.0), |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    // A bright pill takes the text meant for bright fills.
                                    let (fill, ink) = if primary { (colors().focus, colors().on_bright) } else { (colors().overlay_raised, colors().key) };
                                    let btn = egui::Button::new(RichText::new(key.to_string()).color(ink).monospace().strong().size(crate::theme::look().text_body)).fill(fill).corner_radius(crate::theme::look().radius_m);
                                    let tip = detail.map_or_else(|| label.clone(), |d| format!("{label}\n{d}"));
                                    if crate::actions::tip_on(ui.add(btn), &tip).clicked() {
                                        tapped = Some(key);
                                    }
                                    let color = if primary { colors().text_strong } else { colors().text };
                                    let label = ui.add(egui::Label::new(RichText::new(label).size(crate::theme::look().text_body).color(color)).truncate());
                                    if let Some(detail) = detail {
                                        label.on_hover_text(detail);
                                    }
                                });
                            });
                        }
                    });
                }
                }
            });
        });

        if let Some(depth) = back_to {
            self.leader_path.truncate(depth);
        } else if let Some(key) = tapped {
            self.leader_key(key, ctx);
        } else if back && self.leader_path.pop().is_none() {
            self.input_mode = InputMode::Normal;
        }
    }
}
