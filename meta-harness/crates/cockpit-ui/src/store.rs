//! Client-side state: the latest of every entity the server streams. Events are
//! whole-entity upserts, so applying one is "replace by id".

use std::collections::{BTreeMap, HashMap, VecDeque};

use harness_contracts::{
    AdapterEvent, ClientEvent, InstanceId, ItemId, PlanArtifact, ProviderSnapshot, RequestId, RequestStatus, RuntimeRequest, Thread, ThreadId,
    Turn, TurnId, TurnItem, TurnStatus, Usage, Slot, SlotId, Notification, NotificationId, NotificationState,
};

/// The agent's updates kept per thread (each tenth of a second once): a
/// minute's at the most it can send.
const ARRIVALS_KEPT: usize = 600;

/// How long after a resync what comes is the state replayed (what's the
/// same as before isn't news).
const REPLAY_S: f64 = 10.0;

#[derive(Debug, Clone)]
pub struct ThreadView {
    pub thread: Thread,
    turns: Vec<Turn>,
    /// Its items as they came (`stream` has them in the order shown), and
    /// as of which event each last changed.
    items: Vec<TurnItem>,
    changed: Vec<u64>,
    pub requests: Vec<RuntimeRequest>,
    pub plan: Option<PlanArtifact>,
    pub usage: Option<Usage>,
    /// Events applied to this thread; drives the AGENT activity LED.
    pub activity: u64,
    /// The items in the order shown (by their turn's place, then as they
    /// came), each with its turn's place: places in `items`. An item whose
    /// turn hasn't come yet waits for it.
    stream: Vec<(usize, usize)>,
    /// Where each item and turn is (in `items`, `turns`).
    item_at: HashMap<ItemId, usize>,
    turn_at: HashMap<TurnId, usize>,
    /// When the agent's updates came (the clock's seconds), oldest first:
    /// each tenth of a second that had any, and how many. An update is
    /// news from it: a turn, an item, its plan or usage changed, a
    /// question asked.
    arrivals: VecDeque<(f64, u32)>,
}

impl ThreadView {
    fn new(thread: Thread) -> Self {
        Self { thread, turns: Vec::new(), items: Vec::new(), requests: Vec::new(), plan: None, usage: None, activity: 0, changed: Vec::new(), stream: Vec::new(), item_at: HashMap::new(), turn_at: HashMap::new(), arrivals: VecDeque::new() }
    }

    pub fn request(&self, id: &RequestId) -> Option<&RuntimeRequest> {
        self.requests.iter().find(|r| &r.id == id)
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    pub fn item(&self, id: &ItemId) -> Option<&TurnItem> {
        self.item_at.get(id).map(|i| &self.items[*i])
    }

    pub fn turn(&self, id: &TurnId) -> Option<&Turn> {
        self.turn_at.get(id).map(|i| &self.turns[*i])
    }

    /// Its items in the order shown.
    pub fn stream(&self) -> impl DoubleEndedIterator<Item = &TurnItem> + ExactSizeIterator + Clone {
        self.stream.iter().map(|(_, i)| &self.items[*i])
    }

    /// The item at `at` in the order shown.
    pub fn in_stream(&self, at: usize) -> Option<&TurnItem> {
        self.stream.get(at).map(|(_, i)| &self.items[*i])
    }

    /// As of which event the item at `at` (in the order shown) last
    /// changed.
    pub fn changed(&self, at: usize) -> u64 {
        self.stream.get(at).map_or(0, |(_, i)| self.changed[*i])
    }

    /// The items of `turn`, in order.
    pub fn items_of(&self, turn: &TurnId) -> impl DoubleEndedIterator<Item = &TurnItem> {
        let span = self.turn_at.get(turn).map_or(0..0, |p| self.stream.partition_point(|(t, _)| t < p)..self.stream.partition_point(|(t, _)| t <= p));
        self.stream[span].iter().map(|(_, i)| &self.items[*i])
    }

    /// The last turn not ended (queued or running).
    pub fn running_turn(&self) -> Option<&Turn> {
        self.turns.iter().rev().find(|t| !t.status.is_terminal())
    }

    /// The turn running now (not one queued after it).
    pub fn running(&self) -> Option<&Turn> {
        self.turns.iter().rev().find(|t| t.status == TurnStatus::Running)
    }

    /// When the agent's updates came, oldest first (each tenth of a second
    /// once, with how many).
    pub fn arrivals(&self) -> &VecDeque<(f64, u32)> {
        &self.arrivals
    }

    /// An update from the agent came at `now`.
    fn arrived(&mut self, now: f64) {
        match self.arrivals.back_mut() {
            Some((at, n)) if (*at * 10.0).floor() == (now * 10.0).floor() => *n += 1,
            _ => self.arrivals.push_back((now, 1)),
        }
        if self.arrivals.len() > ARRIVALS_KEPT {
            self.arrivals.pop_front();
        }
    }

    /// `item` put in (replacing the one with its id): whether it changed.
    fn put_item(&mut self, item: TurnItem) -> bool {
        if let Some(at) = self.item_at.get(&item.id) {
            let changed = self.items[*at] != item;
            if changed {
                self.changed[*at] = self.activity;
            }
            self.items[*at] = item;
            return changed;
        }
        let at = self.items.len();
        self.item_at.insert(item.id.clone(), at);
        self.changed.push(self.activity);
        let turn = self.turn_at.get(&item.turn_id).copied();
        self.items.push(item);
        if let Some(turn) = turn {
            self.place(turn, at);
        }
        true
    }

    /// `turn` put in (replacing the one with its id): whether it changed.
    /// A new one takes the items that came before it.
    fn put_turn(&mut self, turn: Turn) -> bool {
        if let Some(at) = self.turn_at.get(&turn.id) {
            let changed = self.turns[*at] != turn;
            self.turns[*at] = turn;
            return changed;
        }
        let at = self.turns.len();
        self.turn_at.insert(turn.id.clone(), at);
        let waiting: Vec<usize> = self.items.iter().enumerate().filter(|(_, i)| i.turn_id == turn.id).map(|(n, _)| n).collect();
        self.turns.push(turn);
        for item in waiting {
            self.place(at, item);
        }
        true
    }

    /// Item `item` (of the turn at `turn`) into the order shown: a
    /// sub-agent's (its parent shown, in its turn) after its parent's
    /// others; else after its turn's others.
    fn place(&mut self, turn: usize, item: usize) {
        let parent = self.items[item].parent_id.as_ref().and_then(|p| self.stream.iter().rposition(|(t, i)| *t == turn && self.items[*i].id == *p));
        let at = match parent {
            Some(parent) => {
                let id = &self.items[self.stream[parent].1].id;
                parent + 1 + self.stream[parent + 1..].iter().take_while(|(_, i)| self.descends(*i, id)).count()
            }
            None => self.stream.partition_point(|(t, _)| *t <= turn),
        };
        self.stream.insert(at, (turn, item));
    }

    /// Whether item `item` (its place in `items`) is `ancestor`'s, at any
    /// depth.
    fn descends(&self, item: usize, ancestor: &ItemId) -> bool {
        let mut at = self.items[item].parent_id.as_ref();
        while let Some(parent) = at {
            if parent == ancestor {
                return true;
            }
            at = self.item_at.get(parent).and_then(|i| self.items[*i].parent_id.as_ref());
        }
        false
    }
}

fn upsert<T: PartialEq>(list: &mut Vec<T>, value: T, same: impl Fn(&T, &T) -> bool) -> bool {
    match list.iter_mut().find(|existing| same(existing, &value)) {
        Some(existing) => std::mem::replace(existing, value) != *existing,
        None => {
            list.push(value);
            true
        }
    }
}

#[derive(Debug, Default)]
pub struct Store {
    pub instances: BTreeMap<InstanceId, ProviderSnapshot>,
    pub threads: BTreeMap<ThreadId, ThreadView>,
    pub slots: BTreeMap<SlotId, Slot>,
    /// The open notifications (an ended one leaves).
    pub notifications: BTreeMap<NotificationId, Notification>,
    /// Slots closed (a chat showing one goes back to the lead).
    pub closed: std::collections::BTreeSet<SlotId>,
    /// The threads as they were before the last resync, and when it came
    /// (what's replayed as it was isn't news from the agent).
    before: Option<(f64, BTreeMap<ThreadId, ThreadView>)>,
}

impl Store {
    /// Takes a server event that came at `now` (the clock's seconds).
    pub fn apply(&mut self, event: ClientEvent, now: f64) {
        match event {
            ClientEvent::InstanceUpdated { snapshot } => {
                self.instances.insert(snapshot.instance_id.clone(), snapshot);
            }
            ClientEvent::ThreadClosed { thread_id } => {
                self.threads.remove(&thread_id);
            }
            ClientEvent::Session { event } => self.apply_session(event, now),
            // The whole state follows: start from nothing (what was held
            // kept a while, to tell what's replayed).
            ClientEvent::Resync => {
                let threads = std::mem::take(&mut self.threads);
                *self = Self::default();
                self.before = Some((now, threads));
            }
            ClientEvent::SlotUpdated { slot } => {
                self.closed.remove(&slot.id);
                self.slots.insert(slot.id.clone(), slot);
            }
            ClientEvent::SlotClosed { slot_id } => {
                self.slots.remove(&slot_id);
                self.closed.insert(slot_id);
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
            // The desk's terminals and tags keep these.
            ClientEvent::TerminalUpdated { .. } | ClientEvent::TerminalClosed { .. } | ClientEvent::TerminalNews { .. } | ClientEvent::TagUpdated { .. } | ClientEvent::TagRemoved { .. } | ClientEvent::DiscussionUpdated { .. } | ClientEvent::DiscussionRemoved { .. } => {}
        }
    }

    fn view(&mut self, id: &ThreadId) -> Option<&mut ThreadView> {
        let view = self.threads.get_mut(id)?;
        view.activity += 1;
        Some(view)
    }

    /// Whether what `same` finds was held before a resync just now (it's
    /// replayed as it was).
    fn replayed(&mut self, thread: &ThreadId, now: f64, same: impl Fn(&ThreadView) -> bool) -> bool {
        if self.before.as_ref().is_some_and(|(at, _)| now - at > REPLAY_S) {
            self.before = None;
        }
        self.before.as_ref().and_then(|(_, before)| before.get(thread)).is_some_and(same)
    }

    /// A thread's event, at `now`: news from the agent (something of its
    /// changed, a question asked) is an update that arrived.
    fn apply_session(&mut self, event: AdapterEvent, now: f64) {
        match event {
            AdapterEvent::ThreadUpdated { thread } => match self.threads.get_mut(&thread.id) {
                Some(view) => {
                    view.activity += 1;
                    view.thread = thread;
                }
                // (Back after a resync: when the agent's updates came, kept.)
                None => {
                    let mut view = ThreadView::new(thread);
                    if let Some(before) = self.before.as_ref().and_then(|(_, b)| b.get(&view.thread.id)) {
                        view.arrivals = before.arrivals.clone();
                    }
                    self.threads.insert(view.thread.id.clone(), view);
                }
            },
            AdapterEvent::TurnUpdated { turn } => {
                // (Queued is your prompt waiting, not the agent's news.)
                let replayed = turn.status == TurnStatus::Queued || self.replayed(&turn.thread_id, now, |v| v.turn(&turn.id) == Some(&turn));
                if let Some(view) = self.view(&turn.thread_id.clone())
                    && view.put_turn(turn)
                    && !replayed
                {
                    view.arrived(now);
                }
            }
            AdapterEvent::ItemUpdated { item } => {
                let replayed = self.replayed(&item.thread_id, now, |v| v.item(&item.id) == Some(&item));
                if let Some(view) = self.view(&item.thread_id.clone())
                    && view.put_item(item)
                    && !replayed
                {
                    view.arrived(now);
                }
            }
            AdapterEvent::RequestUpdated { request } => {
                // Asked (an answer's echo isn't the agent's news).
                let asked = request.status == RequestStatus::Pending && !self.replayed(&request.thread_id, now, |v| v.request(&request.id) == Some(&request));
                if let Some(view) = self.view(&request.thread_id.clone())
                    && upsert(&mut view.requests, request, |a, b| a.id == b.id)
                    && asked
                {
                    view.arrived(now);
                }
            }
            AdapterEvent::PlanUpdated { plan } => {
                let replayed = self.replayed(&plan.thread_id, now, |v| v.plan.as_ref() == Some(&plan));
                if let Some(view) = self.view(&plan.thread_id.clone()) {
                    let news = view.plan.as_ref() != Some(&plan) && !replayed;
                    view.plan = Some(plan);
                    if news {
                        view.arrived(now);
                    }
                }
            }
            AdapterEvent::UsageUpdated { thread_id, usage, .. } => {
                let replayed = self.replayed(&thread_id, now, |v| v.usage.as_ref() == Some(&usage));
                if let Some(view) = self.view(&thread_id) {
                    let news = view.usage.as_ref() != Some(&usage) && !replayed;
                    view.usage = Some(usage);
                    if news {
                        view.arrived(now);
                    }
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
        store.apply(thread(ThreadStatus::Starting), 0.0);
        store.apply(item("a", "one"), 0.0);
        store.apply(item("a", "one two"), 0.0);
        store.apply(item("b", "x"), 0.0);
        store.apply(thread(ThreadStatus::Idle), 0.0);
        let view = &store.threads[&ThreadId::new("t")];
        assert_eq!(view.thread.status, ThreadStatus::Idle);
        assert_eq!(view.items.len(), 2);
        assert!(matches!(&view.items[0].body, ItemBody::Reasoning { text, .. } if text == "one two"));

        store.apply(ClientEvent::ThreadClosed { thread_id: ThreadId::new("t") }, 0.0);
        assert!(store.threads.is_empty());
    }

    fn turn(id: &str, ordinal: u32, status: TurnStatus) -> ClientEvent {
        ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new(id), thread_id: ThreadId::new("t"), ordinal, status, failure: None, usage: None } } }
    }

    fn in_turn(id: &str, turn: &str) -> ClientEvent {
        let ClientEvent::Session { event: AdapterEvent::ItemUpdated { mut item } } = item(id, id) else { unreachable!() };
        item.turn_id = TurnId::new(turn);
        ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } }
    }

    /// The stream is by turn, then as the items came: one that came late
    /// to an earlier turn goes under it, one before its turn waits for it.
    #[test]
    fn the_stream_is_in_the_order_shown() {
        let mut store = Store::default();
        store.apply(thread(ThreadStatus::Running), 0.0);
        store.apply(turn("one", 1, TurnStatus::Completed), 0.0);
        store.apply(in_turn("a", "one"), 0.0);
        store.apply(in_turn("c", "two"), 0.0);
        store.apply(turn("two", 2, TurnStatus::Running), 0.0);
        store.apply(in_turn("b", "one"), 0.0);
        store.apply(in_turn("d", "two"), 0.0);
        let view = &store.threads[&ThreadId::new("t")];
        assert_eq!(view.stream().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c", "d"]);
        assert_eq!(view.items_of(&TurnId::new("two")).map(|i| i.id.as_str()).collect::<Vec<_>>(), ["c", "d"]);
        assert_eq!(view.in_stream(1).map(|i| i.id.as_str()), Some("b"));
    }

    /// A sub-agent's items follow the call that started it (after its
    /// earlier ones, theirs too), whatever came between.
    #[test]
    fn a_sub_agents_items_follow_its_call() {
        let mut store = Store::default();
        store.apply(thread(ThreadStatus::Running), 0.0);
        store.apply(turn("one", 1, TurnStatus::Running), 0.0);
        let under = |id: &str, parent: &str| {
            let ClientEvent::Session { event: AdapterEvent::ItemUpdated { mut item } } = in_turn(id, "one") else { unreachable!() };
            item.parent_id = Some(ItemId::new(parent));
            ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } }
        };
        for event in [in_turn("task", "one"), in_turn("other", "one"), under("a", "task"), in_turn("mine", "one"), under("b", "task"), under("a1", "a"), under("c", "task")] {
            store.apply(event, 0.0);
        }
        let view = &store.threads[&ThreadId::new("t")];
        assert_eq!(view.stream().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["task", "a", "a1", "b", "c", "other", "mine"]);
    }

    /// News from the agent is an update that came (each tenth of a second
    /// once, counted); the same again, an answer's echo, a turn queued,
    /// what a resync replays as it was, and the thread's own changes
    /// aren't.
    #[test]
    fn what_came_from_the_agent_is_kept_with_its_thread() {
        let mut store = Store::default();
        let came = |store: &Store| store.threads[&ThreadId::new("t")].arrivals().iter().copied().collect::<Vec<_>>();
        store.apply(thread(ThreadStatus::Running), 1.0);
        store.apply(turn("one", 1, TurnStatus::Running), 1.0);
        store.apply(in_turn("a", "one"), 1.02);
        store.apply(in_turn("a", "one"), 2.0);
        store.apply(thread(ThreadStatus::Running), 3.0);
        store.apply(turn("two", 2, TurnStatus::Queued), 3.5);
        assert_eq!(came(&store), [(1.0, 2)], "two in one tenth; the same again, the thread's own and a turn queued aren't news");
        let mut request = RuntimeRequest { id: RequestId::new("r"), thread_id: ThreadId::new("t"), turn_id: Some(TurnId::new("one")), item_id: None, kind: RequestKind::Question, status: RequestStatus::Pending, title: "ok?".into(), detail: None, options: Vec::new(), questions: Vec::new(), response: None };
        store.apply(ClientEvent::Session { event: AdapterEvent::RequestUpdated { request: request.clone() } }, 4.0);
        request.status = RequestStatus::Resolved;
        store.apply(ClientEvent::Session { event: AdapterEvent::RequestUpdated { request } }, 5.0);
        assert_eq!(came(&store).len(), 2, "asked is news, its answer's echo isn't");
        store.apply(ClientEvent::Resync, 6.0);
        store.apply(thread(ThreadStatus::Running), 6.1);
        store.apply(turn("one", 1, TurnStatus::Running), 6.1);
        store.apply(in_turn("a", "one"), 6.1);
        assert_eq!(came(&store), [(1.0, 2), (4.0, 1)], "replayed as it was: what came before, kept");
        store.apply(in_turn("b", "one"), 6.2);
        assert_eq!(came(&store).last(), Some(&(6.2, 1)), "new after a resync is news");
    }

    #[test]
    fn events_for_unknown_threads_are_ignored() {
        let mut store = Store::default();
        store.apply(item("a", "orphan"), 0.0);
        assert!(store.threads.is_empty());
    }
}
