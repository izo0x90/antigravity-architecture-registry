//! Fan-out of `ClientEvent`s to any number of clients, with a cache of the latest
//! state of every entity so a client that connects late gets a full replay.

use std::collections::HashMap;
use std::sync::Mutex;

use harness_contracts::{AdapterEvent, ClientEvent, InstanceId, SlotId, ThreadId};
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Instance(InstanceId),
    Thread(ThreadId),
    Turn(String),
    Item(String),
    Request(String),
    Plan(String),
    Subagent(String),
    Usage(ThreadId),
    /// The latest request to show part of the architecture graph.
    ArchitectureView,
    /// The latest state of the work queue over a plan.
    Queue(String),
    Slot(SlotId),
}

struct Entry {
    seq: u64,
    thread: Option<ThreadId>,
    event: ClientEvent,
}

#[derive(Default)]
struct Cache {
    next_seq: u64,
    entries: HashMap<Key, Entry>,
}

impl Cache {
    fn apply(&mut self, event: &ClientEvent) {
        let (key, thread) = match event {
            ClientEvent::InstanceUpdated { snapshot } => (Key::Instance(snapshot.instance_id.clone()), None),
            // Said to one client by its stream, never kept.
            ClientEvent::Resync => return,
            ClientEvent::SlotUpdated { slot } => (Key::Slot(slot.id.clone()), None),
            ClientEvent::SlotClosed { slot_id } => {
                self.entries.remove(&Key::Slot(slot_id.clone()));
                return;
            }
            ClientEvent::ThreadClosed { thread_id } => {
                self.entries.retain(|_, e| e.thread.as_ref() != Some(thread_id));
                return;
            }
            ClientEvent::Architecture { event } => match event {
                // A late client should land where the agent last pointed.
                arch_graph::ArchEvent::ViewRequested { .. } => (Key::ArchitectureView, None),
                // ...and see where the work queue is.
                arch_graph::ArchEvent::QueueUpdated { snapshot } => (Key::Queue(snapshot.plan.to_string()), None),
                // Changes and checks are notifications; clients fetch current state.
                _ => return,
            },
            ClientEvent::Session { event } => match event {
                AdapterEvent::ThreadUpdated { thread } => (Key::Thread(thread.id.clone()), Some(thread.id.clone())),
                AdapterEvent::TurnUpdated { turn } => (Key::Turn(turn.id.to_string()), Some(turn.thread_id.clone())),
                AdapterEvent::ItemUpdated { item } => (Key::Item(item.id.to_string()), Some(item.thread_id.clone())),
                AdapterEvent::RequestUpdated { request } => (Key::Request(request.id.to_string()), Some(request.thread_id.clone())),
                AdapterEvent::PlanUpdated { plan } => (Key::Plan(plan.id.to_string()), Some(plan.thread_id.clone())),
                AdapterEvent::SubagentUpdated { subagent } => (Key::Subagent(subagent.id.to_string()), Some(subagent.thread_id.clone())),
                AdapterEvent::UsageUpdated { thread_id, .. } => (Key::Usage(thread_id.clone()), Some(thread_id.clone())),
                // Catalog changes reach clients through the instance snapshot.
                AdapterEvent::CatalogUpdated { .. } => return,
            },
        };
        let seq = match self.entries.get(&key) {
            Some(existing) => existing.seq,
            None => {
                self.next_seq += 1;
                self.next_seq
            }
        };
        self.entries.insert(key, Entry { seq, thread, event: event.clone() });
    }

    fn replay(&self) -> Vec<ClientEvent> {
        let mut entries: Vec<&Entry> = self.entries.values().collect();
        entries.sort_by_key(|e| e.seq);
        entries.into_iter().map(|e| e.event.clone()).collect()
    }
}

pub struct EventHub {
    tx: broadcast::Sender<ClientEvent>,
    cache: Mutex<Cache>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::with_capacity(4096)
    }
}

impl EventHub {
    /// A hub whose subscribers may fall up to `capacity` events behind
    /// before they must resync.
    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx, cache: Mutex::default() }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn publish(&self, event: ClientEvent) {
        let mut cache = self.cache();
        cache.apply(&event);
        let _ = self.tx.send(event);
    }

    /// The latest state of one thread's entities (its turns, items,
    /// requests…), in first-seen order. Scans the cache: for occasional reads.
    pub fn thread_state(&self, thread: &ThreadId) -> Vec<ClientEvent> {
        let cache = self.cache();
        let mut entries: Vec<&Entry> = cache.entries.values().filter(|e| e.thread.as_ref() == Some(thread)).collect();
        entries.sort_by_key(|e| e.seq);
        entries.into_iter().map(|e| e.event.clone()).collect()
    }

    /// Current state of everything, plus a receiver for what happens next.
    pub fn subscribe(&self) -> (Vec<ClientEvent>, broadcast::Receiver<ClientEvent>) {
        let cache = self.cache();
        (cache.replay(), self.tx.subscribe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::{RuntimePolicy, Thread, ThreadStatus};

    fn thread(id: &str, status: ThreadStatus) -> ClientEvent {
        ClientEvent::Session {
            event: AdapterEvent::ThreadUpdated {
                thread: Thread {
                    id: ThreadId::new(id),
                    instance_id: InstanceId::new("i"),
                    native_id: None,
                    title: None,
                    status,
                    policy: RuntimePolicy::default(),
                    model: None,
                    controls: Default::default(),
                    mcp_servers: Vec::new(),
                },
            },
        }
    }

    #[test]
    fn replay_keeps_latest_state_in_first_seen_order_and_drops_closed_threads() {
        let hub = EventHub::default();
        hub.publish(thread("a", ThreadStatus::Starting));
        hub.publish(thread("b", ThreadStatus::Starting));
        hub.publish(thread("a", ThreadStatus::Idle));
        let (replay, _) = hub.subscribe();
        assert_eq!(replay, vec![thread("a", ThreadStatus::Idle), thread("b", ThreadStatus::Starting)]);

        hub.publish(ClientEvent::ThreadClosed { thread_id: ThreadId::new("a") });
        let (replay, _) = hub.subscribe();
        assert_eq!(replay, vec![thread("b", ThreadStatus::Starting)]);
    }
}
