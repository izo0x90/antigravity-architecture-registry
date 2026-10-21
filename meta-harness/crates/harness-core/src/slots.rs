//! Slots: seats for agents, over the session manager (which stays a plain
//! store of threads). A slot has a role, a harness and sessions, one of them
//! current; the lead slot always exists. Each session gets its own key in
//! its MCP address, so the server knows which slot an agent call comes from.
//!
//! What a slot is (role, name, harness, settings, its sessions' harness ids)
//! is kept by a `SlotStore`: written first, then held here, then announced.
//! Whether a session runs is live state (its thread), never stored. After a
//! restart slots come back with nothing running; a slot resumes on first use.
//!
//! Each slot's changes run one at a time (a lock per slot, held across the
//! harness calls); the shared map is only ever locked briefly, never across
//! an await.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use harness_contracts::{
    AdapterEvent, ClientEvent, InstanceId, ItemBody, McpServer, ModelSelection, NativeId, ProviderStatus, RuntimePolicy, SessionKey, Slot, SlotId,
    SlotSession, SlotStatus, Starter, ThreadId, ThreadStatus, TurnId, TurnInput, TurnItem, LEAD,
};

use crate::{EventHub, HarnessError, InstanceRegistry, OpenThread, Result, SessionManager};

/// Where slots are kept.
pub trait SlotStore: Send + Sync {
    /// Every slot, with no session running.
    fn load(&self) -> std::result::Result<Vec<Slot>, String>;
    /// Writes a slot (and its sessions) as it is now.
    fn save(&self, slot: &Slot) -> std::result::Result<(), String>;
    fn remove(&self, id: &SlotId) -> std::result::Result<(), String>;
    /// Keeps what a slot is told until one of its sessions runs.
    fn hold_report(&self, report: &HeldReport) -> std::result::Result<(), String>;
    /// What's held for slot `to`, oldest first, removing it.
    fn take_reports(&self, to: &SlotId) -> std::result::Result<Vec<HeldReport>, String>;
    /// Notes what happened, for looking back (never fails an operation).
    fn note(&self, actor: &str, action: &str, subject: &str, detail: Option<&str>);
}

/// What a slot is told (a sub-slot's report to the lead, a discussion's
/// news to the slot asked), held until one of its sessions runs.
#[derive(Debug, Clone, PartialEq)]
pub struct HeldReport {
    /// The slot told.
    pub to: SlotId,
    /// Who it's from, as the slot reads it (`sub 1 (sub-slot)`).
    pub from: String,
    pub text: String,
    pub at_ms: u64,
}

/// A new slot's harness and how its sessions run.
#[derive(Debug, Clone, Default)]
pub struct SlotSettings {
    pub instance: Option<InstanceId>,
    pub policy: RuntimePolicy,
    pub model: Option<ModelSelection>,
}

/// Which session to open.
#[derive(Debug, Clone)]
pub enum Open {
    /// A new one, or one the harness has saved (by its id).
    New { resume: Option<NativeId> },
    /// One of the slot's own, again (after a restart, or after it ended).
    Again(SessionKey),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub thread: ThreadId,
    pub session: SessionKey,
    /// Said when it didn't open as asked: a resume the harness refused,
    /// opened fresh instead.
    pub note: Option<String>,
    /// Reports from sub-slots delivered to it as it opened (the lead's).
    pub delivered_reports: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// After what it's doing (a new turn).
    Queue,
    /// Into the turn it's running, when its harness takes that.
    AddToRunning,
}

/// Who sends a message to a slot.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    User,
    Lead,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Sent {
    Started { turn: TurnId },
    Replied { turn: TurnId, text: String },
    StillWorking { turn: TurnId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reported {
    Delivered,
    Held,
}

/// A slot as an agent reads it: what it is, what it's doing, what was said.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotReport {
    pub slot: Slot,
    pub state: SlotState,
    /// (who, text), oldest first.
    pub messages: Vec<(String, String)>,
}

/// What a slot is doing, from its current session's thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    NotRunning,
    Starting,
    Idle,
    Running,
    WaitingOnUser,
    Failed,
}

impl SlotState {
    pub fn of(thread: Option<ThreadStatus>) -> Self {
        match thread {
            None => Self::NotRunning,
            Some(ThreadStatus::Starting) => Self::Starting,
            Some(ThreadStatus::Idle) => Self::Idle,
            Some(ThreadStatus::Running) => Self::Running,
            Some(ThreadStatus::WaitingForInput) => Self::WaitingOnUser,
            Some(ThreadStatus::Error | ThreadStatus::Closed) => Self::Failed,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::NotRunning => "not running",
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Running => "running",
            Self::WaitingOnUser => "waiting on the user",
            Self::Failed => "failed",
        }
    }
}

/// The caller behind an MCP key.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyHolder {
    pub slot: SlotId,
    pub role: String,
    pub name: String,
}

/// Builds the server's own MCP entry for a session, with its key.
pub type OurServer = Arc<dyn Fn(&str) -> McpServer + Send + Sync>;
/// The project's tool servers, read as each session opens.
pub type ProjectServers = Arc<dyn Fn() -> std::result::Result<Vec<McpServer>, String> + Send + Sync>;

#[derive(Default)]
struct Inner {
    slots: BTreeMap<SlotId, Slot>,
    /// MCP key → the slot and session it was made for.
    keys: HashMap<String, (SlotId, SessionKey)>,
    /// Running thread → its slot and session.
    threads: HashMap<ThreadId, (SlotId, SessionKey)>,
    /// Threads closed on purpose (not crashed).
    closing: HashSet<ThreadId>,
}

pub struct SlotManager {
    sessions: Arc<SessionManager>,
    registry: Arc<InstanceRegistry>,
    hub: Arc<EventHub>,
    store: Arc<dyn SlotStore>,
    ours: OurServer,
    project_servers: ProjectServers,
    inner: Mutex<Inner>,
    ops: Mutex<HashMap<SlotId, Arc<tokio::sync::Mutex<()>>>>,
    /// How long a harness may take to open a session: a slot isn't held
    /// forever by one that never answers.
    open_within: Duration,
    /// What a prompt hands an agent besides itself, set at start-up (the
    /// tags it names: `#name`); every prompt a slot is sent goes through it.
    expand: std::sync::OnceLock<Expand>,
}

/// A prompt as an agent is handed it (its text with what it names): given
/// a slot, only what that slot may read (what's passed on by an agent);
/// none, all of it (the user's own prompt).
pub type Expand = Arc<dyn Fn(&str, Option<&SlotId>) -> String + Send + Sync>;

fn store_error(reason: String) -> HarnessError {
    HarnessError::Store { reason }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl SlotManager {
    /// The slots as kept, the lead made on first run, worker slots from an
    /// earlier run removed (the queue starts its jobs afresh); then follows
    /// harnesses and threads to keep each slot's status and sessions true.
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        sessions: Arc<SessionManager>,
        registry: Arc<InstanceRegistry>,
        hub: Arc<EventHub>,
        store: Arc<dyn SlotStore>,
        ours: OurServer,
        project_servers: ProjectServers,
        open_within: Duration,
    ) -> Result<Arc<Self>> {
        let mut slots = BTreeMap::new();
        for mut slot in store.load().map_err(store_error)? {
            if slot.role == "worker" {
                store.remove(&slot.id).map_err(store_error)?;
                continue;
            }
            for s in &mut slot.sessions {
                s.thread = None;
            }
            slots.insert(slot.id.clone(), slot);
        }
        let lead = SlotId::new(LEAD);
        if let std::collections::btree_map::Entry::Vacant(place) = slots.entry(lead.clone()) {
            let slot = Slot {
                id: lead,
                role: LEAD.into(),
                name: LEAD.into(),
                instance_id: None,
                policy: RuntimePolicy::default(),
                model: None,
                sessions: Vec::new(),
                current: None,
                started_by: Starter::User,
                status: SlotStatus::Ready,
            };
            store.save(&slot).map_err(store_error)?;
            place.insert(slot);
        }
        let manager = Arc::new(Self {
            sessions,
            registry,
            hub,
            store,
            ours,
            project_servers,
            inner: Mutex::new(Inner { slots, ..Inner::default() }),
            ops: Mutex::default(),
            open_within,
            expand: std::sync::OnceLock::new(),
        });
        let all: Vec<SlotId> = manager.lock().slots.keys().cloned().collect();
        for id in all {
            manager.refresh_status(&id);
            if let Some(slot) = manager.get(&id) {
                manager.hub.publish(ClientEvent::SlotUpdated { slot });
            }
        }
        manager.follow();
        Ok(manager)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// This slot's lock: its changes run one at a time.
    async fn hold(&self, slot: &SlotId) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self.ops.lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(slot.clone()).or_default().clone();
        lock.lock_owned().await
    }

    pub fn lead(&self) -> SlotId {
        SlotId::new(LEAD)
    }

    pub fn list(&self) -> Vec<Slot> {
        self.lock().slots.values().cloned().collect()
    }

    /// The slot named `which`: by its id, or its name (none: no slot, or
    /// several of that name: they're told apart by their ids).
    pub fn named(&self, which: &str) -> Option<Slot> {
        let all = self.list();
        if let Some(slot) = all.iter().find(|s| s.id.as_str() == which) {
            return Some(slot.clone());
        }
        match all.iter().filter(|s| s.name == which).collect::<Vec<_>>().as_slice() {
            [one] => Some((*one).clone()),
            _ => None,
        }
    }

    pub fn get(&self, id: &SlotId) -> Option<Slot> {
        self.lock().slots.get(id).cloned()
    }

    fn slot(&self, id: &SlotId) -> Result<Slot> {
        self.get(id).ok_or_else(|| HarnessError::NotFound { what: format!("slot {id}") })
    }

    /// The slot and session a running thread belongs to.
    pub fn of_thread(&self, thread: &ThreadId) -> Option<(SlotId, SessionKey)> {
        self.lock().threads.get(thread).cloned()
    }

    /// Who holds an MCP key: a slot, while the session it was made for runs.
    pub fn holder(&self, key: &str) -> Option<KeyHolder> {
        let inner = self.lock();
        let (slot, _) = inner.keys.get(key)?;
        let s = inner.slots.get(slot)?;
        Some(KeyHolder { slot: slot.clone(), role: s.role.clone(), name: s.name.clone() })
    }

    /// Writes the slot, then holds it, then announces it.
    fn commit(&self, slot: Slot) -> Result<()> {
        self.store.save(&slot).map_err(store_error)?;
        self.lock().slots.insert(slot.id.clone(), slot.clone());
        self.hub.publish(ClientEvent::SlotUpdated { slot });
        Ok(())
    }

    /// A new slot (never a lead: there's one).
    pub async fn create(&self, role: &str, name: Option<String>, settings: SlotSettings, by: Starter) -> Result<SlotId> {
        if role == LEAD || role.trim().is_empty() {
            return Err(HarnessError::InvalidConfig { reason: format!("a new slot's role can't be {role:?}") });
        }
        let id = SlotId::new(format!("{role}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        let name = match name.filter(|n| !n.trim().is_empty()) {
            // What the user does is signed "user": no agent passes for them.
            // (Nor anything an agent reads as the user, or as no one.)
            Some(name) if ["user", "you", "the user", "someone"].iter().any(|u| name.trim().eq_ignore_ascii_case(u)) => {
                return Err(HarnessError::InvalidConfig { reason: format!("a slot can't be named {name:?}: that's the user") });
            }
            Some(name) => name,
            None => self.next_name(settings.instance.as_ref().map_or(role, |i| i.as_str())),
        };
        let slot = Slot {
            id: id.clone(),
            role: role.into(),
            name: name.clone(),
            instance_id: settings.instance,
            policy: settings.policy,
            model: settings.model,
            sessions: Vec::new(),
            current: None,
            started_by: by,
            status: SlotStatus::Ready,
        };
        let slot = Slot { status: self.status_of(slot.instance_id.as_ref()), ..slot };
        self.commit(slot)?;
        self.store.note(&by_word(by), "started slot", &name, Some(role));
        Ok(id)
    }

    fn next_name(&self, base: &str) -> String {
        let taken: HashSet<String> = self.lock().slots.values().map(|s| s.name.clone()).collect();
        (1..).map(|n| format!("{base} {n}")).find(|n| !taken.contains(n)).unwrap_or_else(|| base.to_owned())
    }

    /// Changes a slot's harness; only while none of its sessions runs.
    pub async fn set_harness(&self, id: &SlotId, instance: InstanceId) -> Result<()> {
        let _held = self.hold(id).await;
        let slot = self.slot(id)?;
        if slot.instance_id.as_ref() == Some(&instance) {
            return Ok(());
        }
        if slot.sessions.iter().any(|s| s.thread.is_some()) {
            return Err(HarnessError::InvalidConfig { reason: format!("{} has a session running: close its sessions first", slot.name) });
        }
        let status = self.status_of(Some(&instance));
        self.commit(Slot { instance_id: Some(instance), sessions: Vec::new(), current: None, status, ..slot })
    }

    /// Changes how a slot's next sessions run (mode, model).
    pub async fn set_settings(&self, id: &SlotId, policy: RuntimePolicy, model: Option<ModelSelection>) -> Result<()> {
        let _held = self.hold(id).await;
        let slot = self.slot(id)?;
        self.commit(Slot { policy, model, ..slot })
    }

    /// Opens a session in a slot (it becomes current).
    pub async fn open_session(&self, id: &SlotId, open: Open) -> Result<Opened> {
        let held = self.hold(id).await;
        let opened = self.open_held(id, open).await?;
        drop(held);
        let delivered_reports = self.deliver_held_reports(id, &opened.thread).await;
        Ok(Opened { delivered_reports, ..opened })
    }

    async fn open_held(&self, id: &SlotId, open: Open) -> Result<Opened> {
        let slot = self.slot(id)?;
        if let SlotStatus::Unavailable { reason, fix } = &slot.status {
            return Err(HarnessError::InvalidConfig { reason: format!("{}: {reason}{}", slot.name, fix.as_ref().map(|f| format!(" ({f})")).unwrap_or_default()) });
        }
        let instance = slot.instance_id.clone().ok_or_else(|| HarnessError::InvalidConfig { reason: format!("{} has no harness: choose one first", slot.name) })?;
        let (session_key, resume) = match &open {
            Open::New { resume } => (SessionKey::new(uuid::Uuid::new_v4().simple().to_string()), resume.clone()),
            Open::Again(key) => {
                let session = slot.session(key).ok_or_else(|| HarnessError::NotFound { what: format!("session {key} of {}", slot.name) })?;
                if let Some(thread) = &session.thread {
                    return Ok(Opened { thread: thread.clone(), session: key.clone(), note: None, delivered_reports: 0 });
                }
                (key.clone(), session.native_id.clone())
            }
        };
        let mut servers = (self.project_servers)().map_err(|reason| HarnessError::InvalidConfig { reason })?;
        // The key holds before the agent starts: its first call is already known.
        let secret = uuid::Uuid::new_v4().simple().to_string();
        self.lock().keys.insert(secret.clone(), (id.clone(), session_key.clone()));
        let ours = (self.ours)(&secret);
        servers.retain(|s| s.name != ours.name);
        servers.insert(0, ours);
        let request = |resume: Option<NativeId>| OpenThread { instance_id: instance.clone(), policy: slot.policy.clone(), model: slot.model.clone(), resume, mcp_servers: servers.clone() };
        let (thread, note) = match self.open_thread(request(resume.clone())).await {
            Ok(thread) => (thread, None),
            Err(e) if resume.is_some() && matches!(open, Open::Again(_)) => {
                let note = format!("{}'s session couldn't be resumed ({e}); a new one was opened", slot.name);
                match self.open_thread(request(None)).await {
                    Ok(thread) => (thread, Some(note)),
                    Err(e) => {
                        self.lock().keys.remove(&secret);
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                self.lock().keys.remove(&secret);
                return Err(e);
            }
        };
        let native_id = self.sessions.get(&thread).ok().and_then(|s| s.native_id());
        let mut next = slot.clone();
        match next.sessions.iter_mut().find(|s| s.key == session_key) {
            Some(s) => {
                s.thread = Some(thread.clone());
                s.native_id = native_id.or(s.native_id.take());
            }
            None => next.sessions.push(SlotSession { key: session_key.clone(), native_id, title: None, thread: Some(thread.clone()) }),
        }
        next.current = Some(session_key.clone());
        if let Err(e) = self.commit(next) {
            // Not kept: the session doesn't stay running unknown to the slot.
            self.lock().keys.remove(&secret);
            let _ = self.sessions.close(&thread).await;
            return Err(e);
        }
        self.lock().threads.insert(thread.clone(), (id.clone(), session_key.clone()));
        self.store.note("user", "opened session", &slot.name, note.as_deref());
        Ok(Opened { thread, session: session_key, note, delivered_reports: 0 })
    }

    /// Opens a thread, giving the harness `open_within` to answer.
    async fn open_thread(&self, request: OpenThread) -> Result<ThreadId> {
        match tokio::time::timeout(self.open_within, self.sessions.open(request)).await {
            Ok(opened) => opened,
            Err(_) => Err(HarnessError::Provider { reason: format!("the harness didn't open a session within {}s", self.open_within.as_secs()) }),
        }
    }

    /// Makes another of the slot's sessions current.
    pub async fn switch(&self, id: &SlotId, session: &SessionKey) -> Result<()> {
        let _held = self.hold(id).await;
        let slot = self.slot(id)?;
        if slot.session(session).is_none() {
            return Err(HarnessError::NotFound { what: format!("session {session} of {}", slot.name) });
        }
        self.commit(Slot { current: Some(session.clone()), ..slot })
    }

    /// Ends one of a slot's sessions and forgets it (the harness keeps its
    /// own saved copy, to resume as a saved session).
    pub async fn close_session(&self, id: &SlotId, session: &SessionKey) -> Result<()> {
        let _held = self.hold(id).await;
        let slot = self.slot(id)?;
        let entry = slot.session(session).cloned().ok_or_else(|| HarnessError::NotFound { what: format!("session {session} of {}", slot.name) })?;
        let mut next = slot.clone();
        next.sessions.retain(|s| &s.key != session);
        if next.current.as_ref() == Some(session) {
            next.current = next.sessions.last().map(|s| s.key.clone());
        }
        self.commit(next)?;
        if let Some(thread) = entry.thread {
            self.end_thread(&thread).await;
        }
        self.store.note("user", "closed session", &slot.name, None);
        Ok(())
    }

    /// Closes a slot and every session in it. The lead is never closed.
    pub async fn close(&self, id: &SlotId) -> Result<()> {
        let held = self.hold(id).await;
        let slot = self.slot(id)?;
        if slot.is_lead() {
            return Err(HarnessError::InvalidConfig { reason: "the lead slot stays: close its sessions instead".into() });
        }
        self.store.remove(id).map_err(store_error)?;
        self.lock().slots.remove(id);
        for thread in slot.sessions.iter().filter_map(|s| s.thread.clone()) {
            self.end_thread(&thread).await;
        }
        self.lock().keys.retain(|_, (s, _)| s != id);
        self.hub.publish(ClientEvent::SlotClosed { slot_id: id.clone() });
        drop(held);
        self.ops.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(id);
        self.store.note("user", "closed slot", &slot.name, None);
        Ok(())
    }

    /// Closes a running thread on purpose (so it isn't taken for a crash).
    async fn end_thread(&self, thread: &ThreadId) {
        {
            let mut inner = self.lock();
            inner.closing.insert(thread.clone());
            if let Some((slot, session)) = inner.threads.remove(thread) {
                inner.keys.retain(|_, held| held != &(slot.clone(), session.clone()));
            }
        }
        if let Err(e) = self.sessions.close(thread).await {
            tracing::warn!("closing thread {thread}: {e}");
        }
    }

    /// Whether a thread's end was asked for (and forgets that it was).
    pub fn closed_on_purpose(&self, thread: &ThreadId) -> bool {
        self.lock().closing.remove(thread)
    }

    /// The thread of the slot's current session, resuming it (or opening
    /// one) first if it isn't running.
    async fn running_thread(&self, id: &SlotId) -> Result<ThreadId> {
        let slot = self.slot(id)?;
        if let Some(thread) = slot.current_thread() {
            return Ok(thread.clone());
        }
        let open = match &slot.current {
            Some(key) => Open::Again(key.clone()),
            None => Open::New { resume: None },
        };
        Ok(self.open_session(id, open).await?.thread)
    }

    /// What every prompt is handed through from now on (`Expand`).
    pub fn expand_prompts_with(&self, expand: Expand) {
        let _ = self.expand.set(expand);
    }

    /// `text` as an agent is handed it (off the async threads: what it
    /// names is read where it's kept).
    pub async fn expanded(&self, text: String, readable_by: Option<SlotId>) -> String {
        let Some(expand) = self.expand.get().cloned() else { return text };
        let written = text.clone();
        tokio::task::spawn_blocking(move || expand(&text, readable_by.as_ref())).await.unwrap_or(written)
    }

    /// Sends a message to a slot (as its user, or as the lead). With `wait`,
    /// waits that long for the turn's answer.
    pub async fn send(&self, to: &SlotId, text: String, delivery: Delivery, from: Origin, wait: Option<Duration>) -> Result<Sent> {
        let thread = self.running_thread(to).await?;
        let session = self.sessions.get(&thread)?;
        let readable_by = match &from {
            Origin::User => None,
            Origin::Lead => Some(to.clone()),
        };
        let text = self.expanded(text, readable_by).await;
        let text = match &from {
            Origin::User => text,
            Origin::Lead => format!("From the lead ({}):\n\n{text}", self.slot(&self.lead()).map(|s| s.name).unwrap_or_else(|_| LEAD.into())),
        };
        // Listening first: a quick turn's end isn't missed.
        let mut events = session.events();
        let input = TurnInput { text, attachments: Vec::new() };
        let turn = match delivery {
            Delivery::Queue => session.start_turn(input).await?,
            Delivery::AddToRunning => session.inject(input).await?,
        };
        if from == Origin::Lead {
            self.store.note(LEAD, "sent", &self.slot(to).map(|s| s.name).unwrap_or_default(), None);
        }
        let Some(wait) = wait else {
            return Ok(Sent::Started { turn });
        };
        let mut items: Vec<TurnItem> = Vec::new();
        let ended = tokio::time::timeout(wait, async {
            while let Some(event) = events.next().await {
                match event {
                    AdapterEvent::ItemUpdated { item } if item.turn_id == turn => upsert(&mut items, item),
                    AdapterEvent::TurnUpdated { turn: t } if t.id == turn && t.status.is_terminal() => return true,
                    _ => {}
                }
            }
            false
        })
        .await;
        Ok(match ended {
            Ok(true) => Sent::Replied { text: turn_reply(&items, &turn), turn },
            _ => Sent::StillWorking { turn },
        })
    }

    /// A sub-slot reports to the lead (`tell`).
    pub async fn report(&self, from: &SlotId, text: String) -> Result<Reported> {
        let sender = self.slot(from)?;
        self.store.note(&sender.name, "reported", LEAD, None);
        self.tell(&self.lead(), &format!("{} (sub-slot)", sender.name), text, false).await
    }

    /// Slot `to` is told `text` (from `from`, as it reads it; `by_user`:
    /// the user said it, else what it names is handed only as far as `to`
    /// may read it): queued in its running session (never added to its
    /// running turn: that would cut off its work), else held, as it was
    /// told, until one of its sessions runs.
    pub async fn tell(&self, to: &SlotId, from: &str, text: String, by_user: bool) -> Result<Reported> {
        let slot = self.slot(to)?;
        let text = self.expanded(text, (!by_user).then(|| to.clone())).await;
        match slot.current_thread().and_then(|t| self.sessions.get(t).ok()) {
            Some(session) => {
                let prompt = format!("From {from}:\n\n{text}");
                session.start_turn(TurnInput { text: prompt, attachments: Vec::new() }).await?;
                Ok(Reported::Delivered)
            }
            None => {
                self.store.hold_report(&HeldReport { to: to.clone(), from: from.to_owned(), text, at_ms: now_ms() }).map_err(store_error)?;
                Ok(Reported::Held)
            }
        }
    }

    /// What's held for slot `id`, to its session (opened now).
    async fn deliver_held_reports(&self, id: &SlotId, thread: &ThreadId) -> usize {
        let reports = match self.store.take_reports(id) {
            Ok(reports) => reports,
            Err(e) => {
                tracing::warn!("what's held for {id} couldn't be read: {e}");
                return 0;
            }
        };
        if reports.is_empty() {
            return 0;
        }
        let mut each = Vec::new();
        for r in &reports {
            each.push(format!("From {}:\n\n{}", r.from, r.text));
        }
        let text = each.join("\n\n---\n\n");
        let sent = match self.sessions.get(thread) {
            Ok(session) => session.start_turn(TurnInput { text, attachments: Vec::new() }).await.map(|_| ()),
            Err(e) => Err(e),
        };
        if let Err(e) = sent {
            tracing::warn!("what's held for {id} couldn't be delivered ({e}); kept for its next session");
            for r in &reports {
                let _ = self.store.hold_report(r);
            }
            return 0;
        }
        reports.len()
    }

    /// A slot as an agent reads it, with its last `last` messages.
    pub fn read(&self, id: &SlotId, last: usize) -> Result<SlotReport> {
        let slot = self.slot(id)?;
        let Some(thread) = slot.current_thread().cloned() else {
            return Ok(SlotReport { slot, state: SlotState::NotRunning, messages: Vec::new() });
        };
        let mut status = None;
        let mut items: Vec<TurnItem> = Vec::new();
        for event in self.hub.thread_state(&thread) {
            match event {
                ClientEvent::Session { event: AdapterEvent::ThreadUpdated { thread } } => status = Some(thread.status),
                ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } } => upsert(&mut items, item),
                _ => {}
            }
        }
        items.sort_by_key(|i| i.ordinal);
        let mut messages: Vec<(String, String)> = items
            .iter()
            .filter_map(|i| match &i.body {
                ItemBody::UserMessage { .. } => Some(("prompt".to_owned(), i.body.said()?)),
                ItemBody::AssistantMessage { .. } => Some((slot.name.clone(), i.body.said()?)),
                _ => None,
            })
            .collect();
        let skip = messages.len().saturating_sub(last);
        messages.drain(..skip);
        Ok(SlotReport { state: SlotState::of(status), slot, messages })
    }

    /// Whether a harness can run, from its instance.
    fn status_of(&self, instance: Option<&InstanceId>) -> SlotStatus {
        let Some(instance) = instance else { return SlotStatus::Ready };
        let snapshot = self.registry.snapshots().into_iter().find(|s| &s.instance_id == instance);
        match snapshot.map(|s| s.status) {
            None => SlotStatus::Unavailable { reason: format!("the harness {instance} isn't set up"), fix: Some(format!("install its pack: meta-harness pack install {instance}")) },
            Some(ProviderStatus::Unavailable { reason }) => SlotStatus::Unavailable { reason, fix: None },
            Some(ProviderStatus::Error { message }) => SlotStatus::Unavailable { reason: message, fix: None },
            Some(ProviderStatus::Ready | ProviderStatus::Starting | ProviderStatus::NeedsAuth) => SlotStatus::Ready,
        }
    }

    /// Recomputes a slot's status; announces it if it changed.
    fn refresh_status(&self, id: &SlotId) {
        let Some(slot) = self.get(id) else { return };
        let status = self.status_of(slot.instance_id.as_ref());
        if status != slot.status {
            let slot = Slot { status, ..slot };
            self.lock().slots.insert(id.clone(), slot.clone());
            self.hub.publish(ClientEvent::SlotUpdated { slot });
        }
    }

    /// Keeps slots true to their harnesses and threads: a harness changing
    /// state, a thread ending (on purpose or not), a session's title.
    fn follow(self: &Arc<Self>) {
        let (_, mut live) = self.hub.subscribe();
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let event = match live.recv().await {
                    Ok(event) => event,
                    // Fell behind: everything it follows is re-read from the registry and threads.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let Some(me) = me.upgrade() else { return };
                        me.resync();
                        continue;
                    }
                    Err(_) => return,
                };
                let Some(me) = me.upgrade() else { return };
                me.apply(&event);
            }
        });
    }

    fn apply(&self, event: &ClientEvent) {
        match event {
            ClientEvent::InstanceUpdated { snapshot } => {
                let ids: Vec<SlotId> = self.lock().slots.values().filter(|s| s.instance_id.as_ref() == Some(&snapshot.instance_id)).map(|s| s.id.clone()).collect();
                for id in ids {
                    self.refresh_status(&id);
                }
            }
            ClientEvent::ThreadClosed { thread_id } => self.thread_gone(thread_id),
            ClientEvent::Session { event: AdapterEvent::ThreadUpdated { thread } } => {
                let Some((slot, key)) = self.of_thread(&thread.id) else { return };
                let Some(current) = self.get(&slot) else { return };
                let Some(session) = current.session(&key) else { return };
                if thread.title.is_some() && session.title != thread.title || thread.native_id.is_some() && session.native_id != thread.native_id {
                    let mut next = current.clone();
                    if let Some(s) = next.sessions.iter_mut().find(|s| s.key == key) {
                        s.title = thread.title.clone().or(s.title.take());
                        s.native_id = thread.native_id.clone().or(s.native_id.take());
                    }
                    if let Err(e) = self.commit(next) {
                        tracing::warn!("slot {slot}: a session's title or id couldn't be kept: {e}");
                    }
                }
            }
            _ => {}
        }
    }

    /// A thread ended: its session is no longer running.
    fn thread_gone(&self, thread: &ThreadId) {
        let Some((slot, key)) = self.lock().threads.remove(thread) else { return };
        self.lock().keys.retain(|_, held| held != &(slot.clone(), key.clone()));
        let Some(mut current) = self.get(&slot) else { return };
        if let Some(s) = current.sessions.iter_mut().find(|s| s.key == key) {
            s.thread = None;
        }
        self.lock().slots.insert(slot, current.clone());
        self.hub.publish(ClientEvent::SlotUpdated { slot: current });
    }

    /// After falling behind: threads that ended meanwhile, statuses that changed.
    fn resync(&self) {
        let running: Vec<ThreadId> = self.lock().threads.keys().cloned().collect();
        for thread in running {
            if self.sessions.get(&thread).is_err() {
                self.thread_gone(&thread);
            }
        }
        let ids: Vec<SlotId> = self.lock().slots.keys().cloned().collect();
        for id in ids {
            self.refresh_status(&id);
        }
    }
}

fn by_word(by: Starter) -> String {
    match by {
        Starter::User => "user",
        Starter::Lead => LEAD,
        Starter::Queue => "queue",
    }
    .to_owned()
}

fn upsert(items: &mut Vec<TurnItem>, item: TurnItem) {
    match items.iter_mut().find(|i| i.id == item.id) {
        Some(existing) => *existing = item,
        None => items.push(item),
    }
}

/// What a turn answered: its assistant messages, in order.
pub fn turn_reply(items: &[TurnItem], turn: &TurnId) -> String {
    let mut mine: Vec<&TurnItem> = items.iter().filter(|i| &i.turn_id == turn).collect();
    mine.sort_by_key(|i| i.ordinal);
    mine.iter().filter(|i| matches!(i.body, ItemBody::AssistantMessage { .. })).filter_map(|i| i.body.said()).collect::<Vec<_>>().join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::{ContentBlock, ItemId, ItemStatus};

    fn item(turn: &str, ordinal: u32, body: ItemBody) -> TurnItem {
        TurnItem { id: ItemId::new(format!("{turn}-{ordinal}")), thread_id: ThreadId::new("t"), turn_id: TurnId::new(turn), native_id: None, parent_id: None, ordinal, status: ItemStatus::Completed, title: None, body }
    }

    fn says(text: &str) -> ItemBody {
        ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: text.into() }], streaming: false }
    }

    #[test]
    fn a_turns_reply_is_its_assistant_messages_in_order() {
        let items = vec![
            item("a", 2, says("second")),
            item("a", 0, ItemBody::UserMessage { text: "asked".into() }),
            item("b", 1, says("another turn")),
            item("a", 1, says("first")),
        ];
        assert_eq!(turn_reply(&items, &TurnId::new("a")), "first\n\nsecond");
        assert_eq!(turn_reply(&items, &TurnId::new("c")), "");
    }

    #[test]
    fn a_slots_state_comes_from_its_thread() {
        assert_eq!(SlotState::of(None), SlotState::NotRunning);
        assert_eq!(SlotState::of(Some(ThreadStatus::WaitingForInput)).word(), "waiting on the user");
        assert_eq!(SlotState::of(Some(ThreadStatus::Error)), SlotState::Failed);
    }
}
