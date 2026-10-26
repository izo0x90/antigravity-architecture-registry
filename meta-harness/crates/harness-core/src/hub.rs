//! Fan-out of `ClientEvent`s to any number of clients, with a cache of the latest
//! state of every entity so a client that connects late gets a full replay.

use std::collections::HashMap;
use std::sync::Mutex;

use harness_contracts::{AdapterEvent, ClientEvent, InstanceId, NotificationId, NotificationState, SlotId, Stamped, TagId, TerminalId, ThreadId};
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Instance(InstanceId),
    Thread(ThreadId),
    Turn(String),
    Item(String),
    Request(String),
    Plan(String),
    /// A sub-agent of a thread's (its id is the harness's: two threads of
    /// one session share them).
    Subagent(ThreadId, String),
    Usage(ThreadId),
    /// The latest request to show part of the architecture graph.
    ArchitectureView,
    /// The latest state of the work queue over a plan.
    Queue(String),
    Slot(SlotId),
    Notification(NotificationId),
    Terminal(TerminalId),
    Tag(TagId),
    Discussion(harness_contracts::DiscussionId),
}

struct Entry {
    seq: u64,
    thread: Option<ThreadId>,
    /// The latest event of it, and when that happened.
    event: Stamped,
}

#[derive(Default)]
struct Cache {
    next_seq: u64,
    entries: HashMap<Key, Entry>,
}

impl Cache {
    fn apply(&mut self, event: &ClientEvent, at_ms: u64) {
        let (key, thread) = match event {
            ClientEvent::InstanceUpdated { snapshot } => (Key::Instance(snapshot.instance_id.clone()), None),
            // Said to one client by its stream, never kept.
            ClientEvent::Resync | ClientEvent::Replayed => return,
            ClientEvent::SlotUpdated { slot } => (Key::Slot(slot.id.clone()), None),
            ClientEvent::SlotClosed { slot_id } => {
                self.entries.remove(&Key::Slot(slot_id.clone()));
                return;
            }
            ClientEvent::TerminalUpdated { terminal } => (Key::Terminal(terminal.id.clone()), None),
            ClientEvent::TerminalClosed { terminal_id } => {
                self.entries.remove(&Key::Terminal(terminal_id.clone()));
                return;
            }
            // News, not state: a late client needs the notifications made of it.
            ClientEvent::TerminalNews { .. } => return,
            ClientEvent::TagUpdated { tag } => (Key::Tag(tag.id.clone()), None),
            ClientEvent::TagRemoved { tag_id } => {
                self.entries.remove(&Key::Tag(tag_id.clone()));
                return;
            }
            ClientEvent::DiscussionUpdated { discussion } => (Key::Discussion(discussion.id.clone()), None),
            ClientEvent::DiscussionRemoved { discussion_id } => {
                self.entries.remove(&Key::Discussion(discussion_id.clone()));
                return;
            }
            // Open ones only: a late client needs what's still open.
            ClientEvent::NotificationUpdated { notification } => {
                let key = Key::Notification(notification.id.clone());
                if notification.state != NotificationState::Open {
                    self.entries.remove(&key);
                    return;
                }
                (key, None)
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
                AdapterEvent::SubagentUpdated { subagent } => (Key::Subagent(subagent.thread_id.clone(), subagent.id.to_string()), Some(subagent.thread_id.clone())),
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
        self.entries.insert(key, Entry { seq, thread, event: Stamped { at_ms, event: event.clone() } });
    }

    fn replay(&self) -> Vec<Stamped> {
        let mut entries: Vec<&Entry> = self.entries.values().collect();
        entries.sort_by_key(|e| e.seq);
        entries.into_iter().map(|e| e.event.clone()).collect()
    }
}

/// `event`, as it happens now (what a stream says of itself: `Resync`,
/// `Replayed`).
pub fn now(event: ClientEvent) -> Stamped {
    Stamped { at_ms: crate::now_ms(), event }
}

pub struct EventHub {
    tx: broadcast::Sender<Stamped>,
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

    /// `event`, as it happens now.
    pub fn publish(&self, event: ClientEvent) {
        self.publish_at(event, crate::now_ms());
    }

    /// `event`, as it happened at `at_ms` (another server's, passed on).
    pub fn publish_at(&self, event: ClientEvent, at_ms: u64) {
        let mut cache = self.cache();
        cache.apply(&event, at_ms);
        let _ = self.tx.send(Stamped { at_ms, event });
    }

    /// Forgets everything (another server's state, about to be replayed
    /// again) and tells subscribers to drop what they hold.
    pub fn reset(&self) {
        let mut cache = self.cache();
        cache.entries.clear();
        let _ = self.tx.send(now(ClientEvent::Resync));
    }

    /// The latest state of one thread's entities (its turns, items,
    /// requests…), in first-seen order. Scans the cache: for occasional reads.
    pub fn thread_state(&self, thread: &ThreadId) -> Vec<ClientEvent> {
        let cache = self.cache();
        let mut entries: Vec<&Entry> = cache.entries.values().filter(|e| e.thread.as_ref() == Some(thread)).collect();
        entries.sort_by_key(|e| e.seq);
        entries.into_iter().map(|e| e.event.event.clone()).collect()
    }

    /// Current state of everything (each event of it with when it
    /// happened), plus a receiver for what happens next.
    pub fn subscribe(&self) -> (Vec<Stamped>, broadcast::Receiver<Stamped>) {
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
        let replay = || hub.subscribe().0.into_iter().map(|s| s.event).collect::<Vec<_>>();
        assert_eq!(replay(), vec![thread("a", ThreadStatus::Idle), thread("b", ThreadStatus::Starting)]);

        hub.publish(ClientEvent::ThreadClosed { thread_id: ThreadId::new("a") });
        assert_eq!(replay(), vec![thread("b", ThreadStatus::Starting)]);
    }

    #[test]
    fn a_late_client_gets_the_open_notifications_only() {
        let note = |id: &str, state| {
            let kind = harness_contracts::NotificationKind::Approval;
            let notification = harness_contracts::Notification { id: NotificationId::new(id), server: None, level: kind.level(), kind, slot: None, thread: None, request: None, terminal: None, discussion: None, title: id.into(), detail: None, at_ms: 0, state };
            ClientEvent::NotificationUpdated { notification }
        };
        let hub = EventHub::default();
        hub.publish(note("req:1", NotificationState::Open));
        hub.publish(note("req:2", NotificationState::Open));
        hub.publish(note("req:1", NotificationState::Done));
        let (replay, _) = hub.subscribe();
        assert_eq!(replay.into_iter().map(|s| s.event).collect::<Vec<_>>(), vec![note("req:2", NotificationState::Open)]);
    }

    /// Each event says when it happened, on the wire too; one passed on
    /// keeps its own time; a replayed one, when it last changed.
    #[test]
    fn each_event_says_when_it_happened() {
        let hub = EventHub::default();
        let (_, mut live) = hub.subscribe();
        hub.publish_at(thread("a", ThreadStatus::Idle), 1_000);
        let stamped = live.try_recv().unwrap();
        assert_eq!(stamped.at_ms, 1_000);
        let wire = serde_json::to_value(&stamped).unwrap();
        assert_eq!((wire["atMs"].as_u64(), wire["type"].as_str()), (Some(1_000), Some("session")));
        assert_eq!(serde_json::from_value::<Stamped>(wire).unwrap(), stamped, "read back");
        hub.publish(thread("b", ThreadStatus::Idle));
        let (replay, _) = hub.subscribe();
        assert_eq!(replay[0].at_ms, 1_000);
        assert!(replay[1].at_ms > 1_000);
    }
}
