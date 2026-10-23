use egui::{Color32, Frame, RichText, Stroke};

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use arch_graph::ComponentId;

use crate::actions::{Action, Bound, Choice, Group, Invocation, LeaderEntry, MINOR, Origin, PRIMARY, Param, RunCx, SECONDARY, Scope, Situation, When, is_primary, leader_entries};
use crate::actions::Layer;
use crate::agents::SessionStatus;
use crate::buffer::Buffer;
use crate::panels::{Panels, BufferId};
use crate::quick_look::QuickLook;
use crate::saved::SavedUi;
use crate::config::UiConfig;
use crate::feedback::FeedbackInspectorState;
use crate::goto::{Candidate, Choosing, Details, Pending, Wait, candidates};
use crate::keymap::{Chord, InputMode, SeqMatch, TapTracker, continuations, match_sequence};
use crate::nav::{GoTo, Jump, JumpList, Motion, Move, Place, SearchEvent, Spot, Step, Target, hint_labels, rank_matches, step_match};
use crate::net::{NetClient, NetResult, WsState};
use crate::theme::colors;

pub(crate) mod attention;
mod asks;
mod capture;
mod commands;
mod desk;
mod desks;
mod draw;
mod going;
mod keeping;
mod keys;
mod layout;
mod linking;
mod lifecycle;
mod look;
pub(crate) mod notes;
mod outside;
mod plan_items;
mod starting;
mod tagging;
mod views;

pub use desk::Desk;

pub use draw::top_bar::{ActivityLed, index_state};
use keys::release_keyboard;
#[cfg(test)]
use {capture::hint_chip, draw::top_bar::server_line, keys::text_input_focused};

pub use crate::kind::BufferKind;

/// What an edit of a tag changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TagEditing {
    Rename,
    Recolour,
    Note,
    ClearNote,
    Delete,
}

/// What the shell itself can do: panels, layout, overlays, and the keys every
/// view shares (moves, hints, search, folds, marks, the jump list, go-tos).
#[derive(Clone, Debug, PartialEq, Eq)]
enum ShellCommand {
    FocusPanel(usize),
    CyclePanel(i32),
    ShowBuffer(BufferKind),
    SetLayout(usize),
    ClosePanel,
    /// The focused panel's view, gone for good.
    DeleteView,
    /// A new view of this kind in the focused panel.
    NewView(BufferKind),
    /// An open view (its value: `kind:number`) in the focused panel.
    PickView,
    ToggleInspector,
    ToggleIssues,
    /// The buttons of what you're on in a bar (true) or floating.
    ItemButtons,
    /// The quick look on what the cursor is on (or closed, when open).
    QuickLook,
    /// The quick look's card in a panel instead.
    QuickLookToPanel,
    /// Talking to the agent: the lead's chat in the quick look, typing.
    Talk,
    /// The desk's pop-up terminal in the quick look (shown: hidden).
    PopTerminal,
    /// Lets agents drive the cockpit (its value: on or off).
    Agents,
    OpenPalette,
    /// Row or line N (its value).
    Line,
    Hint { open: bool },
    Search,
    SearchStep { forward: bool },
    MarkSet,
    MarkJump,
    /// Tags what the cursor's on (its value: a tag's id, or a new one's name).
    Tag,
    /// Takes a tag off what the cursor's on: the one it names (and what
    /// it's on there), or the one picked (its value: its id).
    Untag(Option<(harness_contracts::TagId, harness_contracts::Subject)>),
    /// Goes to what a tag (its value: its id) is on: on one thing, there;
    /// on several, the palette asks which (`TagGoToPick`).
    TagGoTo,
    /// Goes to one of the things the tag picked is on (its value: the
    /// thing, as JSON).
    TagGoToPick,
    /// The next (or previous) thing the tag on what the cursor's on is on.
    TagStep { forward: bool },
    /// Tagging: the next of the things at the cursor (the line, its command).
    TagLevel,
    /// A tag's name (its value: its id) put where you're typing.
    TagInsert,
    LinkThing,
    /// An edit of a tag (the one it was asked for): its value.
    TagEdit(TagEditing, harness_contracts::TagId),
    /// Which of the tags at the cursor an edit is of (its value: its id).
    TagEditWhich(TagEditing),
    /// A plan's item's state (its value: open, decided, done, dropped).
    ItemState(arch_graph::PlanId, arch_graph::ItemId),
    /// A discussion started on the thing at the cursor: what it is (its
    /// value: question, change_request), then what it says.
    Discuss,
    /// Discussing: the next of the things at the cursor.
    DiscussLevel,
    /// Who the discussion picked asks (its value: an agent slot's name).
    DiscussAsk,
    /// What the discussion picked asks (its value).
    DiscussWrite,
    /// Which of the discussions on a mark opens (its value: its id).
    DiscussOpen,
    Back,
    Forward,
    Keys,
    GoTo(GoTo),
    /// One of several places a go-to can land (its value).
    GotoPick,
    Index(IndexOp),
    /// The oldest item waiting on the user, in the quick look.
    AttentionNextWaiting,
    /// An item (`server/id`, the value) in the quick look, or the system
    /// notifications switch (its row in the bell's list).
    AttentionPick,
    /// System alerts (the browser's, the OS's) on or off.
    Alerts,
    /// Shows a server's desk (its name, the value).
    ServerSwitch,
    /// Links another server (`name url secret`, the value).
    ServerAdd,
    ServerConnect,
    ServerDisconnect,
    ServerRemove,
    /// Asks which server to shut down (the value), then to confirm.
    ServerShutdown,
    ServerShutdownConfirm,
}

/// Reading the code again, on the user's word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexOp {
    /// What changed since it was read.
    Changed,
    /// Everything, from scratch (the cache dropped).
    Rebuild,
}

/// Hint labels on what the focused view draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Hinting {
    pub panel: usize,
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

/// Where a list of actions is gathered for: a view, and what holds it (a
/// panel, or the quick look).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Here {
    pub at: crate::keymap::Host,
    pub kind: BufferKind,
}
/// One action's keys: its leader path and direct sequences.
type Keys = (Option<Vec<char>>, Vec<Vec<Chord>>, Vec<String>);

pub struct CockpitApp {
    /// The project shown: its views, layout and server.
    pub desk: Desk,
    /// The other servers' desks, by name.
    pub away: BTreeMap<String, Desk>,
    /// Home (the server serving the cockpit): its socket brings every
    /// server's events; the shell's own calls go to it.
    pub home: NetClient,
    host_origin: String,
    /// The servers home links to, as it last said.
    pub servers: Vec<harness_contracts::ServerView>,
    /// The page's way in (`window.nah`): it drives whatever desk is shown.
    pub page: crate::remote::Door,
    /// A server the user asked to stop, until they confirm.
    confirming_shutdown: Option<String>,
    /// The shell's own state, kept on home.
    shell: desks::ShellKeeper,
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
    /// Every action run, by id (what keys and controls ran).
    #[cfg(test)]
    ran: std::cell::RefCell<Vec<String>>,
    pub feedback: FeedbackInspectorState,

    /// The panel a go-to lands in (a count before its keys).
    landing: Option<usize>,
    /// An action a control asked for this frame (its id and value): run
    /// once everything is drawn, the way its key runs it.
    asked: Option<(String, Option<String>)>,
    /// Fingers, turned into gestures (for every view).
    touch: crate::touch::Gestures,
    /// The pointer is a finger (one touched last; a mouse moving makes it
    /// a mouse's again): views move what a finger drags on rather than
    /// select it.
    fingers: bool,
    /// The quick look, while it's open.
    pub quick_look: Option<QuickLook>,
    /// The browser's notifications.
    pub alerts: crate::alerts::Alerts,
    /// What a call that finished later has to say (shown as the note).
    notes_later: Arc<Mutex<Vec<String>>>,
    pub initialized: bool,

    pub config: UiConfig,
    /// Why the user's UI config was not applied, if it wasn't.
    config_error: Option<String>,
    config_reply: Arc<Mutex<Option<NetResult<Option<String>>>>>,
    /// The config changed and its theme still needs applying.
    theme_dirty: bool,
    pub input_mode: InputMode,
    /// The mode the palette was opened from: closed without a pick, it
    /// goes back to typing when that's where it came from.
    palette_from: InputMode,
    leader_tap: TapTracker,
    /// Typing in a field that takes every key: a leave sequence's first key, held.
    leaving: crate::keymap::Leaving,
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
    /// A short note in the key bar (why something didn't happen), and since when.
    pub note: Option<String>,
    note_since: Option<f64>,
    /// The key sheet (`?`) is open, with this filter.
    pub key_sheet: Option<String>,
    /// Where each panel was drawn last frame (hint labels stay inside theirs).
    panel_rects: Vec<egui::Rect>,
    /// Where the buttons of what you're on show, once changed here (else
    /// the UI config says), and this frame's row kept for them by each host
    /// (the quick look, or a panel by its number) while they're in a bar.
    pub(crate) item_buttons: Option<crate::hosted::ItemButtons>,
    item_bars: Vec<(crate::keymap::Host, egui::Rect)>,
    /// The marks in the gutter of notes as drawn (each host's), with what
    /// they say.
    note_marks: Vec<(crate::keymap::Host, egui::Rect, String)>,
    /// What's noted, as the gutters draw it.
    noted: notes::Noted,
    palette: crate::palette::Palette,

    // Top-bar status LEDs
    pub srv_led: ActivityLed,
    pub agent_led: ActivityLed,
    pub index_led: ActivityLed,
}


impl CockpitApp {
    /// The cockpit over home at `host_origin`; how the platform shows
    /// alerts is handed in by its start-up.
    pub fn new(host_origin: &str, alerts: std::sync::Arc<dyn crate::alerts::Backend>) -> Self {
        let (config, config_error) = match UiConfig::with_overrides("") {
            Ok(config) => (config, None),
            Err(err) => (UiConfig::default(), Some(format!("built-in defaults: {err}"))),
        };
        let alerts = crate::alerts::Alerts::with(alerts);
        let mut home = NetClient::new(host_origin);
        home.on_arrival(alerts.arrival());
        Self {
            desk: Desk::new(host_origin, harness_contracts::home::HERE),
            away: BTreeMap::new(),
            home,
            host_origin: host_origin.to_owned(),
            servers: Vec::new(),
            page: crate::remote::Door::default(),
            confirming_shutdown: None,
            shell: Default::default(),
            bound_cache: Default::default(),
            frame_actions: Default::default(),
            in_frame: Default::default(),
            #[cfg(test)]
            gathered: Default::default(),
            #[cfg(test)]
            ran: Default::default(),
            feedback: FeedbackInspectorState::default(),

            landing: None,
            asked: None,
            touch: Default::default(),
            fingers: false,
            quick_look: None,
            alerts,
            notes_later: Arc::default(),
            initialized: false,

            config,
            config_error,
            config_reply: Arc::default(),
            theme_dirty: true,
            input_mode: InputMode::Normal,
            palette_from: InputMode::Normal,
            leader_tap: TapTracker::default(),
            leaving: Default::default(),
            leader_path: Vec::new(),
            pending_keys: Vec::new(),
            pending_since: 0.0,
            pending_count: None,
            capture: None,
            note: None,
            note_since: None,
            key_sheet: None,
            panel_rects: Vec::new(),
            item_buttons: None,
            item_bars: Vec::new(),
            note_marks: Vec::new(),
            noted: notes::Noted::default(),
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
        self.desk.remote.feed(ctx, raw_input);
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
        let status = self.desk.remote.waiting().then(|| self.remote_status());
        self.desk.remote.end_frame(ctx, status);
        self.desk.remote.flush();
    }





    fn draw_frame(&mut self, ctx: &egui::Context) {
        self.update_lifecycle(ctx);
        self.feedback.begin_frame();
        self.item_bars.clear();

        egui::TopBottomPanel::top("cockpit_top_panel").show(ctx, |ui| {
            self.render_top_bar(ui);
        });
        self.render_key_bar(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            self.panel_rects.resize(self.desk.panels.len(), egui::Rect::NOTHING);
            ui.columns(self.desk.panels.len(), |cols| {
                for (i, col) in cols.iter_mut().enumerate() {
                    // Each panel draws its own view.
                    self.draw_as(self.desk.panels.buffer(i));
                    self.render_panel(i, col);
                }
            });
            self.desk.drawn();
        });
        self.say_seen(ctx);

        // Real-time Inspector targeting overlay
        self.feedback.render_inspection_overlay(ctx);

        // Developer Feedback modal (automatically pops up when an element is targeted)
        let active_slots: Vec<String> = self.desk.panels.buffers().iter().map(|p| p.kind.title().to_lowercase()).collect();
        self.feedback.render_modal(ctx, &self.desk.net, self.desk.panels.len(), &active_slots);

        // Issues & Feedback Explorer window
        self.feedback.render_issues_window(ctx, &self.desk.net);

        // Feedback confirmation toast
        self.feedback.render_toast(ctx);

        self.render_quick_look(ctx);
        self.render_item_controls(ctx);
        self.touch_tick(ctx);
        self.run_asked(ctx);
        // What the buffers asked while drawn (a click, a chat on screen):
        // done in the frame they asked.
        self.take_asks(ctx);
        self.render_hints(ctx);
        self.render_prefix_popup(ctx);
        self.render_which_key_hud(ctx);
        self.render_key_sheet(ctx);
        self.render_palette(ctx);
        self.desk.frame_done();
    }
}

#[cfg(test)]
mod tests;
