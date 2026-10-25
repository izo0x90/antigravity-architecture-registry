//! The chat/trajectory view: an agent slot's session, over the desk's
//! agents (`agents.rs`). Renders only from the store (server events) and
//! the active instance's snapshot: pickers, commands and controls come from
//! what the harness reports and its capabilities, never from hardcoded
//! harness knowledge. What it does with the agents it asks for
//! (`Ask::Agents`).

use std::cell::Ref;

use egui::{Color32, Frame, Pos2, Rect, RichText, Stroke, Vec2};
use harness_contracts::{
    ConfigKind, McpServer, McpTransport, NativeId, OptionValue, ContentBlock, InstanceId, ItemBody, ItemStatus, ModelSelection, NoticeLevel, PlanBody,
    PlanStep, ProviderSnapshot, ProviderStatus, RuntimeMode, RuntimePolicy, SessionKey, Slot, SlotId, SlotStatus,
    StepStatus, SteerInput, ThreadId, TurnItem, TurnStatus, LEAD,
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
mod live;
mod nav;
mod render;
mod rows;
mod session;
pub(crate) mod things;
pub(crate) mod slots;
mod words;


use crate::agents::block_summary;
use render::mcp_label;

#[derive(Default)]
pub struct ChatStreamState {
    /// The desk's agents, read; calls' changes, as the server compares
    /// them, read, and those drawn not compared yet (asked for once drawn).
    agents: crate::shared::Reader<Agents>,
    comparer: crate::shared::Reader<crate::comparer::Comparer>,
    changes_wanted: Vec<crate::comparer::Pair>,
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
    /// The chat's settings (what each view starts with; how live the
    /// agent's said to be), and which of their changes it took.
    settings: crate::config::ChatUi,
    seen_settings: u64,
    /// Unchanged lines shown around a change (the Changes view's).
    context: usize,
    /// Whether the server's event socket is up (the shell's word).
    connected: bool,
}

/// A chat view's own state: what's typed, where the cursor is, what was
/// drawn. The chat's sessions, sign-in and tools are shared by all views.
pub struct ChatView {
    /// The slot it shows: the lead, unless another was picked.
    pub slot: SlotId,
    /// What's typed to the agent.
    pub input: crate::conversation::TypingBox,
    /// What its messages are drawn with (markdown, code blocks' colours).
    pub(crate) messages: crate::conversation::Messages,
    /// The message the cursor is on (normal mode); None follows the latest.
    /// A folded run of calls is its first call's place.
    pub cursor: Option<usize>,
    /// What's folded and open (runs of calls, a call's output, reasoning),
    /// for which thread; the stream's shape as last worked out from them.
    pub(crate) folds: rows::Folds,
    pub(crate) folds_for: Option<ThreadId>,
    /// The item the cursor's on (it follows the item as others are put in
    /// before it: `tend`); which view of the thread the rows' heights and
    /// changes were worked out for.
    pub(crate) cursor_on: Option<harness_contracts::ItemId>,
    /// A place restored, its cursor to be shown once the thread's all here.
    pub(crate) placing: bool,
    pub(crate) epoch: Option<u64>,
    shape: std::cell::RefCell<Option<ShapeOf>>,
    /// What shows the agent at work (the setting's pick); where the running
    /// turn's line was drawn this frame.
    pub(crate) indicator: crate::indicator::Shown,
    /// How much each chat (each slot's) shows and what shows the agent at
    /// work there, as picked for it (kept through a reload; none: the
    /// config's).
    pub(crate) picked: std::collections::BTreeMap<SlotId, Picked>,
    pub(crate) live_at: Option<render::LiveAt>,
    /// How tall each row was last drawn (by its first item), and as what:
    /// a row out of sight takes that room without being drawn.
    pub(crate) heights: std::collections::HashMap<harness_contracts::ItemId, (render::Measure, f32)>,
    /// What in the item a hint picked: one of its links, or code blocks.
    pub(crate) pick: Option<Pick>,
    /// Where it's looking (the shared rules move it, a message a stop);
    /// following the latest, the newest stays in view.
    pub(crate) scrolling: crate::viewport::Measured,
    /// Messages and links drawn last frame (and this one's, as drawn).
    pub(crate) spots: Vec<Spot>,
    pub(crate) drawing: Vec<Spot>,
    /// Each item of the stream as drawn this frame, in order (its stops).
    pub(crate) item_rects: Vec<Rect>,
    /// Where the banners were drawn last frame (none: none showed).
    pub(crate) banners: Option<Rect>,
    /// How tall what's under the stream was last frame (the requests kept
    /// in sight, the typing box): the stream leaves room for it.
    pub(crate) below: f32,
    /// Where its typing box is, while it's typed in.
    pub(crate) typing: Option<Rect>,
    /// A sign-in has the typing box (see `tend_sign_in_box`).
    pub(crate) sign_in_has_box: bool,
}

/// A chat's settings as picked for it (none: the config's).
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Picked {
    pub shows: Option<crate::config::Verbosity>,
    pub at_work: Option<crate::config::IndicatorKind>,
}

impl Default for ChatView {
    fn default() -> Self {
        Self {
            slot: SlotId::new(LEAD),
            input: Default::default(),
            messages: Default::default(),
            cursor: None,
            folds: rows::Folds::default(),
            folds_for: None,
            cursor_on: None,
            placing: false,
            epoch: None,
            shape: Default::default(),
            indicator: Default::default(),
            picked: std::collections::BTreeMap::new(),
            live_at: None,
            heights: Default::default(),
            pick: None,
            scrolling: Default::default(),
            spots: Vec::new(),
            item_rects: Vec::new(),
            banners: None,
            below: 0.0,
            drawing: Vec::new(),
            typing: None,
            sign_in_has_box: false,
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

/// What in an item a hint picked (its place among them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    /// A file it points to.
    Link(usize),
    Code(usize),
    /// A chip in your message (what it links).
    Chip(usize),
}

impl Pick {
    /// Its place's key, in item `index` (`3:1`, `3:c0`, `3:l0`).
    pub(crate) fn key(self, index: usize) -> String {
        match self {
            Self::Link(n) => format!("{index}:{n}"),
            Self::Code(n) => format!("{index}:c{n}"),
            Self::Chip(n) => format!("{index}:l{n}"),
        }
    }

    /// From a place's key after its item's (`1`, `c0`, `l0`).
    fn of(part: &str) -> Option<Self> {
        if let Some(code) = part.strip_prefix('c') {
            return code.parse().ok().map(Self::Code);
        }
        if let Some(chip) = part.strip_prefix('l') {
            return chip.parse().ok().map(Self::Chip);
        }
        part.parse().ok().map(Self::Link)
    }
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

/// A stream's shape, and what it was worked out from: its thread, which
/// view of it and as of which event, which change of the folds; what of
/// the stream it reads (`rows::shape_facts`).
pub(crate) struct ShapeOf {
    of: (ThreadId, u64, u64, u64),
    facts: u64,
    shape: std::rc::Rc<rows::Shape>,
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
        let view = self.thread()?;
        let agents = self.agents();
        view.requests().iter().filter(|r| agents.waits(r)).find_map(|r| view.request_shown_at(&r.id))
    }

    /// What waits on you: each request pending with no answer on its way,
    /// as it was asked (the first asked first), with its item's place in
    /// the stream (none: its item isn't there yet).
    pub(crate) fn pending(&self) -> Vec<(Option<usize>, harness_contracts::RuntimeRequest)> {
        let Some(view) = self.thread() else { return Vec::new() };
        let agents = self.agents();
        view.requests().iter().filter(|r| agents.waits(r)).map(|r| (view.request_shown_at(&r.id), r.clone())).collect()
    }

    /// The stream's shape (its rows, a folded run one of them; whether
    /// anything is folded, anything open), as drawn: worked out once per
    /// change of the thread or the folds.
    pub(crate) fn shape(&self) -> std::rc::Rc<rows::Shape> {
        let Some(view) = self.thread() else { return Default::default() };
        let of = (view.thread.id.clone(), view.epoch(), view.activity, self.ui.folds.changes());
        let mut kept = self.ui.shape.borrow_mut();
        // (The same thread as of the same event, else as of the same facts.)
        let facts = match &*kept {
            Some(was) if was.of == of => return was.shape.clone(),
            Some(was) if (&was.of.0, was.of.1, was.of.3) == (&of.0, of.1, of.3) => {
                let facts = rows::shape_facts(view.stream());
                if facts == was.facts {
                    let shape = was.shape.clone();
                    *kept = Some(ShapeOf { of, facts, shape: shape.clone() });
                    return shape;
                }
                facts
            }
            _ => rows::shape_facts(view.stream()),
        };
        let items: Vec<&TurnItem> = view.stream().collect();
        let shape = std::rc::Rc::new(rows::shape(&items, &self.ui.folds));
        *kept = Some(ShapeOf { of, facts, shape: shape.clone() });
        shape
    }

    /// What folds where the cursor is (the run it's in, its own).
    pub(crate) fn here(&self) -> Option<rows::Here> {
        let at = self.ui.cursor?;
        let shape = self.shape();
        let row = &shape.rows[shape.row_of(at)?];
        let view = self.thread()?;
        let run: Vec<&TurnItem> = row.run.map(|(a, b)| (a..=b).filter_map(|i| view.in_stream(i)).collect()).unwrap_or_default();
        Some(self.ui.folds.here(row, &run, view.in_stream(row.first)?))
    }

    /// The code blocks of the item you're on (a message's, a proposed
    /// plan's), in order.
    pub(crate) fn codes_here(&self) -> Vec<String> {
        let Some(view) = self.thread() else { return Vec::new() };
        let Some(item) = self.ui.cursor.and_then(|at| view.in_stream(at)) else { return Vec::new() };
        let text = match &item.body {
            ItemBody::AssistantMessage { .. } => item.body.said().unwrap_or_default(),
            ItemBody::ProposedPlan { markdown, .. } => markdown.clone(),
            _ => return Vec::new(),
        };
        crate::conversation::parts(&text).into_iter().filter_map(|p| match p {
            crate::conversation::Part::Code { code, .. } => Some(code.to_owned()),
            _ => None,
        }).collect()
    }

    /// What your message you're on links, in order: each chip's words, and
    /// where it goes.
    pub(crate) fn chips_here(&self) -> Vec<(String, Target)> {
        let Some(view) = self.thread() else { return Vec::new() };
        let Some(ItemBody::UserMessage { text: shown, pieces }) = self.ui.cursor.and_then(|at| view.in_stream(at)).map(|i| &i.body) else { return Vec::new() };
        harness_contracts::links_of(pieces)
            .iter()
            .filter_map(|link| {
                let words: String = shown.chars().skip(link.start).take(link.end - link.start).collect();
                let to = match &link.to {
                    harness_contracts::Linked::Tag { tag } => Some(Target::Tag(tag.clone())),
                    harness_contracts::Linked::Thing { subject } => Target::of_subject(subject),
                };
                to.map(|to| (words, to))
            })
            .collect()
    }

    /// Whether request `id` (of `view`) waits on you.
    fn waits_on_you(&self, view: &ThreadView, id: &harness_contracts::RequestId) -> bool {
        view.request(id).is_some_and(|r| self.agents().waits(r))
    }

    /// The item you're on: the cursor's, else (following the latest) what
    /// waits on you first.
    pub(crate) fn on_item(&self) -> Option<usize> {
        self.ui.cursor.or_else(|| self.pending_item())
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

    /// The sign-in under way, when it wants the typing box: it shows its
    /// link and what to paste back, and its time isn't up.
    fn sign_in_wants_box(&self) -> Option<SignInNow> {
        self.sign_in_now().filter(|n| n.waiting.expects.is_some() && n.waiting.url.is_some() && !n.ran_out)
    }

    /// The sign-in under way, while it has the typing box: the one answer
    /// to "whose is the box" that sending, the offered actions and the hint
    /// all read (`tend_sign_in_box` keeps it).
    fn sign_in_box(&self) -> Option<SignInNow> {
        self.ui.sign_in_has_box.then(|| self.sign_in_now()).flatten()
    }

    /// Who has the typing box: a sign-in, while one wants it and the slot's
    /// drafts have been read (so what's in the box is kept by them). Taking
    /// it, what's in it is reported first, and the box is emptied for the
    /// sign-in; giving it back, the box is emptied and the slot's draft as
    /// it is now taken again. Meanwhile nothing typed is saved (a pasted
    /// code never is). Said, when it's taken from typing.
    fn tend_sign_in_box(&mut self, note: &mut Option<String>) {
        let read = self.unsent.get().current(&self.ui.slot).is_some();
        match (self.sign_in_wants_box().is_some() && read, self.ui.sign_in_has_box) {
            (true, false) => {
                // What's in the box is the slot's draft from here: reported.
                self.take_unsent();
                self.report_unsent();
                if !self.ui.input.text.trim().is_empty() {
                    *note = Some("A sign-in is waiting: the box is its now; what you were writing is kept and comes back after.".into());
                }
                self.ui.input = Default::default();
                self.ui.sign_in_has_box = true;
            }
            (false, true) => {
                self.ui.input = Default::default();
                self.unsent_for = None;
                self.ui.sign_in_has_box = false;
            }
            _ => {}
        }
    }

    /// What's typed, sent: to the sign-in while it has the box (not while a
    /// paste is checked: it stays, and that's said), else to the agent
    /// (`add`: into the turn it's running). Every way of sending comes here.
    fn send_typed(&mut self, add: bool, note: &mut Option<String>) {
        if self.ui.input.text.trim().is_empty() {
            return;
        }
        match (self.sign_in_box(), self.active_instance()) {
            (Some(now), _) if now.pasted.as_ref().is_some_and(|p| p.finishing) => *note = Some("What you pasted is being checked: send again once it's answered.".into()),
            (Some(_), Some(instance)) => {
                let answer = self.ui.input.take().text;
                self.act(Act::FinishSignIn { instance, answer });
            }
            (Some(_), None) => {}
            (None, _) => {
                let message = self.ui.input.take();
                if add { self.add_to_turn(message) } else { self.send_prompt(message) }
            }
        }
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
