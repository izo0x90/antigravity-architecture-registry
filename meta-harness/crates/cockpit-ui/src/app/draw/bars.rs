//! The key bar, the prefix popup, and what the action list needs of where
//! you are.

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
        let here = self.here();
        let own = Origin::View(here.kind);
        let mut keys: Vec<(usize, &Bound)> = bound
            .iter()
            .enumerate()
            .filter(|(_, b)| b.layer < Layer::Everywhere && !b.key_hint().is_empty() && self.applies(b, here))
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
            let here = self.here();
            let mut next = continuations(&bound, &self.pending_keys);
            next.retain(|(b, _)| self.applies(b, here));
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

    /// What the action list needs of where you are: which actions apply
    /// (another view's own are judged there: they apply unless it says
    /// not), the layers' titles, and the keys that run nothing themselves
    /// (the mode's it was opened from, the pointer's and fingers').
    pub(in crate::app) fn sheet_of(&self, bound: &[Bound]) -> crate::sheet::Sheet {
        let here = self.here();
        let applies = bound.iter().map(|b| self.applies(b, here)).collect();
        let host = match here.at {
            crate::keymap::Host::Look => "Quick look".to_owned(),
            crate::keymap::Host::Panel(panel) => format!("Panel {}", panel + 1),
        };
        let layers = ["On what you're on".to_owned(), self.desk.view(here.kind).title().text, host, "Everywhere".to_owned()];
        let mut informs = Vec::new();
        if let Some((name, keys)) = &self.sheet_mode {
            informs.push((format!("{name}: its keys"), keys.iter().map(|(k, w)| (k.clone(), (*w).to_owned())).collect()));
        }
        let go_to = format!("{}+click", crate::keymap::local().pointer.go_to.label());
        let pointer = [("click / tap", "put the cursor on an item"), ("double click / tap", "open it"), ("long press", "what can be done with it (the palette)"), (go_to.as_str(), "go to the name under it (code)")];
        informs.push(("Pointer and touch".to_owned(), pointer.iter().map(|(k, w)| ((*k).to_owned(), (*w).to_owned())).collect()));
        crate::sheet::Sheet { applies, layers, informs }
    }
}
