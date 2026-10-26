//! The key bar, the prefix popup, and what the action list needs of where
//! you are.

use super::super::*;

/// The widest a key bar chip is, of the bar's width: a longer one (a
/// request's title can be a whole command) is cut to it, whole on hover;
/// the rest are never cut.
const CHIP_SHARE: f32 = 1.0 / 3.0;

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

    /// While `mode` has the keys (typing, a search, labels, a mark): its
    /// name and its keys, as configured, each with what it does. (The
    /// action list says its own.)
    pub(in crate::app) fn mode_keys(&self, mode: InputMode) -> Option<(&'static str, Vec<(String, &'static str)>)> {
        let local = crate::keymap::local();
        let back = local.back.label();
        match mode {
            InputMode::Capture { .. } => match &self.capture {
                Some(Capture::Search(_)) => {
                    Some(("Searching", vec![(local.input.label(crate::keymap::Sent::Send), "jumps to it"), (back, "gives up")]))
                }
                Some(Capture::Hint(_)) => Some(("Labels", vec![("a label".into(), "goes there"), (back, "gives up")])),
                Some(Capture::MarkSet | Capture::MarkJump) => Some(("Marks", vec![("a letter".into(), "the mark"), (back, "gives up")])),
                None => None,
            },
            InputMode::Insert { at } if self.field_at(at) == crate::keymap::Field::Keys => Some(("Typing", local.input.leave_keys())),
            InputMode::Insert { .. } => Some(("Typing", local.input.keys(&local.back, "leaves the field"))),
            InputMode::Choosing { .. } => Some(("A list", local.list.keys(&local.back, "closes"))),
            InputMode::Normal | InputMode::Leader | InputMode::Palette => None,
        }
    }

    /// The bar at the bottom: what's being typed (a search, a hint, a mark),
    /// notes, and the keys that matter now (tappable).
    pub(in crate::app) fn render_key_bar(&mut self, ctx: &egui::Context) {
        let bound = self.bound_actions();
        let narrow = ctx.content_rect().width() < self.config.keymap.bar.narrow_below;
        let mut run: Option<Bound> = None;
        let status: Option<(String, Color32)> = match &self.capture {
            Some(Capture::Hint(h)) => Some((
                format!("hint: type a label{}", if h.typed.is_empty() { String::new() } else { format!(" ({})", h.typed) }),
                colors().key,
            )),
            Some(Capture::MarkSet) => Some(("mark: press a letter".into(), colors().key)),
            Some(Capture::MarkJump) => Some(("jump to mark: press its letter".into(), colors().key)),
            _ => self.search_status(ctx).map(|(text, typing)| (text, if typing { colors().key } else { colors().muted })),
        };
        let show_keys = self.config.keymap.bar.show;
        if !show_keys && status.is_none() && self.note.is_none() {
            return;
        }
        let bar = egui::TopBottomPanel::bottom("key_bar").show(ctx, |ui| {
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
        // (Its keys act on what's on top: a tap on it is inside what's over
        // the panels, whatever that is.)
        self.overlays.drawn_across(bar.response.rect);
        if let Some(b) = run {
            self.run_bound(&b, None, ctx);
        }
    }

    /// The keys that matter now (while a mode has the keys, its keys; the
    /// action list's, as it says them), tappable; the one tapped in `run`.
    fn draw_bar_keys(&self, ui: &mut egui::Ui, bound: &[Bound], narrow: bool, run: &mut Option<Bound>) {
        let said =
            |ui: &mut egui::Ui, text: String| ui.label(RichText::new(text).size(crate::theme::look().text_body).color(colors().muted));
        if self.input_mode == InputMode::Palette {
            let actions = self.palette_actions();
            for keys in
                self.palette.keys_said(&self.config.keymap, |origin, id| actions.iter().find(|b| b.origin == origin && b.action.id == id))
            {
                said(ui, keys);
            }
            return;
        }
        if let Some((_, typing)) = self.mode_keys(self.input_mode) {
            for (keys, what) in typing {
                said(ui, format!("{keys} {what}"));
            }
            return;
        }
        let widest = ui.max_rect().width() * CHIP_SHARE;
        for b in self.bar_keys(bound) {
            let size = if narrow { crate::theme::look().text_title } else { crate::theme::look().text_body };
            let words = format!("{} {}", b.key_hint(), b.action.label);
            let mut button = egui::Button::new(RichText::new(&words).size(size)).corner_radius(crate::theme::look().radius_m);
            if narrow {
                button = button.min_size(egui::vec2(32.0, 32.0));
            }
            // Too wide for the bar's share: cut to it (whole on hover).
            let wide = ui.painter().layout_no_wrap(words, egui::FontId::proportional(size), colors().text).size().x
                + 2.0 * ui.spacing().button_padding.x;
            let clicked = match wide > widest {
                true => {
                    ui.add_sized(egui::vec2(widest, ui.spacing().interact_size.y.max(if narrow { 32.0 } else { 0.0 })), button.truncate())
                }
                false => ui.add(button),
            };
            if clicked.on_hover_text(&b.action.label).clicked() {
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
        // Each what follows: its keys, its words, what a tap does (runs
        // the action; goes to the mark).
        let rows: Vec<(String, String, Option<Followed>)> = if waiting {
            let bound = self.bound_actions();
            let here = self.here();
            let mut next = continuations(&bound, &self.pending_keys);
            next.retain(|(b, _)| self.applies(b, here));
            // The closest to where you are first.
            next.sort_by_key(|(b, _)| b.layer);
            next.into_iter()
                .map(|(b, rest)| {
                    (
                        rest.iter().map(Chord::label).collect::<Vec<_>>().join(" "),
                        b.action.label.clone(),
                        Some(Followed::Run(b.action.id.clone())),
                    )
                })
                .collect()
        } else {
            let mut rows: Vec<(String, String, Option<Followed>)> = self
                .desk
                .marks
                .iter()
                .map(|(letter, jump)| (letter.to_string(), self.describe_jump(jump), Some(Followed::Mark(*letter))))
                .collect();
            if matches!(self.capture, Some(Capture::MarkSet)) {
                rows.insert(0, ("a–z".into(), "set a mark here (replaces one with that letter)".into(), None));
            }
            rows
        };
        let fingers = if self.fingers { crate::actions::Fingers::Yes } else { crate::actions::Fingers::No };
        let mut tapped = None;
        let title =
            if waiting { self.pending_keys.iter().map(Chord::label).collect::<Vec<_>>().join(" ") + " …" } else { "marks".into() };
        let popup = egui::Area::new(egui::Id::new("prefix_popup"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0))
            .order(egui::Order::Foreground)
            .constrain(true)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(colors().overlay)
                    .stroke(Stroke::new(crate::theme::look().line_thin, colors().focus))
                    .corner_radius(crate::theme::look().radius_xl)
                    .inner_margin(crate::theme::margin2(crate::theme::look().space_l, crate::theme::look().space_m))
                    .show(ui, |ui| {
                        ui.label(RichText::new(title).monospace().strong().size(crate::theme::look().text_body).color(colors().info));
                        if rows.is_empty() {
                            ui.label(RichText::new("nothing applies here").size(crate::theme::look().text_body).color(colors().muted));
                        }
                        let width = (ctx.content_rect().width() - 64.0).min(420.0);
                        for (keys, label, followed) in rows {
                            if crate::actions::entry(ui, &keys, &label, crate::actions::Emphasis::Plain, width, fingers, &label).clicked() {
                                tapped = followed;
                            }
                        }
                        ui.label(
                            RichText::new(format!("{} to cancel", self.config.keymap.back.label()))
                                .size(crate::theme::look().text_small)
                                .color(colors().muted),
                        );
                    });
            });
        if waiting {
            self.overlays.drawn_at(crate::overlay::Overlay::Keys, popup.response.rect);
        }
        match tapped {
            Some(Followed::Run(id)) => {
                (self.pending_keys, self.pending_count) = (Vec::new(), None);
                self.run_by_id(&id, None, ctx);
            }
            Some(Followed::Mark(letter)) => {
                self.on_capture(&[letter], false, ctx);
            }
            None => {}
        }
    }

    /// What the action list lists (`bound`), each judged where you are
    /// (as its key is), and what it needs of where you are: the titles of
    /// what holds you, the keys that run nothing themselves (the mode's it
    /// was opened from, the pointer's and fingers'), the leader's groups'
    /// names, and going to a line.
    pub(in crate::app) fn sheet_of(&self, bound: Vec<Bound>) -> (Vec<crate::actions::Offered>, crate::sheet::Sheet) {
        use crate::actions::Offered;
        use crate::sheet::{Inform, Informs, Sheet};
        #[cfg(test)]
        self.listed.set(self.listed.get() + 1);
        let here = self.here();
        let offered: Vec<Offered> = bound.into_iter().map(|bound| Offered { applies: self.applies(&bound, here), bound }).collect();
        let host = match here.at {
            crate::keymap::Host::Look => "The quick look".to_owned(),
            crate::keymap::Host::Panel(panel) => format!("This panel ({})", panel + 1),
        };
        let inform = |keys: String, does: &str| Inform { keys, does: does.to_owned() };
        let mut informs = Vec::new();
        if let Some((name, keys)) = self.mode_keys(self.palette_from) {
            informs.push(Informs { title: format!("{name}: its keys"), keys: keys.into_iter().map(|(k, does)| inform(k, does)).collect() });
        }
        let go_to = format!("{}+click", crate::keymap::local().pointer.go_to.label());
        let pointer = [
            ("click / tap".to_owned(), "put the cursor on an item"),
            ("double click / tap".to_owned(), "open it"),
            ("long press".to_owned(), "on an item, what can be done with it (the action list)"),
            ("long press / hover".to_owned(), "on a button, what it does and its keys"),
            ("tap a menu".to_owned(), "a panel's top line, or Menus: its actions, as the leader opens them"),
            (go_to, "go to the name under it (code)"),
        ];
        informs
            .push(Informs { title: "Pointer and touch".to_owned(), keys: pointer.into_iter().map(|(k, does)| inform(k, does)).collect() });
        let groups = self.menu_keys().into_iter().map(|key| (key, self.group_name(&[key], here.kind))).collect();
        let line = offered.iter().find(|o| o.bound.action.command::<ShellCommand>() == Some(&ShellCommand::Line)).map(|o| o.bound.clone());
        let sheet = Sheet { view: format!("This view ({})", here.kind.words()), host, informs, groups, line };
        (offered, sheet)
    }
}

/// What a tap on what can follow does.
enum Followed {
    /// Runs its action, by its id (the keys typed so far dropped).
    Run(String),
    /// Goes to (or sets) the mark of that letter.
    Mark(char),
}
