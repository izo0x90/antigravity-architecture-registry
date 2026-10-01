use egui::{Color32, RichText, Ui};
use crate::cockpit::bridge::{AppCommand, AppEvent, CockpitBridge};
use crate::cockpit::chat::ChatStreamState;
use crate::cockpit::code::CodeViewerState;
use crate::cockpit::graph::ArchGraphState;
use crate::cockpit::tree::PlanTreeState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    Chat,
    Code,
    Graph,
    Tree,
}

impl BufferKind {
    pub fn title(&self) -> &'static str {
        match self {
            BufferKind::Chat => "💬 LLM EXECUTION STREAM",
            BufferKind::Code => "📜 CODE VIEWER",
            BufferKind::Graph => "⚡ ARCHITECTURE GRAPH",
            BufferKind::Tree => "🌳 INVARIANTS & PLAN TREE",
        }
    }
}

pub struct CockpitApp {
    pub bridge: CockpitBridge,
    pub chat: ChatStreamState,
    pub code: CodeViewerState,
    pub graph: ArchGraphState,
    pub tree: PlanTreeState,
    pub active_layout: usize, // 1, 2, or 3 columns
    pub slots: [BufferKind; 3],
    pub toast: Option<(String, std::time::Instant)>,
    pub session_id: Option<String>,
}

impl CockpitApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Set up dark theme
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(10, 14, 20);
        visuals.window_fill = Color32::from_rgb(15, 20, 28);
        cc.egui_ctx.set_visuals(visuals);

        let bridge = CockpitBridge::spawn(cc.egui_ctx.clone());

        // Kick off default ACP session
        let _ = bridge.cmd_tx.send(AppCommand::StartSession {
            thread_id: "cockpit-session-1".to_string(),
            cwd: ".".to_string(),
            model: "gemini-3.7-flash-high".to_string(),
            mode: "auto_edit".to_string(),
        });

        Self {
            bridge,
            chat: ChatStreamState::default(),
            code: CodeViewerState::default(),
            graph: ArchGraphState::default(),
            tree: PlanTreeState::default(),
            active_layout: 3,
            slots: [BufferKind::Graph, BufferKind::Chat, BufferKind::Code],
            toast: Some(("Meta-Harness Native egui Cockpit online".to_string(), std::time::Instant::now())),
            session_id: None,
        }
    }

    pub fn set_toast(&mut self, msg: String) {
        self.toast = Some((msg, std::time::Instant::now()));
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.bridge.event_rx.try_recv() {
            match event {
                AppEvent::SessionStarted { session_id, model, mode } => {
                    self.session_id = Some(session_id.clone());
                    self.chat.selected_model = model.clone();
                    self.chat.runtime_mode = mode.clone();
                    self.set_toast(format!("Connected to ACP Session: {session_id}"));
                }
                AppEvent::TurnStarted { turn_id } => {
                    self.chat.start_assistant_turn(&turn_id);
                }
                AppEvent::ContentDelta { delta, is_thought } => {
                    self.chat.append_content_delta(&delta, is_thought);
                }
                AppEvent::TaskUpdated { task_id, title, status } => {
                    self.chat.update_task(&task_id, &title, &status);
                }
                AppEvent::ApprovalRequested { request_id, title, details, options } => {
                    self.chat.add_approval(&request_id, &title, &details, &options);
                }
                AppEvent::TurnCompleted { turn_id } => {
                    self.chat.complete_turn(&turn_id);
                }
                AppEvent::FileLoaded { path, content, highlight_line, .. } => {
                    self.code.load_file(path.clone(), content, highlight_line);
                    // If Code buffer isn't currently visible, switch slot 3 to Code
                    if !self.slots[..self.active_layout].contains(&BufferKind::Code) {
                        self.slots[self.active_layout - 1] = BufferKind::Code;
                    }
                    self.set_toast(format!("Loaded file: {path}"));
                }
                AppEvent::Toast(msg) => {
                    self.set_toast(msg);
                }
                AppEvent::Error(err) => {
                    self.set_toast(format!("⚠️ {err}"));
                }
                AppEvent::ArchitectureFocused {
                    root_id,
                    depth,
                    direction,
                    affected_components,
                } => {
                    self.graph.selected_node = Some(root_id.clone());
                    self.graph.focused_components = affected_components;
                    if !self.slots[..self.active_layout].contains(&BufferKind::Graph) {
                        self.slots[self.active_layout - 1] = BufferKind::Graph;
                    }
                    self.set_toast(format!("🎯 Agent focused architecture: {root_id} (depth {depth}, {direction})"));
                }
                AppEvent::ArchitectureUpdated { component_id, status } => {
                    self.set_toast(format!("📦 Architecture updated: {component_id} -> {status}"));
                }
            }
        }
    }

    fn render_slot_content(&mut self, ui: &mut Ui, buf: BufferKind) {
        match buf {
            BufferKind::Chat => self.chat.ui(ui, &self.bridge.cmd_tx),
            BufferKind::Code => self.code.ui(ui),
            BufferKind::Graph => self.graph.ui(ui),
            BufferKind::Tree => self.tree.ui(ui),
        }
    }
}

impl eframe::App for CockpitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();

        // 1. Top Bar
        egui::TopBottomPanel::top("cockpit_top_panel")
            .frame(egui::Frame::NONE.fill(Color32::from_rgb(13, 17, 23)).inner_margin(8.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("⚡ META-HARNESS COCKPIT").strong().size(13.0).color(Color32::WHITE));
                    ui.add_space(8.0);

                    // Connection status pill
                    let status_pill = if self.session_id.is_some() {
                        RichText::new("● ONLINE").size(10.0).color(Color32::from_rgb(16, 185, 129))
                    } else {
                        RichText::new("○ CONNECTING").size(10.0).color(Color32::from_rgb(245, 158, 11))
                    };
                    ui.label(status_pill);

                    if let Some(sid) = &self.session_id {
                        let short_id = if sid.len() > 8 { &sid[..8] } else { sid };
                        ui.label(RichText::new(format!("({short_id})")).monospace().size(10.0).color(Color32::GRAY));
                    }

                    ui.add_space(16.0);

                    // Layout Mode Toggles (1, 2, 3 columns)
                    ui.label(RichText::new("Layout:").size(11.0).color(Color32::GRAY));
                    for n in 1..=3 {
                        let label = format!("{n}-Col");
                        let is_active = self.active_layout == n;
                        let btn = if is_active {
                            egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
                                .fill(Color32::from_rgb(37, 99, 235))
                        } else {
                            egui::Button::new(RichText::new(label).color(Color32::from_rgb(148, 163, 184)))
                        };
                        if ui.add(btn).clicked() {
                            self.active_layout = n;
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some((msg, created)) = &self.toast
                            && created.elapsed().as_secs() < 6
                        {
                            ui.label(RichText::new(msg).size(11.0).color(Color32::from_rgb(254, 240, 138)));
                        }
                    });
                });
            });

        // 2. Main Multi-Column Viewport
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgb(10, 14, 20)).inner_margin(4.0))
            .show(ctx, |ui| {
                let col_count = self.active_layout;
                let spacing = 6.0;
                let avail_width = ui.available_width();
                let col_width = (avail_width - (col_count as f32 - 1.0) * spacing) / col_count as f32;

                ui.horizontal(|ui| {
                    for slot_idx in 0..col_count {
                        let buf = self.slots[slot_idx];

                        ui.allocate_ui_with_layout(
                            egui::Vec2::new(col_width, ui.available_height()),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                egui::Frame::NONE
                                    .fill(Color32::from_rgb(15, 20, 28))
                                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(30, 41, 59)))
                                    .corner_radius(4.0)
                                    .inner_margin(6.0)
                                    .show(ui, |ui| {
                                        // Slot Header Bar with Buffer Selector
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(buf.title()).strong().size(11.0).color(Color32::from_rgb(147, 197, 253)));
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                let mut selected_buf = buf;
                                                egui::ComboBox::from_id_salt(format!("slot_buf_select_{slot_idx}"))
                                                    .selected_text(match selected_buf {
                                                        BufferKind::Chat => "Chat",
                                                        BufferKind::Code => "Code",
                                                        BufferKind::Graph => "Graph",
                                                        BufferKind::Tree => "Tree",
                                                    })
                                                    .show_ui(ui, |ui| {
                                                        ui.selectable_value(&mut selected_buf, BufferKind::Graph, "Graph");
                                                        ui.selectable_value(&mut selected_buf, BufferKind::Chat, "Chat");
                                                        ui.selectable_value(&mut selected_buf, BufferKind::Code, "Code");
                                                        ui.selectable_value(&mut selected_buf, BufferKind::Tree, "Tree");
                                                    });
                                                if selected_buf != buf {
                                                    self.slots[slot_idx] = selected_buf;
                                                }
                                            });
                                        });
                                        ui.separator();

                                        // Render the Buffer
                                        self.render_slot_content(ui, self.slots[slot_idx]);
                                    });
                            },
                        );
                        if slot_idx + 1 < col_count {
                            ui.add_space(spacing);
                        }
                    }
                });
            });
    }
}
