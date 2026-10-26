//! The top bar: brand, LEDs, the index light, the mode, panels.

use super::super::*;

/// Status LED that flickers on traffic, like a drive or router activity light.
#[derive(Default)]
pub struct ActivityLed {
    last_count: u64,
    last_activity: f64,
}

impl ActivityLed {
    pub(in crate::app) fn show(&mut self, ui: &mut egui::Ui, label: &str, color: Color32, count: u64, hover: &str) -> egui::Response {
        let now = ui.input(|i| i.time);
        if count != self.last_count {
            self.last_count = count;
            self.last_activity = now;
        }
        let active = now - self.last_activity < 0.25;
        let lit = !active || ((now * 16.0) as i64) % 2 == 0;
        if active {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            let fill = if lit { color } else { color.gamma_multiply(0.25) };
            ui.painter().circle_filled(rect.center(), 4.0, fill);
            if lit && color != colors().faint {
                ui.painter().circle_stroke(rect.center(), 5.5, Stroke::new(crate::theme::look().line_thin, color.gamma_multiply(0.35)));
            }
            ui.label(RichText::new(label).monospace().size(crate::theme::look().text_small).color(colors().muted));
        })
        .response
        .on_hover_text(hover)
    }
}

/// The status-vocabulary name for the index's state (one meaning everywhere).
pub fn index_state(index: &arch_graph::IndexStatus) -> &'static str {
    match index.state {
        arch_graph::IndexState::Indexing => "indexing",
        arch_graph::IndexState::Failed => "index_failed",
        arch_graph::IndexState::Idle if !index.stale.is_empty() => "stale",
        arch_graph::IndexState::Idle => "index_fresh",
    }
}

/// One language's server in words: whether it confirms links, and if not, why.
pub(in crate::app) fn server_line(s: &arch_graph::ServerStatus) -> String {
    let tool = match &s.version {
        Some(v) => format!("{} {v}", s.server),
        None => s.server.clone(),
    };
    let why = s.reason.as_deref().unwrap_or("no reason given");
    let dropped = match s.dropped_positions {
        0 => String::new(),
        n => format!(" ({n} places it gave couldn't be read)"),
    };
    // The count is shown in every state (a server that failed may have
    // answered before).
    let line = match s.state {
        arch_graph::ServerState::Ready => format!("{}: {tool} confirms links", s.language),
        arch_graph::ServerState::Starting => format!("{}: {tool} is starting ({why}); links are unconfirmed until it's ready", s.language),
        arch_graph::ServerState::NotInstalled
        | arch_graph::ServerState::OutOfRange
        | arch_graph::ServerState::Broken
        | arch_graph::ServerState::Failed => {
            format!("{}: links unconfirmed: {tool}: {why}", s.language)
        }
    };
    format!("{line}{dropped}")
}

/// The fewest characters of the mode the top bar keeps room for on its row
/// ("NORMAL", "INSERT"), before it goes to the next.
const MODE_SHORTEST: usize = 8;

impl CockpitApp {
    /// The top bar's placements (it's the shell's host): on the left,
    /// reading the code again, the server shown, the bell and the tray; at
    /// the end, the palette, the panels and the developer tools. The
    /// shell's actions, placed with what they say now.
    pub(in crate::app) fn top_bar_placements(&self) -> Vec<(String, crate::actions::Control)> {
        use crate::actions::{At, Control, Style, Tone};
        use crate::alerts::AlertState as A;
        use egui_phosphor::regular as icon;
        let button = |caption: String, icon: Option<&'static str>| Control::new(At::TopBar, Style::Button, caption, icon);
        let end = |caption: String, icon: Option<&'static str>| Control::new(At::TopBar, Style::Button, caption, icon).at_end();
        let mut left = vec![("index.changed".to_owned(), button(String::new(), Some(icon::ARROW_CLOCKWISE)))];
        if self.servers.len() > 1 {
            let elsewhere = self.away.values().any(|d| crate::attention::counts(d.shared.agents.get().store()).0 > 0);
            let shown = button(format!("{}{}", self.desk.server, if elsewhere { " •" } else { "" }), Some(icon::HARD_DRIVES));
            left.push((
                "server.switch".to_owned(),
                if elsewhere { shown.tone(Tone::Warn).hover("Something waits on you on another server") } else { shown },
            ));
        }
        let (waiting, updates) =
            self.desks().map(|d| crate::attention::counts(d.shared.agents.get().store())).fold((0, 0), |(w, u), (dw, du)| (w + dw, u + du));
        let state = self.alerts.state();
        let glyph = if state == A::On { icon::BELL_RINGING } else { icon::BELL };
        let count = if waiting > 0 { format!("{waiting}") } else { String::new() };
        let dot = if matches!(state, A::NeedsPermission | A::Blocked) { " •" } else { "" };
        let tone = match (&state, waiting) {
            (A::NeedsPermission | A::Blocked, _) => Tone::Error,
            (_, 0) => Tone::Muted,
            _ => Tone::Warn,
        };
        let (switch, why) = self.alerts_words();
        let hint = [Some(format!("{waiting} waiting on you")), Some(switch), why].into_iter().flatten().collect::<Vec<_>>().join("\n");
        left.push(("attention.waiting".to_owned(), button(format!("{count}{dot}"), Some(glyph)).tone(tone).hover(hint)));
        if updates > 0 {
            left.push((
                "attention.updates".to_owned(),
                button(updates.to_string(), Some(icon::TRAY)).tone(Tone::Muted).hover(format!("{updates} update(s)")),
            ));
        }
        let (count, max) = (self.desk.panels.len(), self.config.panels.max);
        left.push(("palette.open".to_owned(), end("Action list".to_owned(), Some(icon::COMMAND))));
        let hidden = self.desk.panels.hidden();
        if hidden > 0 {
            left.push(("buffer.list".to_owned(), end(format!("{hidden} hidden"), None).tone(Tone::Info)));
        }
        if count > 1 {
            left.push(("panel.fewer".to_owned(), end(String::new(), Some(icon::MINUS))));
        }
        if count < max {
            left.push(("panel.add".to_owned(), end(String::new(), Some(icon::PLUS))));
        }
        // Where the buttons of what you're on show: its mark says which.
        let (mark, words) = match self.item_buttons_now() {
            crate::hosted::ItemButtons::Bar => (icon::SQUARE_HALF_BOTTOM, "The buttons of what you're on: in a bar (click: floating)"),
            crate::hosted::ItemButtons::Floating => (icon::PICTURE_IN_PICTURE, "The buttons of what you're on: floating (click: in a bar)"),
        };
        left.push(("panel.item_buttons".to_owned(), end(String::new(), Some(mark)).hover(words)));
        let (inspector, inspecting) = if self.feedback.is_inspecting {
            (format!("{} ACTIVE", icon::CROSSHAIR), Tone::Warn)
        } else {
            (format!("{} Inspector", icon::PENCIL_SIMPLE), Tone::Plain)
        };
        left.push(("inspector.toggle".to_owned(), end(inspector, None).tone(inspecting)));
        let issues = match self.feedback.issues.len() {
            0 => format!("{} Issues", icon::WARNING_CIRCLE),
            n => format!("{} Issues ({n})", icon::WARNING_CIRCLE),
        };
        left.push(("issues.toggle".to_owned(), end(issues, None).tone(Tone::Info)));
        left
    }

    pub fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        let top_rect = ui.available_rect_before_wrap();
        self.feedback.register_inspectable(ui, "top_bar", top_rect);

        // Its controls (everywhere's placed actions, from the one list): the
        // start's first, where the lights' words, which come and go, can't
        // push them; the end's on the right.
        let mine: Vec<Bound> = self
            .bound_actions()
            .into_iter()
            .filter(|b| b.layer == Layer::Everywhere && b.action.control.as_ref().is_some_and(|c| c.at == crate::actions::At::TopBar))
            .collect();
        let (end, mut start): (Vec<Bound>, Vec<Bound>) = mine.into_iter().partition(|b| b.action.control.as_ref().is_some_and(|c| c.end));
        Frame::NONE
            .fill(colors().panel)
            .inner_margin(crate::theme::margin2(crate::theme::look().space_l, crate::theme::look().space_sm))
            .show(ui, |ui| {
                // The right first (in the order listed, drawn right to left: the
                // panels, then the developer tools), in the room the left didn't
                // need last frame (its mode counted at its shortest), so its
                // buttons say their keys where there's room and nothing on it
                // moves with the mode; the left is never drawn under it, and
                // wraps where even that doesn't fit. One row high to start (as
                // `horizontal`), taller once wrapped.
                let (width, height) = (ui.available_width(), ui.spacing().interact_size.y);
                let gap = crate::theme::look().space_m + ui.spacing().item_spacing.x;
                ui.allocate_ui_with_layout(egui::vec2(width, height), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let room = (width - self.top_bar_needs - gap).max(0.0);
                    ui.allocate_ui_with_layout(egui::vec2(room, height), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let end: Vec<Bound> = end.into_iter().rev().collect();
                        self.host_controls(ui, &end, self.here().at);
                        ui.label(
                            RichText::new(format!("Panels: {}", self.desk.panels.len()))
                                .size(crate::theme::look().text_body)
                                .color(colors().muted),
                        );
                    });
                    ui.add_space(crate::theme::look().space_m);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true), |ui| {
                        // Each whole: what doesn't fit goes to the next row.
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        let brand_res = ui.horizontal(|ui| {
                            let brand = &self.config.brand;
                            ui.label(RichText::new(&brand.name).strong().size(crate::theme::look().text_title).color(colors().text_strong));
                            ui.label(
                                RichText::new(&brand.tagline).monospace().size(crate::theme::look().text_small).color(colors().accent),
                            );
                        });
                        self.feedback.register_inspectable(ui, "top_bar:brand", brand_res.response.rect);

                        ui.add_space(crate::theme::look().space_m);

                        // In the order it places them: those whose count comes and
                        // goes (the bell, the tray) after the ones that never change.
                        let order: Vec<String> = self.top_bar_placements().into_iter().map(|(id, _)| id).collect();
                        start.sort_by_key(|b| order.iter().position(|id| *id == b.action.id).unwrap_or(usize::MAX));
                        self.host_controls(ui, &start, self.here().at);
                        // Every menu (the leader's), a tap away.
                        let (menus, at, ctx) = ([self.every_menu()], self.here().at, ui.ctx().clone());
                        let mut cx = RunCx {
                            net: &self.desk.net,
                            ctx: &ctx,
                            input_mode: &mut self.input_mode,
                            at,
                            note: &mut self.note,
                            asked: &mut self.asked,
                        };
                        crate::hosted::groups(ui, &menus, &mut cx);

                        ui.add_space(crate::theme::look().space_m);

                        // Status LEDs: SRV = event WebSocket to the meta-harness server,
                        // AGENT = the agent session behind it. Both flicker on traffic.
                        let ws = *self.desk.shared.socket.get();
                        let (srv_color, srv_hint) = match ws {
                            WsState::Open => (colors().ok, "Meta-harness server: connected"),
                            WsState::Connecting => (colors().warn, "Meta-harness server: connecting…"),
                            WsState::Closed => (colors().error, "Meta-harness server: disconnected"),
                        };
                        let srv_count = self.desk.net.activity.load(Ordering::Relaxed);
                        let srv_res = self.srv_led.show(ui, "SRV", srv_color, srv_count, srv_hint);
                        self.feedback.register_inspectable(ui, "top_bar:connection_status", srv_res.rect);

                        ui.add_space(crate::theme::look().space_sm);
                        let agents = self.desk.shared.agents.get();
                        // The lead's state, as everywhere it's named.
                        let state = agents.state(&harness_contracts::SlotId::new(harness_contracts::LEAD));
                        let agent_color = state.colour();
                        let agent_hint = format!("Lead: {}", state.words());
                        let activity = agents.lead_activity();
                        drop(agents);
                        let agent_res = self.agent_led.show(ui, "AGENT", agent_color, activity, &agent_hint);
                        self.feedback.register_inspectable(ui, "top_bar:agent_status", agent_res.rect);

                        ui.add_space(crate::theme::look().space_sm);
                        let index_res = self.render_index_light(ui);
                        self.feedback.register_inspectable(ui, "top_bar:index_status", index_res.rect);
                        ui.add_space(crate::theme::look().space_m);

                        // Input mode indicator.
                        let (mode_label, mode_color, mode_bg) = match self.input_mode {
                            InputMode::Normal => (
                                {
                                    let count = self.pending_count.map(|c| c.to_string());
                                    let keys = self.pending_keys.iter().map(Chord::label);
                                    let pending: Vec<String> = count.into_iter().chain(keys).collect();
                                    if pending.is_empty() { "NORMAL".to_string() } else { format!("NORMAL {}", pending.join(" ")) }
                                },
                                colors().info,
                                colors().info_bg,
                            ),
                            InputMode::Leader => (self.menu_path().join(" \u{203a} "), colors().key, colors().warn_bg),
                            InputMode::Palette => ("ACTION LIST".to_string(), colors().info, colors().info_bg),
                            InputMode::Choosing { .. } => ("CHOOSING".to_string(), colors().info, colors().info_bg),
                            InputMode::Insert { at } => (
                                match at {
                                    crate::keymap::Host::Panel(panel) => format!("INSERT [{}]", panel + 1),
                                    crate::keymap::Host::Look => "INSERT [look]".to_owned(),
                                },
                                colors().ok,
                                colors().ok_bg,
                            ),
                            InputMode::Capture { .. } => (
                                match &self.capture {
                                    Some(Capture::Search(_)) => "SEARCH",
                                    Some(Capture::Hint(_)) => "HINT",
                                    Some(Capture::MarkSet) => "MARK",
                                    Some(Capture::MarkJump) | None => "JUMP TO MARK",
                                }
                                .to_string(),
                                colors().key,
                                colors().warn_bg,
                            ),
                        };

                        // Cut short to the room left (a menu's path is whole in the
                        // menu), so the bar keeps its height whatever the mode; on
                        // the next row only where there's no room for a short word.
                        let look = crate::theme::look();
                        let text = RichText::new(&mode_label).monospace().strong().size(look.text_small).color(mode_color);
                        let shortest = MODE_SHORTEST as f32
                            * ui.fonts_mut(|f| f.glyph_width(&egui::FontId::monospace(look.text_small), 'M'))
                            + 2.0 * (look.space_sm + look.line_thin);
                        // What the left needs on one row: up to here (all of a row
                        // once it has wrapped already), the mode at its shortest,
                        // then what follows it.
                        let (row, spacing) = (ui.max_rect(), ui.spacing().item_spacing.x);
                        let mut needs = shortest + if ui.cursor().min.y <= row.min.y { ui.cursor().min.x - row.min.x } else { row.width() };
                        if shortest > ui.available_size_before_wrap().x {
                            ui.end_row();
                        }
                        Frame::NONE
                            .fill(mode_bg)
                            .stroke(Stroke::new(look.line_thin, mode_color))
                            .corner_radius(look.radius_s)
                            .inner_margin(crate::theme::margin2(look.space_sm, look.space_xs))
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(text).truncate()).on_hover_text(&mode_label);
                            });

                        // What a buffer in use has to say to the whole desk (an answer
                        // cut short or slow).
                        for alert in BufferKind::ALL.into_iter().filter_map(|k| self.desk.view(k).alert()) {
                            let label = ui.label(
                                RichText::new(format!("{} {}", egui_phosphor::regular::WARNING, alert.text))
                                    .size(crate::theme::look().text_body)
                                    .color(alert.tone.color()),
                            );
                            needs += spacing + label.rect.width();
                            if let Some(more) = &alert.detail {
                                label.on_hover_text(more);
                            }
                        }

                        if let Some(err) = &self.config_error {
                            let label = ui.label(
                                RichText::new(format!("{} config", egui_phosphor::regular::WARNING))
                                    .size(crate::theme::look().text_body)
                                    .color(colors().warn),
                            );
                            needs += spacing + label.rect.width();
                            label.on_hover_text(format!("Your UI config was not applied (using defaults):\n{err}"));
                        }
                        self.top_bar_needs = needs;
                    });
                });
            });
    }

    /// The INDEX light: its state's colour and word, progress while it runs,
    /// the last run on hover (reading the code again is a control on the bar).
    pub(in crate::app) fn render_index_light(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let index = self.desk.shared.index.get();
        let state = index_state(index);
        let (_, word, color) = crate::status::look(state);
        let detail = match state {
            "indexing" => format!("{word} {} of {}", index.done, index.total),
            "stale" => format!("{} {word}", index.stale.len()),
            _ => word,
        };
        let mut hover = match &index.last {
            Some(l) => format!(
                "Last read {}: {} files ({} re-read) in {} ms; {} components, {} connections, {} calls not linked for certain.",
                l.at, l.files, l.parsed, l.millis, l.components, l.relations, l.unresolved
            ),
            None => "Not read yet.".to_owned(),
        };
        if let Some(e) = &index.error {
            hover.push_str(&format!("\nThe last run failed: {e}"));
        }
        for s in &index.servers {
            hover.push('\n');
            hover.push_str(&server_line(s));
        }
        if !index.stale.is_empty() {
            hover.push_str(&format!("\nChanged since read: {}", index.stale.join(", ")));
        }
        // Flickers while files go by.
        let activity = if state == "indexing" { index.done as u64 } else { 0 };
        ui.horizontal(|ui| {
            let led = self.index_led.show(ui, "INDEX", color, activity, &hover);
            let text = ui.label(RichText::new(detail).size(crate::theme::look().text_small).color(color)).on_hover_text(&hover);
            led.union(text)
        })
        .inner
    }
}
