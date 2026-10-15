//! The open notifications, kept: fed by the event stream, written to the
//! store first, then held, then published, then handed to the adapters.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, Weak};

use harness_contracts::{AdapterEvent, ClientEvent, InstanceId, Level, Notification, NotificationId, NotificationState, RequestStatus, SlotId, ThreadId, TurnId, TurnStatus};
use harness_core::EventHub;
use tokio::sync::broadcast::error::RecvError;

use crate::rules::{self, Change, SlotRef, World};
use crate::{Adapter, NotificationStore, NotifyError};

/// What only the running sessions know.
pub trait Live: Send + Sync {
    /// Whether the thread's end was asked for (and forgets that it was).
    fn closed_on_purpose(&self, thread: &ThreadId) -> bool;
}

/// A server with no sessions (a home with no project).
pub struct NoSessions;

impl Live for NoSessions {
    fn closed_on_purpose(&self, _: &ThreadId) -> bool {
        false
    }
}

impl Live for harness_core::SlotManager {
    fn closed_on_purpose(&self, thread: &ThreadId) -> bool {
        harness_core::SlotManager::closed_on_purpose(self, thread)
    }
}

#[derive(Default)]
struct Inner {
    open: BTreeMap<NotificationId, Notification>,
    /// The slots, as the stream last said.
    slots: HashMap<SlotId, (SlotRef, Option<InstanceId>)>,
    threads: HashMap<ThreadId, SlotId>,
    /// Turns seen running or queued (until they end).
    running: std::collections::HashSet<TurnId>,
}

impl Inner {
    fn slot_updated(&mut self, slot: &harness_contracts::Slot) {
        self.threads.retain(|_, s| s != &slot.id);
        for thread in slot.sessions.iter().filter_map(|s| s.thread.clone()) {
            self.threads.insert(thread, slot.id.clone());
        }
        self.slots.insert(slot.id.clone(), (SlotRef { id: slot.id.clone(), name: slot.name.clone(), lead: slot.is_lead() }, slot.instance_id.clone()));
    }

    fn slot_closed(&mut self, slot: &SlotId) {
        self.threads.retain(|_, s| s != slot);
        self.slots.remove(slot);
    }
}

pub struct NotifyHub {
    events: Arc<EventHub>,
    store: Arc<dyn NotificationStore>,
    live: Arc<dyn Live>,
    adapters: Vec<Arc<dyn Adapter>>,
    inner: Mutex<Inner>,
}

/// The world as the rules see it: the hub's own view of the slots, the
/// event cache for turns, the sessions for what they know.
struct Seen<'a> {
    hub: &'a NotifyHub,
    slots: &'a HashMap<SlotId, (SlotRef, Option<InstanceId>)>,
    threads: &'a HashMap<ThreadId, SlotId>,
    running: &'a std::collections::HashSet<TurnId>,
}

impl World for Seen<'_> {
    fn slot_of(&self, thread: &ThreadId) -> Option<SlotRef> {
        self.slots.get(self.threads.get(thread)?).map(|(s, _)| s.clone())
    }

    fn in_use(&self, instance: &InstanceId) -> bool {
        self.slots.values().any(|(_, i)| i.as_ref() == Some(instance))
    }

    fn closed_on_purpose(&self, thread: &ThreadId) -> bool {
        self.hub.live.closed_on_purpose(thread)
    }

    fn queued(&self, thread: &ThreadId) -> bool {
        self.hub.events.thread_state(thread).iter().any(|e| matches!(e, ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn } } if turn.status == TurnStatus::Queued))
    }

    fn was_running(&self, turn: &TurnId) -> bool {
        self.running.contains(turn)
    }

    fn reply(&self, thread: &ThreadId, turn: &TurnId) -> String {
        let items: Vec<_> = self
            .hub
            .events
            .thread_state(thread)
            .into_iter()
            .filter_map(|e| match e {
                ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } } => Some(item),
                _ => None,
            })
            .collect();
        harness_core::slots::turn_reply(&items, turn)
    }

    fn sign_ins(&self, instance: &InstanceId) -> u32 {
        self.hub.store.count(&format!("signin:{instance}:")).unwrap_or_else(|e| {
            tracing::warn!("notifications: sign-ins of {instance} couldn't be counted: {e}");
            0
        })
    }

    fn now_ms(&self) -> u64 {
        now_ms()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl NotifyHub {
    /// Loads what's open, catches up with the stream's current state, then
    /// follows it. (What waited on an earlier run was expired before this:
    /// see `NotificationStore::expire_waiting`.)
    pub fn start(events: Arc<EventHub>, store: Arc<dyn NotificationStore>, live: Arc<dyn Live>, adapters: Vec<Arc<dyn Adapter>>) -> Result<Arc<Self>, NotifyError> {
        let open = store.open().map_err(NotifyError::Store)?;
        let hub = Arc::new(Self { events, store, live, adapters, inner: Mutex::new(Inner { open: open.into_iter().map(|n| (n.id.clone(), n)).collect(), ..Default::default() }) });
        let (replay, live) = hub.events.subscribe();
        hub.rebuild(replay);
        Self::follow(Arc::downgrade(&hub), live);
        Ok(hub)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn follow(hub: Weak<Self>, mut live: tokio::sync::broadcast::Receiver<ClientEvent>) {
        tokio::spawn(async move {
            loop {
                let event = match live.recv().await {
                    Ok(event) => event,
                    // Fell behind: rebuilt from the stream's current state (never skipped).
                    Err(RecvError::Lagged(missed)) => {
                        let Some(hub) = hub.upgrade() else { return };
                        tracing::warn!("notifications fell {missed} events behind: rebuilding from the current state");
                        let (replay, next) = hub.events.subscribe();
                        live = next;
                        hub.rebuild(replay);
                        continue;
                    }
                    Err(RecvError::Closed) => return,
                };
                let Some(hub) = hub.upgrade() else { return };
                hub.handle(&event);
            }
        });
    }

    /// From the stream's current state: what it shows is noticed (repeats
    /// change nothing), and a request no longer pending is done.
    fn rebuild(&self, replay: Vec<ClientEvent>) {
        {
            let mut inner = self.lock();
            inner.slots.clear();
            inner.threads.clear();
        }
        for event in &replay {
            self.handle(event);
        }
        let pending: Vec<NotificationId> = replay
            .iter()
            .filter_map(|e| match e {
                ClientEvent::Session { event: AdapterEvent::RequestUpdated { request } } if request.status == RequestStatus::Pending => Some(NotificationId::request(&request.id)),
                _ => None,
            })
            .collect();
        let gone: Vec<NotificationId> = self.lock().open.values().filter(|n| n.request.is_some() && !pending.contains(&n.id)).map(|n| n.id.clone()).collect();
        for id in gone {
            self.apply(Change::End { id, to: NotificationState::Done });
        }
    }

    fn handle(&self, event: &ClientEvent) {
        let changes = {
            let mut inner = self.lock();
            match event {
                ClientEvent::SlotUpdated { slot } => inner.slot_updated(slot),
                ClientEvent::SlotClosed { slot_id } => inner.slot_closed(slot_id),
                ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn } } if !turn.status.is_terminal() => {
                    inner.running.insert(turn.id.clone());
                }
                _ => {}
            }
            let Inner { open, slots, threads, running } = &*inner;
            let changes = rules::derive(event, &Seen { hub: self, slots, threads, running }, open);
            if let ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn } } = event
                && turn.status.is_terminal()
            {
                inner.running.remove(&turn.id);
            }
            changes
        };
        // A thread's end is forgotten once noticed.
        if let ClientEvent::ThreadClosed { thread_id } = event {
            self.lock().threads.remove(thread_id);
        }
        for change in changes {
            self.apply(change);
        }
    }

    /// Kept, then held, then published, then handed on. A failure to keep it
    /// is said and changes nothing else.
    fn apply(&self, change: Change) -> Option<Notification> {
        let next = match change {
            Change::Open(n) => match self.store.get(&n.id) {
                Ok(Some(_)) => return None,
                Ok(None) => n,
                Err(e) => {
                    tracing::warn!("notification {} couldn't be read: {e}", n.id);
                    return None;
                }
            },
            Change::End { id, to } => {
                let open = self.lock().open.get(&id).cloned()?;
                match open.moved(to) {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::warn!("{e}");
                        return None;
                    }
                }
            }
        };
        self.keep(vec![next]).map_err(|e| tracing::warn!("{e}")).ok()?.pop()
    }

    fn keep(&self, changed: Vec<Notification>) -> Result<Vec<Notification>, NotifyError> {
        if changed.is_empty() {
            return Ok(changed);
        }
        self.store.put(&changed).map_err(NotifyError::Store)?;
        {
            let mut inner = self.lock();
            for n in &changed {
                if n.state == NotificationState::Open {
                    inner.open.insert(n.id.clone(), n.clone());
                } else {
                    inner.open.remove(&n.id);
                }
            }
        }
        for n in &changed {
            self.events.publish(ClientEvent::NotificationUpdated { notification: n.clone() });
            for adapter in &self.adapters {
                adapter.deliver(n);
            }
        }
        Ok(changed)
    }

    /// The open ones, in the order they're gone through.
    pub fn open(&self) -> Vec<Notification> {
        let mut all: Vec<Notification> = self.lock().open.values().cloned().collect();
        all.sort_by(harness_contracts::order);
        all
    }

    /// The latest, in any state, newest first.
    pub fn recent(&self, limit: usize) -> Result<Vec<Notification>, NotifyError> {
        self.store.recent(limit).map_err(NotifyError::Store)
    }

    /// The user put it away.
    pub fn dismiss(&self, id: &NotificationId) -> Result<Notification, NotifyError> {
        let current = match self.lock().open.get(id).cloned() {
            Some(open) => open,
            None => self.store.get(id).map_err(NotifyError::Store)?.ok_or_else(|| NotifyError::NotFound(id.clone()))?,
        };
        let next = current.moved(NotificationState::Dismissed)?;
        if next == current {
            return Ok(next);
        }
        Ok(self.keep(vec![next])?.remove(0))
    }

    /// The user saw `slot`'s chat: its updates are dismissed. Returns how many.
    pub fn seen(&self, slot: &SlotId) -> Result<usize, NotifyError> {
        let seen: Vec<Notification> = self
            .lock()
            .open
            .values()
            .filter(|n| n.level == Level::Update && n.slot.as_ref() == Some(slot))
            .map(|n| n.moved(NotificationState::Dismissed))
            .collect::<Result<_, _>>()?;
        Ok(self.keep(seen)?.len())
    }

    /// A server home links to stopped answering: said once until it's back
    /// (each time is a new one, numbered).
    pub fn server_lost(&self, server: &str, reason: &str) -> Result<(), NotifyError> {
        let prefix = format!("lost:{server}:");
        if self.lock().open.keys().any(|id| id.as_str().starts_with(&prefix)) {
            return Ok(());
        }
        let time = self.store.count(&prefix).map_err(NotifyError::Store)? + 1;
        self.keep(vec![rules::server_lost(server, reason, time, now_ms())])?;
        Ok(())
    }

    /// It answers again (or the user disconnected it): its lost one is done.
    pub fn server_back(&self, server: &str) -> Result<(), NotifyError> {
        let prefix = format!("lost:{server}:");
        let back: Vec<Notification> = self.lock().open.values().filter(|n| n.id.as_str().starts_with(&prefix)).map(|n| n.moved(NotificationState::Done)).collect::<Result<_, _>>()?;
        self.keep(back)?;
        Ok(())
    }

    /// Another server's notification (its own rules made it): handed to
    /// this one's adapters, never through the rules again.
    pub fn relay(&self, notification: &Notification) {
        for adapter in &self.adapters {
            adapter.deliver(notification);
        }
    }

    /// An agent tells the user something (a sub-slot's report, too).
    pub fn message(&self, from: Option<&SlotId>, text: &str) -> Result<Notification, NotifyError> {
        let n = {
            let inner = self.lock();
            let world = Seen { hub: self, slots: &inner.slots, threads: &inner.threads, running: &inner.running };
            let slot = from.and_then(|id| inner.slots.get(id)).map(|(s, _)| s.clone());
            rules::message(slot.as_ref(), text, &world)
        };
        Ok(self.keep(vec![n])?.remove(0))
    }
}
