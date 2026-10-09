use egui::{Color32, Frame, RichText, Stroke};

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use arch_graph::ComponentId;

use crate::actions::{Action, ActionProvider, Bound, Choice, Group, Invocation, LeaderEntry, MINOR, Origin, PRIMARY, Param, RunCx, SECONDARY, Scope, Situation, When, is_primary, leader_entries};
use crate::chat::{ChatStreamState, SessionStatus};
use crate::code::CodeViewerState;
use crate::config::UiConfig;
use crate::feedback::FeedbackInspectorState;
use crate::goto::{Candidate, Choosing, Details, Pending, Wait, candidates};
use crate::graph::GraphState;
use crate::keymap::{Chord, InputMode, SeqMatch, TapTracker, continuations, match_sequence};
use crate::nav::{Fold, GoTo, Jump, JumpList, Motion, Navigable, Place, Scroll, SearchEvent, Spot, Step, Target, hint_labels, rank_matches, step_match};
use crate::net::{NetClient, NetResult, WsState};
use crate::review::ReviewState;
use crate::spec::SpecState;
use crate::theme::colors;
use crate::tree::ArchitectureTreeState;

/// Whether one of our text fields has the keyboard (as opposed to a button
/// or nothing): only then do keys go to the widget instead of the keymap.
pub(crate) fn text_input_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.focused()).is_some_and(|id| egui::text_edit::TextEditState::load(ctx, id).is_some())
}

/// The palette's field (or whatever had the keyboard) lets go of it.
fn release_keyboard(ctx: &egui::Context) {
    if let Some(id) = ctx.memory(|m| m.focused()) {
        ctx.memory_mut(|m| m.surrender_focus(id));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BufferKind {
    Chat,
    Code,
    Graph,
    Tree,
    Spec,
    Queue,
}

impl BufferKind {
    pub const ALL: [Self; 6] = [Self::Chat, Self::Code, Self::Graph, Self::Tree, Self::Spec, Self::Queue];

    pub fn title(&self) -> &'static str {
        match self {
            Self::Chat => "CHAT",
            Self::Code => "CODE",
            Self::Graph => "GRAPH",
            Self::Tree => "TREE",
            Self::Spec => "SPEC",
            Self::Queue => "QUEUE",
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
            Self::Queue => "queue",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::Chat => egui_phosphor::regular::CHAT_TEARDROP_TEXT,
            Self::Code => egui_phosphor::regular::CODE,
            Self::Graph => egui_phosphor::regular::GIT_FORK,
            Self::Tree => egui_phosphor::regular::TREE_STRUCTURE,
            Self::Spec => egui_phosphor::regular::ARTICLE,
            Self::Queue => egui_phosphor::regular::LIST_CHECKS,
        }
    }
}

/// What the shell itself can do: slots, layout, overlays, and the keys every
/// view shares (moves, hints, search, folds, marks, the jump list, go-tos).
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
    Motion(Motion),
    /// Row or line N (its value).
    Line,
    Enter,
    Leave,
    Fold(Fold),
    Hint { open: bool },
    Search,
    SearchStep { forward: bool },
    MarkSet,
    MarkJump,
    Back,
    Forward,
    Keys,
    GoTo(GoTo),
    /// One of several places a go-to can land (its value).
    GotoPick,
    Index(IndexOp),
}

/// Reading the code again, on the user's word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexOp {
    /// What changed since it was read.
    Changed,
    /// Everything, from scratch (the cache dropped).
    Rebuild,
    /// The file open in the code view.
    File,
}

/// Hint labels on what the focused view draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Hinting {
    pub slot: usize,
    pub kind: BufferKind,
    /// `F`: open what the label lands on, not just move there.
    pub open: bool,
    pub typed: String,
    pub labels: Vec<(String, Spot)>,
}

/// A search being typed in a view, and where it started.
#[derive(Clone, Debug, PartialEq)]
pub struct Searching {
    pub kind: BufferKind,
    pub query: String,
    pub before: Jump,
}

/// What typed keys mean while the shell captures them.
#[derive(Clone, Debug, PartialEq)]
pub enum Capture {
    Hint(Hinting),
    Search(Searching),
    MarkSet,
    MarkJump,
}

/// How long a note stays in the key bar, in seconds.
const NOTE_SECONDS: f64 = 4.0;

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
    pub queue: crate::queue::QueueState,
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
    /// When the sequence started (its popup waits a moment).
    pending_since: f64,
    /// Digits typed before a key (`3j`).
    pub pending_count: Option<u32>,
    /// What typed keys go to while the shell has them (search, hints, marks).
    pub capture: Option<Capture>,
    /// Where jumps left from, across views and slots.
    pub jumps: JumpList,
    /// Marks by letter.
    pub marks: BTreeMap<char, Jump>,
    /// The last search in each view (one that found something), and the
    /// last that found nothing (so `n` can say so).
    pub searches: BTreeMap<BufferKind, String>,
    missed: BTreeMap<BufferKind, String>,
    /// Go-tos: component details, and one under way or being picked.
    pub details: Details,
    pending_goto: Option<Pending>,
    choosing: Option<Choosing>,
    at_inbox: Arc<Mutex<Vec<NetResult<arch_graph::Component>>>>,
    /// Code indexing: running or not, progress, last run, stale files.
    pub index: arch_graph::IndexStatus,
    index_inbox: Arc<Mutex<Vec<NetResult<arch_graph::IndexStatus>>>>,
    types_inbox: Arc<Mutex<Option<NetResult<arch_graph::TypeSystem>>>>,
    /// A short note in the key bar (why something didn't happen), and since when.
    pub note: Option<String>,
    note_since: Option<f64>,
    /// The key sheet (`?`) is open, with this filter.
    pub key_sheet: Option<String>,
    /// Where each slot was drawn last frame (hint labels stay inside theirs).
    slot_rects: [egui::Rect; MAX_SLOTS],
    /// The slots before fewer columns moved the focused view into view.
    slots_before_shrink: Option<[BufferKind; MAX_SLOTS]>,
    /// Graph selection and code view follow each other.
    pub follow: bool,
    follow_last: Option<ComponentId>,
    /// A code-cursor lookup is on its way: its answer moves the graph but
    /// mustn't move the code back.
    following_code: bool,
    /// The spec's anchor we last refetched the view for (so a mismatch the
    /// server itself holds doesn't refetch every frame).
    anchor_checked: Option<Option<arch_graph::CodeAnchor>>,
    reveal_inbox: Arc<Mutex<Vec<NetResult<arch_graph::Component>>>>,
    palette: crate::palette::Palette,

    // Top-bar status LEDs
    pub srv_led: ActivityLed,
    pub agent_led: ActivityLed,
    pub index_led: ActivityLed,
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
                ui.painter().circle_stroke(rect.center(), 5.5, Stroke::new(crate::theme::look().line_thin, color.gamma_multiply(0.35)));
            }
            ui.label(RichText::new(label).monospace().size(crate::theme::look().text_small).color(colors().muted));
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
            queue: crate::queue::QueueState::default(),
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
            pending_since: 0.0,
            pending_count: None,
            capture: None,
            jumps: JumpList::default(),
            marks: BTreeMap::new(),
            searches: BTreeMap::new(),
            missed: BTreeMap::new(),
            details: Details::default(),
            pending_goto: None,
            choosing: None,
            at_inbox: Arc::default(),
            index: arch_graph::IndexStatus::default(),
            index_inbox: Arc::default(),
            types_inbox: Arc::default(),
            note: None,
            note_since: None,
            key_sheet: None,
            slot_rects: [egui::Rect::NOTHING; MAX_SLOTS],
            slots_before_shrink: None,
            follow: false,
            follow_last: None,
            following_code: false,
            anchor_checked: None,
            reveal_inbox: Arc::default(),
            palette: crate::palette::Palette::default(),

            srv_led: ActivityLed::default(),
            agent_led: ActivityLed::default(),
            index_led: ActivityLed::default(),
        }
    }

    pub fn update_lifecycle(&mut self, ctx: &egui::Context) {
        if !self.initialized {
            self.initialized = true;
            ctx.set_fonts(crate::theme::font_definitions());

            self.net.connect_ws(ctx.clone());

            let reply = self.config_reply.clone();
            self.net.ui_config(ctx.clone(), move |r| {
                if let Ok(mut slot) = reply.lock() {
                    *slot = Some(r);
                }
            });

            // Auto-load default file Cargo.toml into code viewer
            self.code.load_file("Cargo.toml", None, &self.net, ctx.clone());

            // Auto-load dev feedback issues
            self.feedback.load_issues(&self.net, ctx.clone());
            self.fetch_index_status(ctx);
            let types = self.types_inbox.clone();
            self.net.arch_types(ctx.clone(), move |r| {
                if let Ok(mut t) = types.lock() {
                    *t = Some(r);
                }
            });
        }

        self.apply_user_config();
        if let Some(Ok(types)) = self.types_inbox.lock().ok().and_then(|mut t| t.take()) {
            self.spec.types = Some(types);
        }
        if std::mem::take(&mut self.theme_dirty) {
            crate::theme::set(self.config.theme.clone(), ctx);
            crate::graph_style::set(self.config.graph_style.clone());
            crate::status::set(self.config.status.clone());
            crate::theme::set_look(self.config.look.clone());
            self.net.set_backoff(self.config.net.reconnect_min_ms, self.config.net.reconnect_max_ms);
            self.graph.ui = self.config.graph.clone();
            self.code.ui = self.config.code.clone();
        }

        // Poll WebSocket messages
        self.net.poll_ws(ctx);
        if std::mem::take(&mut self.net.reconnected) {
            // Back after a drop: what changed meanwhile wasn't heard.
            self.graph.invalidate();
            self.spec.invalidate();
            self.tree.invalidate();
            self.review.invalidate();
            self.queue.invalidate();
            self.details.clear();
            self.fetch_index_status(ctx);
        }

        let events = self.net.incoming.lock().map(|mut lock| lock.drain(..).collect::<Vec<_>>()).unwrap_or_default();
        for event in events {
            if let harness_contracts::ClientEvent::Architecture { event: arch_graph::ArchEvent::Index { status } } = &event {
                self.index = (**status).clone();
            }
            if let harness_contracts::ClientEvent::Architecture { event } = &event {
                // The agent moving the graph is a jump: back returns from it.
                let before = matches!(event, arch_graph::ArchEvent::ViewRequested { .. }).then(|| self.jump_of(BufferKind::Graph));
                self.graph.on_event(event);
                if let Some(before) = before
                    && self.jump_of(BufferKind::Graph) != before
                {
                    self.jumps.record(before);
                }
                self.queue.on_event(event);
                // Only real changes refetch: a check result (often the spec
                // view's own validation) must not trigger another round.
                if matches!(event, arch_graph::ArchEvent::Changed { .. }) {
                    self.spec.invalidate();
                    self.tree.invalidate();
                    self.review.invalidate();
                    self.details.clear();
                }
            }
            self.chat.process_event(event);
        }
        for reply in std::mem::take(&mut *self.index_inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner)) {
            match reply {
                Ok(status) => self.index = status,
                Err(e) => self.note = Some(format!("Couldn't read the index status: {e}")),
            }
        }
        self.link_views(ctx);
        self.mark_out_of_date();
        self.goto_tick(ctx);
        // The focused target's relations, so the bar and popup offer only
        // the go-tos that lead somewhere.
        if let Some(id) = self.view(self.focused_kind()).target().and_then(|t| t.component().cloned()) {
            let plan = self.graph.nav.plan.clone();
            self.details.want(&id, plan.as_ref(), &self.net, ctx);
        }

        // Notes fade.
        let now = ctx.input(|i| i.time);
        match (&self.note, self.note_since) {
            (Some(_), None) => self.note_since = Some(now),
            (Some(_), Some(since)) if now - since > NOTE_SECONDS => {
                self.note = None;
                self.note_since = None;
            }
            (Some(_), Some(since)) => ctx.request_repaint_after(std::time::Duration::from_secs_f64(NOTE_SECONDS - (now - since) + 0.05)),
            (None, _) => self.note_since = None,
        }

        self.handle_keys(ctx);
    }

    // ---- slots ----

    fn focused_kind(&self) -> BufferKind {
        self.slot_buffers[self.focused_slot]
    }

    /// The slot showing `kind` (the focused one if it does), else the
    /// focused slot.
    fn slot_of(&self, kind: BufferKind) -> usize {
        if self.focused_kind() == kind {
            return self.focused_slot;
        }
        self.slot_buffers[..self.column_count].iter().position(|k| *k == kind).unwrap_or(self.focused_slot)
    }

    /// Shows `kind` in some slot: keeps it where it is, or puts it beside the
    /// focused slot (or in the only slot) without moving focus. Returns the slot.
    fn ensure_visible(&mut self, kind: BufferKind) -> usize {
        if let Some(slot) = self.slot_buffers[..self.column_count].iter().position(|k| *k == kind) {
            return slot;
        }
        let slot = if self.column_count == 1 { 0 } else { (self.focused_slot + 1) % self.column_count };
        self.slot_buffers[slot] = kind;
        slot
    }

    /// Shows `kind` and moves focus to it.
    fn focus_kind(&mut self, kind: BufferKind) {
        self.focused_slot = self.ensure_visible(kind);
    }

    fn fetch_index_status(&self, ctx: &egui::Context) {
        let inbox = self.index_inbox.clone();
        self.net.arch_index_status(ctx.clone(), move |r| {
            if let Ok(mut i) = inbox.lock() {
                i.push(r);
            }
        });
    }

    /// Tells the views what's out of date (they mark it, never wait for it).
    fn mark_out_of_date(&mut self) {
        let code_stale = self.code.file_stale || self.index.is_stale(&self.code.current_file);
        self.code.out_of_date = code_stale;
        let spec_file = self.spec.detail.as_ref().and_then(|d| d.component.anchor.as_ref()).map(|a| a.file.to_string());
        self.spec.out_of_date = self.spec.detail.as_ref().is_some_and(|d| d.stale) || spec_file.is_some_and(|f| self.index.is_stale(&f));
        if self.graph.stale_files != self.index.stale {
            self.graph.stale_files = self.index.stale.clone();
        }
    }

    /// Reads the code again, as the user asked.
    fn run_index(&mut self, op: IndexOp, ctx: &egui::Context) {
        let notes = self.index_inbox.clone();
        let failed = move |r: NetResult<serde_json::Value>| {
            if let Err(e) = r
                && let Ok(mut i) = notes.lock()
            {
                i.push(Err(e));
            }
        };
        match op {
            IndexOp::Changed => self.net.arch_ingest(false, ctx.clone(), failed),
            IndexOp::Rebuild => self.net.arch_ingest(true, ctx.clone(), failed),
            IndexOp::File => {
                let file = self.code.current_file.clone();
                self.net.arch_refresh(&[file], ctx.clone(), failed);
            }
        }
    }

    fn show_code(&mut self, file: &str, start: u32, end: u32, ctx: &egui::Context) {
        self.code.show_span(file, start as usize, end as usize, &self.net, ctx.clone());
        self.ensure_visible(BufferKind::Code);
    }

    // ---- views ----

    fn provider(&self, kind: BufferKind) -> &dyn ActionProvider {
        match kind {
            BufferKind::Chat => &self.chat,
            BufferKind::Code => &self.code,
            BufferKind::Graph => &self.graph,
            BufferKind::Tree => &self.tree,
            BufferKind::Spec => &self.spec,
            BufferKind::Queue => &self.queue,
        }
    }

    fn provider_mut(&mut self, kind: BufferKind) -> &mut dyn ActionProvider {
        match kind {
            BufferKind::Chat => &mut self.chat,
            BufferKind::Code => &mut self.code,
            BufferKind::Graph => &mut self.graph,
            BufferKind::Tree => &mut self.tree,
            BufferKind::Spec => &mut self.spec,
            BufferKind::Queue => &mut self.queue,
        }
    }

    fn view(&self, kind: BufferKind) -> &dyn Navigable {
        match kind {
            BufferKind::Chat => &self.chat,
            BufferKind::Code => &self.code,
            BufferKind::Graph => &self.graph,
            BufferKind::Tree => &self.tree,
            BufferKind::Spec => &self.spec,
            BufferKind::Queue => &self.queue,
        }
    }

    fn view_mut(&mut self, kind: BufferKind) -> &mut dyn Navigable {
        match kind {
            BufferKind::Chat => &mut self.chat,
            BufferKind::Code => &mut self.code,
            BufferKind::Graph => &mut self.graph,
            BufferKind::Tree => &mut self.tree,
            BufferKind::Spec => &mut self.spec,
            BufferKind::Queue => &mut self.queue,
        }
    }

    /// Runs a view's action with what it may touch (disjoint fields).
    fn run_on(&mut self, kind: BufferKind, action: &Action, value: Option<String>, ctx: &egui::Context) {
        let slot = self.slot_of(kind);
        let shell = self.shell_actions();
        let provider: &mut dyn ActionProvider = match kind {
            BufferKind::Chat => &mut self.chat,
            BufferKind::Code => &mut self.code,
            BufferKind::Graph => &mut self.graph,
            BufferKind::Tree => &mut self.tree,
            BufferKind::Spec => &mut self.spec,
            BufferKind::Queue => &mut self.queue,
        };
        provider.run(action, value, &mut RunCx { net: &self.net, ctx, input_mode: &mut self.input_mode, slot, note: &mut self.note, shell: &shell });
    }

    /// The place `kind` is at, in the slot showing it.
    fn jump_of(&self, kind: BufferKind) -> Jump {
        Jump { slot: self.slot_of(kind), kind, place: self.view(kind).place() }
    }

    /// Where the cursor is now: the focused slot, its view, its place.
    fn jump_here(&self) -> Jump {
        self.jump_of(self.focused_kind())
    }

    /// Records `from` in the jump list if the cursor has left it.
    fn after_jump(&mut self, from: Jump) {
        if self.jump_here() != from {
            self.jumps.record(from);
        }
    }

    /// Goes to a recorded place: its view in its slot, focused.
    fn restore_jump(&mut self, jump: Jump, ctx: &egui::Context) {
        let slot = if jump.slot < self.column_count { jump.slot } else { self.ensure_visible(jump.kind) };
        self.slot_buffers[slot] = jump.kind;
        self.focused_slot = slot;
        match &jump.place {
            Place::Code { file, line } if *file != self.code.current_file => self.code.load_file(file, Some(*line), &self.net, ctx.clone()),
            Place::Spec { component: Some(id), .. } if !self.spec.pinned && self.graph.selection.as_deref() != Some(id.as_str()) => {
                // The spec shows the graph's selection: move that first.
                self.graph.reveal(id.clone());
                self.spec.restore(&jump.place);
            }
            place => self.view_mut(jump.kind).restore(place),
        }
    }

    /// What `When` conditions are judged against, for the focused view.
    pub fn situation(&self) -> Situation {
        let kind = self.focused_kind();
        Situation {
            target: self.view(kind).target(),
            searched: self.searches.get(&kind).is_some_and(|q| !q.is_empty()),
            can_back: self.jumps.can_back(),
            can_forward: self.jumps.can_forward(),
            marked: !self.marks.is_empty(),
            can_enter: self.view(kind).can_enter(),
            can_leave: self.view(kind).can_leave(),
            reaches: self.reaches(),
        }
    }

    /// The relational go-tos that lead somewhere from the focused target, if
    /// its relations are here.
    fn reaches(&self) -> Option<Vec<GoTo>> {
        let target = self.view(self.focused_kind()).target()?;
        let detail = self.details.get(target.component()?, self.graph.nav.plan.as_ref())?;
        let ops = [GoTo::Callers, GoTo::Callees, GoTo::Parent, GoTo::Members, GoTo::Implements, GoTo::Flows];
        Some(ops.into_iter().filter(|op| !candidates(*op, detail).is_empty()).collect())
    }

    /// The queue view runs on the graph's plan with the chat's harness, and
    /// sends the user to sessions.
    fn link_queue(&mut self, plan: Option<&arch_graph::PlanId>, ctx: &egui::Context) {
        self.queue.author = self.config.user.name.clone();
        self.queue.instance = self.chat.active_instance.clone();
        self.queue.tick(&self.net, ctx, plan);
        if let Some(thread) = self.queue.requests.open_thread.take() {
            let from = self.jump_of(BufferKind::Queue);
            self.chat.active_thread = Some(harness_contracts::ThreadId::new(thread));
            self.chat.cursor = None;
            self.focus_kind(BufferKind::Chat);
            self.after_jump(from);
        }
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

    /// Carries requests between views: graph ↔ code ↔ spec ↔ outline ↔ chat.
    /// Each one that sends the user somewhere is a jump.
    fn link_views(&mut self, ctx: &egui::Context) {
        self.graph.tick(&self.net, ctx);
        let plan = self.graph.nav.plan.clone();
        self.spec.follow(self.graph.current().as_ref(), plan.as_ref());
        // One truth for where code is: if the spec fetched a newer anchor for
        // the selection than the view on screen has, the view is stale.
        if let (Some(detail), Some(shown)) = (&self.spec.detail, self.graph.selected())
            && detail.component.id == shown.id
            && detail.component.anchor != shown.anchor
            && self.anchor_checked.replace(detail.component.anchor.clone()) != Some(detail.component.anchor.clone())
        {
            self.graph.invalidate();
        }
        self.link_review(plan.as_ref(), ctx);
        self.link_queue(plan.as_ref(), ctx);
        self.tree.set_plan(plan.as_ref());
        // Jumps made in the graph with the pointer or a finger.
        if let Some(place) = self.graph.jumped.take() {
            self.jumps.record(Jump { slot: self.slot_of(BufferKind::Graph), kind: BufferKind::Graph, place });
        }
        if let Some((id, value)) = self.graph.requests.run.take() {
            self.run_by_id(&id, value, ctx);
        }
        if let Some((file, start, end)) = self.graph.requests.open_code.take() {
            self.jumps.record(self.jump_of(BufferKind::Graph));
            self.show_code(&file, start, end, ctx);
        }
        if std::mem::take(&mut self.graph.requests.open_spec) {
            self.ensure_visible(BufferKind::Spec);
        }
        if let Some((file, start, end)) = self.spec.requests.open_code.take() {
            self.jumps.record(self.jump_of(BufferKind::Spec));
            self.show_code(&file, start, end, ctx);
        }
        if std::mem::take(&mut self.code.reread) {
            self.run_index(IndexOp::File, ctx);
        }
        if let Some(file) = self.spec.requests.reread.take() {
            let notes = self.index_inbox.clone();
            self.net.arch_refresh(&[file], ctx.clone(), move |r| {
                if let Err(e) = r
                    && let Ok(mut i) = notes.lock()
                {
                    i.push(Err(e));
                }
            });
        }
        if std::mem::take(&mut self.graph.requests.reindex) {
            self.run_index(IndexOp::Changed, ctx);
        }
        if std::mem::take(&mut self.spec.requests.insert) {
            let slot = self.ensure_visible(BufferKind::Spec);
            self.focused_slot = slot;
            self.input_mode = InputMode::Insert { slot };
        }
        // A name ctrl-clicked in the code: its definition.
        if let Some(id) = self.code.open.take() {
            let from = self.jump_of(BufferKind::Code);
            self.goto_component(GoTo::Code, id, from, BufferKind::Code, ctx);
        }
        for (kind, id) in [(BufferKind::Spec, self.spec.requests.select.take()), (BufferKind::Code, self.code.select.take())] {
            if let Some(id) = id {
                self.jumps.record(self.jump_of(kind));
                self.graph.reveal(id);
                self.ensure_visible(BufferKind::Graph);
            }
        }
        // The outline's Enter: the graph follows (focus stays in the outline).
        if let Some(id) = self.tree.select.take() {
            self.graph.reveal(id);
            self.ensure_visible(BufferKind::Graph);
        }
        if let Some((path, line)) = self.chat.requested_file_jump.take() {
            self.jumps.record(self.jump_of(BufferKind::Chat));
            self.code.load_file(&path, line, &self.net, ctx.clone());
            self.ensure_visible(BufferKind::Code);
        }
        let found = std::mem::take(&mut *self.reveal_inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for component in found.into_iter().flatten() {
            if std::mem::take(&mut self.following_code) {
                self.follow_last = Some(component.id.clone());
            }
            self.graph.reveal(component.id);
            self.ensure_visible(BufferKind::Graph);
        }
        // Follow mode, code to graph: once the code cursor rests, the graph
        // selects the component under it.
        let now = ctx.input(|i| i.time);
        if let Some((file, line)) = self.code.settled_cursor(now, self.config.links.follow_debounce_ms as f64 / 1000.0)
            && self.follow
        {
            self.following_code = true;
            let inbox = self.reveal_inbox.clone();
            self.net.arch_at(&file, line, plan.as_ref(), ctx.clone(), move |r| {
                if let Ok(mut i) = inbox.lock() {
                    i.push(r);
                }
            });
        }
        if self.code.cursor_moved_at.is_some() && self.follow {
            ctx.request_repaint_after(std::time::Duration::from_millis(self.config.links.follow_debounce_ms));
        }
        // Follow mode, graph to code: the code view tracks the graph selection.
        if self.follow && self.graph.current() != self.follow_last {
            self.follow_last = self.graph.current();
            if let Some(c) = self.graph.selected()
                && let Some(a) = c.anchor.as_ref().filter(|a| !crate::status::is_folder(a))
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

    // ---- actions ----

    /// The shell's own actions: slots, layout and overlays, and the keys
    /// every view shares, labelled for the focused view.
    pub(crate) fn shell_actions(&self) -> Vec<Action> {
        use ShellCommand as S;
        let mut actions = Vec::new();
        let admin = |id: String, label: String, c: ShellCommand| Action::new(id, label, c).group(Group::View).importance(MINOR);
        for slot in 0..MAX_SLOTS {
            actions.push(admin(format!("slot.focus.{}", slot + 1), format!("Focus slot {}", slot + 1), S::FocusSlot(slot)));
        }
        actions.push(admin("slot.next".into(), "Next slot".into(), S::CycleSlot(1)));
        actions.push(admin("slot.prev".into(), "Previous slot".into(), S::CycleSlot(-1)));
        for kind in BufferKind::ALL {
            actions.push(admin(format!("buffer.{}", kind.name()), format!("Show {} in this slot", kind.title()), S::ShowBuffer(kind)));
        }
        for cols in 1..=MAX_SLOTS {
            actions.push(admin(format!("layout.{cols}"), format!("{cols}-column layout"), S::SetLayout(cols)));
        }
        actions.push(admin("inspector.toggle".into(), "Toggle element inspector".into(), S::ToggleInspector));
        actions.push(admin("issues.toggle".into(), "Toggle issues window".into(), S::ToggleIssues));
        actions.push(admin("drawer.close".into(), "Close side drawer".into(), S::CloseDrawer));
        let follow = if self.follow { "Stop following (graph and code)" } else { "Follow: code tracks the graph selection" };
        actions.push(admin("link.follow".into(), follow.into(), S::ToggleFollow));
        actions.push(admin("palette.open".into(), "Command palette".into(), S::OpenPalette));
        let index = |id: &str, label: &str, op: IndexOp| Action::new(id, label, S::Index(op)).group(Group::Act).importance(MINOR);
        actions.push(index("index.changed", "Re-read the code that changed", IndexOp::Changed));
        actions.push(index("index.rebuild", "Rebuild the index from scratch", IndexOp::Rebuild));
        actions.push(index("index.file", &format!("Re-read {}", self.code.current_file), IndexOp::File));
        actions.extend(self.nav_actions());
        actions
    }

    /// The keys every view shares, labelled for the focused view and marked
    /// with when they apply.
    fn nav_actions(&self) -> Vec<Action> {
        use BufferKind as K;
        use ShellCommand as S;
        let kind = self.focused_kind();
        let nav = |id: &str, label: &str, c: ShellCommand| Action::new(id, label, c).group(Group::Navigate);
        let mut out = vec![nav("nav.down", "Down", S::Motion(Motion::Down)).importance(SECONDARY), nav("nav.up", "Up", S::Motion(Motion::Up)).importance(SECONDARY)];
        match kind {
            K::Graph => {
                out.push(nav("nav.left", "Left", S::Motion(Motion::Left)).importance(MINOR));
                out.push(nav("nav.right", "Right", S::Motion(Motion::Right)).importance(MINOR));
            }
            K::Tree => {
                out.push(nav("nav.left", "Fold, or up to what holds it", S::Motion(Motion::Left)).importance(SECONDARY));
                out.push(nav("nav.right", "Unfold, or into its first child", S::Motion(Motion::Right)).importance(SECONDARY));
            }
            _ => {}
        }
        out.push(nav("nav.half_down", "Half a page down", S::Motion(Motion::HalfDown)).importance(MINOR));
        out.push(nav("nav.half_up", "Half a page up", S::Motion(Motion::HalfUp)).importance(MINOR));
        out.push(nav("nav.first", "First", S::Motion(Motion::First)).importance(MINOR));
        out.push(nav("nav.last", "Last", S::Motion(Motion::Last)).importance(MINOR));
        let siblings = match kind {
            K::Graph => Some(("Next sibling (same module)", "Previous sibling")),
            K::Tree => Some(("Next sibling row", "Previous sibling row")),
            K::Spec => Some(("Next section", "Previous section")),
            K::Code => Some(("Next definition at this level", "Previous definition at this level")),
            K::Queue | K::Chat => None,
        };
        if let Some((next, prev)) = siblings {
            out.push(nav("nav.next_sibling", next, S::Motion(Motion::NextSibling)).importance(MINOR));
            out.push(nav("nav.prev_sibling", prev, S::Motion(Motion::PrevSibling)).importance(MINOR));
        }
        let (enter, leave) = match kind {
            K::Graph => ("Focus on it", "Back to the whole view"),
            K::Tree => ("Unfold, or select in the graph", "Fold, or up to what holds it"),
            K::Code => ("Go to what's named here", "Out to the definition's start"),
            K::Spec => ("Open: fold a section, follow a relation, reply", "Up to the section title"),
            K::Queue => ("Show why it failed", "Hide that again"),
            K::Chat => ("Open the file it points to", "Follow the latest again"),
        };
        let enter = self.view(kind).enter_label().unwrap_or_else(|| enter.to_owned());
        out.push(nav("nav.enter", &enter, S::Enter).importance(PRIMARY).when(When::Enter));
        let leave = self.view(kind).leave_label().unwrap_or_else(|| leave.to_owned());
        out.push(nav("nav.leave", &leave, S::Leave).importance(SECONDARY).when(When::Leave));
        out.push(nav("nav.hint", "Jump to something on screen", S::Hint { open: false }).importance(PRIMARY));
        out.push(nav("nav.hint_open", "Open something on screen", S::Hint { open: true }).importance(SECONDARY).icon(egui_phosphor::regular::CURSOR_CLICK));
        out.push(nav("nav.search", "Search this view", S::Search).importance(PRIMARY).icon(egui_phosphor::regular::MAGNIFYING_GLASS));
        out.push(nav("nav.search_next", "Next match", S::SearchStep { forward: true }).importance(SECONDARY).when(When::Searched));
        out.push(nav("nav.search_prev", "Previous match", S::SearchStep { forward: false }).importance(MINOR).when(When::Searched));
        if kind != K::Chat {
            let fold = |id: &str, label: &str, f: Fold| Action::new(id, label, S::Fold(f)).group(Group::View).importance(MINOR).when(When::Target);
            out.push(fold("nav.fold_open", "Unfold", Fold::Open));
            out.push(fold("nav.fold_close", "Fold", Fold::Close));
            out.push(fold("nav.fold_toggle", "Fold or unfold", Fold::Toggle).importance(SECONDARY));
            out.push(fold("nav.fold_open_all", "Unfold everything", Fold::OpenAll));
            out.push(fold("nav.fold_close_all", "Fold everything", Fold::CloseAll));
        }
        out.push(nav("nav.mark_set", "Set a mark here", S::MarkSet).importance(MINOR).icon(egui_phosphor::regular::BOOKMARK_SIMPLE));
        out.push(nav("nav.mark_jump", "Jump to a mark", S::MarkJump).importance(MINOR).when(When::Marked));
        out.push(nav("nav.back", "Back (jump list)", S::Back).importance(SECONDARY).when(When::Back).icon(egui_phosphor::regular::ARROW_LEFT));
        out.push(nav("nav.forward", "Forward (jump list)", S::Forward).importance(MINOR).when(When::Forward).icon(egui_phosphor::regular::ARROW_RIGHT));
        out.push(Action::new("nav.keys", "Keys for this view", S::Keys).group(Group::View).importance(MINOR));
        out.push(nav("nav.line", "Go to line or row", S::Line).param(Param::Text { hint: "line or row number".into() }).importance(MINOR));
        let go = |id: &str, label: &str, op: GoTo, when: When, importance: u8| Action::new(id, label, S::GoTo(op)).group(Group::GoTo).when(when).importance(importance);
        out.push(go("goto.code", "Go to its code", GoTo::Code, When::Code, PRIMARY));
        out.push(go("goto.spec", "Go to its spec", GoTo::Spec, When::Component, SECONDARY));
        if kind != K::Graph {
            out.push(go("goto.graph", "Show it in the graph", GoTo::Graph, When::Component, SECONDARY));
        }
        if kind != K::Tree {
            out.push(go("goto.tree", "Show it in the outline", GoTo::Tree, When::Component, MINOR));
        }
        if kind != K::Queue && self.queue.plan.is_some() {
            let has_job = self.view(kind).target().and_then(|t| t.component().cloned()).is_none_or(|id| self.queue.has_job(&id));
            out.push(go("goto.queue", "Go to its queue job", GoTo::Queue, if has_job { When::Component } else { When::Unavailable }, MINOR));
        }
        out.push(go("goto.callers", "Go to a caller", GoTo::Callers, When::Reaches(GoTo::Callers), SECONDARY));
        out.push(go("goto.callees", "Go to a callee", GoTo::Callees, When::Reaches(GoTo::Callees), SECONDARY));
        out.push(go("goto.parent", "Go to what holds it", GoTo::Parent, When::Reaches(GoTo::Parent), MINOR));
        out.push(go("goto.members", "Go to a member", GoTo::Members, When::Reaches(GoTo::Members), MINOR));
        out.push(go("goto.implements", "Go to what it implements", GoTo::Implements, When::Reaches(GoTo::Implements), MINOR));
        out.push(go("goto.flows", "Follow its data flow", GoTo::Flows, When::Reaches(GoTo::Flows), MINOR));
        if let Some(choosing) = &self.choosing {
            let name = |id: &ComponentId| self.component_name(id);
            let choices = choosing.candidates.iter().map(|c| Choice::new(c.id.as_str(), name(&c.id)).detail(Some(c.id.to_string()))).collect();
            out.push(Action::new("goto.pick", format!("Go to which {}?", choosing.op.one()), S::GotoPick).param(Param::Choice(choices)).group(Group::GoTo));
        }
        out
    }

    /// Everything invokable right now, bound to its keys: the focused view's
    /// actions first (they win key conflicts), then the shell, then the global
    /// actions of every other view.
    pub(crate) fn bound_actions(&self) -> Vec<Bound> {
        let focused = self.focused_kind();
        let view = |kind: BufferKind, all: bool| self.provider(kind).actions().into_iter().filter(move |a| all || a.scope == Scope::Global).map(move |a| (Origin::View(kind), a));
        let mut actions: Vec<(Origin, Action)> = view(focused, true).collect();
        actions.extend(self.shell_actions().into_iter().map(|a| (Origin::Shell, a)));
        for kind in BufferKind::ALL.into_iter().filter(|k| *k != focused) {
            actions.extend(view(kind, false));
        }
        let mut bound = self.config.keymap.bind(actions);
        // The focused view's own menu: its actions without a leader path get
        // the next free keys under the view-menu key.
        let menu = self.config.keymap.view_menu;
        let mut taken: Vec<char> = bound.iter().filter_map(|b| b.leader.as_ref().filter(|l| l.len() == 2 && l[0] == menu).map(|l| l[1])).collect();
        for b in bound.iter_mut().filter(|b| b.origin == Origin::View(focused) && b.leader.is_none()) {
            if let Some(key) = crate::keymap::AUTO_KEYS.chars().find(|c| !taken.contains(c)) {
                taken.push(key);
                b.leader = Some(vec![menu, key]);
            }
        }
        bound
    }

    /// The palette's list: everything bound now, plus the other views' own
    /// actions (named for their view; they run there, no keys).
    fn palette_actions(&self) -> Vec<Bound> {
        let mut out = self.bound_actions();
        let focused = self.focused_kind();
        for kind in BufferKind::ALL.into_iter().filter(|k| *k != focused) {
            for mut action in self.provider(kind).actions().into_iter().filter(|a| a.scope == Scope::Focused) {
                action.label = format!("{}{} · {}", &kind.title()[..1], kind.title()[1..].to_lowercase(), action.label);
                out.push(Bound { action, origin: Origin::View(kind), leader: None, keys: Vec::new() });
            }
        }
        out
    }

    pub fn invoke(&mut self, invocation: Invocation, ctx: &egui::Context) {
        self.invoke_counted(invocation, 1, ctx);
    }

    /// Runs an action `count` times (a count before its key): moves take the
    /// count, a view's own actions repeat, anything else runs once.
    fn invoke_counted(&mut self, invocation: Invocation, count: u32, ctx: &egui::Context) {
        let Invocation { action, origin, value } = invocation;
        match origin {
            Origin::Shell => {
                if let Some(command) = action.command::<ShellCommand>() {
                    self.run_shell(*command, value, count.max(1) as usize, ctx);
                }
            }
            Origin::View(kind) => {
                let repeats = if kind == self.focused_kind() && action.param == Param::None { count.max(1) } else { 1 };
                let before = action.jump.then(|| self.jump_here());
                for _ in 0..repeats {
                    self.run_on(kind, &action, value.clone(), ctx);
                }
                if let Some(before) = before {
                    self.after_jump(before);
                }
            }
        }
    }

    /// Runs an action by id the way its key would (chips and toolbars).
    fn run_by_id(&mut self, id: &str, value: Option<String>, ctx: &egui::Context) {
        let bound = self.palette_actions();
        if let Some(b) = bound.into_iter().find(|b| b.action.id == id) {
            if value.is_none() && b.action.param != Param::None {
                self.open_palette(Some(&b));
            } else {
                self.invoke(Invocation { action: b.action, origin: b.origin, value }, ctx);
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
    fn run_bound(&mut self, bound: &Bound, count: u32, ctx: &egui::Context) {
        if bound.action.param == Param::None {
            self.invoke_counted(Invocation { action: bound.action.clone(), origin: bound.origin, value: None }, count, ctx);
        } else {
            self.open_palette(Some(bound));
        }
    }

    fn run_shell(&mut self, command: ShellCommand, value: Option<String>, count: usize, ctx: &egui::Context) {
        let kind = self.focused_kind();
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
            ShellCommand::ShowBuffer(kind) => {
                self.slot_buffers[self.focused_slot] = kind;
                // The arrangement changed: nothing to grow back to.
                self.slots_before_shrink = None;
            }
            // Fewer columns keep the focused view on screen (in the last one left).
            ShellCommand::SetLayout(cols) => {
                let cols = cols.clamp(1, MAX_SLOTS);
                if cols > self.column_count
                    && let Some(before) = self.slots_before_shrink.take()
                {
                    // Growing back: the slots as they were, focus on the same view.
                    let focused = self.focused_kind();
                    self.slot_buffers = before;
                    self.column_count = cols;
                    self.focused_slot = self.slot_buffers[..cols].iter().position(|k| *k == focused).unwrap_or(0);
                    return;
                }
                if self.focused_slot >= cols {
                    self.slots_before_shrink.get_or_insert(self.slot_buffers);
                    self.slot_buffers.swap(self.focused_slot, cols - 1);
                    self.focused_slot = cols - 1;
                }
                self.column_count = cols;
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
            ShellCommand::Motion(motion) => {
                let before = self.jump_here();
                self.view_mut(kind).motion(motion, count);
                // Big moves are jumps; small ones aren't.
                if matches!(motion, Motion::First | Motion::Last | Motion::Line(_)) {
                    self.after_jump(before);
                }
            }
            ShellCommand::Line => match value.as_deref().map(str::trim).and_then(|v| v.parse::<usize>().ok()) {
                Some(n) => self.run_shell(ShellCommand::Motion(Motion::Line(n)), None, 1, ctx),
                None => self.note = Some("Give a line number.".into()),
            },
            ShellCommand::Enter | ShellCommand::Leave => {
                let before = self.jump_here();
                let step = if command == ShellCommand::Enter { self.view_mut(kind).enter() } else { self.view_mut(kind).leave() };
                self.take_step(step, before, ctx);
            }
            ShellCommand::Fold(fold) => self.view_mut(kind).fold(fold),
            ShellCommand::Hint { open } => self.start_hint(open),
            ShellCommand::Search => {
                self.capture = Some(Capture::Search(Searching { kind, query: String::new(), before: self.jump_here() }));
                self.input_mode = InputMode::Capture { slot: self.focused_slot };
                self.view_mut(kind).save_view();
                // An empty search is a search too (a filter typed into goes blank).
                self.view_mut(kind).on_search(SearchEvent::Typing(""));
            }
            ShellCommand::SearchStep { forward } => self.search_step(forward, count),
            ShellCommand::MarkSet => self.start_capture(Capture::MarkSet),
            ShellCommand::MarkJump => self.start_capture(Capture::MarkJump),
            ShellCommand::Back | ShellCommand::Forward => {
                let here = self.jump_here();
                let mut to = None;
                for _ in 0..count {
                    let step = if command == ShellCommand::Back { self.jumps.back(to.clone().unwrap_or_else(|| here.clone())) } else { self.jumps.forward(to.clone().unwrap_or_else(|| here.clone())) };
                    match step {
                        Some(j) => to = Some(j),
                        None => break,
                    }
                }
                match to {
                    Some(jump) => self.restore_jump(jump, ctx),
                    None => self.note = Some(if command == ShellCommand::Back { "Nothing to go back to." } else { "Nothing to go forward to." }.into()),
                }
            }
            ShellCommand::Keys => self.key_sheet = Some(String::new()),
            ShellCommand::GoTo(op) => self.goto(op, ctx),
            ShellCommand::GotoPick => self.goto_pick(value, ctx),
            ShellCommand::Index(op) => self.run_index(op, ctx),
        }
    }

    fn start_capture(&mut self, capture: Capture) {
        self.capture = Some(capture);
        self.input_mode = InputMode::Capture { slot: self.focused_slot };
        self.pending_since = 0.0;
    }

    /// What Enter or Backspace (or `F`) came to.
    fn take_step(&mut self, step: Step, before: Jump, ctx: &egui::Context) {
        match step {
            Step::Done => self.after_jump(before),
            Step::GoTo(op) => {
                self.after_jump(before);
                self.goto(op, ctx);
            }
            Step::Nothing(why) => self.note = Some(why),
        }
    }

    // ---- hints, search, marks ----

    fn start_hint(&mut self, open: bool) {
        let kind = self.focused_kind();
        let spots = self.view(kind).spots();
        let chars: Vec<char> = self.config.keymap.hint_chars.chars().collect();
        let labels = hint_labels(spots.len(), &chars);
        if labels.is_empty() {
            self.note = Some("Nothing on screen to label.".into());
            return;
        }
        self.start_capture(Capture::Hint(Hinting { slot: self.focused_slot, kind, open, typed: String::new(), labels: labels.into_iter().zip(spots).collect() }));
    }

    /// A hint label was typed (or tapped): move there, or open it.
    fn hint_hit(&mut self, kind: BufferKind, key: &str, open: bool, ctx: &egui::Context) {
        let before = self.jump_here();
        if open {
            let step = self.view_mut(kind).open_item(key);
            self.take_step(step, before, ctx);
        } else {
            self.view_mut(kind).move_to(key);
            self.after_jump(before);
        }
    }

    /// Typed keys while the shell captures them; returns whether it still does.
    fn on_capture(&mut self, typed: &[char], back: bool, ctx: &egui::Context) -> bool {
        let Some(capture) = self.capture.take() else { return false };
        match capture {
            Capture::Hint(mut hint) => {
                if back {
                    return false;
                }
                for c in typed {
                    hint.typed.push(*c);
                    if let Some((_, spot)) = hint.labels.iter().find(|(label, _)| *label == hint.typed) {
                        let key = spot.key.clone();
                        self.hint_hit(hint.kind, &key, hint.open, ctx);
                        return false;
                    }
                    if !hint.labels.iter().any(|(label, _)| label.starts_with(&hint.typed)) {
                        return false;
                    }
                }
                self.capture = Some(Capture::Hint(hint));
                true
            }
            Capture::Search(mut search) => {
                let kind = search.kind;
                if back {
                    self.view_mut(kind).on_search(SearchEvent::Cancelled);
                    self.view_mut(kind).restore(&search.before.place);
                    self.view_mut(kind).restore_view();
                    return false;
                }
                for c in typed {
                    match c {
                        '\n' | '\r' => {
                            self.view_mut(kind).on_search(SearchEvent::Confirmed);
                            // Only a search that finds something stays for n/N.
                            let found = !rank_matches(&self.view(kind).items(), &search.query).is_empty();
                            if found {
                                self.searches.insert(kind, search.query.clone());
                                self.missed.remove(&kind);
                            } else {
                                self.searches.remove(&kind);
                                if search.query.is_empty() {
                                    self.missed.remove(&kind);
                                } else {
                                    self.missed.insert(kind, search.query.clone());
                                }
                            }
                            self.after_jump(search.before);
                            return false;
                        }
                        '\u{8}' => {
                            search.query.pop();
                        }
                        c if !c.is_control() => search.query.push(*c),
                        _ => {}
                    }
                }
                // Incremental: the first match shows while typing.
                self.view_mut(kind).on_search(SearchEvent::Typing(&search.query));
                let items = self.view(kind).items();
                match rank_matches(&items, &search.query).first() {
                    Some(&first) => self.view_mut(kind).move_to(&items[first].key),
                    None => {
                        // No match (yet): back where it was, looking as it did.
                        self.view_mut(kind).restore(&search.before.place);
                        self.view_mut(kind).restore_view();
                        self.view_mut(kind).save_view();
                    }
                }
                self.capture = Some(Capture::Search(search));
                true
            }
            Capture::MarkSet | Capture::MarkJump => {
                // Waits for a letter; anything else (or Escape) gives up.
                if !back && typed.is_empty() {
                    self.capture = Some(capture);
                    return true;
                }
                let Some(letter) = typed.iter().copied().find(|c| c.is_alphanumeric()).filter(|_| !back) else { return false };
                if capture == Capture::MarkSet {
                    self.marks.insert(letter, self.jump_here());
                    self.note = Some(format!("Mark {letter} set."));
                } else {
                    match self.marks.get(&letter).cloned() {
                        Some(mark) => {
                            let before = self.jump_here();
                            self.restore_jump(mark, ctx);
                            self.after_jump(before);
                        }
                        None => self.note = Some(format!("No mark {letter}.")),
                    }
                }
                false
            }
        }
    }

    /// The next (or previous) match of the view's last search.
    fn search_step(&mut self, forward: bool, count: usize) {
        let kind = self.focused_kind();
        let Some(query) = self.searches.get(&kind).cloned() else {
            self.note = Some(match self.missed.get(&kind) {
                Some(missed) => format!("No match for {missed} here."),
                None => "Nothing searched here yet (/).".into(),
            });
            return;
        };
        let items = self.view(kind).items();
        let found = rank_matches(&items, &query);
        if found.is_empty() {
            self.note = Some(format!("No match for {query}."));
            return;
        }
        let mut at = self.view(kind).cursor_key().and_then(|k| items.iter().position(|i| i.key == k));
        for _ in 0..count {
            at = step_match(&found, at, forward);
        }
        if let Some(i) = at {
            let before = self.jump_here();
            self.view_mut(kind).move_to(&items[i].key);
            self.after_jump(before);
        }
    }

    /// The search line for the key bar: the prompt while typing, else where
    /// the cursor is among the view's last search's matches.
    fn search_status(&self) -> Option<(String, bool)> {
        if let Some(Capture::Search(search)) = &self.capture {
            let found = rank_matches(&self.view(search.kind).items(), &search.query).len();
            return Some((format!("/{}| {found} {}", search.query, if found == 1 { "match" } else { "matches" }), true));
        }
        let kind = self.focused_kind();
        let query = self.searches.get(&kind)?;
        let items = self.view(kind).items();
        let found = rank_matches(&items, query);
        let cursor = self.view(kind).cursor_key()?;
        let at = found.iter().position(|i| items[*i].key == cursor)?;
        Some((format!("/{query} {}/{}", at + 1, found.len()), false))
    }

    // ---- go-to ----

    fn component_name(&self, id: &ComponentId) -> String {
        self.graph.component(id).map(|c| c.name.clone()).unwrap_or_else(|| {
            let symbol = id.as_str().split_once(':').map_or(id.as_str(), |(_, s)| s);
            symbol.rsplit([':', '.']).next().unwrap_or(symbol).to_owned()
        })
    }

    /// Takes what the cursor is on where a go-to key says.
    fn goto(&mut self, op: GoTo, ctx: &egui::Context) {
        let origin = self.focused_kind();
        let from = self.jump_here();
        let Some(target) = self.view(origin).target() else {
            self.note = Some("Nothing under the cursor.".into());
            return;
        };
        if op == GoTo::Code {
            // A call goes to what it calls; a file link opens as it is.
            if let Target::Line { refs, .. } = &target
                && !refs.is_empty()
            {
                let refs = refs.iter().map(|id| Candidate { id: id.clone(), line: None }).collect();
                self.choose(op, refs, from, origin, ctx);
                return;
            }
            if let (Target::Message { .. }, Some((file, line))) = (&target, target.code()) {
                self.code.load_file(&file, line, &self.net, ctx.clone());
                self.focus_kind(BufferKind::Code);
                self.after_jump(from);
                return;
            }
        }
        match (target.component().cloned(), &target) {
            (Some(id), _) => self.goto_component(op, id, from, origin, ctx),
            // A line the index has nothing on yet: ask the server what's there.
            (None, Target::Line { file, line, .. }) => {
                let inbox = self.at_inbox.clone();
                self.net.arch_at(file, *line, self.graph.nav.plan.as_ref(), ctx.clone(), move |r| {
                    if let Ok(mut i) = inbox.lock() {
                        i.push(r);
                    }
                });
                self.pending_goto = Some(Pending { wait: Wait::At { op }, from, origin });
            }
            (None, _) => self.note = Some("There's no component here.".into()),
        }
    }

    fn goto_component(&mut self, op: GoTo, id: ComponentId, from: Jump, origin: BufferKind, ctx: &egui::Context) {
        match op {
            GoTo::Code => self.open_component_code(id, None, from, ctx),
            GoTo::Spec => self.show_in(BufferKind::Spec, id, from),
            GoTo::Graph => self.show_in(BufferKind::Graph, id, from),
            GoTo::Tree => self.show_in(BufferKind::Tree, id, from),
            GoTo::Queue => self.show_in(BufferKind::Queue, id, from),
            _ => {
                let plan = self.graph.nav.plan.clone();
                match self.details.get(&id, plan.as_ref()) {
                    Some(detail) => {
                        let found = candidates(op, detail);
                        if found.is_empty() {
                            self.note = Some(format!("{} has no {}.", self.component_name(&id), op.noun()));
                        } else {
                            self.choose(op, found, from, origin, ctx);
                        }
                    }
                    None => {
                        self.details.want(&id, plan.as_ref(), &self.net, ctx);
                        self.pending_goto = Some(Pending { wait: Wait::Relations { op, component: id }, from, origin });
                    }
                }
            }
        }
    }

    /// One place: go. Several: the palette asks which.
    fn choose(&mut self, op: GoTo, found: Vec<Candidate>, from: Jump, origin: BufferKind, ctx: &egui::Context) {
        match found.len() {
            0 => {}
            1 => self.arrive(op, found[0].clone(), from, origin, ctx),
            _ => {
                self.choosing = Some(Choosing { op, from, origin, candidates: found });
                if let Some(pick) = self.bound_actions().into_iter().find(|b| b.action.id == "goto.pick") {
                    self.open_palette(Some(&pick));
                }
            }
        }
    }

    fn goto_pick(&mut self, value: Option<String>, ctx: &egui::Context) {
        let (Some(choosing), Some(value)) = (self.choosing.take(), value) else { return };
        if let Some(c) = choosing.candidates.iter().find(|c| c.id.as_str() == value) {
            self.arrive(choosing.op, c.clone(), choosing.from, choosing.origin, ctx);
        }
    }

    /// Lands a go-to: in the code for `gd` (and anything from the code view),
    /// else in the same kind of view it started in, when that view can show
    /// components.
    fn arrive(&mut self, op: GoTo, to: Candidate, from: Jump, origin: BufferKind, ctx: &egui::Context) {
        if op == GoTo::Code || origin == BufferKind::Code {
            self.open_component_code(to.id, to.line, from, ctx);
            return;
        }
        match origin {
            BufferKind::Graph => self.graph.reveal(to.id),
            BufferKind::Tree => self.tree.reveal(&to.id, Scroll::Center),
            BufferKind::Spec => {
                if !self.spec.pinned {
                    self.graph.reveal(to.id.clone());
                }
                self.spec.show(to.id);
            }
            BufferKind::Queue if self.queue.select_component(&to.id) => {}
            BufferKind::Queue | BufferKind::Chat | BufferKind::Code => return self.show_in(BufferKind::Graph, to.id, from),
        }
        self.after_jump(from);
    }

    /// Shows a component in a view and moves focus there.
    fn show_in(&mut self, kind: BufferKind, id: ComponentId, from: Jump) {
        match kind {
            BufferKind::Graph => self.graph.reveal(id),
            BufferKind::Spec => {
                if !self.spec.pinned {
                    self.graph.reveal(id.clone());
                }
                self.spec.show(id);
            }
            BufferKind::Tree => self.tree.reveal(&id, Scroll::Center),
            BufferKind::Queue => {
                if !self.queue.select_component(&id) {
                    self.note = Some(match &self.queue.plan {
                        None => "No plan, so no queue: pick a plan in the graph.".to_owned(),
                        Some(_) => format!("{} has no job in the queue.", self.component_name(&id)),
                    });
                    return;
                }
            }
            BufferKind::Code | BufferKind::Chat => return,
        }
        self.focus_kind(kind);
        self.after_jump(from);
    }

    /// Where a component's code is, from whatever here knows it (the code
    /// view's file, the graph, fetched details): `Some(None)` when they know
    /// the component but it isn't in the code, `None` when nothing here
    /// knows it.
    fn code_location(&self, id: &ComponentId) -> Option<Option<(String, u32, u32)>> {
        let of = |c: &arch_graph::Component| match (&c.anchor, &c.proposed) {
            (Some(a), _) if crate::status::is_folder(a) => None,
            (Some(a), _) => Some((a.file.to_string(), a.span.start_line, a.span.end_line)),
            (None, Some(p)) => Some((p.file.to_string(), 1, 1)),
            (None, None) => None,
        };
        let in_file = self.code.defs.iter().find(|d| &d.id == id).map(|d| Some((self.code.current_file.clone(), d.start as u32, d.end as u32)));
        let known = [in_file, self.graph.component(id).map(of), self.details.get(id, self.graph.nav.plan.as_ref()).map(|d| of(&d.component))];
        let mut answer = None;
        for found in known.into_iter().flatten() {
            if found.is_some() {
                return Some(found);
            }
            answer = Some(None);
        }
        answer
    }

    /// Opens a component's code (at `line` in its file, else its span) and
    /// moves focus there; waits for its detail when it isn't known yet.
    fn open_component_code(&mut self, id: ComponentId, line: Option<u32>, from: Jump, ctx: &egui::Context) {
        match self.code_location(&id) {
            Some(Some((file, start, end))) => {
                let (start, end) = line.map_or((start, end), |l| (l, l));
                self.show_code(&file, start, end, ctx);
                self.focus_kind(BufferKind::Code);
                self.after_jump(from);
                // Planned, not written: the file it's planned in.
                let planned = |c: &arch_graph::Component| c.anchor.is_none() && c.proposed.is_some();
                if self.graph.component(&id).is_some_and(planned) || self.details.get(&id, self.graph.nav.plan.as_ref()).is_some_and(|d| planned(&d.component)) {
                    self.note = Some(format!("{} isn't written yet: this is the file it's planned in.", self.component_name(&id)));
                }
            }
            Some(None) => self.note = Some(format!("{} isn't in the code yet.", self.component_name(&id))),
            None => {
                let plan = self.graph.nav.plan.clone();
                self.details.want(&id, plan.as_ref(), &self.net, ctx);
                self.pending_goto = Some(Pending { wait: Wait::Code { component: id, line }, from, origin: self.focused_kind() });
            }
        }
    }

    /// Continues a go-to once what it waits for is in.
    fn goto_tick(&mut self, ctx: &egui::Context) {
        self.details.tick();
        // A lookup a go-to waits on failed: say so (others were only prefetches).
        for (id, error) in std::mem::take(&mut self.details.failed) {
            if matches!(&self.pending_goto, Some(Pending { wait: Wait::Relations { component, .. } | Wait::Code { component, .. }, .. }) if *component == id) {
                self.note = Some(format!("Couldn't look up {}: {error}", self.component_name(&id)));
                self.pending_goto = None;
            }
        }
        let at = std::mem::take(&mut *self.at_inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in at {
            let Some(Pending { wait: Wait::At { op }, from, origin }) = self.pending_goto.clone() else { continue };
            self.pending_goto = None;
            match reply {
                Ok(component) => self.goto_component(op, component.id, from, origin, ctx),
                Err(_) => self.note = Some("No component at this line.".into()),
            }
        }
        let plan = self.graph.nav.plan.clone();
        let ready = match &self.pending_goto {
            Some(Pending { wait: Wait::Relations { component, .. } | Wait::Code { component, .. }, .. }) => self.details.get(component, plan.as_ref()).is_some(),
            _ => false,
        };
        if ready && let Some(Pending { wait, from, origin }) = self.pending_goto.take() {
            match wait {
                Wait::Relations { op, component } => self.goto_component(op, component, from, origin, ctx),
                Wait::Code { component, line } => self.open_component_code(component, line, from, ctx),
                Wait::At { .. } => {}
            }
        }
    }

    // ---- keys ----

    /// Applies one leader key; returns true when the leader menu is done.
    fn leader_key(&mut self, key: char, ctx: &egui::Context) -> bool {
        let bound = self.bound_actions();
        let entry = leader_entries(&bound, &self.leader_path, Origin::View(self.focused_kind())).into_iter().find(|e| e.key() == key);
        match entry {
            Some(LeaderEntry::Group { key }) => {
                self.leader_path.push(key);
                false
            }
            Some(LeaderEntry::Action { index, .. }) => {
                self.leader_path.clear();
                self.input_mode = InputMode::Normal;
                self.run_bound(&bound[index], 1, ctx);
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
                    release_keyboard(ctx);
                }
                // Everything else flows into the text field.
            }

            // The palette reads its own keys while open.
            InputMode::Palette => {}

            // The shell reads typed keys (search, hint labels, marks).
            InputMode::Capture { .. } => {
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
                        let count = self.pending_count.take().unwrap_or(1);
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

    // ---- drawing ----

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
                    SessionStatus::Idle => (colors().faint, "Agent: not connected".to_string()),
                    SessionStatus::Connecting => (colors().warn, "Agent: connecting…".to_string()),
                    SessionStatus::Connected(tid) => (colors().ok, format!("Agent: connected [{}]", tid)),
                    SessionStatus::Failed(err) => (colors().error, format!("Agent: failed — {}", err)),
                };
                let agent_res = self.agent_led.show(ui, "AGENT", agent_color, self.chat.agent_activity(), &agent_hint);
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
                    // it reads "Layout: 1 2 3".
                    let layout_res = ui.horizontal(|ui| {
                        for cols in (1..=MAX_SLOTS).rev() {
                            ui.selectable_value(&mut self.column_count, cols, cols.to_string());
                        }
                        ui.label(RichText::new("Layout:").size(crate::theme::look().text_body).color(colors().muted));
                    });
                    self.feedback.register_inspectable(ui, "top_bar:layout_selector", layout_res.response.rect);
                });
            });
        });
    }

    /// The INDEX light: its state's colour and word, progress while it runs,
    /// the last run on hover; a click offers the re-index actions.
    fn render_index_light(&mut self, ui: &mut egui::Ui) -> egui::Response {
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
        for i in &self.index.indexers {
            hover.push('\n');
            hover.push_str(&indexer_line(i));
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

    /// Each view's tab key ("SPC b g"), in `BufferKind::ALL` order.
    fn tab_keys(&self, shell: &[Action]) -> Vec<String> {
        let bound = self.config.keymap.bind(shell.iter().map(|a| (Origin::Shell, a.clone())).collect());
        BufferKind::ALL.iter().map(|kind| bound.iter().find(|b| b.action.id == format!("buffer.{}", kind.name())).map(Bound::key_hint).unwrap_or_default()).collect()
    }

    pub fn render_slot(&mut self, slot_idx: usize, ui: &mut egui::Ui, shell: &[Action], tab_keys: &[String]) {
        let is_focused = self.focused_slot == slot_idx;
        let border_color = if is_focused { colors().focus } else { colors().border };

        let slot_id = format!("slot_{}", slot_idx + 1);
        self.feedback.register_inspectable(ui, &slot_id, ui.available_rect_before_wrap());

        Frame::NONE.fill(colors().bg).stroke(Stroke::new(crate::theme::look().line_thin, border_color)).corner_radius(crate::theme::look().radius_m).inner_margin(crate::theme::margin(crate::theme::look().space_m)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(ui.available_height());

            ui.vertical(|ui| {
                let active_kind = self.slot_buffers[slot_idx];
                let is_compact = ui.available_width() < 340.0;

                // Quiet chrome: the slot's number, then a tab per view as an
                // icon (the active one named), each saying its name and key on
                // hover. The row wraps rather than widen the slot.
                let header_res = ui.horizontal_wrapped(|ui| {
                    let number_color = if is_focused { colors().focus } else { colors().faint };
                    let slot_label = ui.add(egui::Label::new(RichText::new((slot_idx + 1).to_string()).strong().monospace().size(crate::theme::look().text_body).color(number_color)).sense(egui::Sense::click()));
                    if slot_label.on_hover_text(format!("Slot {}: {}", slot_idx + 1, active_kind.title().to_lowercase())).clicked() {
                        self.focused_slot = slot_idx;
                    }
                    for (n, kind) in BufferKind::ALL.into_iter().enumerate() {
                        let title = kind.title();
                        let name = format!("{}{}", &title[..1], title[1..].to_lowercase());
                        let label = if !is_compact && active_kind == kind { format!("{} {name}", kind.icon()) } else { kind.icon().to_owned() };
                        let tip = match tab_keys.get(n).filter(|k| !k.is_empty()) {
                            Some(key) => format!("{name} ({key})"),
                            None => name,
                        };
                        let tab = ui.selectable_value(&mut self.slot_buffers[slot_idx], kind, label).on_hover_text(tip);
                        if tab.clicked() {
                            self.focused_slot = slot_idx;
                        }
                        self.feedback.register_inspectable(ui, &format!("slot_{}:tab:{}", slot_idx + 1, kind.name()), tab.rect);
                    }
                });
                self.feedback.register_inspectable(ui, &format!("slot_{}:header", slot_idx + 1), header_res.response.rect);
                ui.separator();

                let ctx = ui.ctx().clone();
                let mut cx = RunCx { net: &self.net, ctx: &ctx, input_mode: &mut self.input_mode, slot: slot_idx, note: &mut self.note, shell };
                match self.slot_buffers[slot_idx] {
                    BufferKind::Chat => self.chat.render(ui, &mut self.feedback, is_focused, &mut cx),
                    BufferKind::Code => self.code.render(ui, &self.net, &mut self.feedback, slot_idx),
                    BufferKind::Graph => {
                        let pan_held = ui.input(|i| self.config.keymap.leader.key_down(i));
                        self.graph.render(ui, &mut self.feedback, slot_idx, pan_held, &mut cx);
                    }
                    BufferKind::Tree => self.tree.render(ui, &self.net, &mut self.feedback, slot_idx),
                    BufferKind::Spec => self.spec.render(ui, &mut cx),
                    BufferKind::Queue => self.queue.render(ui, &mut cx),
                }
            });
        });

        self.slot_rects[slot_idx] = ui.min_rect();
        // Frame-level click-to-focus detection
        if ui.input(|i| i.pointer.primary_clicked())
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            && ui.min_rect().contains(pos)
        {
            self.focused_slot = slot_idx;
        }
    }

    /// A leader submenu's name: the focused view's own, or from config.
    fn group_label(&self, path: &[char]) -> Option<String> {
        if path == [self.config.keymap.view_menu] {
            return Some(format!("All {} actions", self.focused_kind().name()));
        }
        self.config.keymap.group_label(path).map(str::to_owned)
    }

    /// "LEADER > SESSION" style title for the current leader path.
    fn leader_title(&self, root: &str) -> String {
        let mut title = root.to_owned();
        for depth in 1..=self.leader_path.len() {
            let label = match self.group_label(&self.leader_path[..depth]) {
                Some(label) => label.to_uppercase(),
                None => self.leader_path[depth - 1].to_string(),
            };
            title.push_str(" > ");
            title.push_str(&label);
        }
        title
    }

    /// Which-key HUD, generated from the actions available at the current
    /// path: the focused view's first (marked), then the rest. Every entry is
    /// also tappable.
    pub fn render_which_key_hud(&mut self, ctx: &egui::Context) {
        if self.input_mode != InputMode::Leader {
            return;
        }
        let bound = self.bound_actions();
        let focused = Origin::View(self.focused_kind());
        let entries = leader_entries(&bound, &self.leader_path, focused);
        let screen_rect = ctx.content_rect();
        let hud_w = (screen_rect.width() - 32.0).min(760.0);
        let mut tapped: Option<char> = None;
        let mut back = false;

        egui::Area::new(egui::Id::new("which_key_floating_hud")).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0)).order(egui::Order::Foreground).constrain(true).show(ctx, |ui| {
            ui.set_max_width(hud_w);
            Frame::NONE.fill(colors().overlay).stroke(Stroke::new(crate::theme::look().line_thin, colors().focus)).corner_radius(crate::theme::look().radius_xl).inner_margin(crate::theme::margin2(crate::theme::look().space_xl, crate::theme::look().space_sm)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(colors().info, RichText::new(self.leader_title(&self.config.keymap.leader.label().to_uppercase())).strong().monospace().size(crate::theme::look().text_body));
                    let esc = egui::Button::new(RichText::new(self.config.keymap.back.label()).color(colors().key).monospace().size(crate::theme::look().text_body)).corner_radius(crate::theme::look().radius_m);
                    if ui.add(esc).clicked() {
                        back = true;
                    }
                });
                ui.separator();
                // The focused view's entries first, under its name; then the rest.
                let (own, rest): (Vec<&LeaderEntry>, Vec<&LeaderEntry>) = entries.iter().partition(|e| is_primary(e, &bound, &self.leader_path, focused));
                // Explicit rows sized to the screen (wrapping layouts ignore Area widths).
                let per_row = ((hud_w - 28.0) / 210.0).floor().max(1.0) as usize;
                let view_name = self.focused_kind().title();
                let blocks = [(Some(view_name), own), (None, rest)];
                for (heading, block) in blocks.iter().filter(|(_, b)| !b.is_empty()) {
                    match heading {
                        Some(name) => {
                            ui.label(RichText::new(*name).monospace().strong().size(crate::theme::look().text_small).color(colors().focus));
                        }
                        None if blocks[0].1.is_empty() => {}
                        None => {
                            ui.separator();
                        }
                    }
                for row in block.chunks(per_row) {
                    ui.horizontal(|ui| {
                        for entry in row {
                            let primary = heading.is_some();
                            let (key, label) = match entry {
                                LeaderEntry::Group { key } => {
                                    let mut path = self.leader_path.clone();
                                    path.push(*key);
                                    (*key, format!("{}…", self.group_label(&path).unwrap_or_else(|| "more".to_owned())))
                                }
                                LeaderEntry::Action { key, index } => (*key, bound[*index].action.label.clone()),
                            };
                            ui.allocate_ui(egui::vec2(200.0, 22.0), |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    let fill = if primary { colors().focus } else { colors().overlay_raised };
                                    let btn = egui::Button::new(RichText::new(key.to_string()).color(colors().key).monospace().strong().size(crate::theme::look().text_body)).fill(fill).corner_radius(crate::theme::look().radius_m);
                                    if ui.add(btn).clicked() {
                                        tapped = Some(key);
                                    }
                                    let color = if primary { colors().text_strong } else { colors().text };
                                    ui.add(egui::Label::new(RichText::new(label).size(crate::theme::look().text_body).color(color)).truncate());
                                });
                            });
                        }
                    });
                }
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
        egui::Area::new(egui::Id::new("palette_touch_button")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -48.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            let button = egui::Button::new(RichText::new(format!("{} Actions", egui_phosphor::regular::COMMAND)).size(crate::theme::look().text_title)).fill(colors().overlay_raised).corner_radius(crate::theme::look().radius_pill);
            if ui.add(button).on_hover_text("Command palette").clicked() {
                self.open_palette(None);
            }
        });

        let bound = self.palette_actions();
        // A view asked for an action's picker (e.g. Resume once sessions loaded).
        for kind in BufferKind::ALL {
            if let Some(id) = self.provider_mut(kind).take_palette_request()
                && let Some(action) = bound.iter().find(|b| b.action.id == id)
            {
                self.open_palette(Some(action));
            }
        }
        let picked = self.palette.render(ctx, &bound, &self.config.keymap);
        if let Some(ask) = self.palette.take_search()
            && let Origin::View(kind) = ask.origin
        {
            self.provider_mut(kind).search(&ask.action, &ask.query);
        }
        if let Some(invocation) = picked {
            self.input_mode = InputMode::Normal;
            release_keyboard(ctx);
            self.invoke(invocation, ctx);
        }
        if self.input_mode == InputMode::Palette && !self.palette.is_open() {
            self.input_mode = InputMode::Normal;
            release_keyboard(ctx);
        }
    }

    /// The keys the key bar shows: the focused view's and the shared ones
    /// that apply to what the cursor is on, most important first.
    pub(crate) fn bar_keys<'a>(&self, bound: &'a [Bound]) -> Vec<&'a Bound> {
        let focused = Origin::View(self.focused_kind());
        let situation = self.situation();
        let shared = |b: &Bound| b.origin == Origin::Shell && (b.action.id.starts_with("nav.") || b.action.id.starts_with("goto."));
        let mut keys: Vec<(usize, &Bound)> = bound
            .iter()
            .enumerate()
            .filter(|(_, b)| (b.origin == focused || shared(b)) && !b.keys.is_empty() && b.action.when.holds(&situation))
            .collect();
        // Most important first; the view's own before the shared ones; then as listed.
        keys.sort_by_key(|(i, b)| (b.action.importance, b.origin != focused, b.action.group, *i));
        keys.into_iter().take(self.config.keymap.bar.max).map(|(_, b)| b).collect()
    }

    /// The bar at the bottom: what's being typed (a search, a hint, a mark),
    /// notes, and the keys that matter now (tappable).
    fn render_key_bar(&mut self, ctx: &egui::Context) {
        let bound = self.bound_actions();
        let narrow = ctx.content_rect().width() < self.config.keymap.bar.narrow_below;
        let mut run: Option<Bound> = None;
        let status: Option<(String, Color32)> = match &self.capture {
            Some(Capture::Hint(h)) => Some((format!("hint: type a label{}", if h.typed.is_empty() { String::new() } else { format!(" ({})", h.typed) }), colors().key)),
            Some(Capture::MarkSet) => Some(("mark: press a letter".into(), colors().key)),
            Some(Capture::MarkJump) => Some(("jump to mark: press its letter".into(), colors().key)),
            _ => self.search_status().map(|(text, typing)| (text, if typing { colors().key } else { colors().muted })),
        };
        let show_keys = self.config.keymap.bar.show;
        if !show_keys && status.is_none() && self.note.is_none() {
            return;
        }
        egui::TopBottomPanel::bottom("key_bar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Some((text, color)) = &status {
                    ui.label(RichText::new(text).monospace().size(crate::theme::look().text_body).color(*color));
                }
                if let Some(note) = &self.note {
                    ui.label(RichText::new(note).size(crate::theme::look().text_body).color(colors().warn));
                }
                if !show_keys {
                    return;
                }
                // While typing a search, a label or a mark, those keys are what matter.
                let typing: Option<&[(&str, &str)]> = match (&self.capture, self.input_mode) {
                    (Some(Capture::Search(_)), _) => Some(&[("Enter", "jump to it"), ("Backspace", "delete"), ("Escape", "give up")]),
                    (Some(Capture::Hint(_)), _) => Some(&[("a label", "go there"), ("Escape", "give up")]),
                    (Some(Capture::MarkSet | Capture::MarkJump), _) => Some(&[("a letter", "the mark"), ("Escape", "give up")]),
                    (None, InputMode::Insert { .. }) => Some(&[("Enter", "send"), ("Shift+Enter", "new line"), ("Escape", "leave the field")]),
                    (None, InputMode::Palette) => Some(&[("Enter", "run"), ("Up/Down", "choose"), ("Escape", "close")]),
                    (None, _) => None,
                };
                if let Some(typing) = typing {
                    for (keys, what) in typing {
                        ui.label(RichText::new(format!("{keys} {what}")).size(crate::theme::look().text_body).color(colors().muted));
                    }
                    return;
                }
                for b in self.bar_keys(&bound) {
                    let keys = b.keys.first().map(|seq| seq.iter().map(Chord::label).collect::<Vec<_>>().join(" ")).unwrap_or_default();
                    let text = RichText::new(format!("{keys} {}", b.action.label)).size(if narrow { crate::theme::look().text_title } else { crate::theme::look().text_body });
                    let mut button = egui::Button::new(text).corner_radius(crate::theme::look().radius_m);
                    if narrow {
                        button = button.min_size(egui::vec2(32.0, 32.0));
                    }
                    if ui.add(button).on_hover_text(&b.action.label).clicked() {
                        run = Some(b.clone());
                    }
                }
            });
        });
        if let Some(b) = run {
            self.run_bound(&b, 1, ctx);
        }
    }

    /// After a pause on a half-typed sequence (or a mark key), what can
    /// follow, for what the cursor is on now.
    fn render_prefix_popup(&mut self, ctx: &egui::Context) {
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
        let rows: Vec<(String, String)> = if waiting {
            let bound = self.bound_actions();
            let situation = self.situation();
            let focused = Origin::View(self.focused_kind());
            continuations(&bound, &self.pending_keys)
                .into_iter()
                .filter(|(b, _)| b.action.when.holds(&situation) && (b.origin == focused || b.origin == Origin::Shell || b.action.scope == Scope::Global))
                .map(|(b, rest)| (rest.iter().map(Chord::label).collect::<Vec<_>>().join(" "), b.action.label.clone()))
                .collect()
        } else {
            let mut rows: Vec<(String, String)> = self.marks.iter().map(|(letter, jump)| (letter.to_string(), describe_jump(jump))).collect();
            if matches!(self.capture, Some(Capture::MarkSet)) {
                rows.insert(0, ("a–z".into(), "set a mark here (replaces one with that letter)".into()));
            }
            rows
        };
        let title = if waiting { self.pending_keys.iter().map(Chord::label).collect::<Vec<_>>().join(" ") + " …" } else { "marks".into() };
        egui::Area::new(egui::Id::new("prefix_popup")).anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0)).order(egui::Order::Foreground).constrain(true).show(ctx, |ui| {
            Frame::NONE.fill(colors().overlay).stroke(Stroke::new(crate::theme::look().line_thin, colors().focus)).corner_radius(crate::theme::look().radius_xl).inner_margin(crate::theme::margin2(crate::theme::look().space_l, crate::theme::look().space_m)).show(ui, |ui| {
                ui.label(RichText::new(title).monospace().strong().size(crate::theme::look().text_body).color(colors().info));
                if rows.is_empty() {
                    ui.label(RichText::new("nothing applies here").size(crate::theme::look().text_body).color(colors().muted));
                }
                for (keys, label) in rows {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(keys).monospace().strong().size(crate::theme::look().text_body).color(colors().key));
                        ui.label(RichText::new(label).size(crate::theme::look().text_body).color(colors().text));
                    });
                }
                ui.label(RichText::new(format!("{} to cancel", self.config.keymap.back.label())).size(crate::theme::look().text_small).color(colors().muted));
            });
        });
    }

    /// The focused view's keys (`?`), grouped and searchable.
    fn render_key_sheet(&mut self, ctx: &egui::Context) {
        let Some(mut filter) = self.key_sheet.take() else { return };
        if ctx.input(|i| self.config.keymap.back.pressed(i)) {
            release_keyboard(ctx);
            return;
        }
        let bound = self.bound_actions();
        let focused = Origin::View(self.focused_kind());
        let situation = self.situation();
        let rows: Vec<&Bound> = bound.iter().filter(|b| (b.origin == focused || b.origin == Origin::Shell) && (!b.keys.is_empty() || b.leader.is_some())).collect();
        let mut open = true;
        let title = format!("{} keys", self.focused_kind().title());
        egui::Window::new(title).id(egui::Id::new("key_sheet")).collapsible(false).resizable(true).default_width(520.0).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).open(&mut open).show(ctx, |ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut filter).hint_text("filter…").desired_width(f32::INFINITY));
            field.request_focus();
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.6).show(ui, |ui| {
                let matches = |b: &&&Bound| filter.trim().is_empty() || format!("{} {} {}", b.action.label, b.action.id, keys_of(b)).to_lowercase().contains(&filter.trim().to_lowercase());
                if !rows.iter().any(|b| matches(&b)) {
                    ui.label(RichText::new("No keys match.").size(crate::theme::look().text_body).color(colors().muted));
                }
                for group in Group::ALL {
                    let mut in_group: Vec<&&Bound> = rows.iter().filter(|b| b.action.group == group).filter(matches).collect();
                    if in_group.is_empty() {
                        continue;
                    }
                    in_group.sort_by_key(|b| (b.origin != focused, b.action.importance));
                    ui.label(RichText::new(group.title()).strong().size(crate::theme::look().text_label).color(colors().info));
                    egui::Grid::new(("key_sheet", group as u8)).num_columns(2).spacing(egui::vec2(16.0, 2.0)).show(ui, |ui| {
                        for b in in_group {
                            let applies = b.action.when.holds(&situation);
                            ui.label(RichText::new(keys_of(b)).monospace().size(crate::theme::look().text_body).color(colors().key));
                            ui.label(RichText::new(&b.action.label).size(crate::theme::look().text_body).color(if applies { colors().text } else { colors().faint }));
                            ui.end_row();
                        }
                    });
                    ui.add_space(crate::theme::look().space_s);
                }
            });
        });
        if open {
            self.key_sheet = Some(filter);
        } else {
            release_keyboard(ctx);
        }
    }

    /// Hint labels over what the focused view draws; tapping one picks it.
    fn render_hints(&mut self, ctx: &egui::Context) {
        let Some(Capture::Hint(hint)) = &self.capture else { return };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("hint_labels")));
        let tap = ctx.input(|i| if i.pointer.primary_clicked() { i.pointer.interact_pos() } else { None });
        let mut hit: Option<String> = None;
        let mut taken: Vec<egui::Rect> = Vec::new();
        for (label, spot) in &hint.labels {
            if !label.starts_with(&hint.typed) {
                continue;
            }
            let galley = painter.layout_no_wrap(label.to_uppercase(), egui::FontId::monospace(crate::theme::look().text_title), colors().on_bright);
            let chip = hint_chip(spot.rect, galley.size() + egui::vec2(8.0, 4.0), self.slot_rects[hint.slot], &taken);
            taken.push(chip);
            painter.rect_filled(chip, 3.0, colors().key);
            painter.galley(chip.min + egui::vec2(4.0, 2.0), galley, colors().on_bright);
            if tap.is_some_and(|p| chip.contains(p)) {
                hit = Some(spot.key.clone());
            }
        }
        if let Some(key) = hit
            && let Some(Capture::Hint(hint)) = self.capture.take()
        {
            self.input_mode = InputMode::Normal;
            self.hint_hit(hint.kind, &key, hint.open, ctx);
        }
    }
}

/// Where a hint label goes so it hides as little as it can, and never on
/// another label: just left of its item, else just right of it (inside the
/// slot), else at the right end of a wide item, else over its corner.
fn hint_chip(item: egui::Rect, size: egui::Vec2, slot: egui::Rect, taken: &[egui::Rect]) -> egui::Rect {
    let top = item.top() + ((item.height().min(18.0) - size.y) / 2.0).min(0.0);
    let at = |x: f32| egui::Rect::from_min_size(egui::pos2(x, top), size);
    let inside = |r: &egui::Rect| !slot.is_positive() || (r.left() >= slot.left() && r.right() <= slot.right());
    let mut places = vec![at(item.left() - size.x - 2.0), at(item.right() + 2.0)];
    if item.width() > size.x * 3.0 {
        places.push(at(item.right() - size.x - 2.0));
    }
    let corner = egui::Rect::from_min_size(item.left_top() + egui::vec2(-4.0, -4.0), size);
    places.push(corner);
    places.iter().copied().find(|r| inside(r) && !taken.iter().any(|t| t.intersects(*r))).or_else(|| places.iter().copied().find(inside)).unwrap_or(corner)
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

/// One language's indexer in words: whether its links are proven, and by what.
fn indexer_line(i: &arch_graph::IndexerStatus) -> String {
    let tool = match &i.version {
        Some(v) => format!("{} {v}", i.indexer),
        None => i.indexer.clone(),
    };
    match i.state {
        arch_graph::IndexerState::Missing => format!("{}: links matched by name ({} isn't ready: meta-harness pack check {})", i.language, i.indexer, i.indexer),
        arch_graph::IndexerState::Running => format!("{}: {tool} is running", i.language),
        arch_graph::IndexerState::Failed => format!("{}: {tool} failed: {}", i.language, i.error.as_deref().unwrap_or("no reason given")),
        arch_graph::IndexerState::Ready => match i.last_ms {
            Some(ms) => format!("{}: {tool} confirmed {} links in {} files ({} ms)", i.language, i.proven, i.files, ms),
            None => format!("{}: {tool} is ready", i.language),
        },
    }
}

/// An action's keys as the key sheet lists them ("g d", "SPC g v").
fn keys_of(b: &Bound) -> String {
    let mut all: Vec<String> = b.keys.iter().map(|seq| seq.iter().map(Chord::label).collect::<Vec<_>>().join(" ")).collect();
    if let Some(path) = &b.leader {
        all.push(format!("SPC {}", path.iter().map(char::to_string).collect::<Vec<_>>().join(" ")));
    }
    all.join(" / ")
}

/// A mark in words: its view and where.
fn describe_jump(jump: &Jump) -> String {
    let short = |id: &ComponentId| id.as_str().rsplit([':', '.']).next().unwrap_or(id.as_str()).to_owned();
    let place = match &jump.place {
        Place::Chat { message } => message.map_or("latest".to_owned(), |m| format!("message {}", m + 1)),
        Place::Code { file, line } => format!("{file}:{line}"),
        Place::Graph { selection: Some(key), .. } => short(&ComponentId::new(key.clone())),
        Place::Graph { selection: None, nav } => format!("{} view", nav.view.name().replace('_', " ")),
        Place::Tree { cursor } | Place::Queue { component: cursor } => cursor.as_ref().map_or_else(String::new, short),
        Place::Spec { component, .. } => component.as_ref().map_or_else(String::new, short),
    };
    format!("{} · {place}", jump.kind.name())
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
        self.render_key_bar(ctx);

        let shell = self.shell_actions();
        let tab_keys = self.tab_keys(&shell);
        egui::CentralPanel::default().show(ctx, |ui| {
            let col_count = self.column_count.clamp(1, 3);
            ui.columns(col_count, |cols| {
                for (i, col) in cols.iter_mut().enumerate() {
                    self.render_slot(i, col, &shell, &tab_keys);
                }
            });
        });

        // Real-time Inspector targeting overlay
        self.feedback.render_inspection_overlay(ctx);

        // Developer Feedback modal (automatically pops up when an element is targeted)
        let active_slots: Vec<String> = self.slot_buffers[..self.column_count].iter().map(|b| b.title().to_lowercase()).collect();
        self.feedback.render_modal(ctx, &self.net, self.column_count, &active_slots);

        // Issues & Feedback Explorer window
        self.feedback.render_issues_window(ctx, &self.net);

        // Feedback confirmation toast
        self.feedback.render_toast(ctx);

        self.render_hints(ctx);
        self.render_prefix_popup(ctx);
        self.render_which_key_hud(ctx);
        self.render_key_sheet(ctx);
        self.render_palette(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{loading, sample_tree, showing};
    use arch_graph::{CodeAnchor, PlanId, Provenance, Relation, RelationKind, RepoPath, Span};

    fn texts(output: &egui::FullOutput) -> String {
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
        let mut out = String::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut out);
        }
        out
    }

    /// An app with the graph in slot 2 (focused) showing the sample view.
    fn app_with_graph() -> (egui::Context, CockpitApp) {
        let ctx = egui::Context::default();
        let mut app = CockpitApp::new("http://127.0.0.1:9");
        app.initialized = true; // skip network bootstrapping
        app.graph = showing(sample_tree());
        app.focused_slot = 1;
        (ctx, app)
    }

    const SCREEN: egui::Vec2 = egui::vec2(1600.0, 1000.0);

    fn frame(ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)), ..Default::default() }, |ctx| app.frame(ctx))
    }

    fn frame_at(ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>, time: f64) -> egui::FullOutput {
        ctx.run(egui::RawInput { events, time: Some(time), screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)), ..Default::default() }, |ctx| app.frame(ctx))
    }

    fn key(text: &str) -> Vec<egui::Event> {
        vec![egui::Event::Text(text.into())]
    }

    /// Types each character in its own frame.
    fn keys(ctx: &egui::Context, app: &mut CockpitApp, text: &str) {
        for c in text.chars() {
            frame(ctx, app, key(&c.to_string()));
        }
    }

    fn press(key: egui::Key, ctrl: bool) -> Vec<egui::Event> {
        let modifiers = egui::Modifiers { ctrl, command: ctrl, ..Default::default() };
        vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]
    }

    /// ctrl+key (modifiers are held for the whole frame).
    fn ctrl(ctx: &egui::Context, app: &mut CockpitApp, key: egui::Key) {
        let modifiers = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        let raw = egui::RawInput { events: press(key, true), modifiers, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)), ..Default::default() };
        let _ = ctx.run(raw, |ctx| app.frame(ctx));
    }

    fn selected(app: &CockpitApp) -> &str {
        app.graph.selection.as_ref().map_or("", |s| s.as_str())
    }

    fn call(a: &str, b: &str, order: u32, line: u32) -> Relation {
        let mut r = Relation::new(RelationKind::Calls, ComponentId::new(a), ComponentId::new(b), Provenance::OBSERVED);
        r.order = Some(order);
        r.sites = vec![Span { start_line: line, end_line: line, start_byte: 0, end_byte: 1 }];
        r
    }

    /// Every view with something in it: the graph (slot 2, focused) on plan
    /// `p`, the outline, a code file with definitions and calls, a spec
    /// (pinned on step), the queue and a conversation.
    fn full_app() -> (egui::Context, CockpitApp) {
        let (ctx, mut app) = app_with_graph();
        let plan = PlanId::new("p").unwrap();
        app.graph.nav.plan = Some(plan.clone());
        app.tree.set_result(crate::tree::tests::outline());
        app.code = crate::code::tests::sample();
        let step = ComponentId::new("rust:a::step");
        app.spec.pinned = true;
        app.spec.show(step.clone());
        app.spec.plan = Some(plan.clone());
        app.spec.detail = Some(crate::spec::tests::detail(&step, &ComponentId::new("rust:a::save")));
        app.queue.tick(&app.net, &ctx, Some(&plan));
        // Enough jobs and messages that half a page means something.
        let mut snapshot = crate::queue::tests::snapshot(arch_graph::QueueStatus::Idle);
        for i in 0..30 {
            snapshot.jobs.push(arch_graph::Job { component: ComponentId::new(format!("rust:q{i}")), kind: arch_graph::JobKind::Implement, state: arch_graph::JobState::Queued, attempts: 0, findings: vec![] });
        }
        app.queue.on_event(&arch_graph::ArchEvent::QueueUpdated { snapshot: Box::new(snapshot) });
        app.chat = crate::chat::tests::conversation();
        for n in 4..40 {
            app.chat.process_event(crate::chat::tests::item(n, harness_contracts::ItemBody::UserMessage { text: format!("message {n}") }));
        }
        app.column_count = 3;
        (ctx, app)
    }

    /// Shows `kind` in the focused slot and draws it (so it knows its size).
    fn focus(ctx: &egui::Context, app: &mut CockpitApp, kind: BufferKind) {
        app.slot_buffers[app.focused_slot] = kind;
        for _ in 0..3 {
            frame(ctx, app, vec![]);
        }
    }

    fn cursor(app: &CockpitApp) -> Option<String> {
        app.view(app.focused_kind()).cursor_key()
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
        assert!(text.contains("· 4 cards"), "{text}");
        app.graph = loading();
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("Loading the architecture"), "{text}");
        let empty = crate::graph::tests::tree_of(&arch_graph::Graph::new(), None, arch_graph::tree::TreeRequest { view: arch_graph::tree::ViewKind::Calls, ..Default::default() });
        app.graph = showing(empty);
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("Nothing to show in this view yet"), "{text}");
    }

    // req: P9.5, P9.4
    #[test]
    fn vim_sequences_and_leader_drive_the_graph_and_are_rebindable() {
        let (ctx, mut app) = app_with_graph();
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, key("l"));
        assert_eq!(selected(&app), "rust:a::run");
        app.details.insert(crate::goto::tests::detail("rust:a::run", vec![call("rust:a::run", "rust:a::step", 0, 2), call("rust:a::run", "rust:a::log", 1, 3)], vec![]), None);
        frame(&ctx, &mut app, key("g"));
        assert_eq!(app.pending_keys.len(), 1, "waiting for the second key");
        frame(&ctx, &mut app, key("c"));
        assert!(app.pending_keys.is_empty());
        // Two callees: the palette asks which.
        assert_eq!(app.input_mode, InputMode::Palette);
        frame(&ctx, &mut app, press(egui::Key::Escape, false));
        assert_eq!(app.input_mode, InputMode::Normal, "Escape closes the palette");
        frame(&ctx, &mut app, vec![]);
        assert!(!text_input_focused(&ctx), "the palette's field let go of the keyboard");
        app.graph.select(Some("rust:a::run".into()));
        frame(&ctx, &mut app, key("l"));
        assert_eq!(selected(&app), "rust:a::step", "keys work right after the palette");
        // A button holding the keyboard (Tab) doesn't swallow keys either.
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("a button")));
        app.graph.select(Some("rust:a::run".into()));
        frame(&ctx, &mut app, key("l"));
        assert_eq!(selected(&app), "rust:a::step", "keys work while a button has focus");
        assert!(ctx.memory(|m| m.focused()).is_none(), "and the button gave the keyboard back");
        app.details.insert(crate::goto::tests::detail("rust:a::step", vec![call("rust:a::step", "rust:a::save", 0, 5)], vec![]), None);
        keys(&ctx, &mut app, "gc");
        assert_eq!(selected(&app), "rust:a::save", "single callee: straight there");
        ctrl(&ctx, &mut app, egui::Key::O);
        assert_eq!(selected(&app), "rust:a::step", "following an edge is in the jump list: back undoes it");
        keys(&ctx, &mut app, "gw");
        assert!(app.pending_keys.is_empty(), "a key that continues nothing drops the sequence");
        // The leader menu lists the graph's own under g.
        let bound = app.bound_actions();
        let entries = leader_entries(&bound, &['g'], Origin::View(BufferKind::Graph));
        assert!(entries.iter().any(|e| matches!(e, LeaderEntry::Action { key: 'f', index } if bound[*index].action.id == "graph.go_in")));
        assert!(entries.iter().any(|e| matches!(e, LeaderEntry::Action { key: 'v', index } if bound[*index].action.id == "graph.view")));
        // Rebinding from config takes effect.
        app.config = UiConfig::with_overrides("[keymap.bind]\n\"nav.right\" = { keys = [\"w\"] }\n").unwrap();
        app.graph.select(Some("rust:a::run".into()));
        frame(&ctx, &mut app, key("w"));
        assert_eq!(selected(&app), "rust:a::step");
    }

    // req: V.6
    #[test]
    fn tabs_are_icons_that_name_themselves_and_their_key() {
        let (ctx, mut app) = app_with_graph();
        let shell = app.shell_actions();
        let keys = app.tab_keys(&shell);
        let at = |kind: BufferKind| BufferKind::ALL.iter().position(|k| *k == kind).unwrap();
        assert_eq!((keys[at(BufferKind::Graph)].as_str(), keys[at(BufferKind::Queue)].as_str()), ("SPC b g", "SPC b q"));
        // Only each slot's active tab is named.
        let text = texts(&frame(&ctx, &mut app, vec![]));
        let named = |kind: BufferKind| text.lines().any(|l| l == format!("{} {}{}", kind.icon(), &kind.title()[..1], kind.title()[1..].to_lowercase()));
        let active: Vec<BufferKind> = app.slot_buffers.iter().take(app.column_count.clamp(1, 3)).copied().collect();
        for kind in BufferKind::ALL {
            assert_eq!(named(kind), active.contains(&kind), "{kind:?}: {text}");
        }
    }

    // req: P9.8
    #[test]
    fn inspector_summarizes_the_selection_and_links_onward() {
        let (ctx, mut app) = app_with_graph();
        app.graph = showing(sample_tree());
        crate::graph::tests::with_detail(&mut app.graph, "rust:a::step", "src/a.rs", 10, 20);
        app.graph.select(Some("rust:a::step".into()));
        let text = texts(&frame(&ctx, &mut app, vec![]));
        for expected in ["rust:a::step", "function · Existing", "implemented", "src/a.rs:10–20", "Spec view", "Code", "calls", "save", "run"] {
            assert!(text.contains(expected), "drawer shows {expected}: {text}");
        }
        frame(&ctx, &mut app, key("x"));
        assert!(!app.graph.drawer_open, "x closes it");
    }

    // req: P9.15
    #[test]
    fn graph_and_code_link_both_ways_and_follow() {
        let (ctx, mut app) = app_with_graph();
        app.graph = showing(sample_tree());
        crate::graph::tests::with_detail(&mut app.graph, "rust:a::step", "src/a.rs", 10, 20);
        crate::graph::tests::with_detail(&mut app.graph, "rust:a::save", "src/b.rs", 3, 5);
        app.graph.select(Some("rust:a::step".into()));
        frame(&ctx, &mut app, vec![]);
        // Graph -> code: lands on the symbol's span, the code view appears and has focus.
        keys(&ctx, &mut app, "gd");
        frame(&ctx, &mut app, vec![]);
        assert_eq!((app.code.current_file.as_str(), app.code.highlight), ("src/a.rs", Some((10, 20))));
        assert_eq!(app.focused_kind(), BufferKind::Code);
        // Code -> graph: a gutter mark selects its component.
        app.code.select = Some(ComponentId::new("rust:a::save"));
        frame(&ctx, &mut app, vec![]);
        assert_eq!(selected(&app), "rust:a::save");
        // Follow mode: the code view tracks the selection.
        app.run_shell(ShellCommand::ToggleFollow, None, 1, &ctx);
        app.graph.select(Some("rust:a::step".into()));
        frame(&ctx, &mut app, vec![]);
        assert_eq!(app.code.current_file, "src/a.rs");
        app.graph.select(Some("rust:a::save".into()));
        frame(&ctx, &mut app, vec![]);
        assert_eq!((app.code.current_file.as_str(), app.code.highlight), ("src/b.rs", Some((3, 5))));
        // ...and the graph tracks the code cursor once it rests: the lookup
        // goes out, its answer selects the component, the code stays put.
        app.code.set_content((1..=30).map(|i| format!("line {i}\n")).collect());
        app.code.move_cursor(4, -10.0);
        frame(&ctx, &mut app, vec![]);
        assert!(app.following_code, "the cursor rested: its component is being looked up");
        app.reveal_inbox.lock().unwrap().push(Ok(crate::graph::tests::sample_graph().component(&ComponentId::new("rust:a::step")).unwrap().clone()));
        frame(&ctx, &mut app, vec![]);
        assert_eq!(selected(&app), "rust:a::step");
        assert_eq!((app.code.current_file.as_str(), app.code.target_line), ("src/b.rs", Some(4)), "following the code doesn't move the code");
        // Gutter marks come from the file's components.
        let mut file_comp = crate::graph::tests::sample_graph().component(&ComponentId::new("rust:a::log")).unwrap().clone();
        file_comp.anchor = Some(CodeAnchor { language: "rust".into(), file: RepoPath::new("src/b.rs").unwrap(), symbol: "a::log".into(), syntax_kind: "function_item".into(), span: Span { start_line: 7, end_line: 9, start_byte: 70, end_byte: 90 }, content_hash: String::new() });
        file_comp.placeholders.push(Span { start_line: 8, end_line: 8, start_byte: 80, end_byte: 87 });
        let mut missing = Relation::new(RelationKind::Calls, ComponentId::new("rust:a::log"), ComponentId::new("rust:a::save"), Provenance::PLANNED);
        missing.order = Some(0);
        let view = arch_graph::FileView { file: "src/b.rs".into(), components: vec![(file_comp, arch_graph::Alignment::Present)], relations: vec![(missing, arch_graph::Alignment::Missing)], stale: false };
        let marks = crate::code::gutter_marks(&view);
        assert!(matches!(marks[&7][..], [crate::code::Mark::Starts(_, _), crate::code::Mark::MissingCall(_, _)]));
        assert!(matches!(marks[&8][..], [crate::code::Mark::Placeholder(_)]));
    }

    // req: P9.14, P9.3, N.9, N.12
    #[test]
    fn search_marks_counts_and_ends_work_through_the_keys() {
        let (ctx, mut app) = app_with_graph();
        frame(&ctx, &mut app, vec![]);
        // Counts: 2l moves twice (the first move lands on the root).
        frame(&ctx, &mut app, key("2"));
        assert_eq!(app.pending_count, Some(2));
        assert!(texts(&frame(&ctx, &mut app, vec![])).contains("NORMAL 2"), "the count shows");
        frame(&ctx, &mut app, key("l"));
        assert_eq!((selected(&app), app.pending_count), ("rust:a::step", None));
        // ... and repeats a view's own actions: 2+ adds two levels, as one jump.
        keys(&ctx, &mut app, "2+");
        assert_eq!(app.graph.nav.depth, Some(5));
        ctrl(&ctx, &mut app, egui::Key::O);
        assert_eq!(app.graph.nav.depth, Some(3), "back undoes both");
        app.graph.set_tree(sample_tree());
        // G / gg: last and first card in tree order, as jumps.
        frame(&ctx, &mut app, key("G"));
        assert_eq!(selected(&app), "rust:a::save");
        keys(&ctx, &mut app, "gg");
        assert_eq!(selected(&app), "rust:a::run");
        ctrl(&ctx, &mut app, egui::Key::O);
        assert_eq!(selected(&app), "rust:a::save", "ctrl+o returns to where the jump left");
        ctrl(&ctx, &mut app, egui::Key::I);
        assert_eq!(selected(&app), "rust:a::run", "ctrl+i goes forward again");
        // / searches incrementally; Enter jumps; Escape gives up.
        frame(&ctx, &mut app, key("/"));
        assert_eq!(app.input_mode, InputMode::Capture { slot: 1 });
        frame(&ctx, &mut app, key("s"));
        assert_eq!(selected(&app), "rust:a::step", "first match shows while typing");
        assert!(texts(&frame(&ctx, &mut app, vec![])).contains("/s| 2 matches"));
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
        assert_eq!(selected(&app), "rust:a::step", "search jumps are in the jump list");
        keys(&ctx, &mut app, "/lo");
        assert_eq!(selected(&app), "rust:a::log");
        frame(&ctx, &mut app, press(egui::Key::Backspace, false));
        frame(&ctx, &mut app, press(egui::Key::Escape, false));
        assert_eq!((selected(&app), app.input_mode), ("rust:a::step", InputMode::Normal), "Escape restores the selection");
        // Marks: m<letter> sets, '<letter> jumps back (through the jump list).
        keys(&ctx, &mut app, "ma");
        frame(&ctx, &mut app, key("G"));
        assert_eq!(selected(&app), "rust:a::save");
        keys(&ctx, &mut app, "'a");
        assert_eq!(selected(&app), "rust:a::step");
        ctrl(&ctx, &mut app, egui::Key::O);
        assert_eq!(selected(&app), "rust:a::save");
        keys(&ctx, &mut app, "'z");
        assert_eq!((selected(&app), app.input_mode), ("rust:a::save", InputMode::Normal), "an unset mark goes nowhere");
        assert!(app.note.as_deref().unwrap_or_default().contains("No mark z"), "and says so");
        // Keys come from config.
        app.config = UiConfig::with_overrides("[keymap.bind]\n\"nav.last\" = { keys = [\"E\"] }\n").unwrap();
        keys(&ctx, &mut app, "gg");
        frame(&ctx, &mut app, key("E"));
        assert_eq!(selected(&app), "rust:a::save");
    }

    // req: N.2, N.14
    #[test]
    fn the_jump_list_crosses_views_and_slots() {
        let (ctx, mut app) = app_with_graph();
        app.graph = showing(sample_tree());
        crate::graph::tests::with_detail(&mut app.graph, "rust:a::step", "src/a.rs", 10, 20);
        app.graph.select(Some("rust:a::step".into()));
        frame(&ctx, &mut app, vec![]);
        keys(&ctx, &mut app, "gd");
        assert_eq!((app.focused_kind(), app.focused_slot), (BufferKind::Code, 0), "the code opened beside the graph, with focus");
        app.code.set_content((1..=40).map(|i| format!("line {i}\n")).collect());
        keys(&ctx, &mut app, "G");
        assert_eq!(app.code.target_line, Some(40));
        ctrl(&ctx, &mut app, egui::Key::O);
        assert_eq!(app.code.target_line, Some(10), "back within the code");
        ctrl(&ctx, &mut app, egui::Key::O);
        assert_eq!((app.focused_slot, app.focused_kind(), selected(&app)), (1, BufferKind::Graph, "rust:a::step"), "back to the graph's slot, focused, where it was");
        ctrl(&ctx, &mut app, egui::Key::I);
        assert_eq!((app.focused_kind(), app.code.target_line), (BufferKind::Code, Some(10)));
        // Pointer jumps in the graph are in it too.
        app.graph.select(Some("rust:a::step".into()));
        app.graph.jumped = Some(app.graph.place());
        frame(&ctx, &mut app, vec![]);
        assert!(app.jumps.can_back());
        // Nothing left to go back to: says so.
        let mut fresh = CockpitApp::new("http://127.0.0.1:9");
        fresh.initialized = true;
        ctrl(&ctx, &mut fresh, egui::Key::O);
        assert!(fresh.note.as_deref().unwrap_or_default().contains("Nothing to go back to"));
    }

    // req: N.3
    #[test]
    fn every_key_comes_from_config_and_no_two_actions_in_a_view_share_one() {
        let config = UiConfig::with_overrides("").unwrap();
        let only_in = |id: &str| BufferKind::ALL.into_iter().find(|k| id.starts_with(&format!("{}.", k.name())));
        let label = |seq: &crate::keymap::KeySeq| seq.0.iter().map(Chord::label).collect::<Vec<_>>().join(" ");
        for view in BufferKind::ALL {
            let mut seqs: Vec<(String, &str)> = Vec::new();
            for (id, binding) in &config.keymap.bind {
                if only_in(id).is_some_and(|k| k != view) {
                    continue;
                }
                for seq in &binding.keys {
                    seqs.push((label(seq), id.as_str()));
                }
            }
            for (i, (a, id_a)) in seqs.iter().enumerate() {
                assert!(!(a.len() == 1 && a.chars().all(|c| c.is_ascii_digit())), "{id_a}: digits are counts in every view");
                for (b, id_b) in &seqs[i + 1..] {
                    assert_ne!(a, b, "{id_a} and {id_b} share `{a}` in the {} view", view.name());
                    assert!(!b.starts_with(&format!("{a} ")) && !a.starts_with(&format!("{b} ")), "`{a}` ({id_a}) and `{b}` ({id_b}): the shorter can never run in the {} view", view.name());
                }
            }
        }
        // The old clashes are gone: g g is first everywhere, g m is the graph,
        // g r is callers, x reopens in the spec.
        let keys_of = |id: &str| config.keymap.bind.get(id).map(|b| b.keys.iter().map(label).collect::<Vec<_>>()).unwrap_or_default();
        assert_eq!(keys_of("nav.first"), ["g g"]);
        assert_eq!(keys_of("goto.graph"), ["g m"]);
        assert_eq!(keys_of("goto.callers"), ["g r"]);
        assert_eq!(keys_of("spec.reopen"), ["x"]);
        for gone in ["spec.show_in_graph", "queue.show_in_graph", "code.reveal", "graph.back", "graph.search", "graph.hint"] {
            assert!(!config.keymap.bind.contains_key(gone), "{gone} is replaced");
        }
        // Live: whatever each view offers, no key runs two things.
        let (ctx, mut app) = full_app();
        app.graph.select(Some("rust:a::run".into()));
        for kind in BufferKind::ALL {
            focus(&ctx, &mut app, kind);
            let bound = app.bound_actions();
            let mut seen: BTreeMap<String, String> = BTreeMap::new();
            for b in &bound {
                for seq in &b.keys {
                    let k = seq.iter().map(Chord::label).collect::<Vec<_>>().join(" ");
                    if let Some(other) = seen.insert(k.clone(), b.action.id.clone()) {
                        panic!("`{k}` runs both {other} and {} in the {} view", b.action.id, kind.name());
                    }
                }
            }
        }
    }

    // req: N.4, N.21
    #[test]
    fn actions_rank_by_importance_and_apply_to_the_target() {
        let (ctx, mut app) = app_with_graph();
        frame(&ctx, &mut app, vec![]);
        let bound = app.bound_actions();
        assert!(bound.iter().all(|b| (PRIMARY..=MINOR).contains(&b.action.importance)), "every action has an importance");
        let bar: Vec<String> = app.bar_keys(&bound).into_iter().map(|b| b.action.id.clone()).collect();
        assert!(bar.len() <= app.config.keymap.bar.max);
        assert!(!bar.iter().any(|id| id.starts_with("goto.")), "nothing selected: no go-tos: {bar:?}");
        assert!(bar.contains(&"nav.hint".to_owned()) && bar.contains(&"nav.search".to_owned()));
        app.graph.select(Some("rust:a::step".into()));
        let bound = app.bound_actions();
        let bar: Vec<&Bound> = app.bar_keys(&bound);
        let ids: Vec<&str> = bar.iter().map(|b| b.action.id.as_str()).collect();
        assert!(ids.contains(&"nav.enter") && ids.contains(&"goto.code"), "{ids:?}");
        assert!(bar.windows(2).all(|w| w[0].action.importance <= w[1].action.importance), "most important first");
        assert_eq!(bar.iter().find(|b| b.action.id == "nav.enter").unwrap().action.label, "Into step", "says what it does to the target");
        assert!(!ids.contains(&"nav.search_next"), "nothing searched yet");
        app.searches.insert(BufferKind::Graph, "s".into());
        assert!(When::Searched.holds(&app.situation()));
        // The bar draws them, keys first; tappable.
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("Enter Into step") && text.contains("f Jump to something on screen"), "{text}");
        // Narrow screens get big chips; the bar can be turned off.
        app.config = UiConfig::with_overrides("[keymap.bar]\nshow = false\n").unwrap();
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(!text.contains("Enter Into step"));
        // A spec item: the review verbs lead.
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Spec);
        let bound = app.bound_actions();
        let ids: Vec<String> = app.bar_keys(&bound).into_iter().map(|b| b.action.id.clone()).collect();
        assert!(ids[..4].contains(&"spec.comment".to_owned()), "{ids:?}");
        assert!(!When::Named.holds(&app.situation()), "not a code line");
    }

    // req: N.5, N.6, N.7
    #[test]
    fn the_same_moves_work_in_every_view() {
        let (ctx, mut app) = full_app();
        // A longer file, so half pages mean something.
        app.code.set_content((1..=300).map(|i| format!("line {i}\n")).collect());
        app.code.target_line = Some(1);
        for _ in 0..3 {
            app.chat.process_event(harness_contracts::ClientEvent::ThreadClosed { thread_id: harness_contracts::ThreadId::new("none") });
        }
        for kind in [BufferKind::Tree, BufferKind::Code, BufferKind::Spec, BufferKind::Queue, BufferKind::Chat] {
            focus(&ctx, &mut app, kind);
            frame(&ctx, &mut app, key("G"));
            let last = cursor(&app);
            keys(&ctx, &mut app, "gg");
            let first = cursor(&app);
            assert_ne!(first, last, "{kind:?}: gg and G go to the two ends");
            // Where the cursor is in the view's own order (a chat link counts as its message).
            let order = |app: &CockpitApp| {
                let items = app.view(app.focused_kind()).items();
                let c = cursor(app)?;
                items.iter().position(|i| i.key == c).or_else(|| c.split_once(':').and_then(|(head, _)| items.iter().position(|i| i.key == head)))
            };
            let start = order(&app).unwrap_or(0);
            frame(&ctx, &mut app, key("j"));
            let one = order(&app).unwrap();
            assert!(one > start || kind == BufferKind::Chat && one == start, "{kind:?}: j moves down ({start} -> {one})");
            keys(&ctx, &mut app, "2j");
            let three = order(&app).unwrap();
            assert!(three > one, "{kind:?}: 2j moves on ({one} -> {three})");
            for _ in 0..2 {
                frame(&ctx, &mut app, vec![]);
            }
            ctrl(&ctx, &mut app, egui::Key::D);
            let half = order(&app).unwrap();
            assert!(half > three, "{kind:?}: ctrl+d moves half a page ({three} -> {half})");
            ctrl(&ctx, &mut app, egui::Key::U);
            assert!(order(&app).unwrap() < half, "{kind:?}: ctrl+u comes back up");
        }
        // The graph: hjkl, half pages pan.
        focus(&ctx, &mut app, BufferKind::Graph);
        frame(&ctx, &mut app, key("l"));
        assert_eq!(selected(&app), "rust:a::run");
        frame(&ctx, &mut app, key("l"));
        frame(&ctx, &mut app, key("j"));
        assert_eq!(selected(&app), "rust:a::log");
        frame(&ctx, &mut app, key("k"));
        frame(&ctx, &mut app, key("h"));
        assert_eq!(selected(&app), "rust:a::run");
        keys(&ctx, &mut app, "G");
        assert_eq!(selected(&app), "rust:a::save");
    }

    // req: M.8
    #[test]
    fn the_graph_keys_flip_change_depth_switch_view_go_in_and_back() {
        let (ctx, mut app) = app_with_graph();
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, key("r"));
        assert_eq!(app.graph.nav.way, arch_graph::tree::Way::Up, "r flips");
        frame(&ctx, &mut app, key("r"));
        frame(&ctx, &mut app, key("+"));
        assert_eq!(app.graph.nav.depth, Some(4), "+ shows a level more");
        frame(&ctx, &mut app, key("-"));
        assert_eq!(app.graph.nav.depth, Some(3));
        app.graph.select(Some("rust:a::step".into()));
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert_eq!(app.graph.nav.root.as_deref(), Some("rust:a::step"), "Enter goes in");
        frame(&ctx, &mut app, press(egui::Key::Backspace, false));
        assert_eq!(app.graph.nav.root.as_deref(), Some("rust:a::run"), "Backspace comes back");
        frame(&ctx, &mut app, key("v"));
        assert_eq!(app.graph.nav.view, arch_graph::tree::ViewKind::DataFlow, "v: the next view");
    }

    // req: N.8, P9.13
    #[test]
    fn f_labels_what_is_on_screen_in_every_view() {
        let (ctx, mut app) = full_app();
        app.graph.set_tree(sample_tree());
        for kind in BufferKind::ALL {
            focus(&ctx, &mut app, kind);
            let spots = app.view(kind).spots();
            assert!(!spots.is_empty(), "{kind:?}: something drawn to label");
            frame(&ctx, &mut app, key("f"));
            let Some(Capture::Hint(hint)) = &app.capture else { panic!("{kind:?}: hint mode") };
            assert_eq!(hint.labels.len(), spots.len());
            let (label, spot) = hint.labels.last().unwrap().clone();
            let text = texts(&frame(&ctx, &mut app, vec![]));
            assert!(text.contains(&label.to_uppercase()), "{kind:?}: the label is drawn");
            keys(&ctx, &mut app, &label);
            assert_eq!(app.input_mode, InputMode::Normal);
            let at = cursor(&app).unwrap_or_default();
            assert!(spot.key == at || spot.key.starts_with(&format!("{at}:")), "{kind:?}: the cursor went to {} (at {at})", spot.key);
        }
        // F opens: in the graph, it goes into the card.
        focus(&ctx, &mut app, BufferKind::Graph);
        frame(&ctx, &mut app, key("F"));
        let Some(Capture::Hint(hint)) = &app.capture else { panic!() };
        let (label, spot) = hint.labels.last().unwrap().clone();
        keys(&ctx, &mut app, &label);
        assert_eq!((selected(&app), app.graph.nav.root.as_deref()), (spot.key.as_str(), Some(spot.key.as_str())));
        // A key no label starts with gives up; Escape too.
        frame(&ctx, &mut app, key("f"));
        frame(&ctx, &mut app, key("z"));
        assert!(app.capture.is_none() && app.input_mode == InputMode::Normal);
    }

    // req: N.9
    #[test]
    fn slash_searches_every_view() {
        let (ctx, mut app) = full_app();
        for (kind, query, lands) in [
            (BufferKind::Graph, "save", "rust:a::save"),
            (BufferKind::Tree, "log", "rust:a::log"),
            (BufferKind::Code, "log(", "4"),
            (BufferKind::Spec, "write", "logic#Step(1)"),
            (BufferKind::Queue, "c", "2"),
            (BufferKind::Chat, "parser", "0"),
        ] {
            focus(&ctx, &mut app, kind);
            frame(&ctx, &mut app, key("/"));
            keys(&ctx, &mut app, query);
            frame(&ctx, &mut app, press(egui::Key::Enter, false));
            assert_eq!(cursor(&app).as_deref(), Some(lands), "{kind:?}: /{query}");
            assert!(app.searches.contains_key(&kind));
        }
        // n / N step through a view's matches.
        focus(&ctx, &mut app, BufferKind::Code);
        keys(&ctx, &mut app, "/step");
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert_eq!(cursor(&app).as_deref(), Some("3"));
        frame(&ctx, &mut app, key("n"));
        assert_eq!(cursor(&app).as_deref(), Some("6"));
        frame(&ctx, &mut app, key("N"));
        assert_eq!(cursor(&app).as_deref(), Some("3"));
    }

    // req: N.10
    #[test]
    fn enter_goes_in_and_backspace_comes_out_in_every_view() {
        let (ctx, mut app) = full_app();
        // A key, then a frame for what it asked of other views.
        let enter = |ctx: &egui::Context, app: &mut CockpitApp| {
            frame(ctx, app, press(egui::Key::Enter, false));
            frame(ctx, app, vec![])
        };
        let leave = |ctx: &egui::Context, app: &mut CockpitApp| {
            frame(ctx, app, press(egui::Key::Backspace, false));
            frame(ctx, app, vec![])
        };
        // Graph: go in, back.
        focus(&ctx, &mut app, BufferKind::Graph);
        app.graph.select(Some("rust:a::step".into()));
        enter(&ctx, &mut app);
        assert_eq!(app.graph.nav.root.as_deref(), Some("rust:a::step"));
        leave(&ctx, &mut app);
        assert_eq!(app.graph.nav.root.as_deref(), Some("rust:a::run"), "back where going in started");
        app.graph.set_tree(sample_tree());
        // Tree: Enter selects in the graph (focus stays).
        focus(&ctx, &mut app, BufferKind::Tree);
        app.tree.move_to("rust:a::save");
        enter(&ctx, &mut app);
        assert_eq!((selected(&app), app.focused_kind()), ("rust:a::save", BufferKind::Tree));
        leave(&ctx, &mut app);
        assert_eq!(cursor(&app).as_deref(), Some("rust:a"), "Backspace: up to what holds it");
        // Code: Enter on a call goes to its definition.
        focus(&ctx, &mut app, BufferKind::Code);
        app.code.move_to("3");
        enter(&ctx, &mut app);
        assert_eq!(app.code.target_line, Some(6), "step's definition");
        leave(&ctx, &mut app);
        // Spec: Enter on a section title folds it.
        focus(&ctx, &mut app, BufferKind::Spec);
        keys(&ctx, &mut app, "gg");
        enter(&ctx, &mut app);
        assert!(app.spec.folded.contains(&crate::spec::Section::Overview));
        // Queue: Enter shows a failed job's findings.
        focus(&ctx, &mut app, BufferKind::Queue);
        app.queue.move_to("2");
        enter(&ctx, &mut app);
        assert!(app.queue.expanded.contains(&ComponentId::new("rust:c")));
        leave(&ctx, &mut app);
        assert!(app.queue.expanded.is_empty());
        // Chat: Enter on a message with a file opens it in the code view.
        focus(&ctx, &mut app, BufferKind::Chat);
        app.chat.move_to("2");
        enter(&ctx, &mut app);
        assert_eq!((app.focused_kind(), app.code.current_file.as_str()), (BufferKind::Code, "src/parse.rs"));
        // Nothing to do: it says why.
        focus(&ctx, &mut app, BufferKind::Chat);
        app.chat.move_to("0");
        enter(&ctx, &mut app);
        assert!(app.note.as_deref().unwrap_or_default().contains("No file"));
    }

    // req: N.11
    #[test]
    fn z_keys_fold_in_every_view_that_folds() {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Code);
        app.code.move_to("3");
        keys(&ctx, &mut app, "zc");
        assert_eq!(app.code.visible_lines().len(), 6);
        keys(&ctx, &mut app, "zo");
        assert_eq!(app.code.visible_lines().len(), 9);
        keys(&ctx, &mut app, "zM");
        assert_eq!(app.code.visible_lines().len(), 4);
        keys(&ctx, &mut app, "zR");
        focus(&ctx, &mut app, BufferKind::Tree);
        keys(&ctx, &mut app, "zM");
        assert_eq!(app.tree.rows().len(), 2);
        keys(&ctx, &mut app, "zR");
        assert_eq!(app.tree.rows().len(), 6);
        focus(&ctx, &mut app, BufferKind::Spec);
        keys(&ctx, &mut app, "zM");
        assert_eq!(app.spec.folded.len(), crate::spec::Section::ALL.len());
        keys(&ctx, &mut app, "za");
        assert_eq!(app.spec.folded.len(), crate::spec::Section::ALL.len() - 1);
    }

    // req: N.12
    #[test]
    fn marks_work_across_views() {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Code);
        app.code.move_to("6");
        keys(&ctx, &mut app, "mc");
        app.focused_slot = 2;
        focus(&ctx, &mut app, BufferKind::Tree);
        app.tree.move_to("rust:b");
        keys(&ctx, &mut app, "mt");
        app.focused_slot = 0;
        focus(&ctx, &mut app, BufferKind::Graph);
        keys(&ctx, &mut app, "'c");
        assert_eq!((app.focused_slot, app.focused_kind(), app.code.target_line), (1, BufferKind::Code, Some(6)), "back to the code, in its slot");
        keys(&ctx, &mut app, "'t");
        assert_eq!((app.focused_slot, app.tree.cursor.as_ref().map(|c| c.as_str())), (2, Some("rust:b")));
        ctrl(&ctx, &mut app, egui::Key::O);
        assert_eq!(app.focused_kind(), BufferKind::Code, "a mark jump is in the jump list");
        // The popup after ' lists them.
        frame_at(&ctx, &mut app, key("'"), 50.0);
        // (egui lays a new popup out unseen first, then draws it.)
        frame_at(&ctx, &mut app, vec![], 51.0);
        let text = texts(&frame_at(&ctx, &mut app, vec![], 51.05));
        assert!(text.contains("code · src/a.rs:6") && text.contains("tree · b"), "{text}");
        frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 51.1);
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    // req: N.13
    #[test]
    fn question_mark_lists_this_views_keys_grouped_and_searchable() {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Spec);
        frame(&ctx, &mut app, key("?"));
        assert!(app.key_sheet.is_some());
        let text = texts(&frame(&ctx, &mut app, vec![]));
        for expected in ["SPEC keys", "Navigate", "Go to", "Act", "g d", "Go to its code", "Comment on this item", "J", "Next section"] {
            assert!(text.contains(expected), "{expected}: {text}");
        }
        assert!(!text.contains("Next view"), "only this view's keys (and the shared ones)");
        app.key_sheet = Some("fold".into());
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("View") && text.contains("Fold or unfold") && text.contains("z a"), "{text}");
        app.key_sheet = Some("callee".into());
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("Go to a callee") && !text.contains("Half a page down"), "{text}");
        app.graph.drawer_open = true;
        frame(&ctx, &mut app, press(egui::Key::Escape, false));
        assert!(app.key_sheet.is_none());
        frame(&ctx, &mut app, vec![]);
        assert!(!text_input_focused(&ctx));
        assert!(app.graph.drawer_open, "Escape closed the sheet and nothing else");
    }

    // req: N.14, N.19
    #[test]
    fn go_to_keys_work_from_every_view() {
        let (ctx, mut app) = full_app();
        // Outline -> graph: g m shows it and moves focus there.
        focus(&ctx, &mut app, BufferKind::Tree);
        app.tree.move_to("rust:a::step");
        keys(&ctx, &mut app, "gm");
        assert_eq!((app.focused_kind(), selected(&app)), (BufferKind::Graph, "rust:a::step"));
        // Graph -> outline: g t.
        app.graph.select(Some("rust:a::log".into()));
        keys(&ctx, &mut app, "gt");
        assert_eq!((app.focused_kind(), app.tree.cursor.as_ref().map(|c| c.as_str())), (BufferKind::Tree, Some("rust:a::log")));
        // Queue -> spec: g s (a pinned spec shows it anyway).
        focus(&ctx, &mut app, BufferKind::Queue);
        app.queue.move_to("1");
        keys(&ctx, &mut app, "gs");
        assert_eq!((app.focused_kind(), app.spec.target.as_ref().map(|c| c.as_str())), (BufferKind::Spec, Some("rust:a")));
        // The queue's own keys: o opens the running job's session.
        focus(&ctx, &mut app, BufferKind::Queue);
        app.queue.on_event(&arch_graph::ArchEvent::QueueUpdated { snapshot: Box::new(crate::queue::tests::snapshot(arch_graph::QueueStatus::Running)) });
        keys(&ctx, &mut app, "gg");
        frame(&ctx, &mut app, key("o"));
        frame(&ctx, &mut app, vec![]);
        assert_eq!((app.focused_kind(), app.chat.active_thread.as_ref().map(|t| t.as_str())), (BufferKind::Chat, Some("t1")));
        // Graph -> queue: g q finds the component's job, or says there isn't one.
        focus(&ctx, &mut app, BufferKind::Graph);
        app.graph.select(Some("rust:a::run".into()));
        keys(&ctx, &mut app, "gq");
        assert!(app.note.as_deref().unwrap_or_default().contains("no job"), "{:?}", app.note);
        // Code: g r on a definition lists its callers; one: straight to the call.
        focus(&ctx, &mut app, BufferKind::Code);
        app.code.move_to("6");
        app.details.insert(crate::goto::tests::detail("rust:a::step", vec![], vec![call("rust:a::run", "rust:a::step", 0, 3)]), app.graph.nav.plan.as_ref());
        keys(&ctx, &mut app, "gr");
        assert_eq!(app.code.target_line, Some(3), "at the call in run");
        // A detail not here yet: the go-to waits for it.
        app.code.move_to("2");
        keys(&ctx, &mut app, "gc");
        assert!(app.pending_goto.is_some(), "waiting for run's relations");
        app.details.insert(crate::goto::tests::detail("rust:a::run", vec![call("rust:a::run", "rust:a::step", 0, 3), call("rust:a::run", "rust:x::log", 1, 4)], vec![]), app.graph.nav.plan.as_ref());
        frame(&ctx, &mut app, vec![]);
        assert!(app.pending_goto.is_none() && app.input_mode == InputMode::Palette, "two callees: pick one");
        let pick = app.bound_actions().into_iter().find(|b| b.action.id == "goto.pick").unwrap();
        frame(&ctx, &mut app, press(egui::Key::Escape, false));
        app.invoke(Invocation { action: pick.action, origin: pick.origin, value: Some("rust:a::step".into()) }, &ctx);
        assert_eq!(app.code.target_line, Some(6), "picked: step's definition");
        // Spec: a relation's g s (or Enter) follows it.
        let step = ComponentId::new("rust:a::step");
        app.spec.show(step.clone());
        app.spec.detail = Some(crate::spec::tests::detail(&step, &ComponentId::new("rust:a::save")));
        focus(&ctx, &mut app, BufferKind::Spec);
        app.spec.move_to(&crate::spec::Entry { section: crate::spec::Section::Relations, item: crate::nav::SpecItem::Relation { kind: RelationKind::Calls, other: ComponentId::new("rust:a::save"), outgoing: true } }.key());
        keys(&ctx, &mut app, "gs");
        assert_eq!(app.spec.target.as_ref().map(|c| c.as_str()), Some("rust:a::save"));
        // Nothing under the cursor: says so.
        let (ctx, mut empty) = app_with_graph();
        frame(&ctx, &mut empty, vec![]);
        keys(&ctx, &mut empty, "gd");
        assert!(empty.note.as_deref().unwrap_or_default().contains("Nothing under the cursor"));
    }

    // req: N.18
    #[test]
    fn spec_keys_walk_items_and_review_them() {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Spec);
        keys(&ctx, &mut app, "J");
        assert_eq!(app.spec.current, crate::spec::Section::Signature);
        keys(&ctx, &mut app, "]]");
        assert_eq!(app.spec.current, crate::spec::Section::Logic);
        keys(&ctx, &mut app, "j");
        assert_eq!(app.spec.entry().unwrap().item, crate::nav::SpecItem::Step(0));
        frame(&ctx, &mut app, key("c"));
        frame(&ctx, &mut app, vec![]);
        assert_eq!(app.input_mode, InputMode::Insert { slot: 1 }, "c: the composer has the keyboard");
        assert!(app.spec.composer.is_some());
        frame(&ctx, &mut app, press(egui::Key::Escape, false));
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    // req: N.22
    #[test]
    fn a_pause_on_a_prefix_shows_what_can_follow() {
        let (ctx, mut app) = app_with_graph();
        app.graph.select(Some("rust:a::step".into()));
        frame_at(&ctx, &mut app, vec![], 10.0);
        frame_at(&ctx, &mut app, key("g"), 10.0);
        // The popup: its title is the keys so far, and it says how to cancel.
        let popup = |text: &str, prefix: &str| text.contains(&format!("{prefix} …")) && text.contains("Escape to cancel");
        let quick = texts(&frame_at(&ctx, &mut app, vec![], 10.1));
        assert!(!popup(&quick, "g"), "not before the pause");
        frame_at(&ctx, &mut app, vec![], 10.45);
        let text = texts(&frame_at(&ctx, &mut app, vec![], 10.5));
        assert!(popup(&text, "g") && text.contains("Go to its code") && text.contains("Go to a caller") && text.contains("Go to what holds it"), "{text}");
        frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 10.6);
        assert!(app.pending_keys.is_empty());
        assert!(!popup(&texts(&frame_at(&ctx, &mut app, vec![], 11.5)), "g"), "Escape dismisses it");
        // Typed quickly: never shown.
        frame_at(&ctx, &mut app, key("z"), 20.0);
        frame_at(&ctx, &mut app, key("z"), 20.05);
        assert!(!popup(&texts(&frame_at(&ctx, &mut app, vec![], 21.0)), "z"));
        // The delay is config.
        app.config = UiConfig::with_overrides("[keymap]\nwhich_key_delay_ms = 2000\n").unwrap();
        frame_at(&ctx, &mut app, key("z"), 30.0);
        assert!(!popup(&texts(&frame_at(&ctx, &mut app, vec![], 31.0)), "z"));
        frame_at(&ctx, &mut app, vec![], 32.05);
        let text = texts(&frame_at(&ctx, &mut app, vec![], 32.1));
        assert!(popup(&text, "z") && text.contains("Fit to screen") && text.contains("Fold everything"), "{text}");
        // With nothing selected, the go-tos that need a target aren't listed.
        frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 33.0);
        app.graph.select(None);
        frame_at(&ctx, &mut app, key("g"), 40.0);
        frame_at(&ctx, &mut app, vec![], 42.9);
        let text = texts(&frame_at(&ctx, &mut app, vec![], 43.0));
        assert!(popup(&text, "g") && text.contains("First") && !text.contains("Go to what holds it"), "{text}");
    }

    // req: N.23
    #[test]
    fn the_palette_still_reaches_other_views_actions() {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Graph);
        let bound = app.bound_actions();
        assert!(!bound.iter().any(|b| b.action.id == "spec.pin"), "not bound from the graph");
        let palette = app.palette_actions();
        let pin = palette.iter().find(|b| b.action.id == "spec.pin").expect("in the palette");
        assert!(pin.action.label.starts_with("Spec · ") && pin.keys.is_empty() && pin.leader.is_none());
        let was = app.spec.pinned;
        app.invoke(Invocation { action: pin.action.clone(), origin: pin.origin, value: None }, &ctx);
        assert_ne!(app.spec.pinned, was, "it runs on its view");
        // `:` then a number goes to that line or row.
        assert!(palette.iter().any(|b| b.action.id == "nav.line"));
        focus(&ctx, &mut app, BufferKind::Code);
        let line = app.palette_actions().into_iter().find(|b| b.action.id == "nav.line").unwrap();
        app.invoke(Invocation { action: line.action, origin: line.origin, value: Some("7".into()) }, &ctx);
        assert_eq!(app.code.target_line, Some(7));
    }

    // req: M.14
    #[test]
    fn the_graph_says_when_an_answer_was_slow_or_cut() {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Graph);
        let mut tree = crate::graph::tests::sample_tree();
        tree.slow_ms = Some(340);
        tree.cut = Some(arch_graph::tree::Cut { reason: arch_graph::tree::CutReason::Time, starts_left_out: 0, shown: 2, asked: 3, hidden: 7 });
        app.graph.set_tree(tree);
        let shown = texts(&frame(&ctx, &mut app, vec![]));
        assert!(shown.contains("slow: 340 ms"), "{shown}");
        assert!(shown.contains("showing 2 of 3 levels: 7 more (out of time)"), "{shown}");
    }

    // req: M.13
    #[test]
    fn find_in_the_code_asks_the_graph_as_its_typed() {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Graph);
        let find = app.palette_actions().into_iter().find(|b| b.action.id == "graph.find").expect("the graph offers find");
        app.open_palette(Some(&find));
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![egui::Event::Text("run".into())]);
        assert_eq!(app.graph.find_choices(), (Vec::new(), true), "the query went to the graph, which waits for answers");
        // The palette's own list is matched the same way as everywhere.
        let ranked = arch_search::rank("grph fnd", &["Graph · Find in the code", "Code · Reveal"], arch_search::Matching::shared());
        assert_eq!(ranked.first().map(|r| r.index), Some(0));
    }

    // req: N.6, N.16
    #[test]
    fn the_code_view_scrolls_with_a_chat_beside_it() {
        let (ctx, mut app) = full_app();
        app.slot_buffers = [BufferKind::Chat, BufferKind::Code, BufferKind::Tree];
        app.focused_slot = 1;
        app.code.set_content((1..=500).map(|i| format!("fn line_{i}() {{}}\n")).collect());
        for _ in 0..3 {
            frame(&ctx, &mut app, vec![]);
        }
        app.run_shell(ShellCommand::Motion(Motion::Line(300)), None, 1, &ctx);
        let mut text = String::new();
        for _ in 0..3 {
            text = texts(&frame(&ctx, &mut app, vec![]));
        }
        assert!(text.contains("fn line_300() {}"), "the code scrolled to its cursor");
        frame(&ctx, &mut app, key("G"));
        for _ in 0..2 {
            text = texts(&frame(&ctx, &mut app, vec![]));
        }
        assert!(text.contains("fn line_500() {}"));
    }

    // req: N.23
    #[test]
    fn the_leader_menu_opens_with_the_focused_views_own_menu() {
        let (ctx, mut app) = full_app();
        for kind in [BufferKind::Code, BufferKind::Spec, BufferKind::Queue] {
            focus(&ctx, &mut app, kind);
            let bound = app.bound_actions();
            let entries = leader_entries(&bound, &[], Origin::View(kind));
            let menu = app.config.keymap.view_menu;
            assert_eq!(entries.first(), Some(&LeaderEntry::Group { key: menu }), "{kind:?}: its own menu first");
            assert!(is_primary(&entries[0], &bound, &[], Origin::View(kind)));
            assert!(!entries[1..].iter().any(|e| is_primary(e, &bound, &[], Origin::View(kind))) || kind == BufferKind::Graph);
            assert!(!entries.iter().any(|e| *e == LeaderEntry::Group { key: 'g' }), "{kind:?}: the graph's menu isn't here");
            // Inside it: that view's actions.
            let own = leader_entries(&bound, &[menu], Origin::View(kind));
            assert!(own.iter().all(|e| matches!(e, LeaderEntry::Action { index, .. } if bound[*index].origin == Origin::View(kind))));
        }
        // Drawn: the view's name heads its block.
        focus(&ctx, &mut app, BufferKind::Spec);
        app.input_mode = InputMode::Leader;
        frame(&ctx, &mut app, vec![]);
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("SPEC") && text.contains("All spec actions…"), "{text}");
        frame(&ctx, &mut app, key(","));
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("LEADER > ALL SPEC ACTIONS") && text.contains("Unpin (follow the graph)"), "{text}");
    }

    // req: N.22, N.4
    #[test]
    fn a_code_line_offers_every_go_to() {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Code);
        app.code.move_to("1");
        assert!(When::Component.holds(&app.situation()), "a line is in some component, even before the index says which");
        frame_at(&ctx, &mut app, key("g"), 70.0);
        frame_at(&ctx, &mut app, vec![], 71.0);
        let text = texts(&frame_at(&ctx, &mut app, vec![], 71.05));
        for expected in ["Show it in the graph", "Go to a caller", "Go to its spec", "Go to its queue job"] {
            assert!(text.contains(expected), "{expected}: {text}");
        }
        // No plan: no queue to go to.
        frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 72.0);
        app.queue.plan = None;
        assert!(!app.shell_actions().iter().any(|a| a.id == "goto.queue"));
    }

    // req: N.9, N.17, N.18, N.15
    #[test]
    fn what_the_audit_found_stays_fixed() {
        let (ctx, mut app) = full_app();
        // A tree filter clears from the keys: / then Enter on nothing.
        focus(&ctx, &mut app, BufferKind::Tree);
        keys(&ctx, &mut app, "/lo");
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert_eq!(app.tree.search_filter, "lo");
        frame(&ctx, &mut app, key("/"));
        assert_eq!(app.tree.search_filter, "", "an empty search empties the filter");
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert_eq!((app.tree.search_filter.as_str(), app.tree.rows().len()), ("", 6));
        // A graph search that finds nothing puts the view back as it was.
        focus(&ctx, &mut app, BufferKind::Graph);
        app.graph.set_tree(crate::graph::tests::boxes_tree());
        let before = app.graph.place();
        keys(&ctx, &mut app, "/zzz");
        assert_eq!(app.graph.place(), before);
        frame(&ctx, &mut app, press(egui::Key::Escape, false));
        assert_eq!(app.graph.place(), before);
        // c in the spec without a plan says why.
        focus(&ctx, &mut app, BufferKind::Spec);
        app.spec.plan = None;
        frame(&ctx, &mut app, key("c"));
        assert!(app.note.as_deref().unwrap_or_default().contains("pick one in the graph"), "{:?}", app.note);
        assert_eq!(app.input_mode, InputMode::Normal);
        // z o on a card with nothing more under it says why too.
        focus(&ctx, &mut app, BufferKind::Graph);
        app.graph.set_tree(sample_tree());
        app.graph.select(Some("rust:a::save".into()));
        keys(&ctx, &mut app, "zo");
        assert!(app.graph.note.as_deref().unwrap_or_default().contains("nothing more"), "{:?}", app.graph.note);
        // A key that runs clears the last note.
        frame(&ctx, &mut app, key("j"));
        assert!(app.note.is_none());
        // Fewer columns keep the focused view on screen.
        app.focused_slot = 2;
        app.slot_buffers[2] = BufferKind::Spec;
        app.run_shell(ShellCommand::SetLayout(1), None, 1, &ctx);
        assert_eq!((app.focused_slot, app.slot_buffers[0]), (0, BufferKind::Spec));
    }

    // req: N.4, N.10, N.21, N.18
    #[test]
    fn enter_and_backspace_are_only_offered_where_they_act() {
        let (ctx, mut app) = full_app();
        let offered = |app: &CockpitApp, id: &str| {
            let bound = app.bound_actions();
            app.bar_keys(&bound).iter().any(|b| b.action.id == id)
        };
        // Graph: Backspace only once gone in somewhere.
        focus(&ctx, &mut app, BufferKind::Graph);
        app.graph = showing(crate::graph::tests::tree_of(&crate::graph::tests::sample_graph(), None, arch_graph::tree::TreeRequest { view: arch_graph::tree::ViewKind::Calls, ..Default::default() }));
        app.graph.nav.plan = Some(PlanId::new("p").unwrap());
        app.graph.select(Some("rust:a::run".into()));
        assert!(!app.situation().can_leave, "the starting points already");
        assert!(!offered(&app, "nav.leave"));
        app.graph.navigate(crate::graph::NavChange::GoIn("rust:a::run".into()));
        assert!(app.situation().can_leave);
        // Code: Enter on a call, Backspace inside a definition.
        focus(&ctx, &mut app, BufferKind::Code);
        app.code.move_to("2");
        assert!(!app.situation().can_enter && !app.situation().can_leave, "run's first line: no call, not inside");
        app.code.move_to("3");
        assert!(app.situation().can_enter && app.situation().can_leave);
        // Queue: Enter only on a job with findings.
        focus(&ctx, &mut app, BufferKind::Queue);
        app.queue.move_to("0");
        assert!(!app.situation().can_enter, "queued, no findings");
        app.queue.move_to("2");
        assert!(app.situation().can_enter);
        // Spec: on a comment, reply leads the bar.
        focus(&ctx, &mut app, BufferKind::Spec);
        let step = ComponentId::new("rust:a::step");
        app.spec.comments = vec![crate::review::tests::comment(1, arch_graph::CommentAnchor::Section { id: step, section: "logic".into() }, arch_graph::CommentStatus::Open)];
        app.spec.move_to("logic#Comment(CommentId(\"c1\"))");
        let bound = app.bound_actions();
        let bar: Vec<String> = app.bar_keys(&bound).into_iter().map(|b| b.action.id.clone()).collect();
        assert_eq!(bar.first().map(String::as_str), Some("spec.reply"), "{bar:?}");
        assert!(!bar.contains(&"spec.comment".to_owned()) || bar.iter().position(|b| b == "spec.reply") < bar.iter().position(|b| b == "spec.comment"));
    }

    // req: N.8
    #[test]
    fn hint_labels_sit_beside_their_items() {
        let slot = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 400.0));
        let size = egui::vec2(24.0, 18.0);
        let room_left = hint_chip(egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(80.0, 20.0)), size, slot, &[]);
        assert!(room_left.right() <= 100.0, "left of it: {room_left:?}");
        let at_edge = hint_chip(egui::Rect::from_min_size(egui::pos2(4.0, 50.0), egui::vec2(80.0, 20.0)), size, slot, &[]);
        assert!(at_edge.left() >= 84.0, "right of it: {at_edge:?}");
        let full = hint_chip(egui::Rect::from_min_size(egui::pos2(4.0, 50.0), egui::vec2(394.0, 20.0)), size, slot, &[]);
        assert!(full.right() <= 398.0 && full.left() > 300.0, "inside its right end: {full:?}");
        // Never on a label already placed: the next spot over.
        let crowded = hint_chip(egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(80.0, 20.0)), size, slot, &[room_left]);
        assert!(!crowded.intersects(room_left) && crowded.left() >= 180.0, "{crowded:?}");
    }

    // req: N.9, N.2, N.21
    #[test]
    fn searches_and_restores_put_things_back_exactly() {
        let (ctx, mut app) = full_app();
        // A graph search that matches nothing, from no selection, ends with no selection.
        focus(&ctx, &mut app, BufferKind::Graph);
        app.graph.select(None);
        app.graph.drawer_open = false;
        keys(&ctx, &mut app, "/sa");
        assert_eq!(selected(&app), "rust:a::save", "a preview");
        keys(&ctx, &mut app, "zz");
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert_eq!((selected(&app), app.graph.drawer_open), ("", false), "nothing selected, the inspector still closed");
        // An empty search forgets the last one (no stale position in the bar).
        focus(&ctx, &mut app, BufferKind::Tree);
        keys(&ctx, &mut app, "/step");
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert!(app.search_status().is_some());
        frame(&ctx, &mut app, key("/"));
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert!(app.search_status().is_none() && !app.searches.contains_key(&BufferKind::Tree));
        // The spec comes back to the item, not just its old position.
        focus(&ctx, &mut app, BufferKind::Spec);
        app.spec.move_to("logic#Step(1)");
        let place = app.spec.place();
        app.spec.folded.insert(crate::spec::Section::Overview);
        app.spec.motion(Motion::First, 1);
        app.spec.restore(&place);
        assert_eq!(app.spec.entry().unwrap().item, crate::nav::SpecItem::Step(1));
        assert_eq!(app.spec.current, crate::spec::Section::Logic);
        // One column and back: the slots as they were.
        app.focused_slot = 2;
        app.slot_buffers = [BufferKind::Code, BufferKind::Graph, BufferKind::Queue];
        app.run_shell(ShellCommand::SetLayout(1), None, 1, &ctx);
        assert_eq!(app.slot_buffers[0], BufferKind::Queue);
        app.run_shell(ShellCommand::SetLayout(3), None, 1, &ctx);
        assert_eq!((app.slot_buffers, app.focused_slot), ([BufferKind::Code, BufferKind::Graph, BufferKind::Queue], 2));
    }

    // req: N.9, N.18, N.2, N.22, N.19, N.13
    #[test]
    fn what_the_final_audit_found_stays_fixed() {
        let (ctx, mut app) = full_app();
        // Escape after a search leaves a closed inspector closed.
        focus(&ctx, &mut app, BufferKind::Graph);
        app.graph.select(Some("rust:a::run".into()));
        app.graph.drawer_open = false;
        keys(&ctx, &mut app, "/sa");
        frame(&ctx, &mut app, press(egui::Key::Escape, false));
        assert_eq!((selected(&app), app.graph.drawer_open), ("rust:a::run", false));
        keys(&ctx, &mut app, "/sazz");
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert_eq!((selected(&app), app.graph.drawer_open), ("rust:a::run", false), "no match: nothing changed");
        assert!(!app.searches.contains_key(&BufferKind::Graph), "and nothing to step through with n");
        frame(&ctx, &mut app, key("n"));
        assert!(app.note.as_deref().unwrap_or_default().contains("No match for sazz"), "n says why: {:?}", app.note);
        // The review point belongs to its component: moving on forgets it.
        focus(&ctx, &mut app, BufferKind::Spec);
        let step = ComponentId::new("rust:a::step");
        app.spec.comments = vec![crate::review::tests::comment(1, arch_graph::CommentAnchor::Section { id: step.clone(), section: "logic".into() }, arch_graph::CommentStatus::Open)];
        app.spec.step_point(true);
        assert!(app.spec.actions().iter().any(|a| a.id == "spec.reply"));
        app.spec.show(ComponentId::new("rust:a::save"));
        assert!(app.spec.point.is_none() && !app.spec.actions().iter().any(|a| a.id == "spec.reply"), "no replying to another component's point");
        // Back to a relation item.
        app.spec.show(step.clone());
        let relation = crate::spec::Entry { section: crate::spec::Section::Relations, item: crate::nav::SpecItem::Relation { kind: RelationKind::Calls, other: ComponentId::new("rust:a::save"), outgoing: true } }.key();
        app.spec.detail = Some(crate::spec::tests::detail(&step, &ComponentId::new("rust:a::save")));
        app.spec.restore(&Place::Spec { component: Some(step.clone()), item: Some(relation.clone()) });
        assert_eq!(app.spec.cursor_key().as_deref(), Some(relation.as_str()));
        // A second Escape throws a draft away.
        app.spec.plan = Some(PlanId::new("p").unwrap());
        frame(&ctx, &mut app, key("c"));
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, press(egui::Key::Escape, false));
        assert!(app.spec.composer.is_some() && app.input_mode == InputMode::Normal, "the first leaves the field");
        frame(&ctx, &mut app, press(egui::Key::Escape, false));
        assert!(app.spec.composer.is_none(), "the second discards it");
        // Go-tos are offered only where they lead, once the relations are known.
        focus(&ctx, &mut app, BufferKind::Graph);
        app.graph.select(Some("rust:a::save".into()));
        app.details.insert(crate::goto::tests::detail("rust:a::save", vec![], vec![call("rust:a::step", "rust:a::save", 0, 5)]), app.graph.nav.plan.as_ref());
        let situation = app.situation();
        assert!(When::Reaches(GoTo::Callers).holds(&situation) && !When::Reaches(GoTo::Callees).holds(&situation));
        // The queue's verbs are always in its key sheet, applying or not.
        focus(&ctx, &mut app, BufferKind::Queue);
        app.queue.move_to("3");
        let approve = app.bound_actions().into_iter().find(|b| b.action.id == "queue.approve").expect("listed");
        assert_eq!(approve.action.when, When::Unavailable, "a queued job can't be approved");
        frame(&ctx, &mut app, key("a"));
        assert!(app.note.as_deref().unwrap_or_default().contains("waiting for your approval"), "and says why: {:?}", app.note);
    }

    // req: N.1, N.15, N.18
    #[test]
    fn the_selection_survives_a_view_without_it() {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Graph);
        app.graph.select(Some("rust:a::step".into()));
        frame(&ctx, &mut app, vec![]);
        assert_eq!(app.spec.target.as_ref().map(|t| t.as_str()), Some("rust:a::step"));
        // A view without it: nothing selected there, but it's remembered (the spec keeps it).
        app.spec.pinned = false;
        // Flipped: what calls run (nothing does), so step isn't there.
        app.graph.navigate(crate::graph::NavChange::Flip);
        let without = crate::graph::tests::tree_of(&crate::graph::tests::sample_graph(), None, app.graph.nav.request());
        app.graph.set_tree(without);
        frame(&ctx, &mut app, vec![]);
        assert_eq!((app.graph.selection.clone(), app.graph.current().map(|c| c.0)), (None, Some("rust:a::step".to_owned())));
        assert_eq!(app.spec.target.as_ref().map(|t| t.as_str()), Some("rust:a::step"), "the spec still shows it");
        // Back in a view that has it: selected again.
        app.graph.navigate(crate::graph::NavChange::Flip);
        app.graph.set_tree(sample_tree());
        assert_eq!(selected(&app), "rust:a::step");
        // Enter and ]c with nothing to act on say so.
        app.graph.select(None);
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert!(app.note.as_deref().unwrap_or_default().contains("Select a card first"), "{:?}", app.note);
        focus(&ctx, &mut app, BufferKind::Spec);
        app.spec.comments.clear();
        keys(&ctx, &mut app, "]c");
        assert!(app.note.as_deref().unwrap_or_default().contains("No open review points"), "{:?}", app.note);
    }

    fn index_event(status: arch_graph::IndexStatus) -> harness_contracts::ClientEvent {
        harness_contracts::ClientEvent::Architecture { event: arch_graph::ArchEvent::Index { status: Box::new(status) } }
    }

    // req: I.1, I.2, I.3, I.4, I.5
    #[test]
    fn indexing_shows_its_state_everywhere_and_can_be_run_from_the_cockpit() {
        let (ctx, mut app) = full_app();
        crate::status::set(app.config.status.clone());
        // Re-index actions: changed files, from scratch, the open file.
        let bound = app.bound_actions();
        let leader = |id: &str| bound.iter().find(|b| b.action.id == id).and_then(|b| b.leader.clone()).map(|l| l.into_iter().collect::<String>());
        assert_eq!((leader("index.changed"), leader("index.rebuild"), leader("index.file")), (Some("Ir".into()), Some("IR".into()), Some("If".into())));
        assert!(bound.iter().any(|b| b.action.id == "index.file" && b.action.label.contains("src/a.rs")), "names the open file");
        // The light: indexing with progress.
        app.net.incoming.lock().unwrap().push(index_event(arch_graph::IndexStatus { state: arch_graph::IndexState::Indexing, done: 3, total: 10, ..Default::default() }));
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("INDEX") && text.contains("indexing 3 of 10"), "{text}");
        // Done, but two files changed since: one word for it everywhere.
        let stale = arch_graph::IndexStatus { stale: vec!["src/a.rs".into(), "src/b.rs".into()], last: Some(arch_graph::IndexSummary { millis: 40, files: 10, parsed: 2, components: 30, relations: 50, unresolved: 1, at: "now".into() }), ..Default::default() };
        app.net.incoming.lock().unwrap().push(index_event(stale));
        focus(&ctx, &mut app, BufferKind::Code);
        let text = texts(&frame(&ctx, &mut app, vec![]));
        let (_, word, _) = crate::status::look("stale");
        assert_eq!(word, "out of date");
        assert!(text.contains("2 out of date"), "the light: {text}");
        assert!(app.code.out_of_date && text.contains("This file changed since it was indexed"), "the code view says so");
        assert!(text.contains("fn run() {"), "and still shows the code (nothing waits)");
        // The spec's code and the graph's cards say it the same way.
        let mut anchored = crate::spec::tests::detail(&ComponentId::new("rust:a::step"), &ComponentId::new("rust:a::save"));
        anchored.component.anchor = Some(CodeAnchor { language: "rust".into(), file: RepoPath::new("src/b.rs").unwrap(), symbol: "a::step".into(), syntax_kind: "function_item".into(), span: Span { start_line: 1, end_line: 2, start_byte: 0, end_byte: 1 }, content_hash: String::new() });
        anchored.code = Some("fn step() {}".into());
        app.spec.detail = Some(anchored);
        frame(&ctx, &mut app, vec![]);
        assert!(app.spec.out_of_date);
        focus(&ctx, &mut app, BufferKind::Spec);
        app.spec.move_to("code#Header");
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("out of date") && text.contains("This code changed since it was indexed"), "{text}");
        focus(&ctx, &mut app, BufferKind::Graph);
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains("2 out of date") && text.contains("Re-read"), "the graph offers to re-read: {text}");
        // A failed run says so.
        app.net.incoming.lock().unwrap().push(index_event(arch_graph::IndexStatus { state: arch_graph::IndexState::Failed, error: Some("disk".into()), ..Default::default() }));
        assert!(texts(&frame(&ctx, &mut app, vec![])).contains("indexing failed"));
        // Every state names a real icon and colour (config is checked).
        assert!(UiConfig::with_overrides("[status.stale]\nword = \"old\"\nicon = \"nope\"\ncolor = \"warn\"\n").unwrap_err().contains("unknown icon"));
    }
}
