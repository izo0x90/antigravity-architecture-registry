use egui::{Color32, RichText, Ui};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use crate::cockpit::bridge::AppCommand;

#[derive(Debug, Clone, PartialEq)]
pub enum TurnRole {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct ToolCallItem {
    pub id: String,
    pub title: String,
    pub status: String,
    pub details: String,
    pub target_path: Option<String>,
    pub is_expanded: bool,
}

#[derive(Debug, Clone)]
pub struct ApprovalItem {
    pub request_id: String,
    pub title: String,
    pub details: String,
    pub options: Vec<(String, String)>,
    pub resolved: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Turn {
    pub id: String,
    pub role: TurnRole,
    pub prompt: String,
    pub thought: String,
    pub thought_open: bool,
    pub tool_calls: Vec<ToolCallItem>,
    pub approvals: Vec<ApprovalItem>,
    pub content: String,
    pub timestamp: String,
    pub is_streaming: bool,
}

pub struct ChatStreamState {
    pub thread_id: String,
    pub selected_model: String,
    pub selected_effort: String,
    pub runtime_mode: String,
    pub plan_mode: bool,
    pub is_streaming: bool,
    pub input_text: String,
    pub turns: Vec<Turn>,
    pub active_turn_id: Option<String>,
    pub md_cache: CommonMarkCache,
}

impl Default for ChatStreamState {
    fn default() -> Self {
        Self {
            thread_id: "cockpit-session-1".to_string(),
            selected_model: "gemini-3.7-flash-high".to_string(),
            selected_effort: "high".to_string(),
            runtime_mode: "auto_edit".to_string(),
            plan_mode: false,
            is_streaming: false,
            input_text: String::new(),
            turns: vec![
                Turn {
                    id: "init".to_string(),
                    role: TurnRole::Assistant,
                    prompt: String::new(),
                    thought: String::new(),
                    thought_open: false,
                    tool_calls: Vec::new(),
                    approvals: Vec::new(),
                    content: "### Meta-Harness Native Cockpit Initialized\nReady to drive Antigravity agent sessions. Model set to **Gemini 3.7 Flash** with **Auto-Accept Edits**.".to_string(),
                    timestamp: "Startup".to_string(),
                    is_streaming: false,
                }
            ],
            active_turn_id: None,
            md_cache: CommonMarkCache::default(),
        }
    }
}

impl ChatStreamState {
    pub fn available_models() -> &'static [(&'static str, &'static str)] {
        &[
            ("gemini-3.7-flash-high", "Gemini 3.7 Flash (High)"),
            ("gemini-3.7-flash-medium", "Gemini 3.7 Flash (Medium)"),
            ("gemini-3.7-flash-low", "Gemini 3.7 Flash (Low)"),
            ("gemini-3.8-flash-high", "Gemini 3.8 Flash (High)"),
            ("gemini-3.8-flash-medium", "Gemini 3.8 Flash (Medium)"),
            ("gemini-3.8-flash-low", "Gemini 3.8 Flash (Low)"),
            ("gemini-pro-agent", "Gemini 3.1 Pro (High)"),
            ("gemini-3.1-pro-low", "Gemini 3.1 Pro (Low)"),
            ("gemini-3.6-flash-high", "Gemini 3.6 Flash (High)"),
            ("gemini-3.6-flash-medium", "Gemini 3.6 Flash (Medium)"),
            ("gemini-3.6-flash-low", "Gemini 3.6 Flash (Low)"),
        ]
    }

    pub fn append_user_prompt(&mut self, prompt: String) {
        let now = chrono_now();
        self.turns.push(Turn {
            id: format!("turn-{}", self.turns.len() + 1),
            role: TurnRole::User,
            prompt,
            thought: String::new(),
            thought_open: false,
            tool_calls: Vec::new(),
            approvals: Vec::new(),
            content: String::new(),
            timestamp: now,
            is_streaming: false,
        });
    }

    pub fn start_assistant_turn(&mut self, turn_id: &str) {
        self.is_streaming = true;
        self.active_turn_id = Some(turn_id.to_string());
        self.turns.push(Turn {
            id: turn_id.to_string(),
            role: TurnRole::Assistant,
            prompt: String::new(),
            thought: String::new(),
            thought_open: true,
            tool_calls: Vec::new(),
            approvals: Vec::new(),
            content: String::new(),
            timestamp: chrono_now(),
            is_streaming: true,
        });
    }

    pub fn append_content_delta(&mut self, delta: &str, is_thought: bool) {
        if let Some(turn) = self.turns.iter_mut().rev().find(|t| t.role == TurnRole::Assistant) {
            if is_thought {
                turn.thought.push_str(delta);
            } else {
                turn.content.push_str(delta);
            }
        }
    }

    pub fn update_task(&mut self, task_id: &str, title: &str, status: &str) {
        let target_path = detect_file_in_text(title);
        if let Some(turn) = self.turns.iter_mut().rev().find(|t| t.role == TurnRole::Assistant) {
            if let Some(tool) = turn.tool_calls.iter_mut().find(|t| t.id == task_id) {
                tool.status = status.to_string();
                tool.title = title.to_string();
            } else {
                turn.tool_calls.push(ToolCallItem {
                    id: task_id.to_string(),
                    title: title.to_string(),
                    status: status.to_string(),
                    details: String::new(),
                    target_path,
                    is_expanded: false,
                });
            }
        }
    }

    pub fn add_approval(&mut self, req_id: &str, title: &str, details: &str, options: &[(String, String)]) {
        if let Some(turn) = self.turns.iter_mut().rev().find(|t| t.role == TurnRole::Assistant) {
            turn.approvals.push(ApprovalItem {
                request_id: req_id.to_string(),
                title: title.to_string(),
                details: details.to_string(),
                options: options.to_vec(),
                resolved: None,
            });
        }
    }

    pub fn complete_turn(&mut self, _turn_id: &str) {
        self.is_streaming = false;
        if let Some(turn) = self.turns.iter_mut().rev().find(|t| t.role == TurnRole::Assistant) {
            turn.is_streaming = false;
            turn.thought_open = false; // collapse thinking when done
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cmd_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>) {
        ui.vertical(|ui| {
            // 1. Controls Header
            self.render_controls_bar(ui, cmd_tx);
            ui.separator();

            // 2. Full-Width Execution Stream
            let avail_height = ui.available_height() - 70.0;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(avail_height.max(100.0))
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    let mut open_file_request: Option<String> = None;
                    let mut approval_action: Option<(String, String)> = None;

                    for turn in &mut self.turns {
                        render_turn(ui, turn, &mut self.md_cache, &mut open_file_request, &mut approval_action);
                        ui.add_space(8.0);
                    }

                    if let Some(path) = open_file_request {
                        let _ = cmd_tx.send(AppCommand::ReadFile { path, line: None });
                    }
                    if let Some((req_id, dec)) = approval_action {
                        let _ = cmd_tx.send(AppCommand::ApproveTool {
                            thread_id: self.thread_id.clone(),
                            request_id: req_id,
                            decision: dec,
                        });
                    }
                });

            ui.separator();

            // 3. Command Chips & Persistent Prompt Input
            self.render_input_bar(ui, cmd_tx);
        });
    }

    fn render_controls_bar(&mut self, ui: &mut Ui, cmd_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Model:").strong().size(11.0));

            // Model ComboBox
            let current_label = Self::available_models()
                .iter()
                .find(|(slug, _)| *slug == self.selected_model)
                .map(|(_, name)| *name)
                .unwrap_or(&self.selected_model);

            let prev_model = self.selected_model.clone();
            egui::ComboBox::from_id_salt("chat_model_select")
                .selected_text(current_label)
                .show_ui(ui, |ui| {
                    for (slug, name) in Self::available_models() {
                        ui.selectable_value(&mut self.selected_model, slug.to_string(), *name);
                    }
                });

            if prev_model != self.selected_model {
                // Bi-directionally sync effort
                if self.selected_model.ends_with("-low") {
                    self.selected_effort = "low".to_string();
                } else if self.selected_model.ends_with("-medium") {
                    self.selected_effort = "medium".to_string();
                } else {
                    self.selected_effort = "high".to_string();
                }
                let _ = cmd_tx.send(AppCommand::SteerSession {
                    thread_id: self.thread_id.clone(),
                    model: Some(self.selected_model.clone()),
                    mode: Some(self.runtime_mode.clone()),
                });
            }

            // Effort Selector
            ui.label(RichText::new("Effort:").strong().size(11.0));
            let prev_effort = self.selected_effort.clone();
            egui::ComboBox::from_id_salt("chat_effort_select")
                .selected_text(&self.selected_effort)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.selected_effort, "high".to_string(), "High");
                    ui.selectable_value(&mut self.selected_effort, "medium".to_string(), "Medium");
                    ui.selectable_value(&mut self.selected_effort, "low".to_string(), "Low");
                });

            if prev_effort != self.selected_effort {
                // Switch model family variant
                let base = if self.selected_model.starts_with("gemini-3.7-flash") {
                    "gemini-3.7-flash"
                } else if self.selected_model.starts_with("gemini-3.8-flash") {
                    "gemini-3.8-flash"
                } else if self.selected_model.starts_with("gemini-3.6-flash") {
                    "gemini-3.6-flash"
                } else {
                    "gemini-pro"
                };

                let new_slug = if base == "gemini-pro" {
                    if self.selected_effort == "low" { "gemini-3.1-pro-low" } else { "gemini-pro-agent" }
                } else {
                    match self.selected_effort.as_str() {
                        "low" => match base {
                            "gemini-3.7-flash" => "gemini-3.7-flash-low",
                            "gemini-3.8-flash" => "gemini-3.8-flash-low",
                            _ => "gemini-3.6-flash-low",
                        },
                        "medium" => match base {
                            "gemini-3.7-flash" => "gemini-3.7-flash-medium",
                            "gemini-3.8-flash" => "gemini-3.8-flash-medium",
                            _ => "gemini-3.6-flash-medium",
                        },
                        _ => match base {
                            "gemini-3.7-flash" => "gemini-3.7-flash-high",
                            "gemini-3.8-flash" => "gemini-3.8-flash-high",
                            _ => "gemini-3.6-flash-high",
                        },
                    }
                };
                self.selected_model = new_slug.to_string();
                let _ = cmd_tx.send(AppCommand::SteerSession {
                    thread_id: self.thread_id.clone(),
                    model: Some(self.selected_model.clone()),
                    mode: Some(self.runtime_mode.clone()),
                });
            }

            // Mode Selector
            ui.label(RichText::new("Mode:").strong().size(11.0));
            let prev_mode = self.runtime_mode.clone();
            let mode_label = match self.runtime_mode.as_str() {
                "auto_edit" => "⚡ Auto-Accept Edits",
                "yolo" => "🔥 Full Access YOLO",
                _ => "🛡️ Ask on Tools",
            };
            egui::ComboBox::from_id_salt("chat_mode_select")
                .selected_text(mode_label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.runtime_mode, "auto_edit".to_string(), "⚡ Auto-Accept Edits");
                    ui.selectable_value(&mut self.runtime_mode, "default".to_string(), "🛡️ Ask on Tools");
                    ui.selectable_value(&mut self.runtime_mode, "yolo".to_string(), "🔥 Full Access YOLO");
                });

            if prev_mode != self.runtime_mode {
                let _ = cmd_tx.send(AppCommand::SteerSession {
                    thread_id: self.thread_id.clone(),
                    model: Some(self.selected_model.clone()),
                    mode: Some(self.runtime_mode.clone()),
                });
            }

            // Plan Mode Toggle
            let plan_btn_text = if self.plan_mode {
                RichText::new("📋 Plan Mode Active").color(Color32::from_rgb(16, 185, 129)).strong()
            } else {
                RichText::new("📋 Plan Mode").color(Color32::GRAY)
            };
            if ui.button(plan_btn_text).clicked() {
                self.plan_mode = !self.plan_mode;
            }

            // Cancel Button (if active)
            if self.is_streaming
                && ui.button(RichText::new("🛑 Stop").color(Color32::RED).strong()).clicked()
            {
                let _ = cmd_tx.send(AppCommand::CancelTurn { thread_id: self.thread_id.clone() });
            }
        });
    }

    fn render_input_bar(&mut self, ui: &mut Ui, cmd_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("QUICK:").size(10.0).color(Color32::GRAY));
            if ui.button(RichText::new("/plan").monospace().size(10.0)).clicked() {
                self.plan_mode = !self.plan_mode;
            }
            if ui.button(RichText::new("/logout").monospace().size(10.0)).clicked() {
                let _ = cmd_tx.send(AppCommand::StopSession { thread_id: self.thread_id.clone() });
            }
        });

        ui.horizontal(|ui| {
            let placeholder = if self.plan_mode {
                "Describe task to PLAN (will prepend /plan)..."
            } else {
                "Type instruction or question..."
            };

            let text_edit = egui::TextEdit::singleline(&mut self.input_text)
                .id(egui::Id::new("cockpit_prompt_input"))
                .hint_text(placeholder)
                .desired_width(ui.available_width() - 80.0);

            let res = ui.add(text_edit);
            let enter_pressed = res.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            let submit_clicked = ui.button(RichText::new("Send ↵").strong()).clicked();

            if (enter_pressed || submit_clicked) && !self.input_text.trim().is_empty() {
                let mut prompt = self.input_text.trim().to_string();
                if self.plan_mode && !prompt.starts_with('/') {
                    prompt = format!("/plan {prompt}");
                }
                self.append_user_prompt(prompt.clone());
                let _ = cmd_tx.send(AppCommand::SendPrompt {
                    thread_id: self.thread_id.clone(),
                    prompt,
                });
                self.input_text.clear();
                res.request_focus(); // Maintain permanent typing focus
            }
        });
    }
}

fn render_turn(
    ui: &mut Ui,
    turn: &mut Turn,
    md_cache: &mut CommonMarkCache,
    open_file: &mut Option<String>,
    approval_action: &mut Option<(String, String)>,
) {
    match turn.role {
        TurnRole::User => {
            egui::Frame::NONE
                .fill(Color32::from_rgb(24, 38, 64))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(45, 65, 105)))
                .corner_radius(4.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("👤 USER").strong().size(11.0).color(Color32::from_rgb(147, 197, 253)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(&turn.timestamp).size(10.0).color(Color32::GRAY));
                        });
                    });
                    ui.add_space(2.0);
                    ui.label(RichText::new(&turn.prompt).size(13.0).color(Color32::WHITE));
                });
        }
        TurnRole::Assistant => {
            egui::Frame::NONE
                .fill(Color32::from_rgb(18, 22, 31))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(34, 42, 59)))
                .corner_radius(4.0)
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("🤖 ASSISTANT").strong().size(11.0).color(Color32::from_rgb(52, 211, 153)));
                        if turn.is_streaming {
                            ui.spinner();
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(&turn.timestamp).size(10.0).color(Color32::GRAY));
                        });
                    });
                    ui.add_space(6.0);

                    // 1. Collapsible Reasoning Box
                    if !turn.thought.is_empty() {
                        let header_text = format!("🧠 Reasoning ({} chars)", turn.thought.len());
                        egui::CollapsingHeader::new(RichText::new(header_text).size(11.0).color(Color32::from_rgb(148, 163, 184)))
                            .default_open(turn.thought_open)
                            .show(ui, |ui| {
                                egui::Frame::NONE
                                    .fill(Color32::from_rgb(12, 15, 22))
                                    .inner_margin(6.0)
                                    .corner_radius(3.0)
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(&turn.thought).monospace().size(11.0).color(Color32::from_rgb(160, 174, 192)));
                                    });
                            });
                        ui.add_space(4.0);
                    }

                    // 2. Tool Executions
                    for tool in &mut turn.tool_calls {
                        render_tool_card(ui, tool, open_file);
                        ui.add_space(4.0);
                    }

                    // 3. Approvals
                    for approval in &mut turn.approvals {
                        render_approval_card(ui, approval, approval_action);
                        ui.add_space(4.0);
                    }

                    // 4. Markdown Assistant Response
                    if !turn.content.is_empty() {
                        CommonMarkViewer::new().show(ui, md_cache, &turn.content);
                    }
                });
        }
    }
}

fn render_tool_card(ui: &mut Ui, tool: &mut ToolCallItem, open_file: &mut Option<String>) {
    let (status_icon, status_color) = match tool.status.to_lowercase().as_str() {
        "completed" | "done" | "success" => ("✅ DONE", Color32::from_rgb(16, 185, 129)),
        "error" | "failed" => ("❌ ERROR", Color32::from_rgb(239, 68, 68)),
        _ => ("⏳ RUNNING", Color32::from_rgb(245, 158, 11)),
    };

    egui::Frame::NONE
        .fill(Color32::from_rgb(15, 23, 42))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
        .corner_radius(4.0)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("🔧").size(11.0));
                ui.label(RichText::new(&tool.title).strong().size(11.0).color(Color32::WHITE));
                ui.label(RichText::new(status_icon).size(10.0).color(status_color));

                if let Some(path) = &tool.target_path
                    && ui.button(RichText::new(format!("📂 Open {path}")).size(10.0)).clicked()
                {
                    *open_file = Some(path.clone());
                }
            });
        });
}

fn render_approval_card(ui: &mut Ui, approval: &mut ApprovalItem, action: &mut Option<(String, String)>) {
    egui::Frame::NONE
        .fill(Color32::from_rgb(38, 28, 10))
        .stroke(egui::Stroke::new(1.5, Color32::from_rgb(245, 158, 11)))
        .corner_radius(4.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.label(RichText::new(format!("🛡️ {}", approval.title)).strong().color(Color32::from_rgb(245, 158, 11)));
            if !approval.details.is_empty() {
                ui.label(RichText::new(&approval.details).size(11.0).color(Color32::from_rgb(203, 213, 225)));
            }

            if let Some(res) = &approval.resolved {
                ui.label(RichText::new(format!("Resolved: {res}")).color(Color32::GRAY).size(11.0));
            } else {
                ui.horizontal(|ui| {
                    for (dec, lbl) in &approval.options {
                        let btn = if dec == "accept" || dec == "approve" {
                            egui::Button::new(RichText::new(lbl).strong().color(Color32::WHITE))
                                .fill(Color32::from_rgb(16, 185, 129))
                        } else {
                            egui::Button::new(RichText::new(lbl).color(Color32::WHITE))
                                .fill(Color32::from_rgb(239, 68, 68))
                        };

                        if ui.add(btn).clicked() {
                            approval.resolved = Some(dec.clone());
                            *action = Some((approval.request_id.clone(), dec.clone()));
                        }
                    }
                });
            }
        });
}

fn detect_file_in_text(text: &str) -> Option<String> {
    for part in text.split_whitespace() {
        let clean = part.trim_matches(|c| c == '\'' || c == '"' || c == '`' || c == '(' || c == ')');
        if clean.contains('.') && (clean.ends_with(".rs") || clean.ends_with(".toml") || clean.ends_with(".json") || clean.ends_with(".md")) {
            return Some(clean.to_string());
        }
    }
    None
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let hours = (now / 3600) % 24;
    let minutes = (now / 60) % 60;
    let seconds = now % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}
