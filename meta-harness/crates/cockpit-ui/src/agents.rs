//! The desk's agents: the server's agent slots, their sessions and harness
//! instances, and what waits on the user (the store, applied from the
//! server's events), with what the cockpit does with them: opening
//! sessions, prompting, answering, their settings, tool servers, signing
//! in. Kept on the desk (`Shared::agents`): the chat and the inbox show it
//! and ask it to act (`Ask::Agents`); the AGENT light, the bell and the
//!  A buffer holds a reader (`AgentsView`): only the
//! shell changes it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use harness_contracts::{
    ClientEvent, ContentBlock, InstanceId, ItemBody, LEAD, McpServer, ModelSelection, NativeId, ProviderSnapshot, ProviderStatus,
    RequestId, RequestResponse, RuntimePolicy, SavedSession, SessionKey, Slot, SlotId, SteerInput, ThreadId, ThreadStatus, ToolServers,
};

use crate::net::{NetClient, NetResult, OpenSlotSession, SlotOpened};
use crate::store::{Store, ThreadView};

/// How an agent slot is doing: the one answer every place that names an
/// agent reads (the AGENT light, the chat, a discussion, the inbox, the
/// plan tree, the pickers), with its words and what can be done.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentState {
    /// The slot was closed.
    Gone,
    /// It can't run, in words (its harness's, its slot's, or starting a
    /// session failed).
    CantRun(String),
    /// A session starting.
    Starting,
    /// No session running; its last one, when it can be resumed.
    NoSession { resume: Option<NativeId> },
    /// Its session waits on you (a request of its).
    WaitingOnYou,
    /// At work: its turn, or (after it) its sub-agents, how many at work.
    Working { subagents: usize },
    /// Its session open, nothing running.
    Idle,
}

/// How many sub-agents are at work, in words.
pub fn working_words(n: usize) -> String {
    if n == 1 { "1 sub-agent working".to_owned() } else { format!("{n} sub-agents working") }
}

impl AgentState {
    /// Its name in the status vocabulary (`[status.agent_*]`: its word,
    /// icon, colour).
    pub fn status(&self) -> &'static str {
        match self {
            Self::Gone => "agent_gone",
            Self::CantRun(_) => "agent_cant_run",
            Self::Starting => "agent_starting",
            Self::NoSession { .. } => "agent_no_session",
            Self::WaitingOnYou => "agent_waiting",
            Self::Working { .. } => "agent_working",
            Self::Idle => "agent_idle",
        }
    }

    /// In a few words (after its name: "lead: no session running"): its
    /// state's word, or why it can't run, or how many sub-agents are at
    /// work.
    pub fn words(&self) -> String {
        match self {
            Self::CantRun(why) => why.clone(),
            Self::Working { subagents } if *subagents > 0 => working_words(*subagents),
            other => crate::status::look(other.status()).1,
        }
    }

    /// Its colour (its state's).
    pub fn colour(&self) -> egui::Color32 {
        crate::status::look(self.status()).2
    }

    /// What gets it going when it has no session: its words, and what's
    /// asked of the agents.
    pub fn way_out(&self) -> Option<(&'static str, Act)> {
        match self {
            Self::NoSession { resume: Some(native) } => Some(("Resume its last session", Act::Open { resume: Some(native.clone()) })),
            Self::NoSession { resume: None } => Some(("Start a session", Act::Open { resume: None })),
            _ => None,
        }
    }

    /// It's doing something (working, starting, waiting on you): what's
    /// waiting on it isn't stuck.
    pub fn busy(&self) -> bool {
        matches!(self, Self::Starting | Self::WaitingOnYou | Self::Working { .. })
    }
}

/// What a buffer asks the agents to do, for the agent slot it shows (its
/// errors show there).
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// A session in the slot: a new one, or a saved one.
    Open {
        resume: Option<NativeId>,
    },
    /// A prompt to the slot (its session opened first if it isn't
    /// running); `add`: into the turn it's running.
    Prompt {
        message: harness_contracts::Message,
        add: bool,
    },
    /// An answer to an agent's request.
    Answer {
        thread: ThreadId,
        request: RequestId,
        response: RequestResponse,
    },
    /// The harness's saved sessions, listed (to pick one).
    ListSaved {
        instance: InstanceId,
    },
    DeleteSaved {
        instance: InstanceId,
        native: NativeId,
    },
    SwitchSession {
        key: SessionKey,
    },
    SetHarness {
        instance: InstanceId,
    },
    /// A sub-slot on a harness (the chat that asked shows it).
    NewSlot {
        instance: InstanceId,
    },
    CloseSlot,
    CloseSession {
        key: SessionKey,
    },
    Stop {
        thread: ThreadId,
    },
    /// One sub-agent's work stopped, the turn going on.
    StopSubagent {
        thread: ThreadId,
        id: harness_contracts::SubagentId,
    },
    Reprobe {
        instance: InstanceId,
    },
    Steer {
        thread: ThreadId,
        input: SteerInput,
    },
    Settings {
        policy: RuntimePolicy,
        model: Option<ModelSelection>,
    },
    /// The project's tool servers (the slot's session restarts with them).
    ToolServers(Vec<McpServer>),
    SignIn {
        instance: InstanceId,
        method: String,
    },
    FinishSignIn {
        instance: InstanceId,
        answer: String,
    },
    SignOut {
        instance: InstanceId,
    },
    StopSigningIn {
        instance: InstanceId,
        id: u64,
    },
    PutNoticeAway,
    /// A sign-in's end, read: off this device's chat.
    PutSignInEndAway {
        id: u64,
    },
    PutErrorAway,
    /// The slot's updates were seen (its chat is on screen).
    Seen,
}

/// Results of calls, taken once per frame (each says which slot it was for).
enum Reply {
    Opened(SlotId, NetResult<SlotOpened>),
    Sent(SlotId, NetResult<()>),
    /// An answer to `request` was taken, or why not.
    Answered {
        slot: SlotId,
        request: harness_contracts::RequestId,
        result: NetResult<()>,
    },
    /// Stopping sub-agent `id` was taken, or why not.
    Stopping {
        slot: SlotId,
        thread: ThreadId,
        id: harness_contracts::SubagentId,
        result: NetResult<()>,
    },
    SlotMade(SlotId, NetResult<SlotId>),
    /// A session was ended to start again with new tool servers.
    Restart {
        slot: SlotId,
        resume: Option<NativeId>,
        result: NetResult<()>,
    },
    Saved(SlotId, NetResult<Vec<SavedSession>>),
    AuthStarted {
        slot: SlotId,
        instance: InstanceId,
        result: NetResult<Option<harness_contracts::SignInWaiting>>,
    },
    /// What was pasted back to sign-in `id` was taken (signed in), or why not.
    AuthFinished {
        slot: SlotId,
        instance: InstanceId,
        id: u64,
        result: NetResult<()>,
    },
    /// The tool servers every session gets; after a change, the slot whose
    /// session restarts with them.
    Tools {
        result: NetResult<ToolServers>,
        saved: Option<SlotId>,
    },
    Done(SlotId, NetResult<()>),
}

/// What was pasted back to a sign-in under way (its instance's and id):
/// being checked, or why it wasn't taken.
#[derive(Clone, Debug, PartialEq)]
pub struct Pasted {
    pub instance: InstanceId,
    pub id: u64,
    pub finishing: bool,
    pub refused: Option<String>,
}

type Mailbox = Arc<Mutex<Vec<Reply>>>;

fn post(mailbox: &Mailbox, reply: Reply) {
    if let Ok(mut m) = mailbox.lock() {
        m.push(reply);
    }
}

#[derive(Default)]
pub struct Agents {
    store: Store,
    /// Slots whose session is being opened.
    opening: BTreeSet<SlotId>,
    /// What went wrong, by slot.
    errors: BTreeMap<SlotId, String>,
    /// A prompt for a slot with no harness yet, sent once its first session opens.
    queued: BTreeMap<SlotId, harness_contracts::Message>,
    /// Something to tell the user once (a resume that opened fresh,
    /// reports delivered to the lead).
    notice: Option<String>,
    /// The saved sessions, once listed.
    saved: Option<Vec<SavedSession>>,
    /// The tool servers every session gets (ours and the project's), once
    /// asked for.
    tools: Option<ToolServers>,
    tools_asked: bool,
    /// Sign-ins asked for and not answered yet, by instance.
    signing_in: BTreeSet<InstanceId>,
    /// The last paste to a sign-in under way.
    pasted: Option<Pasted>,
    /// Sign-ins whose end was read and put away (here).
    ends_put_away: BTreeSet<u64>,
    /// The sub-slots made, how many, and the last one with the slot that asked.
    made: (u64, Option<(SlotId, SlotId)>),
    /// Requests answered here whose answer is on its way (no longer
    /// waiting on you: another can't go to them meanwhile).
    answering: BTreeSet<harness_contracts::RequestId>,
    /// Why a slot's last session couldn't start (cleared as one starts):
    /// it can't run, everywhere it's named.
    cant_open: BTreeMap<SlotId, String>,
    /// Which change of what it knows this is (an event, a reply, an act).
    version: u64,
    /// How far this clock is ahead of the server's (its least gap seen
    /// between an event happening and coming), in ms.
    skew_ms: Option<i64>,
    /// Sub-agents asked to stop (by thread: ids repeat across them), till
    /// they say they have (or it's refused).
    stopping: BTreeSet<(ThreadId, harness_contracts::SubagentId)>,
    /// The clock (seconds) as of this frame, events applied at it.
    now: f64,
    mailbox: Mailbox,
}

/// A harness's state, in words, after its name ("Claude Code can't run:
/// no claude on the PATH").
pub fn status_words(status: &ProviderStatus) -> String {
    match status {
        ProviderStatus::Ready => "is ready".into(),
        ProviderStatus::Starting => "is starting".into(),
        ProviderStatus::NeedsAuth => "needs you to sign in".into(),
        ProviderStatus::Unavailable { reason } => format!("can't run: {reason}"),
        ProviderStatus::Error { message } => format!("failed: {message}"),
    }
}

/// A server's clock, as this one has it: this one's, less how far it's
/// ahead (ms). What its times are compared with: when a thing happened
/// there, how long ago that is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServerClock(i64);

impl ServerClock {
    /// Now, on the server's clock.
    pub fn now_ms(self) -> u64 {
        (crate::attention::now_ms() as i64 - self.0).max(0) as u64
    }
}

impl Agents {
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Takes a server event, as it happens now (tests).
    #[cfg(test)]
    pub fn apply(&mut self, event: ClientEvent) {
        let now = crate::attention::now_ms();
        self.apply_stamped(harness_contracts::Stamped { at_ms: now, event }, now);
    }

    /// Takes a server event (the shell, for every one), which came at
    /// `arrived_ms` (this clock), as of when it happened: on this clock,
    /// that long before now (never after).
    pub fn apply_stamped(&mut self, stamped: harness_contracts::Stamped, arrived_ms: u64) {
        self.version += 1;
        let harness_contracts::Stamped { at_ms, event } = stamped;
        // (Its server's clock and this one's differ by at most the least
        // gap seen between when it says a thing happened and when it came
        // here, whenever it's taken: nothing comes before it happens. Not
        // said: as it came.)
        if event == ClientEvent::Resync {
            self.skew_ms = None;
        }
        let at_ms = if at_ms == 0 { (arrived_ms as i64 - self.skew_ms.unwrap_or(0)).max(0) as u64 } else { at_ms };
        let gap = arrived_ms as i64 - at_ms as i64;
        let skew = self.skew_ms.map_or(gap, |s| s.min(gap));
        self.skew_ms = Some(skew);
        let ago = (crate::attention::now_ms() as i64 - at_ms as i64 - skew).max(0) as f64 / 1000.0;
        let at = self.now - ago;
        // Answered (as the server says): its answer arrived.
        if let ClientEvent::Session { event: harness_contracts::AdapterEvent::RequestUpdated { request } } = &event
            && request.status != harness_contracts::RequestStatus::Pending
        {
            self.answering.remove(&request.id);
        }
        // Stopped (or ended): no longer being stopped.
        if let ClientEvent::Session { event: harness_contracts::AdapterEvent::SubagentUpdated { subagent } } = &event
            && !subagent.status.working()
        {
            self.stopping.remove(&(subagent.thread_id.clone(), subagent.id.clone()));
        }
        self.store.apply(event, at);
    }

    /// Which change of what it knows this is: the same, nothing of it
    /// changed.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The server's clock, as this one has it.
    pub fn clock(&self) -> ServerClock {
        ServerClock(self.skew_ms.unwrap_or(0))
    }

    /// Whether sub-agent `id` of `thread` was asked to stop, and hasn't
    /// yet.
    pub fn stopping(&self, thread: &ThreadId, id: &harness_contracts::SubagentId) -> bool {
        self.stopping.contains(&(thread.clone(), id.clone()))
    }

    /// Whether `request` waits on you: pending, and no answer to it on its
    /// way.
    pub fn waits(&self, request: &harness_contracts::RuntimeRequest) -> bool {
        request.status == harness_contracts::RequestStatus::Pending && !self.answering.contains(&request.id)
    }

    /// How `instance`'s last sign-in ended (the server's word), unless put
    /// away here.
    pub fn sign_in_ended(&self, instance: &InstanceId) -> Option<&harness_contracts::SignInEnded> {
        self.store.instances.get(instance)?.auth.ended.as_deref().filter(|e| !self.ends_put_away.contains(&e.id))
    }

    pub fn opening(&self, slot: &SlotId) -> bool {
        self.opening.contains(slot)
    }

    pub fn error(&self, slot: &SlotId) -> Option<&String> {
        self.errors.get(slot)
    }

    pub fn notice(&self) -> Option<&String> {
        self.notice.as_ref()
    }

    pub fn saved(&self) -> Option<&[SavedSession]> {
        self.saved.as_deref()
    }

    pub fn tools(&self) -> Option<&ToolServers> {
        self.tools.as_ref()
    }

    /// The sign-in under way for `instance` (the server's, for every device).
    pub fn waiting(&self, instance: &InstanceId) -> Option<&harness_contracts::SignInWaiting> {
        self.store.instances.get(instance)?.auth.waiting.as_deref()
    }

    /// A sign-in asked for `instance` and not answered yet.
    pub fn signing_in(&self, instance: &InstanceId) -> bool {
        self.signing_in.contains(instance)
    }

    /// The last paste to `instance`'s sign-in `id`.
    pub fn pasted(&self, instance: &InstanceId, id: u64) -> Option<&Pasted> {
        self.pasted.as_ref().filter(|p| &p.instance == instance && p.id == id)
    }

    /// How many sub-slots were made, and the last with the slot it was made from.
    pub fn made(&self) -> (u64, Option<&(SlotId, SlotId)>) {
        (self.made.0, self.made.1.as_ref())
    }

    /// The slot's current session's thread, while it runs.
    pub fn thread_id_in(&self, slot: &SlotId) -> Option<ThreadId> {
        self.store.slots.get(slot)?.current_thread().cloned()
    }

    /// The slot's harness, else (a lead with none chosen yet) the first there is.
    pub fn instance_id_in(&self, slot: &SlotId) -> Option<InstanceId> {
        self.store.slots.get(slot).and_then(|s| s.instance_id.clone()).or_else(|| self.store.instances.keys().next().cloned())
    }

    pub fn instance_in(&self, slot: &SlotId) -> Option<&ProviderSnapshot> {
        self.store.instances.get(&self.instance_id_in(slot)?)
    }

    pub fn thread_in(&self, slot: &SlotId) -> Option<&ThreadView> {
        self.store.threads.get(&self.thread_id_in(slot)?)
    }

    pub fn slot(&self, slot: &SlotId) -> Option<&Slot> {
        self.store.slots.get(slot)
    }

    /// How agent slot `slot` is doing.
    pub fn state(&self, slot: &SlotId) -> AgentState {
        // (A session this one couldn't start: so till one runs, from here
        // or anywhere, or its error is put away.)
        let couldnt = || self.cant_open.get(slot).map(|why| AgentState::CantRun(format!("couldn't start a session: {why}")));
        let Some(its) = self.store.slots.get(slot) else {
            if self.store.closed.contains(slot) {
                return AgentState::Gone;
            }
            return couldnt()
                .or_else(|| self.harness_problem(slot).map(AgentState::CantRun))
                .unwrap_or(AgentState::NoSession { resume: None });
        };
        if let harness_contracts::SlotStatus::Unavailable { reason, .. } = &its.status {
            return AgentState::CantRun(format!("can't run: {reason}"));
        }
        let resume = its.current_session().filter(|s| s.thread.is_none()).and_then(|s| s.native_id.clone());
        let Some(view) = self.thread_in(slot) else {
            if self.opening.contains(slot) {
                return AgentState::Starting;
            }
            if let Some(state) = couldnt() {
                return state;
            }
            return self.harness_problem(slot).map_or(AgentState::NoSession { resume }, AgentState::CantRun);
        };
        match view.thread.status {
            ThreadStatus::Starting => AgentState::Starting,
            ThreadStatus::Error => AgentState::CantRun("the agent reported an error".into()),
            ThreadStatus::Closed => AgentState::NoSession { resume },
            _ if view.pending_requests().any(|r| self.waits(r)) => AgentState::WaitingOnYou,
            _ if view.running().is_some() || view.subagents_working() > 0 => AgentState::Working { subagents: view.subagents_working() },
            _ => AgentState::Idle,
        }
    }

    /// Whose move it is in discussion `d`, in a few words ("waiting on
    /// you", "waiting on lead", "settled"; the agent's state when it isn't
    /// at it: "waiting on lead: no session running"), and whether it's
    /// yours.
    pub fn whose(&self, d: &harness_contracts::Discussion) -> (String, bool) {
        match d.whose_move() {
            harness_contracts::Move::User => ("waiting on you".to_owned(), true),
            harness_contracts::Move::Agent(slot) => {
                let state = self.state(&SlotId::new(slot.clone()));
                let waiting = format!("waiting on {}", d.name_of(&slot));
                (if state.busy() || state == AgentState::Idle { waiting } else { format!("{waiting}: {}", state.words()) }, false)
            }
            harness_contracts::Move::Nobody => ("settled".to_owned(), false),
        }
    }

    /// Why `slot`'s harness (else the first) can't start, and how to fix
    /// it, as its instance says.
    fn harness_problem(&self, slot: &SlotId) -> Option<String> {
        let instance = self.instance_in(slot)?;
        matches!(instance.status, ProviderStatus::Unavailable { .. } | ProviderStatus::Error { .. }).then(|| status_words(&instance.status))
    }

    /// How much the lead's session has done (the AGENT light blinks with it).
    pub fn lead_activity(&self) -> u64 {
        self.thread_in(&SlotId::new(LEAD)).map_or(0, |t| t.news)
    }

    /// The agent's last message in `thread`, as text.
    pub fn last_said(&self, thread: &ThreadId) -> Option<String> {
        self.store.threads.get(thread)?.stream().rev().find_map(|item| match &item.body {
            ItemBody::AssistantMessage { content, .. } => Some(content.iter().map(block_summary).collect::<Vec<_>>().join(" ")),
            _ => None,
        })
    }

    /// Does what `slot`'s chat (or the inbox) asked.
    pub fn act(&mut self, slot: SlotId, act: Act, net: &NetClient, ctx: &egui::Context) {
        self.version += 1;
        let mailbox = self.mailbox.clone();
        let for_slot = slot.clone();
        let done = move |r: NetResult<()>| post(&mailbox, Reply::Done(for_slot, r));
        match act {
            Act::Open { resume } => self.open(slot, net, ctx, resume),
            Act::Prompt { message, add } => self.prompt(slot, net, ctx, message, add),
            Act::Answer { thread, request, response } => {
                let mailbox = self.mailbox.clone();
                let slot = self.store.slots.values().find(|s| s.current_thread() == Some(&thread)).map_or(slot, |s| s.id.clone());
                self.answering.insert(request.clone());
                net.respond(&thread, &request.clone(), &response, ctx.clone(), move |result| {
                    post(&mailbox, Reply::Answered { slot, request, result })
                });
            }
            Act::ListSaved { instance } => {
                self.saved = None;
                let mailbox = self.mailbox.clone();
                net.list_saved_sessions(&instance, ctx.clone(), move |r| post(&mailbox, Reply::Saved(slot, r.map(|p| p.sessions))));
            }
            Act::DeleteSaved { instance, native } => {
                net.delete_saved_session(&instance, &native, ctx.clone(), done);
                self.saved = None;
            }
            Act::SwitchSession { key } => net.switch_slot_session(&slot, &key, ctx.clone(), done),
            Act::SetHarness { instance } => net.set_slot_harness(&slot, &instance, ctx.clone(), done),
            Act::NewSlot { instance } => {
                let mailbox = self.mailbox.clone();
                net.new_slot(&instance, ctx.clone(), move |r| post(&mailbox, Reply::SlotMade(slot, r)));
            }
            Act::CloseSlot => net.close_slot(&slot, ctx.clone(), done),
            Act::CloseSession { key } => net.close_slot_session(&slot, &key, ctx.clone(), done),
            Act::Stop { thread } => net.interrupt(&thread, ctx.clone()),
            Act::StopSubagent { thread, id } => {
                self.stopping.insert((thread.clone(), id.clone()));
                let mailbox = self.mailbox.clone();
                let (asked, of) = (thread.clone(), id.clone());
                net.cancel_subagent(&asked, &of, ctx.clone(), move |result| post(&mailbox, Reply::Stopping { slot, thread, id, result }));
            }
            Act::Reprobe { instance } => net.refresh_instance(&instance, ctx.clone(), done),
            Act::Steer { thread, input } => net.steer(&thread, &input, ctx.clone(), done),
            Act::Settings { policy, model } => net.set_slot_settings(&slot, &policy, model.as_ref(), ctx.clone(), done),
            Act::ToolServers(project) => {
                let mailbox = self.mailbox.clone();
                net.set_tool_servers(&project, ctx.clone(), move |result| post(&mailbox, Reply::Tools { result, saved: Some(slot) }));
            }
            Act::SignIn { instance, method } => {
                self.pasted = None;
                // One at a time: a second tap waits for the first.
                if self.signing_in.insert(instance.clone()) {
                    let mailbox = self.mailbox.clone();
                    let asked = instance.clone();
                    net.begin_auth(&instance, &method, ctx.clone(), move |result| {
                        post(&mailbox, Reply::AuthStarted { slot, instance: asked, result })
                    });
                }
            }
            Act::FinishSignIn { instance, answer } => {
                let Some(id) = self.waiting(&instance).map(|w| w.id) else {
                    self.errors.insert(slot, "No sign-in is waiting: sign in again.".into());
                    return;
                };
                self.pasted = Some(Pasted { instance: instance.clone(), id, finishing: true, refused: None });
                let mailbox = self.mailbox.clone();
                let asked = instance.clone();
                net.complete_auth(&instance, id, &answer, ctx.clone(), move |result| {
                    post(&mailbox, Reply::AuthFinished { slot, instance: asked, id, result })
                });
            }
            Act::SignOut { instance } => net.logout(&instance, ctx.clone(), done),
            Act::StopSigningIn { instance, id } => {
                self.pasted = None;
                net.cancel_auth(&instance, id, ctx.clone(), done);
            }
            Act::PutNoticeAway => self.notice = None,
            Act::PutSignInEndAway { id } => {
                self.ends_put_away.insert(id);
            }
            // (A session it couldn't start too: its way out shows.)
            Act::PutErrorAway => {
                self.errors.remove(&slot);
                self.cant_open.remove(&slot);
            }
            Act::Seen => net.notifications_seen(&slot, ctx.clone(), |_| {}),
        }
    }

    /// Opens a session in `slot`. A slot with no harness yet (a new
    /// project's lead) gets the first there is.
    fn open(&mut self, slot: SlotId, net: &NetClient, ctx: &egui::Context, resume: Option<NativeId>) {
        let has_harness = self.store.slots.get(&slot).is_some_and(|s| s.instance_id.is_some());
        let instance_id = match has_harness {
            true => None,
            false => match self.store.instances.keys().next() {
                Some(first) => Some(first.clone()),
                None => {
                    self.queued.remove(&slot);
                    self.errors.insert(slot.clone(), "No harness instance is available yet.".into());
                    self.cant_open.insert(slot, "no harness instance is available yet".into());
                    return;
                }
            },
        };
        if !self.opening.insert(slot.clone()) {
            return;
        }
        self.errors.remove(&slot);
        self.cant_open.remove(&slot);
        let body = OpenSlotSession { instance_id, resume, ..Default::default() };
        let mailbox = self.mailbox.clone();
        let for_slot = slot.clone();
        net.open_slot_session(&slot, &body, ctx.clone(), move |r| post(&mailbox, Reply::Opened(for_slot, r)));
    }

    fn prompt(&mut self, slot: SlotId, net: &NetClient, ctx: &egui::Context, message: harness_contracts::Message, add_to_running: bool) {
        self.errors.remove(&slot);
        if self.store.slots.get(&slot).is_none_or(|s| s.instance_id.is_none()) {
            self.queued.insert(slot.clone(), message);
            self.open(slot, net, ctx, None);
            return;
        }
        let mailbox = self.mailbox.clone();
        let for_slot = slot.clone();
        net.prompt_slot(&slot, &message, add_to_running, ctx.clone(), move |r| post(&mailbox, Reply::Sent(for_slot, r)));
    }

    /// Restarts `slot`'s session with the current tool servers (ACP takes
    /// them only when a session is created or loaded): ended, then resumed.
    fn reconnect(&mut self, slot: &SlotId, net: &NetClient, ctx: &egui::Context) {
        let Some(slot) = self.store.slots.get(slot).cloned() else { return };
        let Some(session) = slot.current_session().filter(|s| s.thread.is_some()).cloned() else { return };
        let mailbox = self.mailbox.clone();
        let (for_slot, resume) = (slot.id.clone(), session.native_id.clone());
        net.close_slot_session(&slot.id, &session.key, ctx.clone(), move |result| {
            post(&mailbox, Reply::Restart { slot: for_slot, resume, result })
        });
    }

    /// Takes what the server answered, and asks for the tool servers once
    /// there's a harness (every frame, shown or not: a message sent from
    /// elsewhere waits on a session opening).
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        self.now = ctx.input(|i| i.time);
        if !self.tools_asked && !self.store.instances.is_empty() {
            self.tools_asked = true;
            let mailbox = self.mailbox.clone();
            net.tool_servers(ctx.clone(), move |result| post(&mailbox, Reply::Tools { result, saved: None }));
        }
        let replies = std::mem::take(&mut *self.mailbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        if !replies.is_empty() {
            self.version += 1;
        }
        for reply in replies {
            match reply {
                Reply::Opened(slot, Ok(opened)) => {
                    self.opening.remove(&slot);
                    self.saved = None;
                    let mut said = Vec::new();
                    said.extend(opened.note);
                    if opened.delivered_reports > 0 {
                        said.push(format!("{} report(s) from sub-slots were delivered to the lead.", opened.delivered_reports));
                    }
                    if !said.is_empty() {
                        self.notice = Some(said.join(" "));
                    }
                    if let Some(message) = self.queued.remove(&slot) {
                        self.prompt(slot, net, ctx, message, false);
                    }
                }
                Reply::Opened(slot, Err(e)) => {
                    self.opening.remove(&slot);
                    self.queued.remove(&slot);
                    let why: String = e.into();
                    self.cant_open.insert(slot.clone(), why.clone());
                    self.errors.insert(slot, why);
                }
                // Not taken: it waits on you again, and why is said.
                Reply::Answered { slot, request, result: Err(e) } => {
                    self.answering.remove(&request);
                    self.errors.insert(slot, e.into());
                }
                Reply::Answered { result: Ok(()), .. } => {}
                // Refused: it's said why, and can be asked again.
                Reply::Stopping { slot, thread, id, result: Err(e) } => {
                    self.stopping.remove(&(thread, id));
                    self.errors.insert(slot, e.into());
                }
                Reply::Stopping { result: Ok(()), .. } => {}
                Reply::Sent(slot, Err(e)) => {
                    self.errors.insert(slot, e.into());
                }
                Reply::SlotMade(from, Ok(made)) => self.made = (self.made.0 + 1, Some((from, made))),
                Reply::Restart { slot, resume, result: Ok(()) } => self.open(slot, net, ctx, resume),
                Reply::Saved(_, Ok(sessions)) => self.saved = Some(sessions),
                Reply::AuthStarted { slot, instance, result } => {
                    self.signing_in.remove(&instance);
                    match result {
                        // What's to be done comes in the instance's snapshot too.
                        Ok(_) => {
                            self.errors.remove(&slot);
                        }
                        Err(e) => {
                            self.errors.insert(slot, e.into());
                        }
                    }
                }
                Reply::AuthFinished { slot, instance, id, result } => {
                    let still = self.waiting(&instance).is_some_and(|w| w.id == id);
                    match (result, self.pasted.as_mut().filter(|p| p.instance == instance && p.id == id)) {
                        (Ok(()), _) => self.pasted = None,
                        // The sign-in still waits: what was wrong with the paste.
                        (Err(e), Some(pasted)) if still => {
                            pasted.finishing = false;
                            pasted.refused = Some(e.into());
                        }
                        // It's over: why, as any error.
                        (Err(e), _) => {
                            self.pasted = None;
                            self.errors.insert(slot, e.into());
                        }
                    }
                }
                Reply::Tools { result: Ok(tools), saved } => {
                    self.tools = Some(tools);
                    // The open session takes tool servers only when it starts.
                    if let Some(slot) = saved {
                        self.reconnect(&slot, net, ctx);
                    }
                }
                Reply::Tools { result: Err(e), saved } => {
                    self.errors.insert(saved.unwrap_or_else(|| SlotId::new(LEAD)), e.into());
                }
                Reply::Saved(slot, Err(e))
                | Reply::Done(slot, Err(e))
                | Reply::SlotMade(slot, Err(e))
                | Reply::Restart { slot, result: Err(e), .. } => {
                    self.errors.insert(slot, e.into());
                }
                Reply::Sent(_, Ok(())) | Reply::Done(_, Ok(())) => {}
            }
        }
    }
}

#[cfg(test)]
impl Agents {
    /// The tool servers, as if the server sent them.
    pub(crate) fn take_tools_for_test(&mut self, tools: ToolServers) {
        self.tools = Some(tools);
    }
}

/// A message's part in a few words (text as it is; the rest named).
pub fn block_summary(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text { text } => text.clone(),
        ContentBlock::Image { mime_type, .. } => format!("[image {mime_type}]"),
        ContentBlock::ResourceLink { uri, name } => format!("[link {}]", name.clone().unwrap_or_else(|| uri.clone())),
        ContentBlock::Resource { uri, .. } => format!("[resource {uri}]"),
        ContentBlock::Diff(d) => format!("[diff {}]", d.path),
        ContentBlock::Unsupported => "[unsupported content]".into(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use harness_contracts::ProviderSnapshot;

    pub(crate) fn snapshot(status: ProviderStatus) -> ProviderSnapshot {
        ProviderSnapshot {
            status,
            ..ProviderSnapshot::new(InstanceId::new("antigravity"), harness_contracts::DriverKind::new("antigravity"), "Antigravity")
        }
    }

    #[test]
    fn a_harness_that_cant_start_says_why_and_the_fix() {
        let mut agents = Agents::default();
        let lead = SlotId::new(LEAD);
        assert_eq!(agents.state(&lead), AgentState::NoSession { resume: None });
        let reason = "antigravity-acp isn't installed (fix: meta-harness pack install antigravity-acp)";
        agents.apply(ClientEvent::InstanceUpdated { snapshot: snapshot(ProviderStatus::Unavailable { reason: reason.into() }) });
        assert_eq!(agents.state(&lead), AgentState::CantRun(format!("can't run: {reason}")), "the AGENT light's hint");
        agents.apply(ClientEvent::InstanceUpdated { snapshot: snapshot(ProviderStatus::Ready) });
        assert_eq!(agents.state(&lead), AgentState::NoSession { resume: None });
    }

    #[test]
    fn a_prompt_with_no_harness_anywhere_says_so() {
        let mut agents = Agents::default();
        let (ctx, net) = (egui::Context::default(), NetClient::new("http://127.0.0.1:9"));
        agents.act(SlotId::new(LEAD), Act::Prompt { message: "hello".into(), add: false }, &net, &ctx);
        assert!(matches!(agents.state(&SlotId::new(LEAD)), AgentState::CantRun(e) if e.contains("no harness instance")));
        assert_eq!(
            agents.error(&SlotId::new(LEAD)).map(String::as_str),
            Some("No harness instance is available yet."),
            "said in the lead's chat"
        );
    }

    /// One answer to how an agent is, as every place names it: idle,
    /// working (its sub-agents too), waiting on you; with no session, its
    /// last to resume; and a discussion waiting on it says so then.
    #[test]
    fn how_an_agent_is_is_one_answer() {
        use harness_contracts::{AdapterEvent, Turn, TurnId, TurnStatus};
        crate::status::set(crate::config::UiConfig::with_overrides("").unwrap().status);
        let lead = SlotId::new(LEAD);
        let mut agents = Agents::default();
        crate::chat::tests::conversation_events().into_iter().for_each(|e| agents.apply(e));
        assert_eq!(agents.state(&lead), AgentState::Idle);
        // One this tab couldn't start, while one runs (started anywhere):
        // it's as the one running is.
        agents.cant_open.insert(lead.clone(), "refused".into());
        assert_eq!(agents.state(&lead), AgentState::Idle);
        agents.cant_open.clear();
        let turn = |status| ClientEvent::Session {
            event: AdapterEvent::TurnUpdated {
                turn: Turn { id: TurnId::new("now"), thread_id: ThreadId::new("t"), ordinal: 2, status, failure: None, usage: None },
            },
        };
        agents.apply(turn(TurnStatus::Running));
        assert_eq!(agents.state(&lead), AgentState::Working { subagents: 0 });
        assert_eq!(agents.state(&lead).words(), "working");
        // Its turn over, a sub-agent of its still at work: working.
        agents.apply(turn(TurnStatus::Completed));
        let sub = harness_contracts::Subagent {
            id: harness_contracts::SubagentId::new("a"),
            thread_id: ThreadId::new("t"),
            item_id: harness_contracts::ItemId::new("i50"),
            title: "Helper".into(),
            kind: None,
            description: None,
            prompt: None,
            answer: None,
            status: harness_contracts::SubagentStatus::Running,
            started_ms: 0,
            ended_ms: None,
            can_cancel: false,
        };
        agents.apply(ClientEvent::Session { event: AdapterEvent::SubagentUpdated { subagent: sub } });
        assert_eq!(agents.state(&lead).words(), "1 sub-agent working");
        // No session running: its last, to resume.
        let mut slot = crate::chat::tests::lead(None);
        slot.sessions[0].native_id = Some(NativeId::new("n1"));
        agents.apply(ClientEvent::SlotUpdated { slot });
        assert_eq!(agents.state(&lead), AgentState::NoSession { resume: Some(NativeId::new("n1")) });
        let discussion = crate::discussions::tests::discussion(
            1,
            harness_contracts::Subject::CodeFile { path: "a.rs".into() },
            harness_contracts::DiscussionStatus::Open,
        );
        assert_eq!(agents.whose(&discussion).0, "waiting on lead: no session running");
    }

    /// The server's clock may differ from this one: an event's age is told
    /// by the least gap seen between its time and when it came (nothing
    /// comes before it happens), so a server a minute ahead doesn't make
    /// its news look as if it's still to come, nor one behind make it look
    /// old; nor events taken late (a hidden tab's) look new.
    #[test]
    fn an_events_time_is_read_on_its_servers_clock() {
        for (ahead_ms, late_ms) in [(60_000_i64, 0_u64), (-60_000, 0), (0, 300_000)] {
            let mut agents = Agents { now: 1000.0, ..Default::default() };
            let (here_now, server_now) = (crate::attention::now_ms(), (crate::attention::now_ms() as i64 + ahead_ms) as u64);
            let came = here_now - late_ms;
            for event in [ClientEvent::Resync, ClientEvent::Replayed] {
                agents.apply_stamped(harness_contracts::Stamped { at_ms: server_now - late_ms, event }, came);
            }
            crate::chat::tests::conversation_events()
                .into_iter()
                .filter(|e| *e != ClientEvent::Replayed)
                .for_each(|event| agents.apply_stamped(harness_contracts::Stamped { at_ms: server_now - late_ms - 3_000, event }, came));
            let heard = agents.thread_in(&SlotId::new(LEAD)).and_then(|t| t.last_heard()).unwrap();
            let ago = (late_ms + 3_000) as f64 / 1000.0;
            assert!((agents.now - heard - ago).abs() < 0.5, "{ahead_ms} {late_ms}: heard {heard}");
            assert!((agents.clock().now_ms() as i64 - server_now as i64).abs() < 500);
        }
    }
}
