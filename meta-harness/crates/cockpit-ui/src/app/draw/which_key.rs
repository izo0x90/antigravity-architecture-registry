//! The which-key menu.

use super::super::*;

impl CockpitApp {
    /// A leader group's name, where `kind` is the view: the view's own
    /// menu is "Graph actions"; the rest are named in config (else their
    /// key).
    pub(in crate::app) fn group_name(&self, path: &[char], kind: BufferKind) -> String {
        if path == [self.config.keymap.view_menu] {
            return format!("{} actions", kind.words());
        }
        self.config.keymap.group_label(path).map_or_else(|| path.last().map(char::to_string).unwrap_or_default(), str::to_owned)
    }

    /// Where the menu is, in words: the leader, then each group gone into.
    pub(in crate::app) fn menu_path(&self) -> Vec<String> {
        let kind = self.focused_kind();
        std::iter::once(self.config.keymap.leader.label())
            .chain((1..=self.leader_path.len()).map(|depth| self.group_name(&self.leader_path[..depth], kind)))
            .collect()
    }

    /// The groups a panel's top line offers where `here` is (`bound`: what's
    /// offered there): the leader's top-level groups with something about
    /// where it is (what you're on, its view, its panel) that applies there
    /// now. Every group, these and those whose actions all act everywhere,
    /// is in the menu the top bar's Menus button opens. Each its name, the
    /// keys that open it, what it is.
    pub(in crate::app) fn groups(&self, bound: &[Bound], here: Here) -> Vec<crate::hosted::GroupButton> {
        // Each group's closest layer, and whether something in it applies.
        let mut seen: Vec<(char, Layer, bool)> = Vec::new();
        for b in bound {
            let Some(&[key, _, ..]) = b.leader.as_deref() else { continue };
            let applies = self.applies(b, here);
            match seen.iter_mut().find(|(k, ..)| *k == key) {
                Some((_, layer, any)) => (*layer, *any) = ((*layer).min(b.layer), *any || applies),
                None => seen.push((key, b.layer, applies)),
            }
        }
        seen.retain(|(_, layer, applies)| *applies && *layer < Layer::Everywhere);
        seen.sort_by_key(|(key, layer, _)| (*layer, *key));
        seen.into_iter()
            .map(|(key, ..)| {
                let name = self.group_name(&[key], here.kind);
                let keys = crate::keymap::local().leader_hint(&[key]);
                crate::hosted::GroupButton {
                    key,
                    tip: format!("{name} ({keys})\nIts menu: what it holds about where it is"),
                    keys,
                    name,
                    asks: (MENU.to_owned(), Some(key.to_string())),
                }
            })
            .collect()
    }

    /// The leader's menus, by their key: the view's own, then each group.
    pub(in crate::app) fn menu_keys(&self) -> Vec<char> {
        let groups = self.config.keymap.groups.keys().filter_map(|k| {
            let mut chars = k.chars();
            chars.next().filter(|_| chars.next().is_none())
        });
        std::iter::once(self.config.keymap.view_menu).chain(groups).collect()
    }

    /// The top bar's button for every menu: the leader's own (its root).
    pub(in crate::app) fn every_menu(&self) -> crate::hosted::GroupButton {
        let keys = self.config.keymap.leader.label();
        crate::hosted::GroupButton {
            key: ' ',
            name: "Menus".into(),
            tip: format!("Menus ({keys})\nEvery menu, as the leader opens them"),
            keys,
            asks: (MENU.to_owned(), Some(String::new())),
        }
    }

    /// The which-key menu, opened (the leader's key, or a group's button):
    /// what else had the keys lets go (the action list, a search being
    /// typed, labels, a mark), then the menu is at its top, and `path` is
    /// gone into as its keys would.
    pub(in crate::app) fn open_menu(&mut self, path: &[char], ctx: &egui::Context) {
        // Each leaves as its way back out leaves it (a search put back,
        // the list's half-done start dropped).
        if self.palette.is_open() {
            self.palette.close();
            self.palette_closed(ctx);
        }
        self.end_capture(ctx);
        self.menus_from(self.input_mode);
        (self.leader_path, self.input_mode) = (Vec::new(), InputMode::Leader);
        for key in path {
            self.leader_key(*key, ctx);
        }
    }

    /// Which-key HUD, generated from the actions available at the current
    /// path: the focused view's first (marked), then the rest. Every entry is
    /// also tappable.
    pub fn render_which_key_hud(&mut self, ctx: &egui::Context) {
        /// Each entry's width.
        const ENTRY_W: f32 = 200.0;
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
        let path: Vec<(usize, String)> = self.menu_path().into_iter().enumerate().collect();
        let mut back_to: Option<usize> = None;

        let hud = egui::Area::new(egui::Id::new("which_key_floating_hud"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0))
            .order(egui::Order::Foreground)
            .constrain(true)
            .show(ctx, |ui| {
                // Its width from its first pass (its measuring one): anchored by
                // its middle, it's where it stays from the first frame shown.
                ui.set_width(hud_w);
                Frame::NONE
                    .fill(colors().overlay)
                    .stroke(Stroke::new(crate::theme::look().line_thin, colors().focus))
                    .corner_radius(crate::theme::look().radius_xl)
                    .inner_margin(crate::theme::margin2(crate::theme::look().space_xl, crate::theme::look().space_sm))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (depth, part) in &path {
                                let here = *depth == self.leader_path.len();
                                let text = RichText::new(format!("{part} \u{203a}"))
                                    .strong()
                                    .size(crate::theme::look().text_body)
                                    .color(if here { colors().info } else { colors().muted });
                                if ui.add(egui::Button::new(text).frame(false)).clicked() && !here {
                                    back_to = Some(*depth);
                                }
                            }
                            let esc = egui::Button::new(
                                RichText::new(self.config.keymap.back.label())
                                    .color(colors().key)
                                    .monospace()
                                    .size(crate::theme::look().text_body),
                            )
                            .corner_radius(crate::theme::look().radius_m);
                            if ui.add(esc).clicked() {
                                back = true;
                            }
                        });
                        ui.separator();
                        // The focused view's entries first, under its name; then the rest.
                        let (own, rest): (Vec<&LeaderEntry>, Vec<&LeaderEntry>) =
                            entries.iter().partition(|e| is_primary(e, &bound, &self.leader_path));
                        // Explicit rows sized to the screen (wrapping layouts ignore Area widths).
                        let per_row = ((hud_w - 28.0) / (ENTRY_W + ui.spacing().item_spacing.x)).floor().max(1.0) as usize;
                        let fingers = if self.fingers { crate::actions::Fingers::Yes } else { crate::actions::Fingers::No };
                        let view_name = self.focused_kind().words();
                        let blocks = [(Some(view_name.as_str()), own), (None, rest)];
                        for (heading, block) in blocks.iter().filter(|(_, b)| !b.is_empty()) {
                            match heading {
                                Some(name) => {
                                    ui.label(
                                        RichText::new(*name)
                                            .monospace()
                                            .strong()
                                            .size(crate::theme::look().text_small)
                                            .color(colors().focus),
                                    );
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
                                        let mut path = self.leader_path.clone();
                                        path.push(entry.key());
                                        let (key, label, detail) = match entry {
                                            LeaderEntry::Group { key } => {
                                                (*key, format!("{}…", self.group_name(&path, self.focused_kind())), "Its actions")
                                            }
                                            LeaderEntry::Action { key, index } => {
                                                (*key, bound[*index].action.label.clone(), bound[*index].action.detail.as_str())
                                            }
                                        };
                                        // What it does and its keys (hover, or a finger held on it).
                                        let tip = format!("{label} ({})\n{detail}", crate::keymap::local().leader_hint(&path));
                                        // One thing to tap, its whole cell (the same size in
                                        // every pass: the menu doesn't move once shown).
                                        let emphasis =
                                            if primary { crate::actions::Emphasis::Own } else { crate::actions::Emphasis::Plain };
                                        if crate::actions::entry(ui, &key.to_string(), &label, emphasis, ENTRY_W, fingers, &tip).clicked() {
                                            tapped = Some(key);
                                        }
                                    }
                                });
                            }
                        }
                    });
            });
        self.overlays.drawn_at(crate::overlay::Overlay::Menu, hud.response.rect);

        if let Some(depth) = back_to {
            self.leader_path.truncate(depth);
        } else if let Some(key) = tapped {
            self.leader_key(key, ctx);
        } else if back && self.leader_path.pop().is_none() {
            self.leave_menu();
        }
    }
}
