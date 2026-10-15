//! The chat/trajectory view. Renders only from the store (server events) and the
//! active instance's snapshot: pickers, commands and controls come from what the
//! harness reports and its capabilities, never from hardcoded harness knowledge.

use std::sync::{Arc, Mutex};

use egui::{Color32, Frame, Pos2, Rect, RichText, Stroke, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use harness_contracts::{
    ClientEvent, ConfigKind, McpServer, McpTransport, NativeId, OptionValue, ContentBlock, InstanceId, ItemBody, ItemStatus, ModelSelection, NoticeLevel, PlanBody,
    PlanStep, ProviderSnapshot, ProviderStatus, RequestStatus, RuntimeMode, RuntimePolicy, SavedSession,
    StepStatus, SteerInput, ThreadId, ThreadStatus, ToolKind, ToolServers, TurnInput, TurnItem, TurnStatus,
};

use egui_phosphor::regular as icon;

use crate::actions::{Action, ActionProvider, Choice, MINOR, Param, RunCx, ToolbarStyle, ToolbarUse, toolbar_control};
use crate::feedback::FeedbackInspectorState;
use crate::nav::{Fold, GoTo, Item, Motion, Navigable, Place, Spot, Step, Target};
use crate::net::{AuthChallenge, NetClient, NetResult, OpenThreadBody};
use crate::request_card::{Drafts, render_request};
use crate::store::{Store, ThreadView};
use crate::theme::colors;

mod actions;
mod nav;
mod render;
mod session;

use render::{block_summary, mcp_label};

/// Agent connection as the status LED and banner see it.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Idle,
    Connecting,
    Connected(String),
    Failed(String),
}

/// Results of async calls, drained once per frame.
enum Reply {
    Opened(NetResult<ThreadId>),
    Sent(NetResult<()>),
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

pub struct ChatStreamState {
    pub store: Store,
    pub active_instance: Option<InstanceId>,
    pub active_thread: Option<ThreadId>,
    opening: bool,
    error: Option<String>,
    queued_prompt: Option<String>,
    pub input_buffer: String,
    pub markdown_cache: CommonMarkCache,
    pub requested_file_jump: Option<(String, Option<usize>)>,
    runtime_mode: RuntimeMode,
    model: Option<String>,
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
    /// The message the cursor is on (normal mode); None follows the latest.
    pub cursor: Option<usize>,
    /// Which of its links a hint picked.
    link_pick: Option<usize>,
    /// Scroll the cursor's message into view next frame.
    scroll: bool,
    /// Messages and links drawn last frame (and this one's, as drawn), and
    /// the stream's viewport.
    spots: Vec<Spot>,
    drawing: Vec<Spot>,
    view: Rect,
}

impl Default for ChatStreamState {
    fn default() -> Self {
        Self {
            store: Store::default(),
            active_instance: None,
            active_thread: None,
            opening: false,
            error: None,
            queued_prompt: None,
            input_buffer: String::new(),
            markdown_cache: CommonMarkCache::default(),
            requested_file_jump: None,
            runtime_mode: RuntimeMode::ApprovalRequired,
            model: None,
            saved_sessions: None,
            after_saved: None,
            pending_palette: None,
            tools: None,
            tools_asked: false,
            login: None,
            auth_paste: String::new(),
            drafts: Drafts::default(),
            mailbox: Mailbox::default(),
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

impl ChatStreamState {
    pub fn process_event(&mut self, event: ClientEvent) {
        if let ClientEvent::InstanceUpdated { snapshot } = &event
            && self.active_instance.is_none()
        {
            self.active_instance = Some(snapshot.instance_id.clone());
        }
        if let ClientEvent::ThreadClosed { thread_id } = &event
            && self.active_thread.as_ref() == Some(thread_id)
        {
            self.active_thread = None;
        }
        // After a reload the server replays live sessions; adopt one if none is active.
        if let ClientEvent::Session { event: harness_contracts::AdapterEvent::ThreadUpdated { thread } } = &event
            && self.active_thread.is_none()
            && !self.opening
            && thread.status != ThreadStatus::Closed
        {
            self.active_thread = Some(thread.id.clone());
            self.active_instance = Some(thread.instance_id.clone());
        }
        self.store.apply(event);
    }

    fn instance(&self) -> Option<&ProviderSnapshot> {
        self.active_instance.as_ref().and_then(|id| self.store.instances.get(id))
    }

    fn thread(&self) -> Option<&ThreadView> {
        self.active_thread.as_ref().and_then(|id| self.store.threads.get(id))
    }

    pub fn session_status(&self) -> SessionStatus {
        if let Some(err) = &self.error {
            return SessionStatus::Failed(err.clone());
        }
        match self.thread().map(|t| t.thread.status) {
            Some(ThreadStatus::Starting) => SessionStatus::Connecting,
            Some(ThreadStatus::Error) => SessionStatus::Failed("the agent reported an error".into()),
            Some(ThreadStatus::Closed) => SessionStatus::Failed("the agent session closed".into()),
            Some(_) => SessionStatus::Connected(self.active_thread.as_ref().map(ToString::to_string).unwrap_or_default()),
            None if self.opening => SessionStatus::Connecting,
            None => SessionStatus::Idle,
        }
    }

    /// Why the chosen harness (else the only one) can't start, and how to
    /// fix it, as its instance says.
    pub fn harness_problem(&self) -> Option<String> {
        let instance = match &self.active_instance {
            Some(id) => self.store.instances.get(id),
            None if self.store.instances.len() == 1 => self.store.instances.values().next(),
            None => None,
        }?;
        match &instance.status {
            ProviderStatus::Unavailable { reason } => Some(format!("{}: {reason}", instance.display_name)),
            ProviderStatus::Error { message } => Some(format!("{}: {message}", instance.display_name)),
            _ => None,
        }
    }

    pub fn agent_activity(&self) -> u64 {
        self.thread().map(|t| t.activity).unwrap_or(0)
    }

    fn policy(&self) -> RuntimePolicy {
        RuntimePolicy { runtime_mode: self.runtime_mode, ..RuntimePolicy::default() }
    }

    fn model_selection(&self) -> Option<ModelSelection> {
        Some(ModelSelection { instance_id: self.active_instance.clone()?, model: self.model.clone()?, options: Vec::new() })
    }

}

#[cfg(test)]
pub(crate) mod tests;
