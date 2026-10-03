use egui::{Color32, Frame, Margin, RichText, Stroke};

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::actions::{Action, ActionProvider, Bound, Invocation, LeaderEntry, Origin, Param, RunCx, Scope, leader_entries};
use crate::chat::{ChatStreamState, SessionStatus};
use crate::code::CodeViewerState;
use crate::config::UiConfig;
use crate::feedback::FeedbackInspectorState;
use crate::graph::GraphState;
use crate::keymap::{Chord, InputMode, SeqMatch, TapTracker, match_sequence};
use crate::review::ReviewState;
use crate::spec::SpecState;
use crate::net::{NetClient, NetResult, WsState};
use crate::tree::ArchitectureTreeState;
use crate::theme::colors;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    Chat,
    Code,
    Graph,
    Tree,
    Spec,
}

impl BufferKind {
    pub const ALL: [Self; 5] = [Self::Chat, Self::Code, Self::Graph, Self::Tree, Self::Spec];

    pub fn title(&self) -> &'static str {
        match self {
            Self::Chat => "CHAT",
            Self::Code => "CODE",
            Self::Graph => "GRAPH",
            Self::Tree => "TREE",
            Self::Spec => "SPEC",
        }
    }

    /// Lowercase name used in action ids ("buffer.chat").
    pub fn name(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Code => "code",
            Self::Graph => "graph",
            Self::Tree => "tree",
            Self::Spec => "spec",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::Chat => egui_phosphor::regular::CHAT_TEARDROP_TEXT,
            Self::Code => egui_phosphor::regular::CODE,
            Self::Graph => egui_phosphor::regular::GIT_FORK,
            Self::Tree => egui_phosphor::regular::TREE_STRUCTURE,
            Self::Spec => egui_phosphor::regular::ARTICLE,
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
    /// Graph selection and code view follow each other.
    ToggleFollow,
}

const MAX_SLOTS: usize = 3;
/// Largest count a key takes (`999j`).
const MAX_COUNT: u32 = 999;

pub struct CockpitApp {
    pub net: NetClient,
    pub chat: ChatStreamState,
    pub code: CodeViewerState,
    pub graph: GraphState,
    pub spec: SpecState,
    pub tree: ArchitectureTreeState,
    pub feedback: FeedbackInspectorState,
    /// The current plan's comments and changes since review.
    pub review: ReviewState,
    review_seen: u64,

    // Layout configuration
    pub column_count: usize, // 1, 2, or 3
    pub slot_buffers: [BufferKind; MAX_SLOTS],
    pub focused_slot: usize,
    pub initialized: bool,

    pub config: UiConfig,
    /// Why the user's UI config was not applied, if it wasn't.
    config_error: Option<String>,
    config_reply: Arc<Mutex<Option<NetResult<Option<String>>>>>,
    /// The config changed and its theme still needs applying.
    theme_dirty: bool,
    pub input_mode: InputMode,
    leader_tap: TapTracker,
    /// Keys typed so far in leader mode (after the leader).
    leader_path: Vec<char>,
    /// The start of a multi-key sequence typed in normal mode (e.g. `g`).
    pub pending_keys: Vec<Chord>,
    /// Digits typed before a key in a view that takes counts (`3j`).
    pub pending_count: Option<u32>,
    /// Graph selection and code view follow each other.
    pub follow: bool,
    follow_last: Option<arch_graph::ComponentId>,
    reveal_inbox: Arc<Mutex<Vec<NetResult<arch_graph::Component>>>>,
    palette: crate::palette::Palette,

    // Top-bar status LEDs
    pub srv_led: ActivityLed,
    pub agent_led: ActivityLed,
}

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
            if lit && color != colors().faint {
                ui.painter().circle_stroke(rect.center(), 5.5, Stroke::new(1.0, color.gamma_multiply(0.35)));
            }
            ui.label(RichText::new(label).monospace().size(10.0).color(colors().muted));
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
            graph: GraphState::default(),
            spec: SpecState::default(),
            tree: ArchitectureTreeState::default(),
            feedback: FeedbackInspectorState::default(),
            review: ReviewState::default(),
            review_seen: 0,

            column_count: 2,
            slot_buffers: [BufferKind::Chat, BufferKind::Graph, BufferKind::Code],
            focused_slot: 0,
            initialized: false,

            config,
            config_error,
            config_reply: Arc::default(),
            theme_dirty: true,
            input_mode: InputMode::Normal,
            leader_tap: TapTracker::default(),
            leader_path: Vec::new(),
            pending_keys: Vec::new(),
            pending_count: None,
            follow: false,
            follow_last: None,
            reveal_inbox: Arc::default(),
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

            // Auto-load default file Cargo.toml into code viewer
            let ctx_file = ctx.clone();
            let net_ref = &self.net;
            self.code.load_file("Cargo.toml", None, net_ref, ctx_file);

            // Auto-load dev feedback issues
            let ctx_feedback = ctx.clone();
            self.feedback.load_issues(&self.net, ctx_feedback);
        }

        self.apply_user_config();
        if std::mem::take(&mut self.theme_dirty) {
            crate::theme::set(self.config.theme.clone(), ctx);
            crate::graph_style::set(self.config.graph_style.clone());
            self.graph.ui = self.config.graph.clone();
        }

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
            if let harness_contracts::ClientEvent::Architecture { event } = &event {
                self.graph.on_event(event);
                if !matches!(event, arch_graph::ArchEvent::ViewRequested { .. }) {
                    self.spec.invalidate();
                    self.tree.invalidate();
                    self.review.invalidate();
                }
            }
            self.chat.process_event(event);
        }
        self.link_views(ctx);

        // Handle auto-jump requests from chat/tool links
        if let Some((path, line)) = self.chat.requested_file_jump.take() {
            self.code.load_file(&path, line, &self.net, ctx.clone());
            self.ensure_visible(BufferKind::Code);
        }

        self.handle_keys(ctx);
    }

    /// Shows `kind` in some slot: keeps it where it is, or puts it beside the
    /// focused slot (or in the only slot) without moving focus.
    fn ensure_visible(&mut self, kind: BufferKind) {
        if self.slot_buffers[..self.column_count].contains(&kind) {
            return;
        }
        let slot = if self.column_count == 1 { 0 } else { (self.focused_slot + 1) % self.column_count };
        self.slot_buffers[slot] = kind;
    }

    fn show_code(&mut self, file: &str, start: u32, end: u32, ctx: &egui::Context) {
        self.code.show_span(file, start as usize, end as usize, &self.net, ctx.clone());
        self.ensure_visible(BufferKind::Code);
    }

    /// The plan's review state feeds the graph's badges and the spec's threads.
    fn link_review(&mut self, plan: Option<&arch_graph::PlanId>, ctx: &egui::Context) {
        if std::mem::take(&mut self.spec.requests.review_changed) {
            self.review.invalidate();
        }
        self.review.tick(&self.net, ctx, plan);
        if self.review.generation != self.review_seen {
            self.review_seen = self.review.generation;
            self.graph.marks = self.review.marks();
            self.spec.comments = self.review.comments.clone();
            self.spec.diff = self.review.diff.clone();
        }
        if self.spec.author != self.config.user.name {
            self.spec.author = self.config.user.name.clone();
        }
    }

    /// Carries requests between views: graph ↔ code ↔ spec ↔ outline.
    fn link_views(&mut self, ctx: &egui::Context) {
        self.graph.tick(&self.net, ctx);
        let plan = self.graph.nav.current().plan.clone();
        self.spec.follow(self.graph.selection.as_ref(), plan.as_ref());
        self.link_review(plan.as_ref(), ctx);
        self.tree.set_plan(plan.as_ref());
        if let Some((file, start, end)) = self.graph.requests.open_code.take() {
            self.show_code(&file, start, end, ctx);
        }
        if std::mem::take(&mut self.graph.requests.open_spec) {
            self.ensure_visible(BufferKind::Spec);
        }
        if let Some((file, start, end)) = self.spec.requests.open_code.take() {
            self.show_code(&file, start, end, ctx);
        }
        for id in [self.spec.requests.select.take(), self.code.select.take(), self.tree.select.take()].into_iter().flatten() {
            self.graph.reveal(id);
            self.ensure_visible(BufferKind::Graph);
        }
        if let Some((file, line)) = self.code.reveal.take() {
            let inbox = self.reveal_inbox.clone();
            self.net.arch_at(&file, line, plan.as_ref(), ctx.clone(), move |r| {
                if let Ok(mut i) = inbox.lock() {
                    i.push(r);
                }
            });
        }
        let found = std::mem::take(&mut *self.reveal_inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for component in found.into_iter().flatten() {
            self.graph.reveal(component.id);
            self.ensure_visible(BufferKind::Graph);
        }
        // Follow mode: the code view tracks the graph selection.
        if self.follow && self.graph.selection != self.follow_last {
            self.follow_last = self.graph.selection.clone();
            if let Some(c) = self.graph.selected()
                && let Some(a) = &c.anchor
            {
                let (file, start, end) = (a.file.to_string(), a.span.start_line, a.span.end_line);
                self.show_code(&file, start, end, ctx);
            }
        }
    }

    /// Merges the user's UI config over the defaults once the server answers.
    fn apply_user_config(&mut self) {
        let reply = self.config_reply.lock().ok().and_then(|mut r| r.take());
        match reply {
            Some(Ok(Some(text))) => match UiConfig::with_overrides(&text) {
                Ok(config) => {
                    self.config = config;
                    self.theme_dirty = true;
                }
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
            BufferKind::Spec => &self.spec,
        }
    }

    /// A view's provider plus what its actions may touch (disjoint fields).
    fn provider_cx(&mut self, kind: BufferKind) -> (&mut dyn ActionProvider, &NetClient, &mut InputMode, &str) {
        let provider: &mut dyn ActionProvider = match kind {
            BufferKind::Chat => &mut self.chat,
            BufferKind::Code => &mut self.code,
            BufferKind::Graph => &mut self.graph,
            BufferKind::Tree => &mut self.tree,
            BufferKind::Spec => &mut self.spec,
        };
        (provider, &self.net, &mut self.input_mode, &self.config.keymap.hint_chars)
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
        actions.push(Action::new("link.follow", if self.follow { "Stop following (graph and code)" } else { "Follow: code tracks the graph selection" }, S::ToggleFollow));
        actions.push(Action::new("palette.open", "Command palette", S::OpenPalette));
        actions
    }

    /// Everything invokable right now, bound to its keys: the focused view's
    /// actions first (they win key conflicts), then the shell, then the global
    /// actions of every other view.
    pub(crate) fn bound_actions(&self) -> Vec<Bound> {
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
                let (provider, net, input_mode, hint_chars) = self.provider_cx(kind);
                provider.run(&action, value, &mut RunCx { net, ctx, input_mode, slot, hint_chars });
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
            ShellCommand::CloseDrawer => self.graph.drawer_open = false,
            ShellCommand::ToggleFollow => {
                self.follow = !self.follow;
                self.follow_last = None;
            }
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

            // The view in this slot reads typed keys (e.g. hint labels).
            InputMode::Capture { slot } => {
                // Text as typed, plus Enter ('\n') and Backspace ('\u{8}') for prompts.
                let typed: Vec<char> = ctx.input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::Text(t) => Some(t.chars().collect::<Vec<_>>()),
                            egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => Some(vec!['\n']),
                            egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => Some(vec!['\u{8}']),
                            _ => None,
                        })
                        .flatten()
                        .collect()
                });
                if typed.is_empty() && !back {
                    return;
                }
                let kind = self.slot_buffers[slot];
                let (provider, net, input_mode, hint_chars) = self.provider_cx(kind);
                let still = provider.on_capture(&typed, back, &mut RunCx { net, ctx, input_mode, slot, hint_chars });
                if !still {
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

            // A focused text field (e.g. a search box) gets its keys.
            InputMode::Normal if ctx.wants_keyboard_input() => {}

            InputMode::Normal => {
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
                // Counts: digits before a key, in a view that takes them.
                let focused = self.slot_buffers[self.focused_slot];
                if self.pending_keys.is_empty() && self.provider(focused).takes_counts() {
                    let digits: Vec<u32> = ctx.input(|i| i.events.iter().filter_map(|e| match e {
                        egui::Event::Text(t) if t.chars().count() == 1 => t.chars().next().and_then(|c| c.to_digit(10)),
                        _ => None,
                    }).collect());
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
                        let count = self.pending_count.take().unwrap_or(1);
                        let b = bound[index].clone();
                        // A count repeats a view's own action; anything else runs once.
                        let repeats = if b.origin == Origin::View(focused) && b.action.param == Param::None { count } else { 1 };
                        for _ in 0..repeats {
                            self.run_bound(&b, ctx);
                        }
                    }
                    SeqMatch::Pending(chord) => self.pending_keys.push(chord),
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

    pub fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        let top_rect = ui.available_rect_before_wrap();
        self.feedback.register_inspectable(ui, "top_bar", top_rect);

        Frame::NONE
            .fill(colors().panel)
            .inner_margin(Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let brand_res = ui.horizontal(|ui| {
                        let brand = &self.config.brand;
                        ui.label(RichText::new(&brand.name).strong().size(13.0).color(colors().text_strong));
                        ui.label(RichText::new(&brand.tagline).monospace().size(10.0).color(colors().accent));
                    });
                    self.feedback.register_inspectable(ui, "top_bar:brand", brand_res.response.rect);

                    ui.add_space(8.0);

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

                    ui.add_space(6.0);
                    let (agent_color, agent_hint) = match &self.chat.session_status() {
                        SessionStatus::Idle => (colors().faint, "Agent: not connected".to_string()),
                        SessionStatus::Connecting => (colors().warn, "Agent: connecting…".to_string()),
                        SessionStatus::Connected(tid) => (colors().ok, format!("Agent: connected [{}]", tid)),
                        SessionStatus::Failed(err) => (colors().error, format!("Agent: failed — {}", err)),
                    };
                    let agent_res = self.agent_led.show(ui, "AGENT", agent_color, self.chat.agent_activity(), &agent_hint);
                    self.feedback.register_inspectable(ui, "top_bar:agent_status", agent_res.rect);

                    ui.add_space(8.0);

                    // Input Mode Indicator (NORMAL vs LEADER vs INSERT)
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
                        InputMode::Leader => (
                            self.leader_title("LEADER"),
                            colors().key,
                            colors().warn_bg,
                        ),
                        InputMode::Palette => (
                            "PALETTE".to_string(),
                            colors().info,
                            colors().info_bg,
                        ),
                        InputMode::Insert { slot } => (
                            format!("INSERT [S{}]", slot + 1),
                            colors().ok,
                            colors().ok_bg,
                        ),
                        InputMode::Capture { .. } => (
                            if self.graph.search.is_some() { "SEARCH".to_string() } else { "HINT".to_string() },
                            colors().key,
                            colors().warn_bg,
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
                        ui.label(RichText::new(format!("{} config", egui_phosphor::regular::WARNING)).size(11.0).color(colors().warn))
                            .on_hover_text(format!("Your UI config was not applied (using defaults):\n{err}"));
                    }

                    // Right-aligned controls group (Inspector, Issues, Layout)
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Inspector button
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

                        // Issues button
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

                        ui.add_space(8.0);

                        // Layout Mode Selector (1, 2, 3 cols)
                        let layout_res = ui.horizontal(|ui| {
                            ui.label(RichText::new("Layout:").size(11.0).color(colors().muted));
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
            colors().focus
        } else {
            colors().border
        };

        let slot_id = format!("slot_{}", slot_idx + 1);
        self.feedback.register_inspectable(ui, &slot_id, ui.available_rect_before_wrap());

        Frame::NONE
            .fill(colors().bg)
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
                        for kind in BufferKind::ALL {
                            let title = kind.title();
                            let label = if is_compact { kind.icon().to_owned() } else { format!("{} {}{}", kind.icon(), &title[..1], title[1..].to_lowercase()) };
                            let tab = ui.selectable_value(&mut self.slot_buffers[slot_idx], kind, label);
                            if tab.clicked() {
                                self.focused_slot = slot_idx;
                            }
                            self.feedback.register_inspectable(ui, &format!("slot_{}:tab:{}", slot_idx + 1, kind.name()), tab.rect);
                        }
                    });
                    self.feedback.register_inspectable(ui, &format!("slot_{}:header", slot_idx + 1), header_res.response.rect);
                    ui.separator();

                    // Render the active buffer content
                    match self.slot_buffers[slot_idx] {
                        BufferKind::Chat => {
                            let ctx = ui.ctx().clone();
                            let mut cx = RunCx { net: &self.net, ctx: &ctx, input_mode: &mut self.input_mode, slot: slot_idx, hint_chars: &self.config.keymap.hint_chars };
                            self.chat.render(ui, &mut self.feedback, is_focused, &mut cx);
                        }
                        BufferKind::Code => {
                            self.code.render(ui, &self.net, &mut self.feedback, slot_idx);
                        }
                        BufferKind::Graph => {
                            let pan_held = ui.input(|i| self.config.keymap.leader.key_down(i));
                            let ctx = ui.ctx().clone();
                            let mut cx = RunCx { net: &self.net, ctx: &ctx, input_mode: &mut self.input_mode, slot: slot_idx, hint_chars: &self.config.keymap.hint_chars };
                            self.graph.render(ui, &mut self.feedback, slot_idx, pan_held, &mut cx);
                        }
                        BufferKind::Tree => {
                            self.tree.render(ui, &self.net, &mut self.feedback, slot_idx);
                        }
                        BufferKind::Spec => {
                            let ctx = ui.ctx().clone();
                            let mut cx = RunCx { net: &self.net, ctx: &ctx, input_mode: &mut self.input_mode, slot: slot_idx, hint_chars: &self.config.keymap.hint_chars };
                            self.spec.render(ui, &mut cx);
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
                    .fill(colors().overlay)
                    .stroke(Stroke::new(1.0, colors().focus))
                    .corner_radius(8)
                    .inner_margin(Margin::symmetric(14, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(colors().info, RichText::new(self.leader_title(&self.config.keymap.leader.label().to_uppercase())).strong().monospace().size(11.0));
                            let esc = egui::Button::new(RichText::new(self.config.keymap.back.label()).color(colors().key).monospace().size(11.0)).corner_radius(4);
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
                                            let btn = egui::Button::new(RichText::new(key.to_string()).color(colors().key).monospace().strong().size(11.0))
                                                .fill(colors().overlay_raised)
                                                .corner_radius(4);
                                            if ui.add(btn).clicked() {
                                                tapped = Some(key);
                                            }
                                            ui.add(egui::Label::new(RichText::new(label).size(11.0).color(colors().text)).truncate());
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
                    .fill(colors().overlay_raised)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{loading, sample_result, showing};
    use arch_graph::{CodeAnchor, ComponentId, RepoPath, Span};

    fn texts(output: &egui::FullOutput) -> String {
        let mut out = String::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut out);
        }
        out
    }

    fn collect(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(list) => list.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }

    /// An app with the graph in slot 2 (focused) showing the sample view.
    fn app_with_graph() -> (egui::Context, CockpitApp) {
        let ctx = egui::Context::default();
        let mut app = CockpitApp::new("http://127.0.0.1:9");
        app.initialized = true; // skip network bootstrapping
        app.graph = showing(sample_result());
        app.focused_slot = 1;
        (ctx, app)
    }

    fn frame(ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() }, |ctx| app.frame(ctx))
    }

    fn key(text: &str) -> Vec<egui::Event> {
        vec![egui::Event::Text(text.into())]
    }

    fn anchored(result: &mut arch_graph::ViewResult, symbol: &str, file: &str, start: u32, end: u32) {
        result.components.get_mut(&ComponentId::new(symbol)).unwrap().anchor = Some(CodeAnchor {
            language: "rust".into(),
            file: RepoPath::new(file).unwrap(),
            symbol: symbol.into(),
            syntax_kind: "function_item".into(),
            span: Span { start_line: start, end_line: end, start_byte: 0, end_byte: 1 },
            content_hash: String::new(),
        });
    }

    // req: P9.1
    #[test]
    fn graph_draws_server_views_with_loading_and_empty_states() {
        assert!(!std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/samples.rs").exists(), "sample graphs are gone");
        let (ctx, mut app) = app_with_graph();
        let out = frame(&ctx, &mut app, vec![]);
        let text = texts(&out);
        for name in ["run", "step", "save", "log"] {
            assert!(text.contains(name), "node {name} drawn: {text}");
        }
        assert!(text.contains("· 4 nodes"), "{text}");
        app.graph = loading();
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("Loading the architecture"), "{text}");
        let mut empty = sample_result();
        empty.view.nodes.clear();
        empty.layout.rects.clear();
        app.graph = showing(empty);
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("Nothing to show in this view yet"), "{text}");
    }

    // req: P9.5
    #[test]
    fn vim_sequences_and_leader_drive_the_graph_and_are_rebindable() {
        let (ctx, mut app) = app_with_graph();
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, key("l"));
        assert_eq!(app.graph.selection.as_ref().unwrap().as_str(), "rust:a::run");
        frame(&ctx, &mut app, key("g"));
        assert_eq!(app.pending_keys.len(), 1, "waiting for the second key");
        frame(&ctx, &mut app, key("c"));
        assert!(app.pending_keys.is_empty());
        assert!(app.graph.selection.is_some());
        // Two callees: the palette asks which.
        assert_eq!(app.input_mode, InputMode::Palette);
        frame(&ctx, &mut app, vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
        assert_eq!(app.input_mode, InputMode::Normal, "Escape closes the palette");
        app.graph.select(Some(ComponentId::new("rust:a::step")));
        frame(&ctx, &mut app, key("g"));
        frame(&ctx, &mut app, key("c"));
        assert_eq!(app.graph.selection.as_ref().unwrap().as_str(), "rust:a::save", "single callee: straight there");
        frame(&ctx, &mut app, key("g"));
        frame(&ctx, &mut app, key("q"));
        assert!(app.pending_keys.is_empty(), "a key that continues nothing drops the sequence");
        // The leader menu lists them under g (callees exist for run).
        app.graph.select(Some(ComponentId::new("rust:a::run")));
        let bound = app.bound_actions();
        let entries = leader_entries(&bound, &['g']);
        assert!(entries.iter().any(|e| matches!(e, LeaderEntry::Action { key: 'c', index } if bound[*index].action.id == "graph.callees")));
        assert!(entries.iter().any(|e| matches!(e, LeaderEntry::Action { key: 'h', index } if bound[*index].action.id == "graph.hint")));
        // Rebinding from config takes effect.
        app.config = UiConfig::with_overrides("[keymap.bind]\n\"graph.right\" = { keys = [\"w\"] }\n").unwrap();
        app.graph.select(Some(ComponentId::new("rust:a::run")));
        frame(&ctx, &mut app, key("w"));
        assert_eq!(app.graph.selection.as_ref().unwrap().as_str(), "rust:a::step");
    }

    // req: P9.8
    #[test]
    fn inspector_summarizes_the_selection_and_links_onward() {
        let (ctx, mut app) = app_with_graph();
        let mut result = sample_result();
        anchored(&mut result, "rust:a::step", "src/a.rs", 10, 20);
        app.graph = showing(result);
        app.graph.select(Some(ComponentId::new("rust:a::step")));
        let text = texts(&frame(&ctx, &mut app, vec![]));
        for expected in ["rust:a::step", "function · Existing · Implemented", "src/a.rs:10–20", "Spec view", "Code", "calls →", "save", "← calls", "run"] {
            assert!(text.contains(expected), "drawer shows {expected}: {text}");
        }
        // x closes it.
        frame(&ctx, &mut app, key("x"));
        assert!(!app.graph.drawer_open);
    }

    // req: P9.15
    #[test]
    fn graph_and_code_link_both_ways_and_follow() {
        let (ctx, mut app) = app_with_graph();
        let mut result = sample_result();
        anchored(&mut result, "rust:a::step", "src/a.rs", 10, 20);
        anchored(&mut result, "rust:a::save", "src/b.rs", 3, 5);
        app.graph = showing(result);
        app.graph.select(Some(ComponentId::new("rust:a::step")));
        frame(&ctx, &mut app, vec![]);
        // Graph -> code: lands on the symbol's span, and the code view appears.
        frame(&ctx, &mut app, key("g"));
        frame(&ctx, &mut app, key("d"));
        frame(&ctx, &mut app, vec![]);
        assert_eq!((app.code.current_file.as_str(), app.code.highlight), ("src/a.rs", Some((10, 20))));
        assert!(app.slot_buffers[..app.column_count].contains(&BufferKind::Code));
        // Code -> graph: a gutter mark selects its component.
        app.code.select = Some(ComponentId::new("rust:a::save"));
        frame(&ctx, &mut app, vec![]);
        assert_eq!(app.graph.selection.as_ref().unwrap().as_str(), "rust:a::save");
        // Follow mode: the code view tracks the selection.
        app.run_shell(ShellCommand::ToggleFollow, &ctx);
        app.graph.select(Some(ComponentId::new("rust:a::step")));
        frame(&ctx, &mut app, vec![]);
        assert_eq!(app.code.current_file, "src/a.rs");
        app.graph.select(Some(ComponentId::new("rust:a::save")));
        frame(&ctx, &mut app, vec![]);
        assert_eq!((app.code.current_file.as_str(), app.code.highlight), ("src/b.rs", Some((3, 5))));
        // Gutter marks come from the file's components.
        let mut file_comp = sample_result().components[&ComponentId::new("rust:a::log")].clone();
        file_comp.anchor = Some(CodeAnchor { language: "rust".into(), file: RepoPath::new("src/b.rs").unwrap(), symbol: "a::log".into(), syntax_kind: "function_item".into(), span: Span { start_line: 7, end_line: 9, start_byte: 70, end_byte: 90 }, content_hash: String::new() });
        file_comp.placeholders.push(Span { start_line: 8, end_line: 8, start_byte: 80, end_byte: 87 });
        let mut missing = arch_graph::Relation::new(arch_graph::RelationKind::Calls, ComponentId::new("rust:a::log"), ComponentId::new("rust:a::save"), arch_graph::Provenance::PLANNED);
        missing.order = Some(0);
        let view = arch_graph::FileView { file: "src/b.rs".into(), components: vec![(file_comp, arch_graph::Alignment::Present)], relations: vec![(missing, arch_graph::Alignment::Missing)] };
        let marks = crate::code::gutter_marks(&view);
        assert!(matches!(marks[&7][..], [crate::code::Mark::Starts(_, _), crate::code::Mark::MissingCall(_, _)]));
        assert!(matches!(marks[&8][..], [crate::code::Mark::Placeholder(_)]));
    }

    fn press(key: egui::Key, ctrl: bool) -> Vec<egui::Event> {
        let modifiers = egui::Modifiers { ctrl, command: ctrl, ..Default::default() };
        vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]
    }

    /// ctrl+key (modifiers are held for the whole frame).
    fn ctrl(ctx: &egui::Context, app: &mut CockpitApp, key: egui::Key) {
        let modifiers = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        let raw = egui::RawInput { events: press(key, true), modifiers, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() };
        let _ = ctx.run(raw, |ctx| app.frame(ctx));
    }

    fn selected(app: &CockpitApp) -> &str {
        app.graph.selection.as_ref().map_or("", |s| s.as_str())
    }

    // req: P9.14
    #[test]
    fn search_marks_counts_and_ends_work_through_the_reducer() {
        let (ctx, mut app) = app_with_graph();
        frame(&ctx, &mut app, vec![]);
        // Counts: 2l moves twice (the first move lands on the root).
        frame(&ctx, &mut app, key("2"));
        assert_eq!(app.pending_count, Some(2));
        assert!(texts(&frame(&ctx, &mut app, vec![])).contains("NORMAL 2"), "the count shows");
        frame(&ctx, &mut app, key("l"));
        assert_eq!((selected(&app), app.pending_count), ("rust:a::step", None));
        // ... and repeats reducer steps: 2+ adds two hops.
        frame(&ctx, &mut app, key("2"));
        frame(&ctx, &mut app, key("+"));
        let base = arch_graph::ViewCatalog::defaults().unwrap().get(&app.graph.nav.current().view).unwrap().depth;
        assert_eq!(app.graph.nav.current().depth, Some(base + 2));
        app.graph.navigate(crate::graph::NavChange::Back);
        app.graph.navigate(crate::graph::NavChange::Back);
        app.graph.set_result(sample_result());
        // G / gg: last and first node in view order, as jumps.
        frame(&ctx, &mut app, key("G"));
        assert_eq!(selected(&app), "rust:a::log");
        frame(&ctx, &mut app, key("g"));
        frame(&ctx, &mut app, key("g"));
        assert_eq!(selected(&app), "rust:a::run");
        ctrl(&ctx, &mut app, egui::Key::O);
        assert_eq!(selected(&app), "rust:a::log", "ctrl+o returns to where the jump left");
        ctrl(&ctx, &mut app, egui::Key::I);
        assert_eq!(selected(&app), "rust:a::run", "ctrl+i goes forward again");
        // / searches incrementally; Enter jumps; Escape gives up.
        frame(&ctx, &mut app, key("/"));
        assert_eq!(app.input_mode, InputMode::Capture { slot: 1 });
        frame(&ctx, &mut app, key("s"));
        assert_eq!(selected(&app), "rust:a::step", "first match shows while typing");
        assert!(texts(&frame(&ctx, &mut app, vec![])).contains("/s▏ 2 matches"));
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert_eq!((selected(&app), app.input_mode), ("rust:a::step", InputMode::Normal));
        frame(&ctx, &mut app, key("n"));
        assert_eq!(selected(&app), "rust:a::save");
        assert!(texts(&frame(&ctx, &mut app, vec![])).contains("/s 2/2"));
        frame(&ctx, &mut app, key("n"));
        assert_eq!(selected(&app), "rust:a::step", "wraps");
        frame(&ctx, &mut app, key("N"));
        assert_eq!(selected(&app), "rust:a::save");
        ctrl(&ctx, &mut app, egui::Key::O);
        assert_eq!(selected(&app), "rust:a::step", "search jumps are in the history");
        frame(&ctx, &mut app, key("/"));
        frame(&ctx, &mut app, key("l"));
        frame(&ctx, &mut app, key("o"));
        assert_eq!(selected(&app), "rust:a::log");
        frame(&ctx, &mut app, press(egui::Key::Backspace, false));
        frame(&ctx, &mut app, press(egui::Key::Escape, false));
        assert_eq!((selected(&app), app.input_mode), ("rust:a::step", InputMode::Normal), "Escape restores the selection");
        // Marks: m<letter> sets, '<letter> jumps back (through the history).
        frame(&ctx, &mut app, key("m"));
        frame(&ctx, &mut app, key("a"));
        frame(&ctx, &mut app, key("G"));
        assert_eq!(selected(&app), "rust:a::log");
        frame(&ctx, &mut app, key("'"));
        frame(&ctx, &mut app, key("a"));
        assert_eq!(selected(&app), "rust:a::step");
        ctrl(&ctx, &mut app, egui::Key::O);
        assert_eq!(selected(&app), "rust:a::log");
        frame(&ctx, &mut app, key("'"));
        frame(&ctx, &mut app, key("z"));
        assert_eq!((selected(&app), app.input_mode), ("rust:a::log", InputMode::Normal), "an unset mark goes nowhere");
        // Keys come from config.
        app.config = UiConfig::with_overrides("[keymap.bind]\n\"graph.last\" = { keys = [\"E\"] }\n").unwrap();
        frame(&ctx, &mut app, key("g"));
        frame(&ctx, &mut app, key("g"));
        frame(&ctx, &mut app, key("E"));
        assert_eq!(selected(&app), "rust:a::log");
    }
}
