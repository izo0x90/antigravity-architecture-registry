//! Client-side state: the latest of every entity the server streams. Events are
//! whole-entity upserts, so applying one is "replace by id".

use std::collections::BTreeMap;

use harness_contracts::{
    AdapterEvent, ClientEvent, InstanceId, PlanArtifact, ProviderSnapshot, RequestId, RuntimeRequest, Thread, ThreadId,
    Turn, TurnId, TurnItem, Usage, Slot, SlotId, Notification, NotificationId, NotificationState,
};

#[derive(Debug, Clone)]
pub struct ThreadView {
    pub thread: Thread,
    pub turns: Vec<Turn>,
    pub items: Vec<TurnItem>,
    pub requests: Vec<RuntimeRequest>,
    pub plan: Option<PlanArtifact>,
    pub usage: Option<Usage>,
    /// Events applied to this thread; drives the AGENT activity LED.
    pub activity: u64,
}

impl ThreadView {
    fn new(thread: Thread) -> Self {
        Self { thread, turns: Vec::new(), items: Vec::new(), requests: Vec::new(), plan: None, usage: None, activity: 0 }
    }

    pub fn request(&self, id: &RequestId) -> Option<&RuntimeRequest> {
        self.requests.iter().find(|r| &r.id == id)
    }

    pub fn items_of(&self, turn: &TurnId) -> impl Iterator<Item = &TurnItem> {
        self.items.iter().filter(move |i| &i.turn_id == turn)
    }

    pub fn running_turn(&self) -> Option<&Turn> {
        self.turns.iter().rev().find(|t| !t.status.is_terminal())
    }
}

fn upsert<T>(list: &mut Vec<T>, value: T, same: impl Fn(&T, &T) -> bool) {
    match list.iter_mut().find(|existing| same(existing, &value)) {
        Some(existing) => *existing = value,
        None => list.push(value),
    }
}

#[derive(Debug, Default)]
pub struct Store {
    pub instances: BTreeMap<InstanceId, ProviderSnapshot>,
    pub threads: BTreeMap<ThreadId, ThreadView>,
    pub slots: BTreeMap<SlotId, Slot>,
    /// The open notifications (an ended one leaves).
    pub notifications: BTreeMap<NotificationId, Notification>,
}

impl Store {
    pub fn apply(&mut self, event: ClientEvent) {
        match event {
            ClientEvent::InstanceUpdated { snapshot } => {
                self.instances.insert(snapshot.instance_id.clone(), snapshot);
            }
            ClientEvent::ThreadClosed { thread_id } => {
                self.threads.remove(&thread_id);
            }
            ClientEvent::Session { event } => self.apply_session(event),
            // The whole state follows: start from nothing.
            ClientEvent::Resync => *self = Self::default(),
            ClientEvent::SlotUpdated { slot } => {
                self.slots.insert(slot.id.clone(), slot);
            }
            ClientEvent::SlotClosed { slot_id } => {
                self.slots.remove(&slot_id);
            }
            ClientEvent::NotificationUpdated { notification } => {
                if notification.state == NotificationState::Open {
                    self.notifications.insert(notification.id.clone(), notification);
                } else {
                    self.notifications.remove(&notification.id);
                }
            }
            // Architecture events go to the graph views, not the session store.
            ClientEvent::Architecture { .. } => {}
        }
    }

    fn view(&mut self, id: &ThreadId) -> Option<&mut ThreadView> {
        let view = self.threads.get_mut(id)?;
        view.activity += 1;
        Some(view)
    }

    fn apply_session(&mut self, event: AdapterEvent) {
        match event {
            AdapterEvent::ThreadUpdated { thread } => match self.threads.get_mut(&thread.id) {
                Some(view) => {
                    view.activity += 1;
                    view.thread = thread;
                }
                None => {
                    self.threads.insert(thread.id.clone(), ThreadView::new(thread));
                }
            },
            AdapterEvent::TurnUpdated { turn } => {
                if let Some(view) = self.view(&turn.thread_id.clone()) {
                    upsert(&mut view.turns, turn, |a, b| a.id == b.id);
                }
            }
            AdapterEvent::ItemUpdated { item } => {
                if let Some(view) = self.view(&item.thread_id.clone()) {
                    upsert(&mut view.items, item, |a, b| a.id == b.id);
                }
            }
            AdapterEvent::RequestUpdated { request } => {
                if let Some(view) = self.view(&request.thread_id.clone()) {
                    upsert(&mut view.requests, request, |a, b| a.id == b.id);
                }
            }
            AdapterEvent::PlanUpdated { plan } => {
                if let Some(view) = self.view(&plan.thread_id.clone()) {
                    view.plan = Some(plan);
                }
            }
            AdapterEvent::UsageUpdated { thread_id, usage, .. } => {
                if let Some(view) = self.view(&thread_id) {
                    view.usage = Some(usage);
                }
            }
            // Subagents render through their item; catalogs arrive via instance snapshots.
            AdapterEvent::SubagentUpdated { .. } | AdapterEvent::CatalogUpdated { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::*;

    fn thread(status: ThreadStatus) -> ClientEvent {
        ClientEvent::Session {
            event: AdapterEvent::ThreadUpdated {
                thread: Thread {
                    id: ThreadId::new("t"),
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

    fn item(id: &str, text: &str) -> ClientEvent {
        ClientEvent::Session {
            event: AdapterEvent::ItemUpdated {
                item: TurnItem {
                    id: ItemId::new(id),
                    thread_id: ThreadId::new("t"),
                    turn_id: TurnId::new("turn"),
                    native_id: None,
                    parent_id: None,
                    ordinal: 0,
                    status: ItemStatus::InProgress,
                    title: None,
                    body: ItemBody::Reasoning { text: text.into(), streaming: true },
                },
            },
        }
    }

    #[test]
    fn upserts_replace_by_id_and_close_removes_thread() {
        let mut store = Store::default();
        store.apply(thread(ThreadStatus::Starting));
        store.apply(item("a", "one"));
        store.apply(item("a", "one two"));
        store.apply(item("b", "x"));
        store.apply(thread(ThreadStatus::Idle));
        let view = &store.threads[&ThreadId::new("t")];
        assert_eq!(view.thread.status, ThreadStatus::Idle);
        assert_eq!(view.items.len(), 2);
        assert!(matches!(&view.items[0].body, ItemBody::Reasoning { text, .. } if text == "one two"));

        store.apply(ClientEvent::ThreadClosed { thread_id: ThreadId::new("t") });
        assert!(store.threads.is_empty());
    }

    #[test]
    fn events_for_unknown_threads_are_ignored() {
        let mut store = Store::default();
        store.apply(item("a", "orphan"));
        assert!(store.threads.is_empty());
    }
}
