//! The key bar, the prefix popup and the key sheet.

use super::super::*;

impl CockpitApp {
    /// A mark in words: its kind of buffer and where (as a buffer of that
    /// kind says its places).
    fn describe_jump(&self, jump: &Jump) -> String {
        format!("{} · {}", jump.kind.name(), self.desk.view(jump.kind).place_in_words(&jump.place))
    }

    /// The keys the key bar shows: what applies where you are (what you're
    /// on, this panel, what holds it), most important first; among equals,
    /// the closest, and in a layer the panel's own before the moves it
    /// shares.
    pub(crate) fn bar_keys<'a>(&self, bound: &'a [Bound]) -> Vec<&'a Bound> {
        let situation = self.situation();
        let own = Origin::View(self.here().kind);
        let mut keys: Vec<(usize, &Bound)> = bound
            .iter()
            .enumerate()
            .filter(|(_, b)| b.layer < Layer::Everywhere && !b.key_hint().is_empty() && b.action.when.holds(&situation))
            .collect();
        keys.sort_by_key(|(i, b)| (b.action.importance, b.layer, b.origin != own, b.action.group, *i));
        keys.into_iter().take(self.config.keymap.bar.max).map(|(_, b)| b).collect()
    }

    /// While a mode has the keys (typing, the palette, a search, labels, a
    /// mark): its name and its keys, as configured, each with what it does.
    pub(in crate::app) fn mode_keys(&self) -> Option<(&'static str, Vec<(String, &'static str)>)> {
        let local = crate::keymap::local();
        let back = local.back.label();
        match (&self.capture, self.input_mode) {
            (Some(Capture::Search(_)), _) => Some(("Searching", vec![(local.input.label(crate::keymap::Sent::Send), "jumps to it"), (back, "gives up")])),
            (Some(Capture::Hint(_)), _) => Some(("Labels", vec![("a label".into(), "goes there"), (back, "gives up")])),
            (Some(Capture::MarkSet | Capture::MarkJump), _) => Some(("Marks", vec![("a letter".into(), "the mark"), (back, "gives up")])),
            (None, InputMode::Insert { at }) if self.field_at(at) == crate::keymap::Field::Keys => Some(("Typing", local.input.leave_keys())),
            (None, InputMode::Insert { .. }) => Some(("Typing", local.input.keys(&local.back, "leaves the field"))),
            (None, InputMode::Palette) => Some(("The palette", local.list.keys(&local.back, "closes"))),
            (None, _) => None,
        }
    }

    /// The bar at the bottom: what's being typed (a search, a hint, a mark),
    /// notes, and the keys that matter now (tappable).
    pub(in crate::app) fn render_key_bar(&mut self, ctx: &egui::Context) {
        let bound = self.bound_actions();
        let narrow = ctx.content_rect().width() < self.config.keymap.bar.narrow_below;
        let mut run: Option<Bound> = None;
        let status: Option<(String, Color32)> = match &self.capture {
            Some(Capture::Hint(h)) => Some((format!("hint: type a label{}", if h.typed.is_empty() { String::new() } else { format!(" ({})", h.typed) }), colors().key)),
            Some(Capture::MarkSet) => Some(("mark: press a letter".into(), colors().key)),
            Some(Capture::MarkJump) => Some(("jump to mark: press its letter".into(), colors().key)),
            _ => self.search_status().map(|(text, typing)| (text, if typing { colors().key } else { colors().muted })),
        };
        let show_keys = self.config.keymap.bar.show;
        if !show_keys && status.is_none() && self.note.is_none() {
            return;
        }
        egui::TopBottomPanel::bottom("key_bar").show(ctx, |ui| {
            // The keys first: what's said after them (a search, a note)
            // grows and wraps after them and never moves them.
            ui.horizontal_wrapped(|ui| {
                if show_keys {
                    self.draw_bar_keys(ui, &bound, narrow, &mut run);
                }
                if let Some((text, color)) = &status {
                    ui.label(RichText::new(text).monospace().size(crate::theme::look().text_body).color(*color));
                }
                if let Some(note) = &self.note {
                    ui.label(RichText::new(note).size(crate::theme::look().text_body).color(colors().warn));
                }
            });
        });
        if let Some(b) = run {
            self.run_bound(&b, None, ctx);
        }
    }

    /// The keys that matter now (while a mode has the keys, its keys),
    /// tappable; the one tapped in `run`.
    fn draw_bar_keys(&self, ui: &mut egui::Ui, bound: &[Bound], narrow: bool, run: &mut Option<Bound>) {
        if let Some((_, typing)) = self.mode_keys() {
            for (keys, what) in typing {
                ui.label(RichText::new(format!("{keys} {what}")).size(crate::theme::look().text_body).color(colors().muted));
            }
            return;
        }
        for b in self.bar_keys(bound) {
            let keys = b.key_hint();
            let text = RichText::new(format!("{keys} {}", b.action.label)).size(if narrow { crate::theme::look().text_title } else { crate::theme::look().text_body });
            let mut button = egui::Button::new(text).corner_radius(crate::theme::look().radius_m);
            if narrow {
                button = button.min_size(egui::vec2(32.0, 32.0));
            }
            if ui.add(button).on_hover_text(&b.action.label).clicked() {
                *run = Some(b.clone());
            }
        }
    }

    /// After a pause on a half-typed sequence (or a mark key), what can
    /// follow, for what the cursor is on now.
    pub(in crate::app) fn render_prefix_popup(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let delay = self.config.keymap.which_key_delay_ms as f64 / 1000.0;
        let waiting = !self.pending_keys.is_empty() && self.input_mode == InputMode::Normal;
        let marking = matches!(self.capture, Some(Capture::MarkJump | Capture::MarkSet));
        if !waiting && !marking {
            return;
        }
        if marking && self.pending_since == 0.0 {
            self.pending_since = now;
        }
        if now - self.pending_since < delay {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay - (now - self.pending_since) + 0.01));
            return;
        }
        let rows: Vec<(String, String)> = if waiting {
            let bound = self.bound_actions();
            let situation = self.situation();
            let mut next = continuations(&bound, &self.pending_keys);
            next.retain(|(b, _)| b.action.when.holds(&situation));
            // The closest to where you are first.
            next.sort_by_key(|(b, _)| b.layer);
            next.into_iter().map(|(b, rest)| (rest.iter().map(Chord::label).collect::<Vec<_>>().join(" "), b.action.label.clone())).collect()
        } else {
            let mut rows: Vec<(String, String)> = self.desk.marks.iter().map(|(letter, jump)| (letter.to_string(), self.describe_jump(jump))).collect();
            if matches!(self.capture, Some(Capture::MarkSet)) {
                rows.insert(0, ("a–z".into(), "set a mark here (replaces one with that letter)".into()));
            }
            rows
        };
        let title = if waiting { self.pending_keys.iter().map(Chord::label).collect::<Vec<_>>().join(" ") + " …" } else { "marks".into() };
        egui::Area::new(egui::Id::new("prefix_popup")).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0)).order(egui::Order::Foreground).constrain(true).show(ctx, |ui| {
            Frame::NONE.fill(colors().overlay).stroke(Stroke::new(crate::theme::look().line_thin, colors().focus)).corner_radius(crate::theme::look().radius_xl).inner_margin(crate::theme::margin2(crate::theme::look().space_l, crate::theme::look().space_m)).show(ui, |ui| {
                ui.label(RichText::new(title).monospace().strong().size(crate::theme::look().text_body).color(colors().info));
                if rows.is_empty() {
                    ui.label(RichText::new("nothing applies here").size(crate::theme::look().text_body).color(colors().muted));
                }
                for (keys, label) in rows {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(keys).monospace().strong().size(crate::theme::look().text_body).color(colors().key));
                        ui.label(RichText::new(label).size(crate::theme::look().text_body).color(colors().text));
                    });
                }
                ui.label(RichText::new(format!("{} to cancel", self.config.keymap.back.label())).size(crate::theme::look().text_small).color(colors().muted));
            });
        });
    }

    /// Every key where you are (`?`): a mode's keys first while one has
    /// them; then a section per layer (what you're on, this panel, what
    /// holds it, everywhere), grouped inside; every way to run each (keys,
    /// leader, gesture); what doesn't apply now, greyed. Searchable.
    pub(in crate::app) fn render_key_sheet(&mut self, ctx: &egui::Context) {
        let Some(mut filter) = self.key_sheet.take() else { return };
        if ctx.input(|i| self.config.keymap.back.pressed(i)) {
            release_keyboard(ctx);
            return;
        }
        let bound = self.bound_actions();
        let situation = self.situation();
        let here = self.here();
        let rows: Vec<&Bound> = bound.iter().filter(|b| !b.ways().is_empty()).collect();
        let layer_title = |layer: Layer| -> String {
            match layer {
                Layer::On => "On what you're on".to_owned(),
                Layer::Panel => self.desk.view(here.kind).title().text,
                Layer::Host => match here.at {
                    crate::keymap::Host::Look => "Quick look".to_owned(),
                    crate::keymap::Host::Panel(panel) => format!("Panel {}", panel + 1),
                },
                Layer::Everywhere => "Everywhere".to_owned(),
            }
        };
        let mode = self.mode_keys();
        let mut open = true;
        egui::Window::new("Keys").id(egui::Id::new("key_sheet")).collapsible(false).resizable(true).default_width(560.0).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).open(&mut open).show(ctx, |ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut filter).hint_text("filter…").desired_width(f32::INFINITY));
            field.request_focus();
            let wanted = filter.trim().to_lowercase();
            let matches = |text: &str| wanted.is_empty() || text.to_lowercase().contains(&wanted);
            let look = crate::theme::look();
            let row = |ui: &mut egui::Ui, keys: &str, what: &str, applies: bool| {
                ui.label(RichText::new(keys).monospace().size(look.text_body).color(colors().key));
                ui.label(RichText::new(what).size(look.text_body).color(if applies { colors().text } else { colors().faint }));
                ui.end_row();
            };
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.6).show(ui, |ui| {
                let mut any = false;
                if let Some((name, keys)) = &mode {
                    let keys: Vec<&(String, &str)> = keys.iter().filter(|(k, w)| matches(&format!("{k} {w}"))).collect();
                    if !keys.is_empty() {
                        any = true;
                        ui.label(RichText::new(*name).strong().size(look.text_title).color(colors().accent));
                        egui::Grid::new("key_sheet_mode").num_columns(2).spacing(egui::vec2(16.0, 2.0)).show(ui, |ui| keys.iter().for_each(|(k, w)| row(ui, k, w, true)));
                        ui.add_space(look.space_m);
                    }
                }
                for layer in [Layer::On, Layer::Panel, Layer::Host, Layer::Everywhere] {
                    let here: Vec<&&Bound> = rows.iter().filter(|b| b.layer == layer && matches(&format!("{} {} {}", b.action.label, b.action.id, b.ways().join(" ")))).collect();
                    if here.is_empty() {
                        continue;
                    }
                    any = true;
                    ui.label(RichText::new(layer_title(layer)).strong().size(look.text_title).color(colors().accent));
                    for group in Group::ALL {
                        let mut in_group: Vec<&&&Bound> = here.iter().filter(|b| b.action.group == group).collect();
                        if in_group.is_empty() {
                            continue;
                        }
                        in_group.sort_by_key(|b| b.action.importance);
                        ui.label(RichText::new(group.title()).strong().size(look.text_label).color(colors().info));
                        egui::Grid::new(("key_sheet", layer, group as u8)).num_columns(2).spacing(egui::vec2(16.0, 2.0)).show(ui, |ui| {
                            for b in in_group {
                                row(ui, &b.ways().join(" · "), &b.action.label, b.action.when.holds(&situation));
                            }
                        });
                    }
                    ui.add_space(look.space_m);
                }
                // The pointer and fingers, the same in every view.
                let go_to = format!("{}+click", crate::keymap::local().pointer.go_to.label());
                let pointer = [("click / tap", "put the cursor on an item"), ("double click / tap", "open it"), ("long press", "what can be done with it (the palette)"), (go_to.as_str(), "go to the name under it (code)")];
                let pointer: Vec<&(&str, &str)> = pointer.iter().filter(|(k, w)| matches(&format!("{k} {w}"))).collect();
                if !pointer.is_empty() {
                    any = true;
                    ui.label(RichText::new("Pointer and touch").strong().size(look.text_title).color(colors().accent));
                    egui::Grid::new("key_sheet_pointer").num_columns(2).spacing(egui::vec2(16.0, 2.0)).show(ui, |ui| pointer.iter().for_each(|(k, w)| row(ui, k, w, true)));
                }
                if !any {
                    ui.label(RichText::new("No keys match.").size(look.text_body).color(colors().muted));
                }
            });
        });
        if open {
            self.key_sheet = Some(filter);
        } else {
            release_keyboard(ctx);
        }
    }
}
