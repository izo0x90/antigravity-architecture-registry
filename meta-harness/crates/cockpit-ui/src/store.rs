//! Client-side state: the latest of every entity the server streams. Events are
//! whole-entity upserts, so applying one is "replace by id".

use std::collections::{BTreeMap, HashMap, VecDeque};

use harness_contracts::{
    AdapterEvent, ClientEvent, InstanceId, ItemId, PlanArtifact, ProviderSnapshot, RequestId, RequestStatus, RuntimeRequest, Thread, ThreadId,
    Turn, TurnId, TurnItem, TurnStatus, Usage, Slot, SlotId, Notification, NotificationId, NotificationState,
};

/// The agent's updates kept per thread (each tenth of a second once, the
/// newest): a minute of steady updates.
const ARRIVALS_KEPT: usize = 600;

/// Each thread view made, numbered (so what's worked out from one isn't
/// taken for another's, a resync's new view of the same thread).
static EPOCHS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct ThreadView {
    pub thread: Thread,
    turns: Vec<Turn>,
    /// Its items as they came (`stream` has them in the order shown), and
    /// as of which event each last changed.
    items: Vec<TurnItem>,
    changed: Vec<u64>,
    requests: Vec<RuntimeRequest>,
    request_at: HashMap<RequestId, usize>,
    pub plan: Option<PlanArtifact>,
    pub usage: Option<Usage>,
    /// Events applied to this thread (each changes what's shown).
    pub activity: u64,
    /// Which view of the thread this is (a new one after a resync), and
    /// as of which change its stream's shape is (an item put in, moved,
    /// or changed in kind, status, parent, turn or whether it has more).
    epoch: u64,
    shape: u64,
    /// How many updates came from the agent (its news): the AGENT light.
    pub news: u64,
    /// The items in the order shown (by their turn's place, then as they
    /// came), each with its turn's place: places in `items`. An item whose
    /// turn hasn't come yet waits for it.
    stream: Vec<(usize, usize)>,
    /// Where each item and turn is (in `items`, `turns`); each item's
    /// place in the order shown (none: its turn hasn't come); each
    /// request's item.
    item_at: HashMap<ItemId, usize>,
    turn_at: HashMap<TurnId, usize>,
    shown_at: Vec<Option<usize>>,
    request_items: HashMap<RequestId, usize>,
    /// When the agent's updates came (the clock's seconds), oldest first:
    /// each tenth of a second that had any, and how many. An update is
    /// news from it: a turn, an item, its plan or usage changed, a
    /// question asked.
    arrivals: VecDeque<(f64, u32)>,
}

impl ThreadView {
    fn new(thread: Thread) -> Self {
        let epoch = EPOCHS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { thread, turns: Vec::new(), items: Vec::new(), requests: Vec::new(), request_at: HashMap::new(), plan: None, usage: None, activity: 0, epoch, shape: 0, news: 0, changed: Vec::new(), stream: Vec::new(), item_at: HashMap::new(), turn_at: HashMap::new(), shown_at: Vec::new(), request_items: HashMap::new(), arrivals: VecDeque::new() }
    }

    pub fn request(&self, id: &RequestId) -> Option<&RuntimeRequest> {
        self.request_at.get(id).map(|i| &self.requests[*i])
    }

    /// Its requests, as they were asked.
    pub fn requests(&self) -> &[RuntimeRequest] {
        &self.requests
    }

    /// Which view of the thread this is, and as of which change its
    /// stream's shape is: what's worked out from the stream's shape is
    /// kept by them.
    pub fn shape_of(&self) -> (u64, u64) {
        (self.epoch, self.shape)
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

    /// Where item `id` is in the order shown.
    pub fn shown_at(&self, id: &ItemId) -> Option<usize> {
        self.item_at.get(id).and_then(|i| self.shown_at[*i])
    }

    /// Where request `id`'s item is in the order shown.
    pub fn request_shown_at(&self, id: &RequestId) -> Option<usize> {
        self.request_items.get(id).and_then(|i| self.shown_at[*i])
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
        self.news += 1;
        match self.arrivals.back_mut() {
            Some((at, n)) if (*at * 10.0).floor() == (now * 10.0).floor() => *n += 1,
            _ => self.arrivals.push_back((now, 1)),
        }
        if self.arrivals.len() > ARRIVALS_KEPT {
            self.arrivals.pop_front();
        }
    }

    /// `item` put in (replacing the one with its id): whether it changed.
    /// A new turn moves it there (a message the agent took into a turn of
    /// its own).
    fn put_item(&mut self, item: TurnItem) -> bool {
        if let Some(&at) = self.item_at.get(&item.id) {
            let changed = self.items[at] != item;
            if changed {
                self.changed[at] = self.activity;
                if shape_facts(&self.items[at]) != shape_facts(&item) {
                    self.shape += 1;
                }
            }
            let moved = self.items[at].turn_id != item.turn_id;
            self.items[at] = item;
            if moved {
                self.stream.retain(|(_, i)| *i != at);
                self.shown_at[at] = None;
                if let Some(turn) = self.turn_at.get(&self.items[at].turn_id).copied() {
                    self.place(turn, at);
                } else {
                    self.number_shown();
                }
            }
            return changed;
        }
        let at = self.items.len();
        self.item_at.insert(item.id.clone(), at);
        if let harness_contracts::ItemBody::Request { request_id } = &item.body {
            self.request_items.insert(request_id.clone(), at);
        }
        self.changed.push(self.activity);
        self.shown_at.push(None);
        let turn = self.turn_at.get(&item.turn_id).copied();
        self.items.push(item);
        self.shape += 1;
        if let Some(turn) = turn {
            self.place(turn, at);
        }
        true
    }

    /// `request` put in (replacing the one with its id): whether it changed.
    fn put_request(&mut self, request: RuntimeRequest) -> bool {
        match self.request_at.get(&request.id) {
            Some(&at) => std::mem::replace(&mut self.requests[at], request) != self.requests[at],
            None => {
                self.request_at.insert(request.id.clone(), self.requests.len());
                self.requests.push(request);
                true
            }
        }
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
        self.shape += 1;
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
        self.number_shown();
    }

    /// Each item's place in the order shown, as it is now.
    fn number_shown(&mut self) {
        for (n, (_, i)) in self.stream.iter().enumerate() {
            self.shown_at[*i] = Some(n);
        }
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

/// What of an item its stream's shape depends on: its kind, status,
/// parent and turn, whether it has more than its line.
fn shape_facts(item: &TurnItem) -> (std::mem::Discriminant<harness_contracts::ItemBody>, harness_contracts::ItemStatus, Option<&ItemId>, &TurnId, bool) {
    (std::mem::discriminant(&item.body), item.status, item.parent_id.as_ref(), &item.turn_id, item.body.has_detail())
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
    /// The server's state as it was is all here (`Replayed`; till then,
    /// what comes is replayed, not news); when the agent's updates came in
    /// each thread, kept through a resync until the thread's back.
    live: bool,
    arrived_before: BTreeMap<ThreadId, VecDeque<(f64, u32)>>,
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
            // The whole state follows: start from nothing (when the agent's
            // updates came, kept).
            ClientEvent::Resync => {
                let arrived = std::mem::take(&mut self.threads).into_iter().map(|(id, view)| (id, view.arrivals)).collect();
                *self = Self::default();
                self.arrived_before = arrived;
            }
            // What follows is news.
            ClientEvent::Replayed => {
                self.live = true;
                self.arrived_before.clear();
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

    /// A thread's event, at `now`: news from the agent (something of its
    /// changed, a question asked; once the state as it was is all here) is
    /// an update that arrived.
    fn apply_session(&mut self, event: AdapterEvent, now: f64) {
        let live = self.live;
        match event {
            AdapterEvent::ThreadUpdated { thread } => match self.threads.get_mut(&thread.id) {
                Some(view) => {
                    view.activity += 1;
                    view.thread = thread;
                }
                // (Back after a resync: when the agent's updates came, kept.)
                None => {
                    let mut view = ThreadView::new(thread);
                    view.arrivals = self.arrived_before.remove(&view.thread.id).unwrap_or_default();
                    self.threads.insert(view.thread.id.clone(), view);
                }
            },
            AdapterEvent::TurnUpdated { turn } => {
                // (Queued is your prompt waiting, not the agent's news.)
                let news = live && turn.status != TurnStatus::Queued;
                if let Some(view) = self.view(&turn.thread_id.clone())
                    && view.put_turn(turn)
                    && news
                {
                    view.arrived(now);
                }
            }
            AdapterEvent::ItemUpdated { item } => {
                if let Some(view) = self.view(&item.thread_id.clone())
                    && view.put_item(item)
                    && live
                {
                    view.arrived(now);
                }
            }
            AdapterEvent::RequestUpdated { request } => {
                // Asked (an answer's echo isn't the agent's news).
                let asked = live && request.status == RequestStatus::Pending;
                if let Some(view) = self.view(&request.thread_id.clone())
                    && view.put_request(request)
                    && asked
                {
                    view.arrived(now);
                }
            }
            AdapterEvent::PlanUpdated { plan } => {
                if let Some(view) = self.view(&plan.thread_id.clone()) {
                    let news = live && view.plan.as_ref() != Some(&plan);
                    view.plan = Some(plan);
                    if news {
                        view.arrived(now);
                    }
                }
            }
            AdapterEvent::UsageUpdated { thread_id, usage, .. } => {
                if let Some(view) = self.view(&thread_id) {
                    let news = live && view.usage.as_ref() != Some(&usage);
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
    /// once, counted), once the state as it was is all here: the replay,
    /// the same again, an answer's echo, a turn queued and the thread's own
    /// changes aren't. Through a resync, what came is kept; what's
    /// replayed after it isn't news.
    #[test]
    fn what_came_from_the_agent_is_kept_with_its_thread() {
        let mut store = Store::default();
        let came = |store: &Store| store.threads[&ThreadId::new("t")].arrivals().iter().copied().collect::<Vec<_>>();
        store.apply(thread(ThreadStatus::Running), 0.5);
        store.apply(turn("one", 1, TurnStatus::Running), 0.5);
        assert!(came(&store).is_empty(), "the state as it was: not news");
        store.apply(ClientEvent::Replayed, 0.6);
        store.apply(in_turn("a", "one"), 1.0);
        store.apply(in_turn("a", "one"), 1.02);
        store.apply(in_turn("b", "one"), 1.05);
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
        assert_eq!(came(&store), [(1.0, 2), (4.0, 1)], "replayed: what came before, kept");
        store.apply(ClientEvent::Replayed, 6.15);
        store.apply(in_turn("c", "one"), 6.2);
        assert_eq!(came(&store).last(), Some(&(6.2, 1)), "new after it is news");
    }

    /// A message the agent takes into a turn of its own moves there in the
    /// order shown.
    #[test]
    fn an_item_moved_to_another_turn_is_shown_there() {
        let mut store = Store::default();
        store.apply(thread(ThreadStatus::Running), 0.0);
        store.apply(turn("one", 1, TurnStatus::Running), 0.0);
        store.apply(in_turn("a", "one"), 0.0);
        store.apply(in_turn("steered", "one"), 0.0);
        store.apply(in_turn("b", "one"), 0.0);
        store.apply(turn("two", 2, TurnStatus::Running), 0.0);
        store.apply(in_turn("steered", "two"), 0.0);
        store.apply(in_turn("c", "two"), 0.0);
        let view = &store.threads[&ThreadId::new("t")];
        assert_eq!(view.stream().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a", "b", "steered", "c"]);
        assert_eq!(view.items_of(&TurnId::new("two")).map(|i| i.id.as_str()).collect::<Vec<_>>(), ["steered", "c"]);
    }

    #[test]
    fn events_for_unknown_threads_are_ignored() {
        let mut store = Store::default();
        store.apply(item("a", "orphan"), 0.0);
        assert!(store.threads.is_empty());
    }
}
