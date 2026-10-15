//! The key bar, the prefix popup and the key sheet.

use super::super::*;

/// An action's keys as the key sheet lists them ("g d", "SPC g v").
pub(in crate::app) fn keys_of(b: &Bound) -> String {
    let mut all: Vec<String> = b.keys.iter().map(|seq| seq.iter().map(Chord::label).collect::<Vec<_>>().join(" ")).collect();
    if let Some(path) = &b.leader {
        all.push(format!("SPC {}", path.iter().map(char::to_string).collect::<Vec<_>>().join(" ")));
    }
    all.join(" / ")
}

/// A mark in words: its view and where.
pub(in crate::app) fn describe_jump(jump: &Jump) -> String {
    let short = |id: &ComponentId| id.as_str().rsplit([':', '.']).next().unwrap_or(id.as_str()).to_owned();
    let place = match &jump.place {
        Place::Chat { message, .. } => message.map_or("latest".to_owned(), |m| format!("message {}", m + 1)),
        Place::Code { file, line } => format!("{file}:{line}"),
        Place::Graph { selection: Some(key), .. } => short(&ComponentId::new(key.clone())),
        Place::Graph { selection: None, nav } => format!("{} view", nav.view.name().replace('_', " ")),
        Place::Tree { cursor } | Place::Queue { component: cursor } => cursor.as_ref().map_or_else(String::new, short),
        Place::Spec { component, .. } => component.as_ref().map_or_else(String::new, short),
    };
    format!("{} · {place}", jump.kind.name())
}

impl CockpitApp {
    /// The keys the key bar shows: the focused view's and the shared ones
    /// that apply to what the cursor is on, most important first.
    pub(crate) fn bar_keys<'a>(&self, bound: &'a [Bound]) -> Vec<&'a Bound> {
        let focused = Origin::View(self.focused_kind());
        let situation = self.situation();
        let shared = |b: &Bound| b.origin == Origin::Shell && ["nav.", "goto.", "quicklook.", "talk."].iter().any(|p| b.action.id.starts_with(p));
        let mut keys: Vec<(usize, &Bound)> = bound
            .iter()
            .enumerate()
            .filter(|(_, b)| (b.origin == focused || shared(b)) && !b.keys.is_empty() && b.action.when.holds(&situation))
            .collect();
        // Most important first; the view's own before the shared ones; then as listed.
        keys.sort_by_key(|(i, b)| (b.action.importance, b.origin != focused, b.action.group, *i));
        keys.into_iter().take(self.config.keymap.bar.max).map(|(_, b)| b).collect()
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
            ui.horizontal_wrapped(|ui| {
                if let Some((text, color)) = &status {
                    ui.label(RichText::new(text).monospace().size(crate::theme::look().text_body).color(*color));
                }
                if let Some(note) = &self.note {
                    ui.label(RichText::new(note).size(crate::theme::look().text_body).color(colors().warn));
                }
                if !show_keys {
                    return;
                }
                // While typing a search, a label or a mark, those keys are what matter.
                let typing: Option<&[(&str, &str)]> = match (&self.capture, self.input_mode) {
                    (Some(Capture::Search(_)), _) => Some(&[("Enter", "jump to it"), ("Backspace", "delete"), ("Escape", "give up")]),
                    (Some(Capture::Hint(_)), _) => Some(&[("a label", "go there"), ("Escape", "give up")]),
                    (Some(Capture::MarkSet | Capture::MarkJump), _) => Some(&[("a letter", "the mark"), ("Escape", "give up")]),
                    (None, InputMode::Insert { .. }) => Some(&[("Enter", "send"), ("Shift+Enter", "new line"), ("Escape", "leave the field")]),
                    (None, InputMode::Palette) => Some(&[("Enter", "run"), ("Up/Down", "choose"), ("Escape", "close")]),
                    (None, _) => None,
                };
                if let Some(typing) = typing {
                    for (keys, what) in typing {
                        ui.label(RichText::new(format!("{keys} {what}")).size(crate::theme::look().text_body).color(colors().muted));
                    }
                    return;
                }
                for b in self.bar_keys(&bound) {
                    let keys = b.keys.first().map(|seq| seq.iter().map(Chord::label).collect::<Vec<_>>().join(" ")).unwrap_or_default();
                    let text = RichText::new(format!("{keys} {}", b.action.label)).size(if narrow { crate::theme::look().text_title } else { crate::theme::look().text_body });
                    let mut button = egui::Button::new(text).corner_radius(crate::theme::look().radius_m);
                    if narrow {
                        button = button.min_size(egui::vec2(32.0, 32.0));
                    }
                    if ui.add(button).on_hover_text(&b.action.label).clicked() {
                        run = Some(b.clone());
                    }
                }
            });
        });
        if let Some(b) = run {
            self.run_bound(&b, None, ctx);
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
            let focused = Origin::View(self.focused_kind());
            continuations(&bound, &self.pending_keys)
                .into_iter()
                .filter(|(b, _)| b.action.when.holds(&situation) && (b.origin == focused || b.origin == Origin::Shell || b.action.scope == Scope::Global))
                .map(|(b, rest)| (rest.iter().map(Chord::label).collect::<Vec<_>>().join(" "), b.action.label.clone()))
                .collect()
        } else {
            let mut rows: Vec<(String, String)> = self.desk.marks.iter().map(|(letter, jump)| (letter.to_string(), describe_jump(jump))).collect();
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

    /// The focused view's keys (`?`), grouped and searchable.
    pub(in crate::app) fn render_key_sheet(&mut self, ctx: &egui::Context) {
        let Some(mut filter) = self.key_sheet.take() else { return };
        if ctx.input(|i| self.config.keymap.back.pressed(i)) {
            release_keyboard(ctx);
            return;
        }
        let bound = self.bound_actions();
        let focused = Origin::View(self.focused_kind());
        let situation = self.situation();
        let rows: Vec<&Bound> = bound.iter().filter(|b| (b.origin == focused || b.origin == Origin::Shell) && (!b.keys.is_empty() || b.leader.is_some())).collect();
        let mut open = true;
        let title = format!("{} keys", self.focused_kind().title());
        egui::Window::new(title).id(egui::Id::new("key_sheet")).collapsible(false).resizable(true).default_width(520.0).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).open(&mut open).show(ctx, |ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut filter).hint_text("filter…").desired_width(f32::INFINITY));
            field.request_focus();
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.6).show(ui, |ui| {
                let matches = |b: &&&Bound| filter.trim().is_empty() || format!("{} {} {}", b.action.label, b.action.id, keys_of(b)).to_lowercase().contains(&filter.trim().to_lowercase());
                if !rows.iter().any(|b| matches(&b)) {
                    ui.label(RichText::new("No keys match.").size(crate::theme::look().text_body).color(colors().muted));
                }
                for group in Group::ALL {
                    let mut in_group: Vec<&&Bound> = rows.iter().filter(|b| b.action.group == group).filter(matches).collect();
                    if in_group.is_empty() {
                        continue;
                    }
                    in_group.sort_by_key(|b| (b.origin != focused, b.action.importance));
                    ui.label(RichText::new(group.title()).strong().size(crate::theme::look().text_label).color(colors().info));
                    egui::Grid::new(("key_sheet", group as u8)).num_columns(2).spacing(egui::vec2(16.0, 2.0)).show(ui, |ui| {
                        for b in in_group {
                            let applies = b.action.when.holds(&situation);
                            ui.label(RichText::new(keys_of(b)).monospace().size(crate::theme::look().text_body).color(colors().key));
                            ui.label(RichText::new(&b.action.label).size(crate::theme::look().text_body).color(if applies { colors().text } else { colors().faint }));
                            ui.end_row();
                        }
                    });
                    ui.add_space(crate::theme::look().space_s);
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
