//! The chat/trajectory view: an agent slot's session, over the desk's
//! agents (`agents.rs`). Renders only from the store (server events) and
//! the active instance's snapshot: pickers, commands and controls come from
//! what the harness reports and its capabilities, never from hardcoded
//! harness knowledge. What it does with the agents it asks for
//! (`Ask::Agents`).

use std::cell::Ref;

use egui::{Color32, Frame, Pos2, Rect, RichText, Stroke, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use harness_contracts::{
    ConfigKind, McpServer, McpTransport, NativeId, OptionValue, ContentBlock, InstanceId, ItemBody, ItemStatus, ModelSelection, NoticeLevel, PlanBody,
    PlanStep, ProviderSnapshot, ProviderStatus, RequestStatus, RuntimeMode, RuntimePolicy, SessionKey, Slot, SlotId, SlotStatus,
    StepStatus, SteerInput, ThreadId, ToolKind, TurnItem, TurnStatus, LEAD,
};

use egui_phosphor::regular as icon;

use crate::actions::{Action, ActionProvider, Choice, MINOR, Param, RunCx, Style};
use crate::agents::{Act, Agents};
use crate::feedback::FeedbackInspectorState;
use crate::nav::{Ask, Fold, GoTo, Item, Motion, Move, Navigable, Place, Spot, Step, Target};
use crate::request_card::{Drafts, render_request};
use crate::store::ThreadView;
use crate::theme::colors;

pub(crate) mod actions;
mod nav;
mod render;
mod session;
pub(crate) mod things;
pub(crate) mod slots;

use crate::agents::block_summary;
use render::mcp_label;

#[derive(Default)]
pub struct ChatStreamState {
    /// The desk's agents, read.
    agents: crate::shared::Reader<Agents>,
    /// This view's own state (each chat view has one; the one in use is here).
    pub ui: ChatView,
    /// What it asked of the rest of the cockpit, not taken yet.
    asks: Vec<Ask>,
    /// When it last said each slot's updates were seen.
    seen: crate::attention::Seen,
    /// The sub-slots made that it saw (one it asked for, it shows).
    made_seen: u64,
    /// Action to open in the palette once saved sessions arrive.
    after_saved: Option<String>,
    /// Asks the app to open the palette at this action (set by toolbar buttons
    /// and by actions that first need to load their choices).
    pending_palette: Option<String>,
    /// Answers being given to the agent's questions.
    drafts: Drafts,
    /// How many files each turn changed (from the desk's timeline), and
    /// which read of it that is.
    pub turn_files: std::collections::BTreeMap<String, u32>,
    pub turn_files_read: u64,
    /// Where each item of the stream is as a tag names it, worked out once
    /// per change of the thread (its id, its activity count).
    tag_places: std::cell::RefCell<Option<things::Places>>,
    /// The chats' unsent text as the server keeps it, read; the slot this
    /// view took its draft for; the text it last reported.
    unsent: crate::shared::Reader<crate::drafts::Drafts>,
    unsent_for: Option<SlotId>,
    unsent_said: harness_contracts::Message,
}

/// A chat view's own state: what's typed, where the cursor is, what was
/// drawn. The chat's sessions, sign-in and tools are shared by all views.
pub struct ChatView {
    /// The slot it shows: the lead, unless another was picked.
    pub slot: SlotId,
    /// What's typed to the agent.
    pub input: crate::conversation::TypingBox,
    pub markdown_cache: CommonMarkCache,
    /// The message the cursor is on (normal mode); None follows the latest.
    pub cursor: Option<usize>,
    /// Which of its links a hint picked.
    pub(crate) link_pick: Option<usize>,
    /// Where it's looking (the shared rules move it, a message a stop);
    /// following the latest, the newest stays in view.
    pub(crate) scrolling: crate::viewport::Measured,
    /// Messages and links drawn last frame (and this one's, as drawn).
    pub(crate) spots: Vec<Spot>,
    pub(crate) drawing: Vec<Spot>,
    /// Where its typing box is, while it's typed in.
    pub(crate) typing: Option<Rect>,
}

impl Default for ChatView {
    fn default() -> Self {
        Self {
            slot: SlotId::new(LEAD),
            input: Default::default(),
            markdown_cache: CommonMarkCache::default(),
            cursor: None,
            link_pick: None,
            scrolling: Default::default(),
            spots: Vec::new(),
            drawing: Vec::new(),
            typing: None,
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

/// A sign-in under way, as it stands now: what the server said, the
/// seconds left (if it says), and the last paste.
struct SignInNow {
    waiting: harness_contracts::SignInWaiting,
    left_s: Option<u64>,
    ran_out: bool,
    pasted: Option<crate::agents::Pasted>,
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

/// Where a chat is: its slot, and the message.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct ChatPlace {
    pub message: Option<usize>,
    #[serde(default)]
    pub slot: Option<SlotId>,
}

impl ChatView {
    /// Where it is: its slot, and its message.
    pub fn place(&self) -> Place {
        Place::of(&ChatPlace { message: self.cursor, slot: Some(self.slot.clone()) })
    }
}

/// How often a chat on screen says its slot's updates were seen (seconds).
const SEEN_EVERY: f64 = 2.0;

impl ChatStreamState {
    /// The desk's agents.
    fn agents(&self) -> Ref<'_, Agents> {
        self.agents.get()
    }

    /// Asks the agents to act for the slot it shows.
    fn act(&mut self, act: Act) {
        self.asks.push(Ask::Agents { slot: self.ui.slot.clone(), act });
    }

    /// Answers `request` with `option`, what was picked and typed for its
    /// questions going with it. (The keys, the card's buttons and the quick
    /// look all answer so.)
    fn answer(&mut self, thread: &ThreadId, request: &harness_contracts::RequestId, option: &str) -> Result<(), String> {
        let Some(pending) = self.agents().store().threads.get(thread).and_then(|t| t.request(request)).cloned() else { return Ok(()) };
        let says_yes = pending.options.iter().find(|o| o.id == option).is_some_and(|o| matches!(o.decision, harness_contracts::ApprovalDecision::Accept | harness_contracts::ApprovalDecision::AcceptForSession | harness_contracts::ApprovalDecision::AcceptAlways));
        if says_yes && !self.drafts.complete(&pending) {
            return Err("Answer the questions it needs first.".into());
        }
        let response = self.drafts.response(&pending, option);
        self.act(Act::Answer { thread: thread.clone(), request: request.clone(), response });
        Ok(())
    }

    /// The stream item (its index) of the request waiting on you here.
    pub(crate) fn pending_item(&self) -> Option<usize> {
        let pending = self.thread()?.requests.iter().find(|r| r.status == RequestStatus::Pending)?.id.clone();
        self.stream().iter().position(|i| matches!(&i.body, ItemBody::Request { request_id } if *request_id == pending))
    }

    /// The slot this view shows.
    pub fn slot(&self) -> Option<Ref<'_, Slot>> {
        Ref::filter_map(self.agents(), |a| a.slot(&self.ui.slot)).ok()
    }

    /// Its current session's thread, while it runs.
    pub fn active_thread(&self) -> Option<ThreadId> {
        self.agents().thread_id_in(&self.ui.slot)
    }

    /// Its harness: the slot's, else (a lead with none chosen yet) the
    /// first there is.
    pub fn active_instance(&self) -> Option<InstanceId> {
        self.agents().instance_id_in(&self.ui.slot)
    }

    fn instance(&self) -> Option<Ref<'_, ProviderSnapshot>> {
        Ref::filter_map(self.agents(), |a| a.instance_in(&self.ui.slot)).ok()
    }

    /// Its harness's sign-in under way, as it stands now.
    fn sign_in_now(&self) -> Option<SignInNow> {
        let instance = self.active_instance()?;
        let agents = self.agents();
        let waiting = agents.waiting(&instance)?.clone();
        let left_s = waiting.until_ms.map(|until| until.saturating_sub(crate::attention::now_ms()).div_ceil(1000));
        let pasted = agents.pasted(&instance, waiting.id).cloned();
        Some(SignInNow { ran_out: left_s == Some(0), left_s, pasted, waiting })
    }

    /// The sign-in under way, when what's typed goes to it: it shows what to
    /// paste back, its time isn't up and a paste isn't being checked. Then
    /// the typing box is the sign-in's.
    fn sign_in_takes(&self) -> Option<SignInNow> {
        self.sign_in_now().filter(|n| n.waiting.expects.is_some() && n.waiting.url.is_some() && !n.ran_out && !n.pasted.as_ref().is_some_and(|p| p.finishing))
    }

    fn thread(&self) -> Option<Ref<'_, ThreadView>> {
        Ref::filter_map(self.agents(), |a| a.thread_in(&self.ui.slot)).ok()
    }

    fn error(&self) -> Option<String> {
        self.agents().error(&self.ui.slot).cloned()
    }

    /// The slot `by` places from the one it shows, in the chat's order.
    fn beside(&self, by: i32) -> Option<SlotId> {
        slots::beside(self.agents().store(), &self.ui.slot, by)
    }

    fn opening(&self) -> bool {
        self.agents().opening(&self.ui.slot)
    }

    /// How the slot's sessions run (its settings).
    fn policy(&self) -> RuntimePolicy {
        self.slot().map(|s| s.policy.clone()).unwrap_or_default()
    }

    fn model(&self) -> Option<String> {
        self.slot().and_then(|s| s.model.as_ref().map(|m| m.model.clone()))
    }

    fn model_selection(&self) -> Option<ModelSelection> {
        Some(ModelSelection { instance_id: self.active_instance()?, model: self.model()?, options: Vec::new() })
    }
}

#[cfg(test)]
pub(crate) mod tests;
