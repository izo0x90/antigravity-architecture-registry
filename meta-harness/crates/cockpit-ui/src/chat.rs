//! The chat/trajectory view. Renders only from the store (server events) and the
//! active instance's snapshot: pickers, commands and controls come from what the
//! harness reports and its capabilities, never from hardcoded harness knowledge.

use std::sync::{Arc, Mutex};

use egui::{Color32, Frame, Pos2, Rect, RichText, Stroke, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use harness_contracts::{
    ClientEvent, ConfigKind, McpServer, McpTransport, NativeId, OptionValue, ContentBlock, InstanceId, ItemBody, ItemStatus, ModelSelection, NoticeLevel, PlanBody,
    PlanStep, ProviderSnapshot, ProviderStatus, RequestStatus, RuntimeMode, RuntimePolicy, SavedSession, SessionKey, Slot, SlotId, SlotStatus,
    StepStatus, SteerInput, ThreadId, ThreadStatus, ToolKind, ToolServers, TurnItem, TurnStatus, LEAD,
};

use egui_phosphor::regular as icon;

use crate::actions::{Action, ActionProvider, Choice, MINOR, Param, RunCx, Style};
use crate::feedback::FeedbackInspectorState;
use crate::nav::{Fold, GoTo, Item, Motion, Navigable, Place, Spot, Step, Target};
use crate::net::{AuthChallenge, NetClient, NetResult, OpenSlotSession, SlotOpened};
use crate::request_card::{Drafts, Drawn, Shown, render_request};
use crate::store::{Store, ThreadView};
use crate::theme::colors;

pub(crate) mod actions;
mod nav;
mod render;
mod session;
pub(crate) mod slots;

use render::{block_summary, mcp_label};

/// Agent connection as the status LED and banner see it.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Idle,
    Connecting,
    Connected(String),
    Failed(String),
}

/// Results of async calls, drained once per frame (each says which slot
/// it was for: the view may show another by then).
enum Reply {
    Opened(SlotId, NetResult<SlotOpened>),
    Sent(SlotId, NetResult<()>),
    /// A sub-slot was started (shown in the view that asked).
    SlotMade(NetResult<SlotId>),
    /// A session was ended to start again with new tool servers.
    Restart { slot: SlotId, resume: Option<NativeId>, result: NetResult<()> },
    Saved(NetResult<Vec<SavedSession>>),
    AuthStarted(NetResult<AuthChallenge>),
    /// The tool servers every session gets; `saved`: after a change.
    Tools { result: NetResult<ToolServers>, saved: bool },
    Done(NetResult<()>),
}

type Mailbox = Arc<Mutex<Vec<Reply>>>;

fn post(mailbox: &Mailbox, reply: Reply) {
    if let Ok(mut m) = mailbox.lock() {
        m.push(reply);
    }
}

#[derive(Default)]
pub struct ChatStreamState {
    pub store: Store,
    /// Slots whose session is being opened.
    opening: std::collections::BTreeSet<SlotId>,
    /// What went wrong, by slot.
    errors: std::collections::BTreeMap<SlotId, String>,
    /// A prompt for a slot with no harness yet, sent once its first session opens.
    queued: std::collections::BTreeMap<SlotId, String>,
    /// Something to tell the user once (a resume that opened fresh,
    /// reports delivered to the lead).
    notice: Option<String>,
    /// This view's own state (each chat view has one; the one in use is here).
    pub ui: ChatView,
    /// The slots a chat view showed this frame (their updates are seen).
    pub drawn: std::collections::BTreeSet<SlotId>,
    saved_sessions: Option<Vec<SavedSession>>,
    /// Action to open in the palette once saved sessions arrive.
    after_saved: Option<String>,
    /// Asks the app to open the palette at this action (set by toolbar buttons
    /// and by actions that first need to load their choices).
    pending_palette: Option<String>,
    /// The tool servers every session gets (ours and the project's), once
    /// asked for.
    tools: Option<ToolServers>,
    tools_asked: bool,
    /// A sign-in under way: the page to open and what to paste back.
    login: Option<AuthChallenge>,
    auth_paste: String,
    /// Answers being given to the agent's questions.
    drafts: Drafts,
    mailbox: Mailbox,
}

/// A chat view's own state: what's typed, where the cursor is, what was
/// drawn. The chat's sessions, sign-in and tools are shared by all views.
pub struct ChatView {
    /// The slot it shows: the lead, unless another was picked.
    pub slot: SlotId,
    pub input_buffer: String,
    pub markdown_cache: CommonMarkCache,
    pub requested_file_jump: Option<(String, Option<usize>)>,
    /// The message the cursor is on (normal mode); None follows the latest.
    pub cursor: Option<usize>,
    /// Which of its links a hint picked.
    pub(crate) link_pick: Option<usize>,
    /// Scroll the cursor's message into view next frame.
    pub(crate) scroll: bool,
    /// Messages and links drawn last frame (and this one's, as drawn), and
    /// the stream's viewport.
    pub(crate) spots: Vec<Spot>,
    pub(crate) drawing: Vec<Spot>,
    pub(crate) view: Rect,
}

impl Default for ChatView {
    fn default() -> Self {
        Self {
            slot: SlotId::new(LEAD),
            input_buffer: String::new(),
            markdown_cache: CommonMarkCache::default(),
            requested_file_jump: None,
            cursor: None,
            link_pick: None,
            scroll: false,
            spots: Vec::new(),
            drawing: Vec::new(),
            view: Rect::NOTHING,
        }
    }
}

fn mode_label(mode: RuntimeMode) -> String {
    match mode {
        RuntimeMode::ApprovalRequired => format!("{} Ask first", icon::SHIELD_CHECK),
        RuntimeMode::AutoAcceptEdits => format!("{} Auto-edit", icon::PENCIL_SIMPLE),
        RuntimeMode::Auto => format!("{} Auto", icon::LIGHTNING),
        RuntimeMode::FullAccess => format!("{} Full access", icon::FIRE),
    }
}

fn tool_icon(kind: &ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => icon::BOOK_OPEN,
        ToolKind::Edit => icon::PENCIL_SIMPLE,
        ToolKind::Delete => icon::TRASH,
        ToolKind::Move => icon::ARROWS_LEFT_RIGHT,
        ToolKind::Search => icon::MAGNIFYING_GLASS,
        ToolKind::Execute => icon::PLAY,
        ToolKind::Think => icon::LIGHTBULB,
        ToolKind::Fetch => icon::GLOBE,
        ToolKind::Other => icon::WRENCH,
    }
}

fn status_tag(status: ItemStatus) -> (&'static str, Color32) {
    match status {
        ItemStatus::Pending => ("[..]", colors().muted),
        ItemStatus::InProgress => ("[RUN]", colors().warn),
        ItemStatus::Completed => ("[OK]", colors().ok),
        ItemStatus::Failed => ("[FAIL]", colors().error),
        ItemStatus::Cancelled => ("[--]", colors().muted),
    }
}

fn banner(ui: &mut egui::Ui, fill: Color32, stroke: Color32, add: impl FnOnce(&mut egui::Ui)) {
    Frame::NONE
        .fill(fill)
        .stroke(Stroke::new(crate::theme::look().line_thin, stroke))
        .corner_radius(crate::theme::look().radius_m)
        .inner_margin(crate::theme::margin2(crate::theme::look().space_m, crate::theme::look().space_sm))
        .show(ui, |ui| ui.horizontal_wrapped(add));
    ui.add_space(crate::theme::look().space_s);
}

impl ChatView {
    /// Where it is: its slot, and its message.
    pub fn place(&self) -> Place {
        Place::Chat { message: self.cursor, slot: Some(self.slot.clone()) }
    }
}

impl ChatStreamState {
    /// Shows the slot running `thread` (one of the queue's workers, say);
    /// false when no slot runs it.
    pub fn show_thread(&mut self, thread: &ThreadId) -> bool {
        let Some(slot) = self.store.slots.values().find(|s| s.current_thread() == Some(thread)) else { return false };
        self.ui.slot = slot.id.clone();
        self.ui.cursor = None;
        true
    }

    pub fn process_event(&mut self, event: ClientEvent) {
        if let ClientEvent::SlotClosed { slot_id } = &event
            && slot_id == &self.ui.slot
        {
            self.ui.slot = SlotId::new(LEAD);
            self.ui.cursor = None;
        }
        self.store.apply(event);
    }

    /// Answers `request` with `option`, what was picked and typed for its
    /// questions going with it; a failure shows in the chat of the slot
    /// that asked. (The keys, the card's buttons and the quick look all
    /// answer so.)
    pub fn answer(&mut self, net: &NetClient, ctx: &egui::Context, thread: &ThreadId, request: &harness_contracts::RequestId, option: &str) {
        let Some(pending) = self.store.threads.get(thread).and_then(|t| t.request(request)) else { return };
        let response = self.drafts.response(pending, option);
        let slot = self.store.slots.values().find(|s| s.current_thread() == Some(thread)).map_or_else(|| self.ui.slot.clone(), |s| s.id.clone());
        let mailbox = self.mailbox.clone();
        net.respond(thread, request, &response, ctx.clone(), move |r| post(&mailbox, Reply::Sent(slot, r)));
    }

    /// Draws `thread`'s request `request` (the inbox's) as `shown`,
    /// answering what's clicked as the keys do; none when it's no longer there.
    pub fn render_request_of(&mut self, ui: &mut egui::Ui, net: &NetClient, thread: &ThreadId, request: &harness_contracts::RequestId, shown: &Shown) -> Option<Drawn> {
        let asked = self.store.threads.get(thread).and_then(|t| t.request(request)).cloned()?;
        let drawn = render_request(ui, &asked, &mut self.drafts, shown);
        if let Some(option) = &drawn.clicked {
            self.answer(net, ui.ctx(), thread, request, option);
        }
        Some(drawn)
    }

    /// The slot this view shows.
    pub fn slot(&self) -> Option<&Slot> {
        self.store.slots.get(&self.ui.slot)
    }

    /// Its current session's thread, while it runs.
    pub fn active_thread(&self) -> Option<ThreadId> {
        self.thread_id_in(&self.ui.slot)
    }

    /// Its harness: the slot's, else (a lead with none chosen yet) the
    /// first there is.
    pub fn active_instance(&self) -> Option<InstanceId> {
        self.instance_id_in(&self.ui.slot)
    }

    fn thread_id_in(&self, slot: &SlotId) -> Option<ThreadId> {
        self.store.slots.get(slot)?.current_thread().cloned()
    }

    fn instance_id_in(&self, slot: &SlotId) -> Option<InstanceId> {
        self.store.slots.get(slot).and_then(|s| s.instance_id.clone()).or_else(|| self.store.instances.keys().next().cloned())
    }

    fn instance(&self) -> Option<&ProviderSnapshot> {
        self.store.instances.get(&self.active_instance()?)
    }

    fn thread(&self) -> Option<&ThreadView> {
        self.store.threads.get(&self.active_thread()?)
    }

    fn error(&self) -> Option<&String> {
        self.errors.get(&self.ui.slot)
    }

    fn set_error(&mut self, slot: SlotId, message: String) {
        self.errors.insert(slot, message);
    }

    fn opening(&self) -> bool {
        self.opening.contains(&self.ui.slot)
    }

    /// The lead's session (the AGENT light): whichever slot a view shows.
    pub fn session_status(&self) -> SessionStatus {
        let lead = SlotId::new(LEAD);
        if let Some(err) = self.errors.get(&lead) {
            return SessionStatus::Failed(err.clone());
        }
        let thread = self.thread_id_in(&lead);
        match thread.as_ref().and_then(|t| self.store.threads.get(t)).map(|t| t.thread.status) {
            Some(ThreadStatus::Starting) => SessionStatus::Connecting,
            Some(ThreadStatus::Error) => SessionStatus::Failed("the agent reported an error".into()),
            Some(ThreadStatus::Closed) => SessionStatus::Failed("the agent session closed".into()),
            Some(_) => SessionStatus::Connected(thread.map(|t| t.to_string()).unwrap_or_default()),
            None if self.opening.contains(&lead) => SessionStatus::Connecting,
            None => SessionStatus::Idle,
        }
    }

    /// Why the lead's harness (else the first) can't start, and how to fix
    /// it, as its instance says.
    pub fn harness_problem(&self) -> Option<String> {
        let instance = self.store.instances.get(&self.instance_id_in(&SlotId::new(LEAD))?)?;
        match &instance.status {
            ProviderStatus::Unavailable { reason } => Some(format!("{}: {reason}", instance.display_name)),
            ProviderStatus::Error { message } => Some(format!("{}: {message}", instance.display_name)),
            _ => None,
        }
    }

    /// How much the lead's session has done (the AGENT light blinks with it).
    pub fn agent_activity(&self) -> u64 {
        self.thread_id_in(&SlotId::new(LEAD)).and_then(|t| self.store.threads.get(&t)).map(|t| t.activity).unwrap_or(0)
    }

    /// How the slot's sessions run (its settings).
    fn policy(&self) -> RuntimePolicy {
        self.slot().map(|s| s.policy.clone()).unwrap_or_default()
    }

    fn model(&self) -> Option<String> {
        self.slot().and_then(|s| s.model.as_ref()).map(|m| m.model.clone())
    }

    fn model_selection(&self) -> Option<ModelSelection> {
        Some(ModelSelection { instance_id: self.active_instance()?, model: self.model()?, options: Vec::new() })
    }
}

#[cfg(test)]
pub(crate) mod tests;
