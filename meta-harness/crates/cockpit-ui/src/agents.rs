//! The desk's agents: the server's agent slots, their sessions and harness
//! instances, and what waits on the user (the store, applied from the
//! server's events), with what the cockpit does with them: opening
//! sessions, prompting, answering, their settings, tool servers, signing
//! in. Kept on the desk (`Shared::agents`): the chat and the inbox show it
//! and ask it to act (`Ask::Agents`); the AGENT light, the bell and the
//!  A buffer holds a reader (`AgentsView`): only the
//! shell changes it.

use std::cell::{Ref, RefCell, RefMut};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use harness_contracts::{
    ClientEvent, ContentBlock, InstanceId, ItemBody, McpServer, ModelSelection, NativeId, ProviderSnapshot, ProviderStatus, RequestId, RequestResponse, RuntimePolicy, SavedSession, SessionKey, Slot, SlotId, SteerInput, ThreadId, ThreadStatus,
    ToolServers, LEAD,
};

use crate::net::{AuthChallenge, NetClient, NetResult, OpenSlotSession, SlotOpened};
use crate::store::{Store, ThreadView};

/// The lead's session as the AGENT light and the chat's banner see it.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Idle,
    Connecting,
    Connected(String),
    Failed(String),
}

/// What a buffer asks the agents to do, for the agent slot it shows (its
/// errors show there).
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// A session in the slot: a new one, or a saved one.
    Open { resume: Option<NativeId> },
    /// A prompt to the slot (its session opened first if it isn't
    /// running); `add`: into the turn it's running.
    Prompt { text: String, add: bool },
    /// An answer to an agent's request.
    Answer { thread: ThreadId, request: RequestId, response: RequestResponse },
    /// The harness's saved sessions, listed (to pick one).
    ListSaved { instance: InstanceId },
    DeleteSaved { instance: InstanceId, native: NativeId },
    SwitchSession { key: SessionKey },
    SetHarness { instance: InstanceId },
    /// A sub-slot on a harness (the chat that asked shows it).
    NewSlot { instance: InstanceId },
    CloseSlot,
    CloseSession { key: SessionKey },
    Stop { thread: ThreadId },
    Reprobe { instance: InstanceId },
    Steer { thread: ThreadId, input: SteerInput },
    Settings { policy: RuntimePolicy, model: Option<ModelSelection> },
    /// The project's tool servers (the slot's session restarts with them).
    ToolServers(Vec<McpServer>),
    SignIn { instance: InstanceId, method: String },
    FinishSignIn { instance: InstanceId, answer: String },
    SignOut { instance: InstanceId },
    StopSigningIn,
    PutNoticeAway,
    PutErrorAway,
    /// The slot's updates were seen (its chat is on screen).
    Seen,
}

/// Results of calls, taken once per frame (each says which slot it was for).
enum Reply {
    Opened(SlotId, NetResult<SlotOpened>),
    Sent(SlotId, NetResult<()>),
    SlotMade(SlotId, NetResult<SlotId>),
    /// A session was ended to start again with new tool servers.
    Restart { slot: SlotId, resume: Option<NativeId>, result: NetResult<()> },
    Saved(SlotId, NetResult<Vec<SavedSession>>),
    AuthStarted(SlotId, NetResult<AuthChallenge>),
    /// The tool servers every session gets; after a change, the slot whose
    /// session restarts with them.
    Tools { result: NetResult<ToolServers>, saved: Option<SlotId> },
    Done(SlotId, NetResult<()>),
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
    queued: BTreeMap<SlotId, String>,
    /// Something to tell the user once (a resume that opened fresh,
    /// reports delivered to the lead).
    notice: Option<String>,
    /// The saved sessions, once listed.
    saved: Option<Vec<SavedSession>>,
    /// The tool servers every session gets (ours and the project's), once
    /// asked for.
    tools: Option<ToolServers>,
    tools_asked: bool,
    /// A sign-in under way: the page to open and what to paste back.
    login: Option<AuthChallenge>,
    /// The sub-slots made, how many, and the last one with the slot that asked.
    made: (u64, Option<(SlotId, SlotId)>),
    mailbox: Mailbox,
}

impl Agents {
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Takes a server event (the shell, for every one).
    pub fn apply(&mut self, event: ClientEvent) {
        self.store.apply(event);
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

    pub fn login(&self) -> Option<&AuthChallenge> {
        self.login.as_ref()
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

    /// The lead's session (the AGENT light).
    pub fn lead_status(&self) -> SessionStatus {
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
        let instance = self.instance_in(&SlotId::new(LEAD))?;
        match &instance.status {
            ProviderStatus::Unavailable { reason } => Some(format!("{}: {reason}", instance.display_name)),
            ProviderStatus::Error { message } => Some(format!("{}: {message}", instance.display_name)),
            _ => None,
        }
    }

    /// How much the lead's session has done (the AGENT light blinks with it).
    pub fn lead_activity(&self) -> u64 {
        self.thread_in(&SlotId::new(LEAD)).map_or(0, |t| t.activity)
    }

    /// The agent's last message in `thread`, as text.
    pub fn last_said(&self, thread: &ThreadId) -> Option<String> {
        self.store.threads.get(thread)?.items.iter().rev().find_map(|item| match &item.body {
            ItemBody::AssistantMessage { content, .. } => Some(content.iter().map(block_summary).collect::<Vec<_>>().join(" ")),
            _ => None,
        })
    }

    /// Does what `slot`'s chat (or the inbox) asked.
    pub fn act(&mut self, slot: SlotId, act: Act, net: &NetClient, ctx: &egui::Context) {
        let mailbox = self.mailbox.clone();
        let for_slot = slot.clone();
        let done = move |r: NetResult<()>| post(&mailbox, Reply::Done(for_slot, r));
        match act {
            Act::Open { resume } => self.open(slot, net, ctx, resume),
            Act::Prompt { text, add } => self.prompt(slot, net, ctx, text, add),
            Act::Answer { thread, request, response } => {
                let mailbox = self.mailbox.clone();
                let slot = self.store.slots.values().find(|s| s.current_thread() == Some(&thread)).map_or(slot, |s| s.id.clone());
                net.respond(&thread, &request, &response, ctx.clone(), move |r| post(&mailbox, Reply::Sent(slot, r)));
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
            Act::Reprobe { instance } => net.refresh_instance(&instance, ctx.clone(), done),
            Act::Steer { thread, input } => net.steer(&thread, &input, ctx.clone(), done),
            Act::Settings { policy, model } => net.set_slot_settings(&slot, &policy, model.as_ref(), ctx.clone(), done),
            Act::ToolServers(project) => {
                let mailbox = self.mailbox.clone();
                net.set_tool_servers(&project, ctx.clone(), move |result| post(&mailbox, Reply::Tools { result, saved: Some(slot) }));
            }
            Act::SignIn { instance, method } => {
                let mailbox = self.mailbox.clone();
                net.begin_auth(&instance, &method, ctx.clone(), move |r| post(&mailbox, Reply::AuthStarted(slot, r)));
            }
            Act::FinishSignIn { instance, answer } => {
                net.complete_auth(&instance, &answer, ctx.clone(), done);
                self.login = None;
            }
            Act::SignOut { instance } => net.logout(&instance, ctx.clone(), done),
            Act::StopSigningIn => self.login = None,
            Act::PutNoticeAway => self.notice = None,
            Act::PutErrorAway => {
                self.errors.remove(&slot);
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
                    self.errors.insert(slot, "No harness instance is available yet.".into());
                    return;
                }
            },
        };
        if !self.opening.insert(slot.clone()) {
            return;
        }
        self.errors.remove(&slot);
        let body = OpenSlotSession { instance_id, resume, ..Default::default() };
        let mailbox = self.mailbox.clone();
        let for_slot = slot.clone();
        net.open_slot_session(&slot, &body, ctx.clone(), move |r| post(&mailbox, Reply::Opened(for_slot, r)));
    }

    fn prompt(&mut self, slot: SlotId, net: &NetClient, ctx: &egui::Context, text: String, add_to_running: bool) {
        self.errors.remove(&slot);
        if self.store.slots.get(&slot).is_none_or(|s| s.instance_id.is_none()) {
            self.queued.insert(slot.clone(), text);
            self.open(slot, net, ctx, None);
            return;
        }
        let mailbox = self.mailbox.clone();
        let for_slot = slot.clone();
        net.prompt_slot(&slot, &text, add_to_running, ctx.clone(), move |r| post(&mailbox, Reply::Sent(for_slot, r)));
    }

    /// Restarts `slot`'s session with the current tool servers (ACP takes
    /// them only when a session is created or loaded): ended, then resumed.
    fn reconnect(&mut self, slot: &SlotId, net: &NetClient, ctx: &egui::Context) {
        let Some(slot) = self.store.slots.get(slot).cloned() else { return };
        let Some(session) = slot.current_session().filter(|s| s.thread.is_some()).cloned() else { return };
        let mailbox = self.mailbox.clone();
        let (for_slot, resume) = (slot.id.clone(), session.native_id.clone());
        net.close_slot_session(&slot.id, &session.key, ctx.clone(), move |result| post(&mailbox, Reply::Restart { slot: for_slot, resume, result }));
    }

    /// Takes what the server answered, and asks for the tool servers once
    /// there's a harness (every frame, shown or not: a message sent from
    /// elsewhere waits on a session opening).
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        if !self.tools_asked && !self.store.instances.is_empty() {
            self.tools_asked = true;
            let mailbox = self.mailbox.clone();
            net.tool_servers(ctx.clone(), move |result| post(&mailbox, Reply::Tools { result, saved: None }));
        }
        let replies = std::mem::take(&mut *self.mailbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
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
                    if let Some(text) = self.queued.remove(&slot) {
                        self.prompt(slot, net, ctx, text, false);
                    }
                }
                Reply::Opened(slot, Err(e)) => {
                    self.opening.remove(&slot);
                    self.queued.remove(&slot);
                    self.errors.insert(slot, e.into());
                }
                Reply::Sent(slot, Err(e)) => {
                    self.errors.insert(slot, e.into());
                }
                Reply::SlotMade(from, Ok(made)) => self.made = (self.made.0 + 1, Some((from, made))),
                Reply::Restart { slot, resume, result: Ok(()) } => self.open(slot, net, ctx, resume),
                Reply::Saved(_, Ok(sessions)) => self.saved = Some(sessions),
                Reply::AuthStarted(slot, Ok(challenge)) => {
                    self.errors.remove(&slot);
                    // Nothing to open or paste: it's done (the harness checks again).
                    self.login = (challenge.url.is_some() || challenge.expects.is_some()).then_some(challenge);
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
                Reply::Saved(slot, Err(e)) | Reply::AuthStarted(slot, Err(e)) | Reply::Done(slot, Err(e)) | Reply::SlotMade(slot, Err(e)) | Reply::Restart { slot, result: Err(e), .. } => {
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

/// The desk's agents, kept by the shell: buffers get a reader.
#[derive(Default)]
pub struct AgentsCell(Rc<RefCell<Agents>>);

impl AgentsCell {
    pub fn get(&self) -> Ref<'_, Agents> {
        self.0.borrow()
    }

    /// To change them: the shell's alone (it holds the cell mutably).
    pub fn get_mut(&mut self) -> RefMut<'_, Agents> {
        self.0.borrow_mut()
    }

    /// A reader for a buffer.
    pub fn view(&self) -> AgentsView {
        AgentsView(self.0.clone())
    }
}

/// A buffer's reader of the desk's agents (it changes them only by asking).
#[derive(Clone, Default)]
pub struct AgentsView(Rc<RefCell<Agents>>);

impl AgentsView {
    pub fn get(&self) -> Ref<'_, Agents> {
        self.0.borrow()
    }

    /// Whether it reads `cell`.
    pub fn reads(&self, cell: &AgentsCell) -> bool {
        Rc::ptr_eq(&self.0, &cell.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::ProviderSnapshot;

    fn snapshot(status: ProviderStatus) -> ProviderSnapshot {
        ProviderSnapshot {
            instance_id: InstanceId::new("antigravity"),
            driver: harness_contracts::DriverKind::new("antigravity"),
            display_name: "Antigravity".into(),
            status,
            auth: harness_contracts::AuthState { status: harness_contracts::AuthStatus::Unknown, account: None, methods: Vec::new() },
            capabilities: Default::default(),
            models: Vec::new(),
            commands: Vec::new(),
            skills: Vec::new(),
            runtime_modes: Vec::new(),
            supports_plan_mode: false,
            tool_servers: Vec::new(),
            problems: Vec::new(),
        }
    }

    #[test]
    fn a_harness_that_cant_start_says_why_and_the_fix() {
        let mut agents = Agents::default();
        assert_eq!(agents.harness_problem(), None);
        let reason = "antigravity-acp isn't installed (fix: meta-harness pack install antigravity-acp)";
        agents.apply(ClientEvent::InstanceUpdated { snapshot: snapshot(ProviderStatus::Unavailable { reason: reason.into() }) });
        assert_eq!(agents.harness_problem().as_deref(), Some(format!("Antigravity: {reason}").as_str()), "the AGENT light's hint");
        agents.apply(ClientEvent::InstanceUpdated { snapshot: snapshot(ProviderStatus::Ready) });
        assert_eq!(agents.harness_problem(), None);
    }

    #[test]
    fn a_prompt_with_no_harness_anywhere_says_so() {
        let mut agents = Agents::default();
        let (ctx, net) = (egui::Context::default(), NetClient::new("http://127.0.0.1:9"));
        agents.act(SlotId::new(LEAD), Act::Prompt { text: "hello".into(), add: false }, &net, &ctx);
        assert!(matches!(agents.lead_status(), SessionStatus::Failed(e) if e.contains("No harness instance")));
        assert_eq!(agents.error(&SlotId::new(LEAD)).map(String::as_str), Some("No harness instance is available yet."), "said in the lead's chat");
    }

    #[test]
    fn a_reader_sees_what_the_shell_applies_and_cant_change_it() {
        let mut cell = AgentsCell::default();
        let view = cell.view();
        assert!(view.reads(&cell) && !AgentsView::default().reads(&cell));
        cell.get_mut().apply(ClientEvent::InstanceUpdated { snapshot: snapshot(ProviderStatus::Ready) });
        assert_eq!(view.get().store().instances.len(), 1);
    }
}
