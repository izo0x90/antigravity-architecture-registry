use egui::{Color32, Frame, Margin, Pos2, RichText, Stroke};

use crate::chat::ChatStreamState;
use crate::code::CodeViewerState;
use crate::feedback::FeedbackInspectorState;
use crate::graph::ArchitectureGraphState;
use crate::net::NetClient;
use crate::tree::ArchitectureTreeState;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BufferKind {
    Chat,
    Code,
    Graph,
    Tree,
}

impl BufferKind {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Chat => "CHAT",
            Self::Code => "CODE",
            Self::Graph => "GRAPH",
            Self::Tree => "TREE",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::Chat => egui_phosphor::regular::CHAT_TEARDROP_TEXT,
            Self::Code => egui_phosphor::regular::CODE,
            Self::Graph => egui_phosphor::regular::GIT_FORK,
            Self::Tree => egui_phosphor::regular::TREE_STRUCTURE,
        }
    }

    pub fn supports_insert_mode(&self) -> bool {
        matches!(self, Self::Chat)
    }
}

pub struct CockpitApp {
    pub net: NetClient,
    pub chat: ChatStreamState,
    pub code: CodeViewerState,
    pub graph: ArchitectureGraphState,
    pub tree: ArchitectureTreeState,
    pub feedback: FeedbackInspectorState,

    // Layout configuration
    pub column_count: usize, // 1, 2, or 3
    pub slot_buffers: [BufferKind; 3],
    pub focused_slot: usize,
    pub initialized: bool,

    // Modal Input & Configurable Keybindings
    pub keymap: crate::keymap::KeymapConfig,
    pub input_mode: crate::keymap::InputMode,
    pub gesture_tracker: crate::keymap::SpaceGestureTracker,
}

impl CockpitApp {
    pub fn new(host_origin: &str) -> Self {
        Self {
            net: NetClient::new(host_origin),
            chat: ChatStreamState::default(),
            code: CodeViewerState::default(),
            graph: ArchitectureGraphState::default(),
            tree: ArchitectureTreeState::default(),
            feedback: FeedbackInspectorState::default(),

            column_count: 2,
            slot_buffers: [BufferKind::Chat, BufferKind::Graph, BufferKind::Code],
            focused_slot: 0,
            initialized: false,

            keymap: crate::keymap::KeymapConfig::default(),
            input_mode: crate::keymap::InputMode::Normal,
            gesture_tracker: crate::keymap::SpaceGestureTracker::default(),
        }
    }

    pub fn update_lifecycle(&mut self, ctx: &egui::Context) {
        if !self.initialized {
            self.initialized = true;
            let mut fonts = egui::FontDefinitions::default();
            egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
            if let Some(font_keys) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                font_keys.insert(1, "phosphor".into());
            }
            ctx.set_fonts(fonts);

            self.net.connect_ws(ctx.clone());
            self.net.fetch_models(ctx.clone());

            // Auto-start active harness session
            let ctx_chat = ctx.clone();
            self.chat.start_session(&self.net, ctx_chat);

            // Auto-load system_architecture.json into graph
            let ctx_arch = ctx.clone();
            self.graph.load_architecture(&self.net, ctx_arch);

            // Auto-load default file Cargo.toml into code viewer
            let ctx_file = ctx.clone();
            let net_ref = &self.net;
            self.code.load_file("Cargo.toml", None, net_ref, ctx_file);

            // Auto-load dev feedback issues
            let ctx_feedback = ctx.clone();
            self.feedback.load_issues(&self.net, ctx_feedback);
        }

        // Poll WebSocket messages
        self.net.poll_ws();

        // Process incoming events
        let events = {
            if let Ok(mut lock) = self.net.incoming_events.lock() {
                let drained: Vec<_> = lock.drain(..).collect();
                drained
            } else {
                Vec::new()
            }
        };

        for event in events {
            self.chat.process_event(event);
        }

        // Handle auto-jump requests from chat/tool links
        if let Some((path, line)) = self.chat.requested_file_jump.take() {
            let ctx_clone = ctx.clone();
            self.code.load_file(&path, line, &self.net, ctx_clone);

            // If Code buffer is not visible in current slots, swap current slot or set slot to Code
            let is_code_visible = self.slot_buffers[..self.column_count].contains(&BufferKind::Code);
            if !is_code_visible {
                if self.column_count == 1 {
                    self.slot_buffers[0] = BufferKind::Code;
                } else {
                    self.slot_buffers[1] = BufferKind::Code;
                }
            }
        }

        // Handle global keyboard shortcuts
        self.handle_keyboard_shortcuts(ctx);
    }

    pub fn execute_global_action(&mut self, action: crate::keymap::GlobalAction, ctx: &egui::Context) {
        match action {
            crate::keymap::GlobalAction::FocusSlot(slot) => {
                if slot < self.column_count {
                    self.focused_slot = slot;
                }
            }
            crate::keymap::GlobalAction::CycleSlot(dir) => {
                if self.column_count > 0 {
                    if dir > 0 {
                        self.focused_slot = (self.focused_slot + 1) % self.column_count;
                    } else {
                        self.focused_slot = if self.focused_slot == 0 {
                            self.column_count - 1
                        } else {
                            self.focused_slot - 1
                        };
                    }
                }
            }
            crate::keymap::GlobalAction::SwitchBuffer { slot, kind } => {
                if slot < self.slot_buffers.len() {
                    self.slot_buffers[slot] = kind;
                }
            }
            crate::keymap::GlobalAction::SetLayout(cols) => {
                self.column_count = cols.clamp(1, 3);
                if self.focused_slot >= self.column_count {
                    self.focused_slot = self.column_count - 1;
                }
            }
            crate::keymap::GlobalAction::ToggleInspector => {
                self.feedback.toggle_inspection();
            }
            crate::keymap::GlobalAction::ToggleIssues => {
                self.feedback.is_issues_window_open = !self.feedback.is_issues_window_open;
                if self.feedback.is_issues_window_open {
                    self.feedback.load_issues(&self.net, ctx.clone());
                }
            }
            crate::keymap::GlobalAction::CloseActiveDrawer => {
                self.graph.execute_action(crate::keymap::GraphAction::CloseInspector);
            }
        }
    }

    fn handle_keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        // Step 1: Space Tap-vs-Drag Gesture Tracking
        let (space_pressed, space_released, pointer_dragged) = ctx.input(|i| {
            let pressed = i.key_pressed(egui::Key::Space);
            let released = i.key_released(egui::Key::Space);
            let dragged = i.pointer.primary_down() && (i.pointer.delta().length_sq() > 2.0 || i.pointer.is_decidedly_dragging());
            (pressed, released, dragged)
        });

        if space_pressed {
            self.gesture_tracker.on_space_pressed();
        }
        if pointer_dragged {
            self.gesture_tracker.on_drag();
        }
        let space_tapped = space_released && self.gesture_tracker.on_space_released();

        // Step 2: Handle Mode-Specific Key Event Dispatching
        match self.input_mode {
            crate::keymap::InputMode::Insert { .. } => {
                let exit_pressed = ctx.input(|i| self.keymap.global.exit_insert.is_pressed(i) || i.key_pressed(egui::Key::Escape));
                if exit_pressed {
                    self.input_mode = crate::keymap::InputMode::Normal;
                    ctx.memory_mut(|m| {
                        if let Some(id) = m.focused() {
                            m.surrender_focus(id);
                        }
                    });
                }
                // In insert mode, all other keys flow into the text inputs
                return;
            }

            crate::keymap::InputMode::Leader(prefix) => {
                // If user taps Space again while Leader HUD is open, dismiss it
                if space_tapped {
                    self.input_mode = crate::keymap::InputMode::Normal;
                    return;
                }

                // Check for keys bound in leader tables
                let mut leader_res = crate::keymap::LeaderResult::None;
                ctx.input(|i| {
                    for key in [
                        egui::Key::Num1, egui::Key::Num2, egui::Key::Num3,
                        egui::Key::B, egui::Key::L, egui::Key::I, egui::Key::D, egui::Key::X,
                        egui::Key::C, egui::Key::V, egui::Key::G, egui::Key::T,
                        egui::Key::Escape,
                    ] {
                        if i.key_pressed(key) {
                            leader_res = crate::keymap::resolve_leader_key(prefix, key, self.focused_slot);
                            break;
                        }
                    }
                });

                match leader_res {
                    crate::keymap::LeaderResult::Navigate(next_prefix) => {
                        self.input_mode = crate::keymap::InputMode::Leader(next_prefix);
                    }
                    crate::keymap::LeaderResult::Execute(action) => {
                        self.execute_global_action(action, ctx);
                        self.input_mode = crate::keymap::InputMode::Normal;
                    }
                    crate::keymap::LeaderResult::Dismiss => {
                        self.input_mode = crate::keymap::InputMode::Normal;
                    }
                    crate::keymap::LeaderResult::None => {}
                }
                return;
            }

            crate::keymap::InputMode::Normal => {
                // Space tap opens Which-Key Leader HUD
                if space_tapped {
                    self.input_mode = crate::keymap::InputMode::Leader(crate::keymap::LeaderPrefix::Root);
                    return;
                }

                let mut action_to_execute = None;
                let mut mode_to_set = None;

                ctx.input(|i| {
                    // 1. Global Inspector / Issues toggles
                    if self.keymap.global.toggle_inspector.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::ToggleInspector);
                        return;
                    }
                    if self.keymap.global.toggle_issues.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::ToggleIssues);
                        return;
                    }

                    // 2. Layout shortcuts (Alt+1, Alt+2, Alt+3)
                    if self.keymap.global.layout_1.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::SetLayout(1));
                        return;
                    }
                    if self.keymap.global.layout_2.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::SetLayout(2));
                        return;
                    }
                    if self.keymap.global.layout_3.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::SetLayout(3));
                        return;
                    }

                    // 3. Slot Navigation (1, 2, 3, Tab, Shift+Tab)
                    if self.keymap.global.focus_slot_1.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::FocusSlot(0));
                        return;
                    }
                    if self.keymap.global.focus_slot_2.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::FocusSlot(1));
                        return;
                    }
                    if self.keymap.global.focus_slot_3.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::FocusSlot(2));
                        return;
                    }
                    if self.keymap.global.next_slot.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::CycleSlot(1));
                        return;
                    }
                    if self.keymap.global.prev_slot.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::CycleSlot(-1));
                        return;
                    }

                    // 4. Buffer switching for focused slot (c, v, g, t)
                    if self.keymap.global.switch_chat.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::SwitchBuffer {
                            slot: self.focused_slot,
                            kind: BufferKind::Chat,
                        });
                        return;
                    }
                    if self.keymap.global.switch_code.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::SwitchBuffer {
                            slot: self.focused_slot,
                            kind: BufferKind::Code,
                        });
                        return;
                    }
                    if self.keymap.global.switch_graph.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::SwitchBuffer {
                            slot: self.focused_slot,
                            kind: BufferKind::Graph,
                        });
                        return;
                    }
                    if self.keymap.global.switch_tree.is_pressed(i) {
                        action_to_execute = Some(crate::keymap::GlobalAction::SwitchBuffer {
                            slot: self.focused_slot,
                            kind: BufferKind::Tree,
                        });
                        return;
                    }

                    // 5. Enter Insert mode on focused slot ('i') ONLY if buffer supports insert mode
                    if self.keymap.global.enter_insert.is_pressed(i) {
                        let current_buf = self.slot_buffers[self.focused_slot];
                        if current_buf.supports_insert_mode() {
                            mode_to_set = Some(crate::keymap::InputMode::Insert { slot: self.focused_slot });
                        }
                        return;
                    }

                    // 6. View-Specific Input Routing for Focused Slot
                    let focused_buffer = self.slot_buffers[self.focused_slot];
                    match focused_buffer {
                        BufferKind::Graph => {
                            self.graph.handle_input(i, &self.keymap.graph);
                        }
                        BufferKind::Chat => {
                            self.chat.handle_input(i, &self.keymap.chat, &mut self.input_mode, self.focused_slot);
                        }
                        BufferKind::Code => {
                            self.code.handle_input(i, &self.keymap.code);
                        }
                        BufferKind::Tree => {
                            self.tree.handle_input(i, &self.keymap.tree);
                        }
                    }
                });

                if let Some(action) = action_to_execute {
                    self.execute_global_action(action, ctx);
                }
                if let Some(mode) = mode_to_set {
                    self.input_mode = mode;
                }
            }
        }
    }

    pub fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        let top_rect = ui.available_rect_before_wrap();
        self.feedback.register_inspectable(ui, "top_bar", top_rect);

        Frame::NONE
            .fill(Color32::from_rgb(14, 17, 23))
            .inner_margin(Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let brand_res = ui.horizontal(|ui| {
                        ui.label(RichText::new("ANTIGRAVITY COCKPIT").strong().size(13.0).color(Color32::WHITE));
                        ui.label(RichText::new("v2.0").monospace().size(10.0).color(Color32::from_rgb(100, 160, 240)));
                    });
                    self.feedback.register_inspectable(ui, "top_bar:brand", brand_res.response.rect);

                    ui.add_space(8.0);

                    // Connection status
                    let is_conn = self.net.is_connected.lock().map(|c| *c).unwrap_or(false);
                    let (status_text, status_color) = if is_conn {
                        ("●", Color32::from_rgb(80, 200, 120))
                    } else {
                        ("○", Color32::from_rgb(220, 80, 80))
                    };
                    let conn_res = ui.label(RichText::new(status_text).monospace().size(11.0).strong().color(status_color));
                    self.feedback.register_inspectable(ui, "top_bar:connection_status", conn_res.rect);

                    ui.add_space(8.0);

                    // Input Mode Indicator (NORMAL vs LEADER vs INSERT)
                    let (mode_label, mode_color, mode_bg) = match self.input_mode {
                        crate::keymap::InputMode::Normal => (
                            "NORMAL".to_string(),
                            Color32::from_rgb(140, 190, 255),
                            Color32::from_rgb(20, 30, 48),
                        ),
                        crate::keymap::InputMode::Leader(prefix) => (
                            match prefix {
                                crate::keymap::LeaderPrefix::Root => "LEADER".to_string(),
                                crate::keymap::LeaderPrefix::Buffer => "LEADER > BUFFER".to_string(),
                                crate::keymap::LeaderPrefix::Layout => "LEADER > LAYOUT".to_string(),
                            },
                            Color32::from_rgb(255, 205, 50),
                            Color32::from_rgb(48, 40, 15),
                        ),
                        crate::keymap::InputMode::Insert { slot } => (
                            format!("INSERT [S{}]", slot + 1),
                            Color32::from_rgb(100, 240, 150),
                            Color32::from_rgb(20, 48, 30),
                        ),
                    };

                    Frame::NONE
                        .fill(mode_bg)
                        .stroke(Stroke::new(1.0, mode_color))
                        .corner_radius(3)
                        .inner_margin(Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(RichText::new(mode_label).monospace().strong().size(10.5).color(mode_color));
                        });

                    // Right-aligned controls group (Inspector, Issues, Layout)
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Inspector button
                        let (pencil_label, pencil_color) = if self.feedback.is_inspecting {
                            (format!("{} ACTIVE", egui_phosphor::regular::CROSSHAIR), Color32::from_rgb(250, 180, 50))
                        } else {
                            (format!("{} Inspector", egui_phosphor::regular::PENCIL_SIMPLE), Color32::WHITE)
                        };

                        let insp_btn = ui.button(RichText::new(&pencil_label).strong().color(pencil_color));
                        if insp_btn.clicked() {
                            self.feedback.toggle_inspection();
                        }
                        self.feedback.register_inspectable(ui, "top_bar:inspector_button", insp_btn.rect);

                        // Issues button
                        let issues_label = if self.feedback.issues.is_empty() {
                            format!("{} Issues", egui_phosphor::regular::WARNING_CIRCLE)
                        } else {
                            format!("{} Issues ({})", egui_phosphor::regular::WARNING_CIRCLE, self.feedback.issues.len())
                        };
                        let issues_btn = ui.button(RichText::new(&issues_label).color(Color32::from_rgb(140, 200, 255)));
                        if issues_btn.clicked() {
                            self.feedback.is_issues_window_open = !self.feedback.is_issues_window_open;
                            if self.feedback.is_issues_window_open {
                                self.feedback.load_issues(&self.net, ui.ctx().clone());
                            }
                        }
                        self.feedback.register_inspectable(ui, "top_bar:issues_button", issues_btn.rect);

                        ui.add_space(8.0);

                        // Layout Mode Selector (1, 2, 3 cols)
                        let layout_res = ui.horizontal(|ui| {
                            ui.label(RichText::new("Layout:").size(11.0).color(Color32::GRAY));
                            ui.selectable_value(&mut self.column_count, 1, "1");
                            ui.selectable_value(&mut self.column_count, 2, "2");
                            ui.selectable_value(&mut self.column_count, 3, "3");
                        });
                        self.feedback.register_inspectable(ui, "top_bar:layout_selector", layout_res.response.rect);
                    });
                });
            });
    }

    pub fn render_slot(&mut self, slot_idx: usize, ui: &mut egui::Ui) {
        let is_focused = self.focused_slot == slot_idx;
        let border_color = if is_focused {
            Color32::from_rgb(80, 140, 220)
        } else {
            Color32::from_rgb(30, 36, 48)
        };

        let slot_id = format!("slot_{}", slot_idx + 1);
        self.feedback.register_inspectable(ui, &slot_id, ui.available_rect_before_wrap());

        Frame::NONE
            .fill(Color32::from_rgb(12, 14, 18))
            .stroke(Stroke::new(1.0, border_color))
            .corner_radius(4)
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(ui.available_height());

                ui.vertical(|ui| {
                    // Synchronized Slot Header
                    let active_kind = self.slot_buffers[slot_idx];
                    let is_compact = ui.available_width() < 340.0;

                    let header_res = ui.horizontal(|ui| {
                        let title_text = if is_compact {
                            format!("[S{}]", slot_idx + 1)
                        } else {
                            format!("[SLOT {}: {}]", slot_idx + 1, active_kind.title())
                        };
                        let slot_label = ui.selectable_label(
                            is_focused,
                            RichText::new(title_text)
                                .strong()
                                .monospace()
                                .size(11.0)
                        );
                        if slot_label.clicked() {
                            self.focused_slot = slot_idx;
                        }

                        // Buffer tabs for this slot
                        let (chat_lbl, code_lbl, graph_lbl, tree_lbl) = if is_compact {
                            (
                                egui_phosphor::regular::CHAT_TEARDROP_TEXT.to_string(),
                                egui_phosphor::regular::CODE.to_string(),
                                egui_phosphor::regular::GIT_FORK.to_string(),
                                egui_phosphor::regular::TREE_STRUCTURE.to_string(),
                            )
                        } else {
                            (
                                format!("{} Chat", egui_phosphor::regular::CHAT_TEARDROP_TEXT),
                                format!("{} Code", egui_phosphor::regular::CODE),
                                format!("{} Graph", egui_phosphor::regular::GIT_FORK),
                                format!("{} Tree", egui_phosphor::regular::TREE_STRUCTURE),
                            )
                        };

                        let t_chat = ui.selectable_value(&mut self.slot_buffers[slot_idx], BufferKind::Chat, &chat_lbl);
                        let t_code = ui.selectable_value(&mut self.slot_buffers[slot_idx], BufferKind::Code, &code_lbl);
                        let t_graph = ui.selectable_value(&mut self.slot_buffers[slot_idx], BufferKind::Graph, &graph_lbl);
                        let t_tree = ui.selectable_value(&mut self.slot_buffers[slot_idx], BufferKind::Tree, &tree_lbl);

                        if t_chat.clicked() || t_code.clicked() || t_graph.clicked() || t_tree.clicked() {
                            self.focused_slot = slot_idx;
                        }

                        self.feedback.register_inspectable(ui, &format!("slot_{}:tab:chat", slot_idx + 1), t_chat.rect);
                        self.feedback.register_inspectable(ui, &format!("slot_{}:tab:code", slot_idx + 1), t_code.rect);
                        self.feedback.register_inspectable(ui, &format!("slot_{}:tab:graph", slot_idx + 1), t_graph.rect);
                        self.feedback.register_inspectable(ui, &format!("slot_{}:tab:tree", slot_idx + 1), t_tree.rect);
                    });
                    self.feedback.register_inspectable(ui, &format!("slot_{}:header", slot_idx + 1), header_res.response.rect);
                    ui.separator();

                    // Render the active buffer content
                    let dynamic_models = self.net.models.lock().map(|m| m.clone()).unwrap_or_default();
                    match self.slot_buffers[slot_idx] {
                        BufferKind::Chat => {
                            self.chat.render(
                                ui,
                                &self.net,
                                &dynamic_models,
                                &mut self.feedback,
                                slot_idx,
                                is_focused,
                                &mut self.input_mode,
                            );
                        }
                        BufferKind::Code => {
                            self.code.render(ui, &self.net, &mut self.feedback, slot_idx);
                        }
                        BufferKind::Graph => {
                            self.graph.render(ui, &mut self.feedback, slot_idx);
                        }
                        BufferKind::Tree => {
                            self.tree.render(ui, &self.graph.components, &mut self.feedback, slot_idx);
                        }
                    }
                });
            });

        // Frame-level click-to-focus detection
        if ui.input(|i| i.pointer.primary_clicked()) {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                let slot_bounds = ui.min_rect();
                if slot_bounds.contains(pos) {
                    self.focused_slot = slot_idx;
                }
            }
        }
    }

    pub fn render_which_key_hud(&mut self, ctx: &egui::Context) {
        let prefix = match self.input_mode {
            crate::keymap::InputMode::Leader(p) => p,
            _ => return,
        };

        let screen_rect = ctx.content_rect();
        let hud_w = 540.0;
        let hud_h = 36.0;
        let hud_pos = Pos2::new((screen_rect.width() - hud_w) / 2.0, screen_rect.max.y - hud_h - 16.0);

        let mut next_mode = None;
        let mut action_to_run = None;

        egui::Area::new(egui::Id::new("which_key_floating_hud"))
            .fixed_pos(hud_pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(Color32::from_rgba_premultiplied(16, 21, 32, 245))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(56, 120, 210)))
                    .corner_radius(8)
                    .inner_margin(Margin::symmetric(14, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let prefix_name = match prefix {
                                crate::keymap::LeaderPrefix::Root => "SPACE",
                                crate::keymap::LeaderPrefix::Buffer => "SPACE > BUFFER",
                                crate::keymap::LeaderPrefix::Layout => "SPACE > LAYOUT",
                            };
                            ui.colored_label(Color32::from_rgb(90, 200, 255), RichText::new(prefix_name).strong().monospace().size(11.0));
                            ui.separator();

                            let entries = crate::keymap::which_key_entries(prefix);
                            for (key_str, label_str) in entries {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    let btn = egui::Button::new(
                                        RichText::new(*key_str)
                                            .color(Color32::from_rgb(255, 215, 0))
                                            .monospace()
                                            .strong()
                                            .size(11.0),
                                    )
                                    .fill(Color32::from_rgba_premultiplied(35, 48, 68, 220))
                                    .corner_radius(4);

                                    if ui.add(btn).clicked() {
                                        if *key_str == "Esc" {
                                            if prefix == crate::keymap::LeaderPrefix::Root {
                                                next_mode = Some(crate::keymap::InputMode::Normal);
                                            } else {
                                                next_mode = Some(crate::keymap::InputMode::Leader(crate::keymap::LeaderPrefix::Root));
                                            }
                                        } else if *key_str == "b" {
                                            next_mode = Some(crate::keymap::InputMode::Leader(crate::keymap::LeaderPrefix::Buffer));
                                        } else if *key_str == "l" {
                                            next_mode = Some(crate::keymap::InputMode::Leader(crate::keymap::LeaderPrefix::Layout));
                                        } else if *key_str == "c" {
                                            action_to_run = Some(crate::keymap::GlobalAction::SwitchBuffer { slot: self.focused_slot, kind: BufferKind::Chat });
                                        } else if *key_str == "v" {
                                            action_to_run = Some(crate::keymap::GlobalAction::SwitchBuffer { slot: self.focused_slot, kind: BufferKind::Code });
                                        } else if *key_str == "g" {
                                            action_to_run = Some(crate::keymap::GlobalAction::SwitchBuffer { slot: self.focused_slot, kind: BufferKind::Graph });
                                        } else if *key_str == "t" {
                                            action_to_run = Some(crate::keymap::GlobalAction::SwitchBuffer { slot: self.focused_slot, kind: BufferKind::Tree });
                                        } else if *key_str == "i" {
                                            action_to_run = Some(crate::keymap::GlobalAction::ToggleInspector);
                                        } else if *key_str == "d" {
                                            action_to_run = Some(crate::keymap::GlobalAction::ToggleIssues);
                                        } else if *key_str == "x" {
                                            action_to_run = Some(crate::keymap::GlobalAction::CloseActiveDrawer);
                                        } else if *key_str == "1" && prefix == crate::keymap::LeaderPrefix::Layout {
                                            action_to_run = Some(crate::keymap::GlobalAction::SetLayout(1));
                                        } else if *key_str == "2" && prefix == crate::keymap::LeaderPrefix::Layout {
                                            action_to_run = Some(crate::keymap::GlobalAction::SetLayout(2));
                                        } else if *key_str == "3" && prefix == crate::keymap::LeaderPrefix::Layout {
                                            action_to_run = Some(crate::keymap::GlobalAction::SetLayout(3));
                                        }
                                    }
                                    ui.colored_label(Color32::from_rgb(200, 215, 230), RichText::new(*label_str).size(11.0));
                                });
                                ui.add_space(4.0);
                            }
                        });
                    });
            });

        if let Some(action) = action_to_run {
            self.execute_global_action(action, ctx);
            self.input_mode = crate::keymap::InputMode::Normal;
        } else if let Some(mode) = next_mode {
            self.input_mode = mode;
        }
    }
}

impl eframe::App for CockpitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_lifecycle(ctx);
        self.feedback.begin_frame();

        egui::TopBottomPanel::top("cockpit_top_panel").show(ctx, |ui| {
            self.render_top_bar(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let col_count = self.column_count.clamp(1, 3);
            ui.columns(col_count, |cols| {
                for i in 0..col_count {
                    self.render_slot(i, &mut cols[i]);
                }
            });
        });

        // Real-time Inspector targeting overlay
        self.feedback.render_inspection_overlay(ctx);

        // Developer Feedback modal (automatically pops up when an element is targeted)
        let active_slots: Vec<String> = self.slot_buffers[..self.column_count]
            .iter()
            .map(|b| b.title().to_lowercase())
            .collect();
        self.feedback.render_modal(ctx, &self.net, self.column_count, &active_slots);

        // Issues & Feedback Explorer window
        self.feedback.render_issues_window(ctx, &self.net);

        // Feedback confirmation toast
        self.feedback.render_toast(ctx);

        // Which-Key Leader Overlay HUD
        self.render_which_key_hud(ctx);
    }
}
