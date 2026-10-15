use egui::{Color32, Frame, RichText, Stroke};

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use arch_graph::ComponentId;

use crate::actions::{Action, ActionProvider, Bound, Choice, Group, Invocation, LeaderEntry, MINOR, Origin, PRIMARY, Param, RunCx, SECONDARY, Scope, Situation, When, is_primary, leader_entries};
use crate::chat::{ChatStreamState, SessionStatus};
use crate::code::CodeViewerState;
use crate::columns::{Columns, Pane};
use crate::panes::{Parked, swap_view};
use crate::quick_look::QuickLook;
use crate::saved::SavedUi;
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

mod capture;
mod commands;
mod draw;
mod going;
mod keeping;
mod keys;
mod layout;
mod lifecycle;
mod look;
mod outside;
mod views;

pub use draw::top_bar::{ActivityLed, index_state};
use keys::release_keyboard;
#[cfg(test)]
use {capture::hint_chip, draw::bars::keys_of, draw::top_bar::server_line, keys::text_input_focused};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

    /// There is one of these (a chat, a queue); of the others, each column
    /// can have its own.
    pub fn one_only(&self) -> bool {
        matches!(self, Self::Chat | Self::Queue)
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
    CloseColumn,
    /// The focused column's view, gone for good.
    DeleteView,
    /// A new view of this kind in the focused column.
    NewView(BufferKind),
    /// An open view (its value: `kind:number`) in the focused column.
    PickView,
    ToggleInspector,
    ToggleIssues,
    /// The quick look on what the cursor is on (or closed, when open).
    QuickLook,
    /// The quick look's card in a column instead.
    QuickLookToColumn,
    /// The box for talking to the agent, over whatever is on screen.
    Talk,
    /// Lets agents drive the cockpit (its value: on or off).
    Agents,
    OpenPalette,
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

/// Largest count a key takes (`999j`).
const MAX_COUNT: u32 = 999;

/// Which actions keys were given to: each one's view, name and aliases, in
/// order, and the focused view (its actions get the view menu's letters).
type KeyedActions = (BufferKind, Vec<(Origin, String, Vec<String>)>);
/// One action's keys: its leader path and direct sequences.
type Keys = (Option<Vec<char>>, Vec<Vec<Chord>>);

pub struct CockpitApp {
    /// The keys as last given out, and the actions they were given to (by
    /// view, name and aliases, in order): keys are given out again only when
    /// those change. Labels and commands are always the current ones.
    bound_cache: std::cell::RefCell<Option<(KeyedActions, Vec<Keys>)>>,
    /// Within a frame, the actions as gathered once (dropped when one runs;
    /// none between frames, so the next one gathers them afresh).
    frame_actions: std::cell::RefCell<Option<Vec<Bound>>>,
    /// Drawing a frame now.
    in_frame: std::cell::Cell<bool>,
    /// How many times the actions were gathered (counted for the tests).
    #[cfg(test)]
    gathered: std::cell::Cell<usize>,
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

    /// The columns: the view each shows, and which has focus.
    pub columns: Columns,
    /// The views not in use now (each column's own; see `panes`).
    parked: Parked,
    /// For each kind with a view per column: the view in use now, and the
    /// one used last (what keys, links and the agent act on).
    active: BTreeMap<BufferKind, u32>,
    last_used: BTreeMap<BufferKind, u32>,
    /// New views, to start where the view of their kind used before was.
    fresh: Vec<(Pane, Place)>,
    /// The column a go-to lands in (a count before its keys).
    landing: Option<usize>,
    /// A column whose × was clicked (closed after drawing).
    closing: Option<usize>,
    /// The component types, for each spec view.
    types: Option<arch_graph::TypeSystem>,
    /// The quick look, while it's open.
    pub quick_look: Option<QuickLook>,
    /// The box for talking to the agent, while it's open.
    pub talk: Option<crate::talk::Talk>,
    /// Loads and saves the cockpit's state on the server.
    keeper: crate::saved::Keeper,
    /// Calls from outside: scripts in the page, and agents while allowed.
    pub remote: crate::remote::Remote,
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
    slot_rects: Vec<egui::Rect>,
    /// The spec's anchor we last refetched the view for (so a mismatch the
    /// server itself holds doesn't refetch every frame).
    anchor_checked: Option<Option<arch_graph::CodeAnchor>>,
    palette: crate::palette::Palette,

    // Top-bar status LEDs
    pub srv_led: ActivityLed,
    pub agent_led: ActivityLed,
    pub index_led: ActivityLed,
}


impl CockpitApp {
    pub fn new(host_origin: &str) -> Self {
        let (config, config_error) = match UiConfig::with_overrides("") {
            Ok(config) => (config, None),
            Err(err) => (UiConfig::default(), Some(format!("built-in defaults: {err}"))),
        };
        Self {
            bound_cache: Default::default(),
            frame_actions: Default::default(),
            in_frame: Default::default(),
            #[cfg(test)]
            gathered: Default::default(),
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

            columns: Columns::default(),
            parked: Parked::default(),
            active: BTreeMap::new(),
            last_used: BTreeMap::new(),
            fresh: Vec::new(),
            landing: None,
            closing: None,
            types: None,
            quick_look: None,
            talk: None,
            keeper: Default::default(),
            remote: crate::remote::Remote::new(host_origin),
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
            slot_rects: Vec::new(),
            anchor_checked: None,
            palette: crate::palette::Palette::default(),

            srv_led: ActivityLed::default(),
            agent_led: ActivityLed::default(),
            index_led: ActivityLed::default(),
        }
    }















































































































}






impl eframe::App for CockpitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.remote.feed(ctx, raw_input);
    }
}

impl CockpitApp {
    /// One UI frame.
    pub fn frame(&mut self, ctx: &egui::Context) {
        self.frame_actions.borrow_mut().take();
        self.serve_calls(ctx);
        self.in_frame.set(true);
        self.draw_frame(ctx);
        self.in_frame.set(false);
        self.frame_actions.borrow_mut().take();
        let status = self.remote.waiting().then(|| self.remote_status());
        self.remote.end_frame(ctx, status);
        self.remote.flush();
    }





    fn draw_frame(&mut self, ctx: &egui::Context) {
        self.update_lifecycle(ctx);
        self.feedback.begin_frame();

        egui::TopBottomPanel::top("cockpit_top_panel").show(ctx, |ui| {
            self.render_top_bar(ui);
        });
        self.render_key_bar(ctx);

        let shell = self.shell_actions();
        let tab_keys = self.tab_keys(&shell);
        egui::CentralPanel::default().show(ctx, |ui| {
            self.slot_rects.resize(self.columns.len(), egui::Rect::NOTHING);
            ui.columns(self.columns.len(), |cols| {
                for (i, col) in cols.iter_mut().enumerate() {
                    // Each column draws its own view.
                    self.swap_in(self.columns.pane(i));
                    self.render_slot(i, col, &shell, &tab_keys);
                }
            });
            self.back_to_used();
            // A column's × closes it once every column is drawn.
            if let Some(slot) = self.closing.take() {
                self.close_column(slot);
            }
        });

        // Real-time Inspector targeting overlay
        self.feedback.render_inspection_overlay(ctx);

        // Developer Feedback modal (automatically pops up when an element is targeted)
        let active_slots: Vec<String> = self.columns.panes().iter().map(|p| p.kind.title().to_lowercase()).collect();
        self.feedback.render_modal(ctx, &self.net, self.columns.len(), &active_slots);

        // Issues & Feedback Explorer window
        self.feedback.render_issues_window(ctx, &self.net);

        // Feedback confirmation toast
        self.feedback.render_toast(ctx);

        self.render_quick_look(ctx, &shell);
        self.render_talk(ctx);
        self.render_hints(ctx);
        self.render_prefix_popup(ctx);
        self.render_which_key_hud(ctx);
        self.render_key_sheet(ctx);
        self.render_palette(ctx);
    }
}

#[cfg(test)]
mod tests;
