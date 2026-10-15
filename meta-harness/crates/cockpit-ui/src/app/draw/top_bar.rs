//! The top bar: brand, LEDs, the index light, the mode, columns.

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
        arch_graph::ServerState::NotInstalled | arch_graph::ServerState::OutOfRange | arch_graph::ServerState::Broken | arch_graph::ServerState::Failed => {
            format!("{}: links unconfirmed: {tool}: {why}", s.language)
        }
    };
    format!("{line}{dropped}")
}

impl CockpitApp {
    pub fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        let top_rect = ui.available_rect_before_wrap();
        self.feedback.register_inspectable(ui, "top_bar", top_rect);

        Frame::NONE.fill(colors().panel).inner_margin(crate::theme::margin2(crate::theme::look().space_l, crate::theme::look().space_sm)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let brand_res = ui.horizontal(|ui| {
                    let brand = &self.config.brand;
                    ui.label(RichText::new(&brand.name).strong().size(crate::theme::look().text_title).color(colors().text_strong));
                    ui.label(RichText::new(&brand.tagline).monospace().size(crate::theme::look().text_small).color(colors().accent));
                });
                self.feedback.register_inspectable(ui, "top_bar:brand", brand_res.response.rect);

                ui.add_space(crate::theme::look().space_m);

                // Status LEDs: SRV = event WebSocket to the meta-harness server,
                // AGENT = the agent session behind it. Both flicker on traffic.
                let ws = self.net.ws_state.lock().map(|s| *s).unwrap_or(WsState::Closed);
                let (srv_color, srv_hint) = match ws {
                    WsState::Open => (colors().ok, "Meta-harness server: connected"),
                    WsState::Connecting => (colors().warn, "Meta-harness server: connecting…"),
                    WsState::Closed => (colors().error, "Meta-harness server: disconnected"),
                };
                let srv_count = self.net.activity.load(Ordering::Relaxed);
                let srv_res = self.srv_led.show(ui, "SRV", srv_color, srv_count, srv_hint);
                self.feedback.register_inspectable(ui, "top_bar:connection_status", srv_res.rect);

                ui.add_space(crate::theme::look().space_sm);
                let (agent_color, agent_hint) = match &self.chat.session_status() {
                    SessionStatus::Idle => match self.chat.harness_problem() {
                        Some(problem) => (colors().error, format!("Agent: unavailable — {problem}")),
                        None => (colors().faint, "Agent: not connected".to_string()),
                    },
                    SessionStatus::Connecting => (colors().warn, "Agent: connecting…".to_string()),
                    SessionStatus::Connected(tid) => (colors().ok, format!("Agent: connected [{}]", tid)),
                    SessionStatus::Failed(err) => (colors().error, format!("Agent: failed — {}", err)),
                };
                let agent_res = self.agent_led.show(ui, "AGENT", agent_color, self.chat.agent_activity(), &agent_hint);
                self.feedback.register_inspectable(ui, "top_bar:agent_status", agent_res.rect);

                ui.add_space(crate::theme::look().space_sm);
                let index_res = self.render_index_light(ui);
                self.feedback.register_inspectable(ui, "top_bar:index_status", index_res.rect);
                ui.add_space(crate::theme::look().space_sm);
                self.render_attention_counters(ui);

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
                    InputMode::Leader => (self.leader_title("LEADER"), colors().key, colors().warn_bg),
                    InputMode::Palette => ("PALETTE".to_string(), colors().info, colors().info_bg),
                    InputMode::Insert { slot } => (format!("INSERT [S{}]", slot + 1), colors().ok, colors().ok_bg),
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

                Frame::NONE.fill(mode_bg).stroke(Stroke::new(crate::theme::look().line_thin, mode_color)).corner_radius(crate::theme::look().radius_s).inner_margin(crate::theme::margin2(crate::theme::look().space_sm, crate::theme::look().space_xs)).show(ui, |ui| {
                    ui.label(RichText::new(mode_label).monospace().strong().size(crate::theme::look().text_small).color(mode_color));
                });

                // An answer cut short or slow, whichever view is showing it.
                if let Some((words, more)) = self.graph.answer_status() {
                    ui.label(RichText::new(format!("{} {words}", egui_phosphor::regular::WARNING)).size(crate::theme::look().text_body).color(colors().warn)).on_hover_text(more);
                }

                if let Some(err) = &self.config_error {
                    ui.label(RichText::new(format!("{} config", egui_phosphor::regular::WARNING)).size(crate::theme::look().text_body).color(colors().warn))
                        .on_hover_text(format!("Your UI config was not applied (using defaults):\n{err}"));
                }

                // Right-aligned controls group (Inspector, Issues, Layout)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (pencil_label, pencil_color) = if self.feedback.is_inspecting {
                        (format!("{} ACTIVE", egui_phosphor::regular::CROSSHAIR), colors().warn)
                    } else {
                        (format!("{} Inspector", egui_phosphor::regular::PENCIL_SIMPLE), colors().text_strong)
                    };
                    let insp_btn = ui.button(RichText::new(&pencil_label).strong().color(pencil_color));
                    if insp_btn.clicked() {
                        self.feedback.toggle_inspection();
                    }
                    self.feedback.register_inspectable(ui, "top_bar:inspector_button", insp_btn.rect);

                    let issues_label = if self.feedback.issues.is_empty() {
                        format!("{} Issues", egui_phosphor::regular::WARNING_CIRCLE)
                    } else {
                        format!("{} Issues ({})", egui_phosphor::regular::WARNING_CIRCLE, self.feedback.issues.len())
                    };
                    let issues_btn = ui.button(RichText::new(&issues_label).color(colors().info));
                    if issues_btn.clicked() {
                        self.feedback.is_issues_window_open = !self.feedback.is_issues_window_open;
                        if self.feedback.is_issues_window_open {
                            self.feedback.load_issues(&self.net, ui.ctx().clone());
                        }
                    }
                    self.feedback.register_inspectable(ui, "top_bar:issues_button", issues_btn.rect);

                    ui.add_space(crate::theme::look().space_m);

                    // This row runs right to left: add from the right end so
                    // it reads "Columns: - 2 +".
                    let layout_res = ui.horizontal(|ui| {
                        let (count, max) = (self.columns.len(), self.config.columns.max);
                        if ui.add_enabled(count < max, egui::Button::new(egui_phosphor::regular::PLUS)).on_hover_text("Add a column").clicked() {
                            self.columns.set_count(count + 1, max);
                        }
                        ui.label(RichText::new(count.to_string()).monospace().size(crate::theme::look().text_body).color(colors().text_strong));
                        if ui.add_enabled(count > 1, egui::Button::new(egui_phosphor::regular::MINUS)).on_hover_text("One column fewer").clicked() {
                            self.columns.set_count(count - 1, max);
                        }
                        ui.label(RichText::new("Columns:").size(crate::theme::look().text_body).color(colors().muted));
                        let hidden = self.columns.hidden();
                        if hidden > 0 {
                            let list = self.bound_actions().into_iter().find(|b| b.action.id == "buffer.list").map(|b| b.key_hint()).unwrap_or_default();
                            let chip = ui.add(egui::Label::new(RichText::new(format!("{hidden} hidden")).size(crate::theme::look().text_body).color(colors().info)).sense(egui::Sense::click()));
                            if chip.on_hover_text(format!("Views open but not on screen ({list} lists them all)")).clicked() {
                                self.palette.open_with("buffer.list");
                                self.input_mode = InputMode::Palette;
                            }
                        }
                    });
                    self.feedback.register_inspectable(ui, "top_bar:layout_selector", layout_res.response.rect);
                });
            });
        });
    }

    /// The INDEX light: its state's colour and word, progress while it runs,
    /// the last run on hover; a click offers the re-index actions.
    pub(in crate::app) fn render_index_light(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let state = index_state(&self.index);
        let (_, word, color) = crate::status::look(state);
        let detail = match state {
            "indexing" => format!("{word} {} of {}", self.index.done, self.index.total),
            "stale" => format!("{} {word}", self.index.stale.len()),
            _ => word,
        };
        let mut hover = match &self.index.last {
            Some(l) => format!("Last read {}: {} files ({} re-read) in {} ms; {} components, {} connections, {} calls not linked for certain.", l.at, l.files, l.parsed, l.millis, l.components, l.relations, l.unresolved),
            None => "Not read yet.".to_owned(),
        };
        if let Some(e) = &self.index.error {
            hover.push_str(&format!("\nThe last run failed: {e}"));
        }
        for s in &self.index.servers {
            hover.push('\n');
            hover.push_str(&server_line(s));
        }
        if !self.index.stale.is_empty() {
            hover.push_str(&format!("\nChanged since read: {}", self.index.stale.join(", ")));
        }
        hover.push_str("\nClick to read the code again.");
        // Flickers while files go by.
        let activity = if state == "indexing" { self.index.done as u64 } else { 0 };
        let response = ui
            .horizontal(|ui| {
                let led = self.index_led.show(ui, "INDEX", color, activity, &hover);
                let text = ui.add(egui::Label::new(RichText::new(detail).size(crate::theme::look().text_small).color(color)).sense(egui::Sense::click())).on_hover_text(&hover);
                led.union(text)
            })
            .inner;
        if response.interact(egui::Sense::click()).clicked() {
            self.palette.open_with("index.");
            self.input_mode = InputMode::Palette;
        }
        response
    }
}
