use std::sync::{Arc, Mutex};
use egui::{Color32, Frame, Margin, Pos2, Rect, RichText, Stroke, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::feedback::FeedbackInspectorState;
use crate::net::{ModelSnapshot, NetClient, ProviderRuntimeEvent, StreamKind};

#[derive(Debug, Clone, PartialEq)]
pub enum ToolStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ToolCard {
    pub task_id: String,
    pub title: String,
    pub status: ToolStatus,
    pub output: Option<String>,
    pub file_target: Option<(String, Option<usize>)>,
}

#[derive(Debug, Clone)]
pub struct ApprovalCard {
    pub request_id: String,
    pub prompt: String,
    pub options: Vec<String>,
    pub resolved: bool,
}

#[derive(Debug, Clone)]
pub struct TrajectoryTurn {
    pub turn_id: String,
    pub user_prompt: Option<String>,
    pub assistant_text: String,
    pub thoughts: String,
    pub tools: Vec<ToolCard>,
    pub approvals: Vec<ApprovalCard>,
    pub timestamp: String,
    pub is_streaming: bool,
}

pub struct ChatStreamState {
    pub thread_id: Option<String>,
    pub turns: Vec<TrajectoryTurn>,
    pub input_buffer: String,
    pub selected_model: String,
    pub selected_effort: String,
    pub selected_mode: String,
    pub markdown_cache: CommonMarkCache,
    pub thoughts_collapsed: bool,
    pub requested_file_jump: Option<(String, Option<usize>)>,
    pub pending_session_start: Arc<Mutex<Option<Result<String, String>>>>,
    pub is_starting_session: bool,
    pub queued_prompt: Option<String>,
}

impl Default for ChatStreamState {
    fn default() -> Self {
        Self {
            thread_id: None,
            turns: Vec::new(),
            input_buffer: String::new(),
            selected_model: "gemini-3.7-flash-high".to_string(),
            selected_effort: "High".to_string(),
            selected_mode: "auto-accept-edits".to_string(),
            markdown_cache: CommonMarkCache::default(),
            thoughts_collapsed: false,
            requested_file_jump: None,
            pending_session_start: Arc::new(Mutex::new(None)),
            is_starting_session: false,
            queued_prompt: None,
        }
    }
}

impl ChatStreamState {
    pub fn start_session(&mut self, net: &NetClient, ctx: egui::Context) {
        if self.is_starting_session {
            return;
        }
        self.is_starting_session = true;
        let mailbox = self.pending_session_start.clone();
        net.start_session(None, ctx, move |res| {
            if let Ok(mut lock) = mailbox.lock() {
                *lock = Some(res);
            }
        });
    }

    fn find_turn_mut<'a>(turns: &'a mut [TrajectoryTurn], turn_id: &str) -> Option<&'a mut TrajectoryTurn> {
        if let Some(pos) = turns.iter().rposition(|t| t.turn_id == turn_id) {
            Some(&mut turns[pos])
        } else {
            turns.last_mut()
        }
    }

    pub fn process_event(&mut self, event: ProviderRuntimeEvent) {
        match event {
            ProviderRuntimeEvent::TurnStarted { turn_id, payload: _ } => {
                if let Some(last) = self.turns.last_mut() {
                    if last.is_streaming && last.assistant_text.is_empty() && last.thoughts.is_empty() && last.tools.is_empty() {
                        last.turn_id = turn_id;
                        return;
                    }
                }
                self.turns.push(TrajectoryTurn {
                    turn_id,
                    user_prompt: None,
                    assistant_text: String::new(),
                    thoughts: String::new(),
                    tools: Vec::new(),
                    approvals: Vec::new(),
                    timestamp: "Just now".to_string(),
                    is_streaming: true,
                });
            }
            ProviderRuntimeEvent::ContentDelta { turn_id, payload } => {
                if let Some(turn) = Self::find_turn_mut(&mut self.turns, &turn_id) {
                    match payload.stream_kind {
                        StreamKind::ReasoningText => {
                            turn.thoughts.push_str(&payload.delta);
                        }
                        StreamKind::AssistantText => {
                            turn.assistant_text.push_str(&payload.delta);
                            // Scan for file references in delta
                            Self::scan_for_file_refs(&payload.delta, &mut self.requested_file_jump);
                        }
                    }
                }
            }
            ProviderRuntimeEvent::TurnCompleted { turn_id, payload: _ } => {
                if let Some(turn) = Self::find_turn_mut(&mut self.turns, &turn_id) {
                    turn.is_streaming = false;
                }
            }
            ProviderRuntimeEvent::TaskStarted { turn_id, payload } => {
                let file_target = payload.description.as_deref().and_then(Self::extract_file_target);
                if let Some(turn) = Self::find_turn_mut(&mut self.turns, &turn_id) {
                    turn.tools.push(ToolCard {
                        task_id: payload.task_id,
                        title: payload.description.unwrap_or(payload.title),
                        status: ToolStatus::Running,
                        output: None,
                        file_target,
                    });
                }
            }
            ProviderRuntimeEvent::TaskProgress { turn_id, payload } | ProviderRuntimeEvent::TaskUpdated { turn_id, payload } => {
                if let Some(turn) = Self::find_turn_mut(&mut self.turns, &turn_id) {
                    if let Some(t) = turn.tools.iter_mut().find(|t| t.task_id == payload.task_id) {
                        t.output = payload.summary.or(payload.description);
                    }
                }
            }
            ProviderRuntimeEvent::TaskCompleted { turn_id, payload } => {
                if let Some(turn) = Self::find_turn_mut(&mut self.turns, &turn_id) {
                    if let Some(t) = turn.tools.iter_mut().find(|t| t.task_id == payload.task_id) {
                        t.status = if payload.status.to_lowercase() == "success" || payload.status.to_lowercase() == "done" {
                            ToolStatus::Completed
                        } else {
                            ToolStatus::Failed
                        };
                    }
                }
            }
            ProviderRuntimeEvent::RequestOpened { request_id, payload } => {
                if let Some(turn) = self.turns.last_mut() {
                    let prompt = payload.tool_call.title.unwrap_or_else(|| "Permission requested".to_string());
                    let options = payload.options.into_iter().map(|o| o.label).collect();
                    turn.approvals.push(ApprovalCard {
                        request_id,
                        prompt,
                        options,
                        resolved: false,
                    });
                }
            }
            ProviderRuntimeEvent::RequestResolved { request_id } => {
                if let Some(turn) = self.turns.last_mut() {
                    if let Some(a) = turn.approvals.iter_mut().find(|a| a.request_id == request_id) {
                        a.resolved = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn extract_file_target(text: &str) -> Option<(String, Option<usize>)> {
        for word in text.split_whitespace() {
            let clean = word.trim_matches(|c| c == '\'' || c == '"' || c == '(' || c == ')' || c == '`');
            if clean.contains('/') || clean.ends_with(".rs") || clean.ends_with(".toml") || clean.ends_with(".json") || clean.ends_with(".md") {
                let parts: Vec<&str> = clean.split('#').collect();
                let file = parts[0].to_string();
                let line = parts.get(1).and_then(|l| {
                    l.trim_start_matches('L').split('-').next().and_then(|num| num.parse::<usize>().ok())
                });
                return Some((file, line));
            }
        }
        None
    }

    fn scan_for_file_refs(text: &str, target: &mut Option<(String, Option<usize>)>) {
        if target.is_none() {
            if let Some(res) = Self::extract_file_target(text) {
                *target = Some(res);
            }
        }
    }

    pub fn execute_action(
        &mut self,
        action: crate::keymap::ChatAction,
        input_mode: &mut crate::keymap::InputMode,
        slot_idx: usize,
    ) {
        match action {
            crate::keymap::ChatAction::EnterInsert => {
                *input_mode = crate::keymap::InputMode::Insert { slot: slot_idx };
            }
            crate::keymap::ChatAction::ClearPrompt => {
                self.input_buffer.clear();
            }
        }
    }

    pub fn handle_input(
        &mut self,
        i: &egui::InputState,
        keymap: &crate::keymap::ChatKeymap,
        input_mode: &mut crate::keymap::InputMode,
        slot_idx: usize,
    ) {
        if keymap.enter_prompt.is_pressed(i) {
            self.execute_action(crate::keymap::ChatAction::EnterInsert, input_mode, slot_idx);
        }
        if keymap.clear_prompt.is_pressed(i) {
            self.execute_action(crate::keymap::ChatAction::ClearPrompt, input_mode, slot_idx);
        }
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        net: &NetClient,
        dynamic_models: &[ModelSnapshot],
        feedback: &mut FeedbackInspectorState,
        slot_idx: usize,
        is_focused: bool,
        input_mode: &mut crate::keymap::InputMode,
    ) {
        // Drain pending session start
        if let Ok(mut lock) = self.pending_session_start.lock() {
            if let Some(res) = lock.take() {
                self.is_starting_session = false;
                match res {
                    Ok(tid) => {
                        self.thread_id = Some(tid.clone());
                        if let Some(prompt) = self.queued_prompt.take() {
                            net.send_turn(&tid, &prompt, &self.selected_model, &self.selected_effort, |_| {});
                        }
                    }
                    Err(e) => {
                        if let Some(turn) = self.turns.last_mut() {
                            turn.assistant_text = format!("❌ Failed to connect session: {}", e);
                            turn.is_streaming = false;
                        }
                    }
                }
            }
        }

        ui.vertical(|ui| {
            // 1. Controls Toolbar
            let start_y = ui.cursor().min.y;
            let start_x = ui.cursor().min.x;
            let w = ui.available_width();
            self.render_controls_bar(ui, net, dynamic_models);
            let end_y = ui.cursor().min.y;
            let controls_rect = Rect::from_min_size(Pos2::new(start_x, start_y), Vec2::new(w, (end_y - start_y).max(24.0)));
            feedback.register_inspectable(ui, &format!("slot_{}:chat:controls", slot_idx + 1), controls_rect);
            ui.separator();

            // 2. Trajectory Stream (Harness Trajectory Document)
            let input_reserve = 105.0;
            let scroll_height = (ui.available_height() - input_reserve).max(50.0);
            let stream_res = egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .max_height(scroll_height)
                .show(ui, |ui| {
                    if self.turns.is_empty() {
                        ui.add_space(20.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("Meta-Harness Trajectory Stream").strong().size(15.0).color(Color32::from_rgb(100, 160, 240)));
                            ui.add_space(4.0);
                            let status_hint = if let Some(tid) = &self.thread_id {
                                format!("Active Session [{}] • Antigravity Agent Ready", tid)
                            } else if self.is_starting_session {
                                "Connecting to Antigravity session...".to_string()
                            } else {
                                "Ready to execute. Enter instructions below to start session.".to_string()
                            };
                            ui.label(RichText::new(status_hint).size(12.0).color(Color32::GRAY));
                        });
                        ui.add_space(20.0);
                    }

                    let active_thread_id = self.thread_id.as_deref();
                    for turn in &mut self.turns {
                        Self::render_turn(turn, ui, net, &mut self.markdown_cache, &mut self.thoughts_collapsed, &mut self.requested_file_jump, active_thread_id);
                        ui.add_space(10.0);
                    }
                });
            feedback.register_inspectable(ui, &format!("slot_{}:chat:stream", slot_idx + 1), stream_res.inner_rect);

            ui.separator();

            // 3. Persistent Input Area
            let start_in_y = ui.cursor().min.y;
            let start_in_x = ui.cursor().min.x;
            let in_w = ui.available_width();
            self.render_input_bar(ui, net, feedback, slot_idx, is_focused, input_mode);
            let end_in_y = ui.cursor().min.y;
            let input_rect = Rect::from_min_size(Pos2::new(start_in_x, start_in_y), Vec2::new(in_w, (end_in_y - start_in_y).max(40.0)));
            feedback.register_inspectable(ui, &format!("slot_{}:chat:input_bar", slot_idx + 1), input_rect);
        });
    }

    fn render_controls_bar(&mut self, ui: &mut egui::Ui, net: &NetClient, dynamic_models: &[ModelSnapshot]) {
        ui.vertical(|ui| {
            // Row 1: Model selector & Session Connect/Status
            let display_model: String = match self.selected_model.as_str() {
                "gemini-3.7-flash-high" => "Flash 3.7".to_string(),
                "gemini-pro-agent" => "Pro 3.1".to_string(),
                other => {
                    if other.len() > 14 {
                        other[..14].to_string()
                    } else {
                        other.to_string()
                    }
                }
            };

            ui.horizontal(|ui| {
                ui.label(RichText::new("Model:").strong().size(11.0));
                let model_combo_w = (ui.available_width() - 85.0).clamp(70.0, 180.0);
                egui::ComboBox::from_id_salt("chat_model_selector")
                    .width(model_combo_w)
                    .selected_text(display_model)
                    .show_ui(ui, |ui| {
                        if dynamic_models.is_empty() {
                            ui.selectable_value(&mut self.selected_model, "gemini-3.7-flash-high".to_string(), "Gemini 3.7 Flash");
                            ui.selectable_value(&mut self.selected_model, "gemini-pro-agent".to_string(), "Gemini 3.1 Pro");
                        } else {
                            for m in dynamic_models {
                                ui.selectable_value(&mut self.selected_model, m.slug.clone(), &m.name);
                            }
                        }
                    });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(tid) = &self.thread_id {
                        ui.label(RichText::new(format!("[{}]", &tid[..8.min(tid.len())])).monospace().size(10.0).color(Color32::from_rgb(140, 200, 255)));
                    } else if self.is_starting_session {
                        ui.label(RichText::new("⏳ Connecting...").size(10.0).color(Color32::from_rgb(250, 180, 50)));
                    } else {
                        if ui.button("⚡ Connect").clicked() {
                            let ctx = ui.ctx().clone();
                            self.start_session(net, ctx);
                        }
                    }
                });
            });

            ui.add_space(2.0);

            // Row 2: Effort & Mode selectors
            let is_narrow = ui.available_width() < 260.0;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Effort:").strong().size(11.0));
                let effort_disp = match self.selected_effort.as_str() {
                    "High" => "H",
                    "Medium" => "M",
                    "Low" => "L",
                    other => other,
                };
                egui::ComboBox::from_id_salt("chat_effort_selector")
                    .width(if is_narrow { 36.0 } else { 55.0 })
                    .selected_text(if is_narrow { effort_disp } else { &self.selected_effort })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.selected_effort, "High".to_string(), "High");
                        ui.selectable_value(&mut self.selected_effort, "Medium".to_string(), "Med");
                        ui.selectable_value(&mut self.selected_effort, "Low".to_string(), "Low");
                    });

                ui.add_space(4.0);
                ui.label(RichText::new("Mode:").strong().size(11.0));
                let prev_mode = self.selected_mode.clone();
                let mode_disp = match self.selected_mode.as_str() {
                    "auto-accept-edits" => if is_narrow { "Auto" } else { "⚡ Auto" },
                    "ask-tools" => if is_narrow { "Ask" } else { "🛡️ Ask" },
                    "yolo" => "🔥 YOLO",
                    other => other,
                };
                egui::ComboBox::from_id_salt("chat_mode_selector")
                    .width(if is_narrow { 60.0 } else { 90.0 })
                    .selected_text(mode_disp)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.selected_mode, "auto-accept-edits".to_string(), "⚡ Auto-Accept");
                        ui.selectable_value(&mut self.selected_mode, "ask-tools".to_string(), "🛡️ Ask Tools");
                        ui.selectable_value(&mut self.selected_mode, "yolo".to_string(), "🔥 YOLO");
                    });

                if prev_mode != self.selected_mode {
                    if let Some(tid) = &self.thread_id {
                        net.steer_session(tid, &self.selected_model, &self.selected_mode);
                    }
                }
            });
        });
    }

    fn render_turn(
        turn: &mut TrajectoryTurn,
        ui: &mut egui::Ui,
        net: &NetClient,
        cache: &mut CommonMarkCache,
        _thoughts_collapsed: &mut bool,
        requested_jump: &mut Option<(String, Option<usize>)>,
        thread_id: Option<&str>,
    ) {
        ui.vertical(|ui| {
            // 1. User Command / Prompt Header (CLI / REPL style)
            let disp_id = if turn.turn_id.len() > 8 { &turn.turn_id[..8] } else { &turn.turn_id };
            if let Some(user_prompt) = &turn.user_prompt {
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(">").monospace().size(14.0).strong().color(Color32::from_rgb(80, 180, 255)));
                    ui.label(RichText::new(user_prompt).size(13.0).strong().color(Color32::WHITE));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Turn {} • {}", disp_id, turn.timestamp)).monospace().size(10.0).color(Color32::from_rgb(90, 110, 135)));
                    if turn.is_streaming {
                        ui.label(RichText::new("* RUNNING").size(9.0).color(Color32::from_rgb(250, 180, 50)));
                    }
                });
                ui.add(egui::Separator::default().spacing(8.0));
            } else {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Turn {} • {}", disp_id, turn.timestamp)).monospace().size(10.0).color(Color32::from_rgb(90, 110, 135)));
                    if turn.is_streaming {
                        ui.label(RichText::new("* RUNNING").size(9.0).color(Color32::from_rgb(250, 180, 50)));
                    }
                });
                ui.add(egui::Separator::default().spacing(4.0));
            }

            // 2. Collapsible Reasoning Block (inline developer trace)
            if !turn.thoughts.is_empty() {
                let count = turn.thoughts.len();
                let label = if turn.is_streaming && turn.assistant_text.is_empty() {
                    format!("🧠 Thinking... ({} chars)", count)
                } else {
                    format!("🧠 Reasoning ({} chars)", count)
                };
                ui.collapsing(RichText::new(label).size(11.0).color(Color32::from_rgb(175, 155, 225)), |ui| {
                    Frame::NONE
                        .fill(Color32::from_rgb(12, 15, 20))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(30, 36, 48)))
                        .corner_radius(2)
                        .inner_margin(Margin::same(6))
                        .show(ui, |ui| {
                            ui.label(RichText::new(&turn.thoughts).monospace().size(11.0).color(Color32::from_rgb(150, 155, 175)));
                        });
                });
                ui.add_space(4.0);
            }

            // 3. Tool Execution Traces (inline terminal style)
            for tool in &turn.tools {
                Self::render_tool_trace(tool, ui, requested_jump);
                ui.add_space(2.0);
            }

            // 4. Permission / Approval Prompts
            let active_tid = thread_id.unwrap_or(&turn.turn_id);
            for approval in &mut turn.approvals {
                Self::render_approval_card(approval, ui, net, active_tid);
                ui.add_space(4.0);
            }

            // 5. Assistant Output (prose + code streamed directly into the session)
            if !turn.assistant_text.is_empty() {
                CommonMarkViewer::new().show(ui, cache, &turn.assistant_text);
                ui.add_space(6.0);
            }
        });
    }

    fn render_tool_trace(tool: &ToolCard, ui: &mut egui::Ui, requested_jump: &mut Option<(String, Option<usize>)>) {
        let (tag, color) = match tool.status {
            ToolStatus::Running => ("[RUN]", Color32::from_rgb(250, 180, 50)),
            ToolStatus::Completed => ("[OK]", Color32::from_rgb(80, 200, 120)),
            ToolStatus::Failed => ("[FAIL]", Color32::from_rgb(240, 80, 80)),
        };

        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("  {} {}", tag, tool.title)).monospace().size(11.0).color(color));

            if let Some((file, line)) = &tool.file_target {
                let btn_label = if let Some(l) = line {
                    format!("📂 {}:{}", file, l)
                } else {
                    format!("📂 {}", file)
                };
                if ui.small_button(RichText::new(btn_label).monospace().size(10.0)).clicked() {
                    *requested_jump = Some((file.clone(), *line));
                }
            }
        });

        if let Some(out) = &tool.output {
            let first_line = out.lines().next().unwrap_or(out);
            let display_text = if out.lines().count() > 1 {
                format!("     └─ {} (+{} lines)", first_line, out.lines().count() - 1)
            } else {
                format!("     └─ {}", first_line)
            };
            ui.label(RichText::new(display_text).monospace().size(10.0).color(Color32::from_rgb(120, 130, 145)));
        }
    }

    fn render_approval_card(approval: &mut ApprovalCard, ui: &mut egui::Ui, net: &NetClient, thread_id: &str) {
        Frame::NONE
            .fill(Color32::from_rgb(36, 30, 18))
            .stroke(Stroke::new(1.0, Color32::from_rgb(220, 150, 40)))
            .corner_radius(4)
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.label(RichText::new("⚠️ PERMISSION REQUIRED").strong().size(11.0).color(Color32::from_rgb(250, 180, 50)));
                ui.label(RichText::new(&approval.prompt).size(12.0));
                ui.add_space(4.0);

                if approval.resolved {
                    ui.label(RichText::new("✓ Resolved").size(11.0).color(Color32::GREEN));
                } else {
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("✓ Approve").color(Color32::GREEN)).clicked() {
                            net.respond_request(thread_id, &approval.request_id, "approved");
                            approval.resolved = true;
                        }
                        if ui.button(RichText::new("✗ Reject").color(Color32::RED)).clicked() {
                            net.respond_request(thread_id, &approval.request_id, "rejected");
                            approval.resolved = true;
                        }
                    });
                }
            });
    }

    fn render_input_bar(
        &mut self,
        ui: &mut egui::Ui,
        net: &NetClient,
        feedback: &FeedbackInspectorState,
        slot_idx: usize,
        is_focused: bool,
        input_mode: &mut crate::keymap::InputMode,
    ) {
        ui.horizontal(|ui| {
            // Quick chips
            if ui.small_button("/plan").clicked() {
                self.input_buffer.push_str("/plan ");
            }
            if ui.small_button("/logout").clicked() {
                self.input_buffer.push_str("/logout");
            }
            if ui.small_button("Clear").clicked() {
                self.input_buffer.clear();
            }
        });
        ui.add_space(2.0);

        ui.horizontal(|ui| {
            let input_id = egui::Id::new("chat_persistent_input_text_edit");
            let send_btn_w = 64.0;
            let spacing = ui.spacing().item_spacing.x;
            let text_w = (ui.available_width() - send_btn_w - spacing - 4.0).max(60.0);

            let is_this_slot_insert = *input_mode == crate::keymap::InputMode::Insert { slot: slot_idx };
            let hint = if is_this_slot_insert {
                "Send instructions to Antigravity... (Enter to send, Shift+Enter for newline, Esc for Normal)"
            } else {
                "Press 'i' or click to enter insert mode..."
            };

            let text_edit = egui::TextEdit::multiline(&mut self.input_buffer)
                .id(input_id)
                .desired_rows(2)
                .desired_width(text_w)
                .hint_text(hint);

            let output = ui.add(text_edit);
            if is_this_slot_insert && is_focused && !feedback.is_modal_open && !feedback.is_issues_window_open && !feedback.is_inspecting {
                output.request_focus();
            }

            if output.clicked() || output.gained_focus() {
                *input_mode = crate::keymap::InputMode::Insert { slot: slot_idx };
            }

            let send_btn = egui::Button::new(RichText::new("Send").strong().size(11.0));
            let send_clicked = ui.add_sized([send_btn_w, 36.0], send_btn).clicked();
            let enter_pressed = is_this_slot_insert && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);

            if (send_clicked || enter_pressed) && !self.input_buffer.trim().is_empty() {
                let prompt = self.input_buffer.trim().to_string();
                self.input_buffer.clear();

                // Record user prompt in turn
                let turn_id = format!("turn-{}", self.turns.len() + 1);
                self.turns.push(TrajectoryTurn {
                    turn_id: turn_id.clone(),
                    user_prompt: Some(prompt.clone()),
                    assistant_text: String::new(),
                    thoughts: String::new(),
                    tools: Vec::new(),
                    approvals: Vec::new(),
                    timestamp: "Just now".to_string(),
                    is_streaming: true,
                });

                if let Some(tid) = &self.thread_id {
                    net.send_turn(tid, &prompt, &self.selected_model, &self.selected_effort, |_| {});
                } else {
                    self.queued_prompt = Some(prompt);
                    self.start_session(net, ui.ctx().clone());
                }
            }
        });
    }
}
