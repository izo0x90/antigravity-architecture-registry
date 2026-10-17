//! Keys: the input modes and what each key does.

use super::*;

/// Whether one of our text fields has the keyboard (as opposed to a button
/// or nothing): only then do keys go to the widget instead of the keymap.
pub(crate) fn text_input_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.focused()).is_some_and(|id| egui::text_edit::TextEditState::load(ctx, id).is_some())
}

/// The palette's field (or whatever had the keyboard) lets go of it.
pub(super) fn release_keyboard(ctx: &egui::Context) {
    if let Some(id) = ctx.memory(|m| m.focused()) {
        ctx.memory_mut(|m| m.surrender_focus(id));
    }
}

impl CockpitApp {
    /// Applies one leader key; returns true when the leader menu is done.
    pub(super) fn leader_key(&mut self, key: char, ctx: &egui::Context) -> bool {
        let bound = self.bound_actions();
        let entry = leader_entries(&bound, &self.leader_path).into_iter().find(|e| e.key() == key);
        match entry {
            Some(LeaderEntry::Group { key }) => {
                self.leader_path.push(key);
                false
            }
            Some(LeaderEntry::Action { index, .. }) => {
                self.leader_path.clear();
                self.input_mode = InputMode::Normal;
                self.run_bound(&bound[index], None, ctx);
                true
            }
            None => false,
        }
    }

    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
        let keymap = &self.config.keymap;
        let (leader_pressed, leader_released, dragged, back) = ctx.input(|i| {
            let dragged = i.pointer.primary_down() && (i.pointer.delta().length_sq() > 2.0 || i.pointer.is_decidedly_dragging());
            (keymap.leader.pressed(i), keymap.leader.released(i), dragged, keymap.back.pressed(i))
        });
        if leader_pressed {
            self.leader_tap.press();
        }
        if dragged {
            self.leader_tap.drag();
        }
        let leader_tapped = leader_released && self.leader_tap.release();

        match self.input_mode {
            // A field that takes every key: only the leave keys leave it
            // (a sequence's first key held until the next, or the wait).
            InputMode::Insert { at } if self.field_at(at) == crate::keymap::Field::Keys => {
                let keys = &self.config.keymap.input;
                let typing = ctx.input_mut(|i| self.leaving.filter(i, keys));
                if typing == crate::keymap::Typing::Leaves {
                    self.input_mode = InputMode::Normal;
                    release_keyboard(ctx);
                } else if let Some(wait) = self.leaving.waiting(keys, ctx.input(|i| i.time)) {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
                }
            }
            InputMode::Insert { .. } => {
                if back {
                    self.input_mode = InputMode::Normal;
                    release_keyboard(ctx);
                }
                // Everything else flows into the text field.
            }

            // The palette reads its own keys while open.
            InputMode::Palette => {}

            // The shell reads typed keys (search, hint labels, marks).
            InputMode::Capture { .. } => {
                // Text as typed, its own erasing ('\u{8}'), then the send key
                // ('\n': the prompt's done) as configured.
                let typed: Vec<char> = ctx.input(|i| {
                    let mut typed: Vec<char> = i
                        .events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::Text(t) => Some(t.chars().collect::<Vec<_>>()),
                            egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => Some(vec!['\u{8}']),
                            _ => None,
                        })
                        .flatten()
                        .collect();
                    if keymap.input.sent(i).is_some() {
                        typed.push('\n');
                    }
                    typed
                });
                if typed.is_empty() && !back {
                    if self.capture.is_none() {
                        self.input_mode = InputMode::Normal;
                    }
                    return;
                }
                if !self.on_capture(&typed, back, ctx) && matches!(self.input_mode, InputMode::Capture { .. }) {
                    self.input_mode = InputMode::Normal;
                }
            }

            InputMode::Leader => {
                if leader_tapped {
                    self.leader_path.clear();
                    self.input_mode = InputMode::Normal;
                    return;
                }
                if back {
                    if self.leader_path.pop().is_none() {
                        self.input_mode = InputMode::Normal;
                    }
                    return;
                }
                let typed: Vec<char> = ctx.input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::Text(t) => Some(t.chars().collect::<Vec<_>>()),
                            _ => None,
                        })
                        .flatten()
                        .filter(|c| !c.is_whitespace())
                        .collect()
                });
                for key in typed {
                    if self.leader_key(key, ctx) {
                        break;
                    }
                }
            }

            // A focused text field (e.g. a search box) gets its keys; the key
            // sheet reads its own (Escape closes it and nothing else).
            InputMode::Normal if text_input_focused(ctx) || self.key_sheet.is_some() => {}

            InputMode::Normal => {
                // Anything else with keyboard focus (a button reached by Tab,
                // a field that just closed) gives it back: keys are commands.
                release_keyboard(ctx);
                if leader_tapped {
                    self.leader_path.clear();
                    self.input_mode = InputMode::Leader;
                    return;
                }
                if back && (!self.pending_keys.is_empty() || self.pending_count.is_some()) {
                    self.pending_keys.clear();
                    self.pending_count = None;
                    return;
                }
                if back && self.quick_look.is_some() {
                    self.close_quick_look();
                    return;
                }
                // Counts: digits before a key, in every view.
                if self.pending_keys.is_empty() {
                    let digits: Vec<u32> = ctx.input(|i| {
                        i.events
                            .iter()
                            .filter_map(|e| match e {
                                egui::Event::Text(t) if t.chars().count() == 1 => t.chars().next().and_then(|c| c.to_digit(10)),
                                _ => None,
                            })
                            .collect()
                    });
                    if let Some(&d) = digits.first()
                        && (d != 0 || self.pending_count.is_some())
                    {
                        self.pending_count = Some((self.pending_count.unwrap_or(0) * 10 + d).min(MAX_COUNT));
                        return;
                    }
                }
                let bound = self.bound_actions();
                match ctx.input(|i| match_sequence(&bound, &self.pending_keys, i)) {
                    SeqMatch::Run(index) => {
                        self.pending_keys.clear();
                        self.note = None;
                        let count = self.pending_count.take();
                        let b = bound[index].clone();
                        self.run_bound(&b, count, ctx);
                    }
                    SeqMatch::Pending(chord) => {
                        if self.pending_keys.is_empty() {
                            self.pending_since = ctx.input(|i| i.time);
                        }
                        self.pending_keys.push(chord);
                        ctx.request_repaint_after(std::time::Duration::from_millis(self.config.keymap.which_key_delay_ms));
                    }
                    SeqMatch::None => {
                        // Any other key typed abandons a half-typed sequence (and its count).
                        if (!self.pending_keys.is_empty() || self.pending_count.is_some())
                            && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_) | egui::Event::Key { pressed: true, .. })))
                        {
                            self.pending_keys.clear();
                            self.pending_count = None;
                        }
                    }
                }
            }
        }
    }
}
