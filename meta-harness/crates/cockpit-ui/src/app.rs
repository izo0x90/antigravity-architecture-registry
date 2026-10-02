use egui::{Color32, Frame, Margin, RichText, Stroke};

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::actions::{Action, ActionProvider, Bound, Invocation, LeaderEntry, Origin, Param, RunCx, Scope, leader_entries};
use crate::chat::{ChatStreamState, SessionStatus};
use crate::code::CodeViewerState;
use crate::config::UiConfig;
use crate::feedback::FeedbackInspectorState;
use crate::graph::ArchitectureGraphState;
use crate::keymap::{InputMode, TapTracker};
use crate::net::{NetClient, NetResult, WsState};
use crate::tree::ArchitectureTreeState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    Chat,
    Code,
    Graph,
    Tree,
}

impl BufferKind {
    pub const ALL: [Self; 4] = [Self::Chat, Self::Code, Self::Graph, Self::Tree];

    pub fn title(&self) -> &'static str {
        match self {
            Self::Chat => "CHAT",
            Self::Code => "CODE",
            Self::Graph => "GRAPH",
            Self::Tree => "TREE",
        }
    }

    /// Lowercase name used in action ids ("buffer.chat").
    pub fn name(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Code => "code",
            Self::Graph => "graph",
            Self::Tree => "tree",
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
}

/// What the shell itself can do: slots, layout and overlays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellCommand {
    FocusSlot(usize),
    CycleSlot(i32),
    ShowBuffer(BufferKind),
    SetLayout(usize),
    ToggleInspector,
    ToggleIssues,
    CloseDrawer,
    OpenPalette,
}

const MAX_SLOTS: usize = 3;

pub struct CockpitApp {
    pub net: NetClient,
    pub chat: ChatStreamState,
    pub code: CodeViewerState,
    pub graph: ArchitectureGraphState,
    pub tree: ArchitectureTreeState,
    pub feedback: FeedbackInspectorState,

    // Layout configuration
    pub column_count: usize, // 1, 2, or 3
    pub slot_buffers: [BufferKind; MAX_SLOTS],
    pub focused_slot: usize,
    pub initialized: bool,

    pub config: UiConfig,
    /// Why the user's UI config was not applied, if it wasn't.
    config_error: Option<String>,
    config_reply: Arc<Mutex<Option<NetResult<Option<String>>>>>,
    pub input_mode: InputMode,
    leader_tap: TapTracker,
    /// Keys typed so far in leader mode (after the leader).
    leader_path: Vec<char>,
    palette: crate::palette::Palette,

    // Top-bar status LEDs
    pub srv_led: ActivityLed,
    pub agent_led: ActivityLed,
}

const LED_IDLE: Color32 = Color32::from_rgb(90, 90, 100);
const LED_BUSY: Color32 = Color32::from_rgb(250, 180, 50);
const LED_OK: Color32 = Color32::from_rgb(80, 200, 120);
const LED_FAIL: Color32 = Color32::from_rgb(220, 80, 80);

/// Status LED that flickers on traffic, like a drive or router activity light.
#[derive(Default)]
pub struct ActivityLed {
    last_count: u64,
    last_activity: f64,
}

impl ActivityLed {
    fn show(&mut self, ui: &mut egui::Ui, label: &str, color: Color32, count: u64, hover: &str) -> egui::Response {
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
            if lit && color != LED_IDLE {
                ui.painter().circle_stroke(rect.center(), 5.5, Stroke::new(1.0, color.gamma_multiply(0.35)));
            }
            ui.label(RichText::new(label).monospace().size(10.0).color(Color32::from_gray(150)));
        })
        .response
        .on_hover_text(hover)
    }
}

impl CockpitApp {
    pub fn new(host_origin: &str) -> Self {
        let (config, config_error) = match UiConfig::with_overrides("") {
            Ok(config) => (config, None),
            Err(err) => (UiConfig::default(), Some(format!("built-in defaults: {err}"))),
        };
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

            config,
            config_error,
            config_reply: Arc::default(),
            input_mode: InputMode::Normal,
            leader_tap: TapTracker::default(),
            leader_path: Vec::new(),
            palette: crate::palette::Palette::default(),

            srv_led: ActivityLed::default(),
            agent_led: ActivityLed::default(),
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

            let reply = self.config_reply.clone();
            self.net.ui_config(ctx.clone(), move |r| {
                if let Ok(mut slot) = reply.lock() {
                    *slot = Some(r);
                }
            });

            // Auto-load the registry architecture into graph
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

        self.apply_user_config();

        // Poll WebSocket messages
        self.net.poll_ws();

        // Process incoming events
        let events = {
            if let Ok(mut lock) = self.net.incoming.lock() {
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

        self.handle_keys(ctx);
    }

    /// Merges the user's UI config over the defaults once the server answers.
    fn apply_user_config(&mut self) {
        let reply = self.config_reply.lock().ok().and_then(|mut r| r.take());
        match reply {
            Some(Ok(Some(text))) => match UiConfig::with_overrides(&text) {
                Ok(config) => self.config = config,
                Err(err) => self.config_error = Some(err),
            },
            Some(Err(err)) => self.config_error = Some(format!("ui config: {err}")),
            Some(Ok(None)) | None => {}
        }
    }

    fn provider(&self, kind: BufferKind) -> &dyn ActionProvider {
        match kind {
            BufferKind::Chat => &self.chat,
            BufferKind::Code => &self.code,
            BufferKind::Graph => &self.graph,
            BufferKind::Tree => &self.tree,
        }
    }

    /// A view's provider plus what its actions may touch (disjoint fields).
    fn provider_cx(&mut self, kind: BufferKind) -> (&mut dyn ActionProvider, &NetClient, &mut InputMode) {
        let provider: &mut dyn ActionProvider = match kind {
            BufferKind::Chat => &mut self.chat,
            BufferKind::Code => &mut self.code,
            BufferKind::Graph => &mut self.graph,
            BufferKind::Tree => &mut self.tree,
        };
        (provider, &self.net, &mut self.input_mode)
    }

    fn shell_actions(&self) -> Vec<Action> {
        use ShellCommand as S;
        let mut actions = Vec::new();
        for slot in 0..MAX_SLOTS {
            actions.push(Action::new(format!("slot.focus.{}", slot + 1), format!("Focus slot {}", slot + 1), S::FocusSlot(slot)));
        }
        actions.push(Action::new("slot.next", "Next slot", S::CycleSlot(1)));
        actions.push(Action::new("slot.prev", "Previous slot", S::CycleSlot(-1)));
        for kind in BufferKind::ALL {
            actions.push(Action::new(format!("buffer.{}", kind.name()), format!("Show {} in this slot", kind.title()), S::ShowBuffer(kind)));
        }
        for cols in 1..=MAX_SLOTS {
            actions.push(Action::new(format!("layout.{cols}"), format!("{cols}-column layout"), S::SetLayout(cols)));
        }
        actions.push(Action::new("inspector.toggle", "Toggle element inspector", S::ToggleInspector));
        actions.push(Action::new("issues.toggle", "Toggle issues window", S::ToggleIssues));
        actions.push(Action::new("drawer.close", "Close side drawer", S::CloseDrawer));
        actions.push(Action::new("palette.open", "Command palette", S::OpenPalette));
        actions
    }

    /// Everything invokable right now, bound to its keys: the focused view's
    /// actions first (they win key conflicts), then the shell, then the global
    /// actions of every other view.
    fn bound_actions(&self) -> Vec<Bound> {
        let focused = self.slot_buffers[self.focused_slot];
        let view = |kind: BufferKind, all: bool| {
            self.provider(kind).actions().into_iter().filter(move |a| all || a.scope == Scope::Global).map(move |a| (Origin::View(kind), a))
        };
        let mut actions: Vec<(Origin, Action)> = view(focused, true).collect();
        actions.extend(self.shell_actions().into_iter().map(|a| (Origin::Shell, a)));
        for kind in BufferKind::ALL.into_iter().filter(|k| *k != focused) {
            actions.extend(view(kind, false));
        }
        self.config.keymap.bind(actions)
    }

    pub fn invoke(&mut self, invocation: Invocation, ctx: &egui::Context) {
        let Invocation { action, origin, value } = invocation;
        match origin {
            Origin::Shell => {
                if let Some(command) = action.command::<ShellCommand>() {
                    self.run_shell(*command, ctx);
                }
            }
            Origin::View(kind) => {
                let slot = self.focused_slot;
                let (provider, net, input_mode) = self.provider_cx(kind);
                provider.run(&action, value, &mut RunCx { net, ctx, input_mode, slot });
            }
        }
    }

    /// Opens the palette (at an action's parameter, if given) and gives it the keyboard.
    fn open_palette(&mut self, action: Option<&Bound>) {
        match action {
            Some(action) => self.palette.open_for(action),
            None => self.palette.open(),
        }
        self.input_mode = InputMode::Palette;
    }

    /// Runs an action, or opens the palette at its parameter if it needs one.
    fn run_bound(&mut self, bound: &Bound, ctx: &egui::Context) {
        if bound.action.param == Param::None {
            self.invoke(Invocation { action: bound.action.clone(), origin: bound.origin, value: None }, ctx);
        } else {
            self.open_palette(Some(bound));
        }
    }

    fn run_shell(&mut self, command: ShellCommand, ctx: &egui::Context) {
        match command {
            ShellCommand::FocusSlot(slot) => {
                if slot < self.column_count {
                    self.focused_slot = slot;
                }
            }
            ShellCommand::CycleSlot(dir) => {
                let n = self.column_count as i32;
                self.focused_slot = (self.focused_slot as i32 + dir).rem_euclid(n) as usize;
            }
            ShellCommand::ShowBuffer(kind) => self.slot_buffers[self.focused_slot] = kind,
            ShellCommand::SetLayout(cols) => {
                self.column_count = cols.clamp(1, MAX_SLOTS);
                self.focused_slot = self.focused_slot.min(self.column_count - 1);
            }
            ShellCommand::ToggleInspector => self.feedback.toggle_inspection(),
            ShellCommand::ToggleIssues => {
                self.feedback.is_issues_window_open = !self.feedback.is_issues_window_open;
                if self.feedback.is_issues_window_open {
                    self.feedback.load_issues(&self.net, ctx.clone());
                }
            }
            ShellCommand::CloseDrawer => self.graph.close_inspector(),
            ShellCommand::OpenPalette => self.open_palette(None),
        }
    }

    /// Applies one leader key; returns true when the leader menu is done.
    fn leader_key(&mut self, key: char, ctx: &egui::Context) -> bool {
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
                self.run_bound(&bound[index], ctx);
                true
            }
            None => false,
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
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
            InputMode::Insert { .. } => {
                if back {
                    self.input_mode = InputMode::Normal;
                    ctx.memory_mut(|m| {
                        if let Some(id) = m.focused() {
                            m.surrender_focus(id);
                        }
                    });
                }
                // Everything else flows into the text field.
            }

            // The palette reads its own keys while open.
            InputMode::Palette => {}

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

            // A focused text field (e.g. a search box) gets its keys.
            InputMode::Normal if ctx.wants_keyboard_input() => {}

            InputMode::Normal => {
                if leader_tapped {
                    self.leader_path.clear();
                    self.input_mode = InputMode::Leader;
                    return;
                }
                let bound = self.bound_actions();
                let hit = ctx.input(|i| bound.iter().find(|b| b.keys.iter().any(|k| k.pressed(i))).cloned());
                if let Some(b) = hit {
                    self.run_bound(&b, ctx);
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
                        let brand = &self.config.brand;
                        ui.label(RichText::new(&brand.name).strong().size(13.0).color(Color32::WHITE));
                        ui.label(RichText::new(&brand.tagline).monospace().size(10.0).color(Color32::from_rgb(100, 160, 240)));
                    });
                    self.feedback.register_inspectable(ui, "top_bar:brand", brand_res.response.rect);

                    ui.add_space(8.0);

                    // Status LEDs: SRV = event WebSocket to the meta-harness server,
                    // AGENT = the agent session behind it. Both flicker on traffic.
                    let ws = self.net.ws_state.lock().map(|s| *s).unwrap_or(WsState::Closed);
                    let (srv_color, srv_hint) = match ws {
                        WsState::Open => (LED_OK, "Meta-harness server: connected"),
                        WsState::Connecting => (LED_BUSY, "Meta-harness server: connecting…"),
                        WsState::Closed => (LED_FAIL, "Meta-harness server: disconnected"),
                    };
                    let srv_count = self.net.activity.load(Ordering::Relaxed);
                    let srv_res = self.srv_led.show(ui, "SRV", srv_color, srv_count, srv_hint);
                    self.feedback.register_inspectable(ui, "top_bar:connection_status", srv_res.rect);

                    ui.add_space(6.0);
                    let (agent_color, agent_hint) = match &self.chat.session_status() {
                        SessionStatus::Idle => (LED_IDLE, "Agent: not connected".to_string()),
                        SessionStatus::Connecting => (LED_BUSY, "Agent: connecting…".to_string()),
                        SessionStatus::Connected(tid) => (LED_OK, format!("Agent: connected [{}]", tid)),
                        SessionStatus::Failed(err) => (LED_FAIL, format!("Agent: failed — {}", err)),
                    };
                    let agent_res = self.agent_led.show(ui, "AGENT", agent_color, self.chat.agent_activity(), &agent_hint);
                    self.feedback.register_inspectable(ui, "top_bar:agent_status", agent_res.rect);

                    ui.add_space(8.0);

                    // Input Mode Indicator (NORMAL vs LEADER vs INSERT)
                    let (mode_label, mode_color, mode_bg) = match self.input_mode {
                        InputMode::Normal => (
                            "NORMAL".to_string(),
                            Color32::from_rgb(140, 190, 255),
                            Color32::from_rgb(20, 30, 48),
                        ),
                        InputMode::Leader => (
                            self.leader_title("LEADER"),
                            Color32::from_rgb(255, 205, 50),
                            Color32::from_rgb(48, 40, 15),
                        ),
                        InputMode::Palette => (
                            "PALETTE".to_string(),
                            Color32::from_rgb(90, 200, 255),
                            Color32::from_rgb(18, 34, 52),
                        ),
                        InputMode::Insert { slot } => (
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

                    if let Some(err) = &self.config_error {
                        ui.label(RichText::new(format!("{} config", egui_phosphor::regular::WARNING)).size(11.0).color(LED_BUSY))
                            .on_hover_text(format!("Your UI config was not applied (using defaults):\n{err}"));
                    }

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
                    match self.slot_buffers[slot_idx] {
                        BufferKind::Chat => {
                            self.chat.render(
                                ui,
                                &self.net,
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
                            let pan_held = ui.input(|i| self.config.keymap.leader.key_down(i));
                            self.graph.render(ui, &mut self.feedback, slot_idx, pan_held);
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

    /// "LEADER > SESSION" style title for the current leader path.
    fn leader_title(&self, root: &str) -> String {
        let mut title = root.to_owned();
        for depth in 1..=self.leader_path.len() {
            let label = match self.config.keymap.group_label(&self.leader_path[..depth]) {
                Some(label) => label.to_uppercase(),
                None => self.leader_path[depth - 1].to_string(),
            };
            title.push_str(" > ");
            title.push_str(&label);
        }
        title
    }

    /// Which-key HUD, generated from the actions available at the current path.
    /// Every entry is also tappable.
    pub fn render_which_key_hud(&mut self, ctx: &egui::Context) {
        if self.input_mode != InputMode::Leader {
            return;
        }
        let bound = self.bound_actions();
        let entries = leader_entries(&bound, &self.leader_path);
        let screen_rect = ctx.content_rect();
        let hud_w = (screen_rect.width() - 32.0).min(760.0);
        let mut tapped: Option<char> = None;
        let mut back = false;

        egui::Area::new(egui::Id::new("which_key_floating_hud"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -16.0))
            .order(egui::Order::Foreground)
            .constrain(true)
            .show(ctx, |ui| {
                ui.set_max_width(hud_w);
                Frame::NONE
                    .fill(Color32::from_rgba_premultiplied(16, 21, 32, 245))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(56, 120, 210)))
                    .corner_radius(8)
                    .inner_margin(Margin::symmetric(14, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(Color32::from_rgb(90, 200, 255), RichText::new(self.leader_title(&self.config.keymap.leader.label().to_uppercase())).strong().monospace().size(11.0));
                            let esc = egui::Button::new(RichText::new(self.config.keymap.back.label()).color(Color32::from_rgb(255, 215, 0)).monospace().size(11.0)).corner_radius(4);
                            if ui.add(esc).clicked() {
                                back = true;
                            }
                        });
                        ui.separator();
                        // Explicit rows sized to the screen (wrapping layouts ignore Area widths).
                        let per_row = ((hud_w - 28.0) / 210.0).floor().max(1.0) as usize;
                        for row in entries.chunks(per_row) {
                            ui.horizontal(|ui| {
                                for entry in row {
                                    let (key, label) = match entry {
                                        LeaderEntry::Group { key } => {
                                            let mut path = self.leader_path.clone();
                                            path.push(*key);
                                            (*key, format!("{}…", self.config.keymap.group_label(&path).unwrap_or("more")))
                                        }
                                        LeaderEntry::Action { key, index } => (*key, bound[*index].action.label.clone()),
                                    };
                                    ui.allocate_ui(egui::vec2(200.0, 22.0), |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            let btn = egui::Button::new(RichText::new(key.to_string()).color(Color32::from_rgb(255, 215, 0)).monospace().strong().size(11.0))
                                                .fill(Color32::from_rgba_premultiplied(35, 48, 68, 220))
                                                .corner_radius(4);
                                            if ui.add(btn).clicked() {
                                                tapped = Some(key);
                                            }
                                            ui.add(egui::Label::new(RichText::new(label).size(11.0).color(Color32::from_rgb(200, 215, 230))).truncate());
                                        });
                                    });
                                }
                            });
                        }
                    });
            });

        if let Some(key) = tapped {
            self.leader_key(key, ctx);
        } else if back && self.leader_path.pop().is_none() {
            self.input_mode = InputMode::Normal;
        }
    }

    /// The palette overlay plus a floating button that opens it (touch entry point).
    pub fn render_palette(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("palette_touch_button"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let button = egui::Button::new(RichText::new(format!("{} Actions", egui_phosphor::regular::COMMAND)).size(13.0))
                    .fill(Color32::from_rgba_premultiplied(30, 44, 66, 235))
                    .corner_radius(16);
                if ui.add(button).on_hover_text("Command palette").clicked() {
                    self.open_palette(None);
                }
            });

        let bound = self.bound_actions();
        // A view asked for an action's picker (e.g. Resume once sessions loaded).
        for kind in BufferKind::ALL {
            if let Some(id) = self.provider_cx(kind).0.take_palette_request()
                && let Some(action) = bound.iter().find(|b| b.action.id == id)
            {
                self.open_palette(Some(action));
            }
        }
        if let Some(invocation) = self.palette.render(ctx, &bound, &self.config.keymap) {
            self.input_mode = InputMode::Normal;
            self.invoke(invocation, ctx);
        }
        if self.input_mode == InputMode::Palette && !self.palette.is_open() {
            self.input_mode = InputMode::Normal;
        }
    }
}

impl eframe::App for CockpitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }
}

impl CockpitApp {
    /// One UI frame.
    pub fn frame(&mut self, ctx: &egui::Context) {
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
        self.render_palette(ctx);
    }
}
