//! Per-session state: folds ACP updates into harness entities and emits each change
//! as an `AdapterEvent`. Synchronous and connection-free, so it is unit-testable.
//! Emission happens under the state lock, which lets `events()` subscribe and take a
//! replay snapshot atomically.

use std::collections::{HashMap, VecDeque};

use agent_client_protocol::schema::v1 as acp;
use harness_contracts::{
    AdapterEvent, ApprovalOption, CatalogUpdate, ContentBlock, Failure, ItemBody, ItemId, ItemStatus,
    NativeId, PlanArtifact, PlanBody, PlanId, PlanStatus, Question, RequestId, RequestKind, RequestStatus,
    RuntimeRequest, SignIn, Thread, ThreadStatus, ToolKind, Turn, TurnId, TurnInput, TurnItem, TurnStatus, Usage,
};
use tokio::sync::broadcast;

use crate::convert;

mod subagents;

/// What a chunk of a message an agent replays is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chunks {
    /// Part of the message its id names (none: of the one before it).
    OfItsId,
    /// The whole message.
    Whole,
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub struct SessionState {
    thread: Thread,
    turns: Vec<Turn>,
    items: Vec<TurnItem>,
    item_index: HashMap<ItemId, usize>,
    /// Native tool call id -> item.
    tool_items: HashMap<String, ItemId>,
    /// Items receiving streamed text (a message, reasoning), by whose
    /// they are (none: the agent's own; a sub-agent's call's item): each
    /// sender's closed by anything else it sends, the others' left open.
    open_message: HashMap<Option<ItemId>, ItemId>,
    open_reasoning: HashMap<Option<ItemId>, ItemId>,
    requests: Vec<RuntimeRequest>,
    plan: Option<PlanArtifact>,
    usage: Option<Usage>,
    catalog: CatalogUpdate,
    /// Turns sent and not ended: the first is running (items go to it),
    /// the rest wait their turn.
    running: VecDeque<TurnId>,
    /// Turn that replayed history (no prompt running) attaches to.
    replay_turn: Option<TurnId>,
    /// The id the agent gave the last message of yours it replayed: a
    /// chunk with another is another message.
    replayed_message: Option<String>,
    /// The message of yours being replayed, as it's come so far: its item
    /// is made once it's all come, read back to its text and pieces.
    open_user: Option<String>,
    /// How a replayed message's pieces are read back (none: it's shown as
    /// it comes).
    pieces: Option<crate::pieces::Replay>,
    /// What a chunk of a message of yours the agent replays is.
    chunks: Chunks,
    /// The sub-agents started, in order.
    subagents: Vec<harness_contracts::Subagent>,
    /// Each told-of sub-agent's own session: which it is (`subagents`).
    children: HashMap<String, subagents::Told>,
    /// How the agent tells of its sub-agents by the calls that start them.
    started_by: Option<crate::settings::StartedBy>,
    /// The calls that start sub-agents, this turn, as each goes.
    starting: HashMap<String, subagents::Starting>,
    /// A prompt's turn split by a message added to it: the turn it goes on
    /// in (its end ends that one).
    continues: HashMap<TurnId, TurnId>,
    /// Whose the item being made is: the sub-agent that sent the update
    /// being applied, or a request's call's (none: the agent's own, and
    /// what isn't an update: your messages); and the turn it's in, when
    /// it's not the one now (a sub-agent's: its own item's).
    parent: Option<ItemId>,
    turn: Option<TurnId>,
    events: broadcast::Sender<AdapterEvent>,
}

impl SessionState {
    pub fn new(thread: Thread) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            thread,
            turns: Vec::new(),
            items: Vec::new(),
            item_index: HashMap::new(),
            tool_items: HashMap::new(),
            open_message: HashMap::new(),
            open_reasoning: HashMap::new(),
            requests: Vec::new(),
            plan: None,
            usage: None,
            catalog: CatalogUpdate::default(),
            running: VecDeque::new(),
            replay_turn: None,
            replayed_message: None,
            open_user: None,
            pieces: None,
            chunks: Chunks::OfItsId,
            subagents: Vec::new(),
            children: HashMap::new(),
            started_by: None,
            starting: HashMap::new(),
            continues: HashMap::new(),
            parent: None,
            turn: None,
            events,
        }
    }

    /// Latest state of every entity, in an order clients can apply directly.
    pub fn replay(&self) -> Vec<AdapterEvent> {
        let mut out = vec![AdapterEvent::ThreadUpdated { thread: self.thread.clone() }];
        out.extend(self.turns.iter().map(|t| AdapterEvent::TurnUpdated { turn: t.clone() }));
        out.extend(self.items.iter().map(|i| AdapterEvent::ItemUpdated { item: i.clone() }));
        out.extend(self.requests.iter().map(|r| AdapterEvent::RequestUpdated { request: r.clone() }));
        out.extend(self.subagents_now());
        if let Some(plan) = &self.plan {
            out.push(AdapterEvent::PlanUpdated { plan: plan.clone() });
        }
        if let Some(usage) = &self.usage {
            out.push(AdapterEvent::UsageUpdated { thread_id: self.thread.id.clone(), turn_id: None, usage: usage.clone() });
        }
        if self.catalog != CatalogUpdate::default() {
            out.push(AdapterEvent::CatalogUpdated { update: self.catalog.clone() });
        }
        out
    }

    /// Subscribes to live events; pair with `replay()` under the same lock.
    pub fn subscribe(&self) -> broadcast::Receiver<AdapterEvent> {
        self.events.subscribe()
    }

    fn emit(&self, event: AdapterEvent) {
        let _ = self.events.send(event);
    }

    /// Replaces the session's config controls; a model option also refreshes the
    /// instance's model catalog.
    pub fn set_config(&mut self, controls: Vec<harness_contracts::ConfigControl>) {
        if let Some(models) = convert::models_from_controls(&controls) {
            self.catalog.models = Some(models.clone());
            self.emit(AdapterEvent::CatalogUpdated { update: CatalogUpdate { models: Some(models), ..CatalogUpdate::default() } });
        }
        self.set_thread(|t| t.controls.config = controls);
    }

    /// Sets a control's current value locally after the harness accepted it.
    pub fn set_config_value(&mut self, id: &str, value: &serde_json::Value) {
        self.set_thread(|t| {
            if let Some(control) = t.controls.config.iter_mut().find(|c| c.id == id) {
                match (&mut control.kind, value) {
                    (harness_contracts::ConfigKind::Select { current, .. }, serde_json::Value::String(v)) => *current = v.clone(),
                    (harness_contracts::ConfigKind::Boolean { current }, serde_json::Value::Bool(v)) => *current = *v,
                    _ => {}
                }
            }
        });
    }

    pub fn controls(&self) -> &harness_contracts::SessionControls {
        &self.thread.controls
    }

    pub fn set_thread(&mut self, update: impl FnOnce(&mut Thread)) {
        update(&mut self.thread);
        self.emit(AdapterEvent::ThreadUpdated { thread: self.thread.clone() });
    }

    // ---- turns ----

    pub fn turn_running(&self) -> bool {
        !self.running.is_empty()
    }

    /// The turn items go to now.
    pub fn active_turn(&self) -> Option<&TurnId> {
        self.running.front()
    }

    /// How replayed messages are read from now on: their pieces (none: as
    /// they come), and what a chunk is.
    pub fn replays_by(&mut self, pieces: Option<crate::pieces::Replay>, chunks: Chunks) {
        (self.pieces, self.chunks) = (pieces, chunks);
    }

    /// The history's replay is over: its last message of yours is read
    /// back; its sub-agents end. (What it emitted meanwhile reached no one:
    /// the session's events are followed from its state once it's open.)
    pub fn replayed(&mut self) {
        self.close_user();
        self.subagents_replayed();
    }

    /// The message of yours being replayed, all come: its item, read back
    /// to its text and its pieces (one with none, as it came).
    fn close_user(&mut self) {
        let Some(raw) = self.open_user.take() else { return };
        let body = match self.pieces.as_ref().and_then(|r| r.read(&raw)) {
            Some((text, pieces)) => ItemBody::UserMessage { text, pieces },
            None => ItemBody::UserMessage { text: raw, pieces: Vec::new() },
        };
        self.push_item(body, ItemStatus::Completed, None, None);
    }

    /// Your message as its item has it: its text and its pieces.
    fn yours(input: &TurnInput) -> ItemBody {
        ItemBody::UserMessage { text: input.text.clone(), pieces: input.pieces.clone() }
    }

    /// A turn for `input`: running if none is, else queued behind those
    /// that are. Its message is in it either way.
    pub fn begin_turn(&mut self, input: &TurnInput) -> TurnId {
        self.close_user();
        let starts = self.running.is_empty();
        if starts {
            self.close_all_streams();
            self.replay_turn = None;
        }
        let turn_id = self.start_turn_record(if starts { TurnStatus::Running } else { TurnStatus::Queued });
        self.running.push_back(turn_id.clone());
        self.push_item_in(turn_id.clone(), Self::yours(input), ItemStatus::Completed, None, None);
        if starts {
            self.set_thread(|t| t.status = ThreadStatus::Running);
        }
        turn_id
    }

    /// A message added to the running turn (steering), and its item; none
    /// running: None. It's in that turn until the agent takes it (`steered`).
    pub fn inject(&mut self, input: &TurnInput) -> Option<(TurnId, ItemId)> {
        let turn_id = self.running.front()?.clone();
        self.close_all_streams();
        let item = self.push_item_in(turn_id.clone(), Self::yours(input), ItemStatus::Completed, None, None);
        Some((turn_id, item))
    }

    /// The agent took message `item` added to a running turn: it starts the
    /// next turn, as the session's history has it once replayed (a turn is
    /// a message of yours and what follows it), so turns keep their numbers
    /// after a reload. The agent says it took it as it's queued, before it
    /// reads it: what came before that is the turn's that ran, so the
    /// message moves to now (the end) and the new turn starts there. The
    /// prompt goes on in it (its end ends it). The turn it's in now.
    pub fn steered(&mut self, item: &ItemId) -> Option<TurnId> {
        let &at = self.item_index.get(item)?;
        let was = self.items[at].turn_id.clone();
        let place = self.turns.iter().position(|t| t.id == was)?;
        let status = self.turns[place].status;
        let turn = Turn { id: TurnId::new(new_id()), thread_id: self.thread.id.clone(), ordinal: 0, status, failure: None, usage: None };
        let id = turn.id.clone();
        self.turns.insert(place + 1, turn);
        // It and the turns after it numbered anew, in order.
        for n in place + 1..self.turns.len() {
            self.turns[n].ordinal = n as u32 + 1;
            let turn = self.turns[n].clone();
            self.emit(AdapterEvent::TurnUpdated { turn });
        }
        // The message, now: last, the new turn's.
        let mut message = self.items.remove(at);
        for (n, i) in self.items.iter().enumerate().skip(at) {
            self.item_index.insert(i.id.clone(), n);
        }
        message.turn_id = id.clone();
        message.ordinal = self.items.iter().map(|i| i.ordinal).max().map_or(0, |o| o + 1);
        self.item_index.insert(message.id.clone(), self.items.len());
        self.items.push(message.clone());
        self.emit(AdapterEvent::ItemUpdated { item: message });
        if let Some(running) = self.running.iter_mut().find(|t| **t == was) {
            *running = id.clone();
            self.continues.insert(was.clone(), id.clone());
            self.set_turn(&was, |t| t.status = TurnStatus::Completed);
        }
        Some(id)
    }

    /// A message the agent didn't take after all.
    pub fn withdraw(&mut self, item: &ItemId) {
        self.update_item(item, |i| i.status = ItemStatus::Cancelled);
    }

    /// Ends `turn_id`. The running one ending closes its streams and
    /// cancels what it was still asking (returned); the next starts.
    pub fn end_turn(&mut self, turn_id: &TurnId, status: TurnStatus, failure: Option<Failure>) -> Vec<RequestId> {
        // A prompt's turn split by a message added to it: the turns it went on in.
        let mut chain = vec![turn_id.clone()];
        while let Some(next) = self.continues.remove(chain.last().unwrap_or(turn_id)) {
            chain.push(next);
        }
        let turn_id = chain.last().unwrap_or(turn_id).clone();
        let turn_id = &turn_id;
        let Some(at) = self.running.iter().position(|t| t == turn_id) else { return Vec::new() };
        self.running.remove(at);
        // (Its sub-agents end first: once it's said to have ended, none
        // of its is still at work.)
        if self.running.is_empty() {
            self.subagents_end_with_the_turn(status);
        }
        let mut cancelled = Vec::new();
        if at == 0 {
            self.close_all_streams();
            cancelled = self.requests.iter().filter(|r| r.status == RequestStatus::Pending && r.turn_id.as_ref().is_some_and(|t| chain.contains(t))).map(|r| r.id.clone()).collect();
            for id in &cancelled {
                self.set_request_status(id, RequestStatus::Cancelled);
            }
        }
        let usage = (at == 0).then(|| self.usage.clone()).flatten();
        self.set_turn(turn_id, |turn| {
            turn.status = status;
            turn.failure = failure;
            turn.usage = usage;
        });
        if at == 0
            && let Some(next) = self.running.front().cloned()
        {
            self.set_turn(&next, |turn| turn.status = TurnStatus::Running);
        }
        let next = match (self.running.is_empty(), status) {
            (false, _) => ThreadStatus::Running,
            (true, TurnStatus::Failed) => ThreadStatus::Error,
            (true, _) => ThreadStatus::Idle,
        };
        self.set_thread(|t| t.status = next);
        cancelled
    }

    /// Ends every turn not ended (the agent went away).
    pub fn end_all(&mut self, failure: Failure) {
        let waiting: Vec<TurnId> = self.running.iter().cloned().collect();
        for id in waiting {
            self.end_turn(&id, TurnStatus::Failed, Some(failure.clone()));
        }
        self.subagents_end_with_the_agent();
    }

    fn set_turn(&mut self, id: &TurnId, update: impl FnOnce(&mut Turn)) {
        if let Some(turn) = self.turns.iter_mut().find(|t| &t.id == id) {
            update(turn);
            let turn = turn.clone();
            self.emit(AdapterEvent::TurnUpdated { turn });
        }
    }

    /// Who the agent says it's signed in as.
    pub fn sign_in(&mut self, sign_in: SignIn) {
        self.catalog.sign_in = Some(sign_in.clone());
        self.emit(AdapterEvent::CatalogUpdated { update: CatalogUpdate { sign_in: Some(sign_in), ..CatalogUpdate::default() } });
    }

    fn start_turn_record(&mut self, status: TurnStatus) -> TurnId {
        let turn = Turn {
            id: TurnId::new(new_id()),
            thread_id: self.thread.id.clone(),
            ordinal: self.turns.len() as u32 + 1,
            status,
            failure: None,
            usage: None,
        };
        let id = turn.id.clone();
        self.turns.push(turn.clone());
        self.emit(AdapterEvent::TurnUpdated { turn });
        id
    }

    /// Turn for an incoming item: its sender's (a sub-agent's), else the
    /// running prompt, else the current replay turn (history arriving on
    /// load), else a new completed replay turn.
    fn item_turn(&mut self) -> TurnId {
        if let Some(id) = self.turn.clone().or_else(|| self.running.front().cloned()).or_else(|| self.replay_turn.clone()) {
            return id;
        }
        let id = self.start_turn_record(TurnStatus::Completed);
        self.replay_turn = Some(id.clone());
        id
    }

    // ---- items ----

    fn push_item(&mut self, body: ItemBody, status: ItemStatus, title: Option<String>, native: Option<String>) -> ItemId {
        // Anything after a replayed message of yours: it's all come (its
        // item first, in its own turn).
        self.close_user();
        let turn_id = self.item_turn();
        self.push_item_in(turn_id, body, status, title, native)
    }

    fn push_item_in(&mut self, turn_id: TurnId, body: ItemBody, status: ItemStatus, title: Option<String>, native: Option<String>) -> ItemId {
        let item = TurnItem {
            id: ItemId::new(new_id()),
            thread_id: self.thread.id.clone(),
            turn_id,
            native_id: native.map(NativeId::new),
            parent_id: self.parent.clone(),
            ordinal: self.items.len() as u32,
            status,
            title,
            body,
        };
        let id = item.id.clone();
        self.item_index.insert(id.clone(), self.items.len());
        self.items.push(item.clone());
        self.emit(AdapterEvent::ItemUpdated { item });
        id
    }

    fn update_item(&mut self, id: &ItemId, update: impl FnOnce(&mut TurnItem)) {
        if let Some(&idx) = self.item_index.get(id) {
            update(&mut self.items[idx]);
            let item = self.items[idx].clone();
            self.emit(AdapterEvent::ItemUpdated { item });
        }
    }

    /// What the sender of the update being applied was streaming, ended.
    fn close_streams(&mut self) {
        let whose = self.parent.clone();
        let ended: Vec<ItemId> = [self.open_message.remove(&whose), self.open_reasoning.remove(&whose)].into_iter().flatten().collect();
        self.end_streamed(ended);
    }

    /// What every sender was streaming, ended (a turn begins or ends).
    fn close_all_streams(&mut self) {
        let ended: Vec<ItemId> = self.open_message.drain().chain(self.open_reasoning.drain()).map(|(_, id)| id).collect();
        self.end_streamed(ended);
    }

    fn end_streamed(&mut self, ended: Vec<ItemId>) {
        for id in ended {
            self.update_item(&id, |item| {
                item.status = ItemStatus::Completed;
                match &mut item.body {
                    ItemBody::AssistantMessage { streaming, .. } | ItemBody::Reasoning { streaming, .. } => *streaming = false,
                    _ => {}
                }
            });
        }
    }

    pub fn tool_kind(&self, native_tool_id: &str) -> ToolKind {
        self.tool_items
            .get(native_tool_id)
            .and_then(|id| self.item_index.get(id))
            .and_then(|&idx| match &self.items[idx].body {
                ItemBody::ToolCall { kind, .. } => Some(kind.clone()),
                _ => None,
            })
            .unwrap_or(ToolKind::Other)
    }

    // ---- ACP updates ----

    /// A `session/update`, under `session`: the agent's own, or one of its
    /// sub-agents' (only its steps shown, under it), or about them. The
    /// agent's own mode, when it's changed to it (its policy follows).
    pub fn session_update(&mut self, session: &str, update: convert::Update) -> Option<String> {
        let child = self.child(session);
        // (Neither the agent's own session, once known, nor a sub-agent's
        // it announced: nobody's here. Nothing of it is taken.)
        let ours = self.thread.native_id.as_ref().is_none_or(|own| own.as_str() == session);
        if !ours && child.is_none() {
            return None;
        }
        match update {
            convert::Update::Subagent(update) => self.subagent_update(session, update),
            convert::Update::Message(message) => self.session_message(message),
            convert::Update::Typed(update) => match child {
                Some(at) => self.apply_child(at, &update),
                None => {
                    self.apply(&update);
                    if let acp::SessionUpdate::CurrentModeUpdate(mode) = update {
                        return Some(mode.current_mode_id.to_string());
                    }
                }
            },
            convert::Update::Unread { kind, why } if child.is_none() => {
                self.close_streams();
                self.push_item(ItemBody::Unknown { native_kind: kind, display: why }, ItemStatus::Completed, None, None);
            }
            convert::Update::Unread { .. } => {}
        }
        None
    }

    /// An update the agent sent (whose: `parent`, while a sub-agent's is
    /// applied).
    pub fn apply(&mut self, update: &acp::SessionUpdate) {
        match update {
            acp::SessionUpdate::UserMessageChunk(chunk) => self.user_chunk(chunk),
            acp::SessionUpdate::AgentMessageChunk(chunk) => self.message_chunk(&chunk.content),
            acp::SessionUpdate::AgentThoughtChunk(chunk) => self.thought_chunk(&chunk.content),
            acp::SessionUpdate::ToolCall(call) => self.tool_call(call),
            acp::SessionUpdate::ToolCallUpdate(update) => self.tool_call_update(update),
            acp::SessionUpdate::Plan(plan) => self.plan(plan),
            acp::SessionUpdate::AvailableCommandsUpdate(update) => {
                self.catalog.commands = Some(convert::commands(update));
                self.emit(AdapterEvent::CatalogUpdated { update: CatalogUpdate { commands: self.catalog.commands.clone(), ..CatalogUpdate::default() } });
            }
            // The app-level runtime mode is mapped by the session (it needs the flavor).
            acp::SessionUpdate::CurrentModeUpdate(update) => {
                let mode = update.current_mode_id.to_string();
                self.set_thread(|t| t.controls.current_mode = Some(mode));
            }
            acp::SessionUpdate::ConfigOptionUpdate(update) => self.set_config(convert::config_controls(&update.config_options)),
            acp::SessionUpdate::SessionInfoUpdate(info) => {
                if let agent_client_protocol::schema::MaybeUndefined::Value(title) = info.title.clone() {
                    self.set_thread(|t| t.title = Some(title));
                }
            }
            acp::SessionUpdate::UsageUpdate(update) => {
                let usage = convert::usage(update);
                self.usage = Some(usage.clone());
                self.emit(AdapterEvent::UsageUpdated { thread_id: self.thread.id.clone(), turn_id: self.running.front().cloned(), usage });
            }
            other => {
                let native_kind = serde_json::to_value(other)
                    .ok()
                    .and_then(|v| v.get("sessionUpdate").and_then(|k| k.as_str()).map(str::to_owned))
                    .unwrap_or_else(|| "unknown".into());
                self.close_streams();
                self.push_item(ItemBody::Unknown { native_kind, display: format!("{other:?}") }, ItemStatus::Completed, None, None);
            }
        }
    }

    /// Only seen during history replay: a new user message starts a new replayed turn;
    /// consecutive chunks extend the same message (one with another id
    /// than the last, when the agent gives them, is another message).
    /// A sub-agent's (its prompt, its call's input already) isn't one.
    fn user_chunk(&mut self, chunk: &acp::ContentChunk) {
        if self.turn_running() || self.parent.is_some() {
            return; // the live prompt is already recorded by begin_turn
        }
        let text = match convert::content_block(&chunk.content) {
            ContentBlock::Text { text } => text,
            other => format!("{other:?}"),
        };
        let id = chunk.message_id.as_ref().map(|m| m.0.to_string());
        let same = match (&id, &self.replayed_message) {
            _ if self.chunks == Chunks::Whole => false,
            (Some(id), Some(last)) => id == last,
            _ => true,
        };
        self.replayed_message = id;
        if let Some(raw) = self.open_user.as_mut()
            && same
        {
            raw.push_str(&text);
            return;
        }
        self.close_user();
        self.close_all_streams();
        self.replay_turn = None;
        self.open_user = Some(text);
    }

    fn message_chunk(&mut self, content: &acp::ContentBlock) {
        self.close_reasoning();
        let block = convert::content_block(content);
        match self.open_message.get(&self.parent).cloned() {
            Some(id) => self.update_item(&id, |item| {
                if let ItemBody::AssistantMessage { content, .. } = &mut item.body {
                    append_block(content, block);
                }
            }),
            None => {
                let id = self.push_item(
                    ItemBody::AssistantMessage { content: vec![block], streaming: true },
                    ItemStatus::InProgress,
                    None,
                    None,
                );
                self.open_message.insert(self.parent.clone(), id);
            }
        }
    }

    fn thought_chunk(&mut self, content: &acp::ContentBlock) {
        let text = match convert::content_block(content) {
            ContentBlock::Text { text } => text,
            _ => return,
        };
        match self.open_reasoning.get(&self.parent).cloned() {
            Some(id) => self.update_item(&id, |item| {
                if let ItemBody::Reasoning { text: existing, .. } = &mut item.body {
                    existing.push_str(&text);
                }
            }),
            None => {
                let id = self.push_item(ItemBody::Reasoning { text, streaming: true }, ItemStatus::InProgress, None, None);
                self.open_reasoning.insert(self.parent.clone(), id);
            }
        }
    }

    fn close_reasoning(&mut self) {
        if let Some(id) = self.open_reasoning.remove(&self.parent) {
            self.update_item(&id, |item| {
                item.status = ItemStatus::Completed;
                if let ItemBody::Reasoning { streaming, .. } = &mut item.body {
                    *streaming = false;
                }
            });
        }
    }

    fn tool_call(&mut self, call: &acp::ToolCall) {
        self.close_streams();
        let native = call.tool_call_id.to_string();
        let body = ItemBody::ToolCall {
            kind: convert::tool_kind(&call.kind),
            name: call.name.clone().unwrap_or_else(|| call.title.clone()),
            input: call.raw_input.clone(),
            output: call.content.iter().map(convert::tool_content).collect(),
            locations: call.locations.iter().map(convert::location).collect(),
        };
        if let Some(id) = self.tool_items.get(&native).cloned() {
            let status = convert::tool_status(&call.status);
            let title = call.title.clone();
            self.update_item(&id, |item| {
                item.body = body;
                item.status = status;
                item.title = Some(title);
            });
        } else {
            let id = self.push_item(body, convert::tool_status(&call.status), Some(call.title.clone()), Some(native.clone()));
            self.tool_items.insert(native.clone(), id);
        }
        self.subagents_started(&native, call.raw_input.as_ref(), convert::tool_status(&call.status));
    }

    pub fn tool_call_update(&mut self, update: &acp::ToolCallUpdate) {
        let native = update.tool_call_id.to_string();
        let fields = &update.fields;
        let id = match self.tool_items.get(&native).cloned() {
            Some(id) => id,
            None => {
                self.close_streams();
                let body = ItemBody::ToolCall {
                    kind: fields.kind.as_ref().map(convert::tool_kind).unwrap_or(ToolKind::Other),
                    name: fields.name.clone().or_else(|| fields.title.clone()).unwrap_or_else(|| "tool".into()),
                    input: None,
                    output: Vec::new(),
                    locations: Vec::new(),
                };
                let id = self.push_item(body, ItemStatus::Pending, fields.title.clone(), Some(native.clone()));
                self.tool_items.insert(native.clone(), id.clone());
                id
            }
        };
        self.update_item(&id, |item| {
            if let Some(status) = &fields.status {
                item.status = convert::tool_status(status);
            }
            if let Some(title) = &fields.title {
                item.title = Some(title.clone());
            }
            if let ItemBody::ToolCall { kind, name, input, output, locations } = &mut item.body {
                if let Some(k) = &fields.kind {
                    *kind = convert::tool_kind(k);
                }
                if let Some(n) = &fields.name {
                    *name = n.clone();
                }
                if let Some(raw) = &fields.raw_input {
                    *input = Some(raw.clone());
                }
                if let Some(content) = &fields.content {
                    *output = content.iter().map(convert::tool_content).collect();
                }
                if let Some(locs) = &fields.locations {
                    *locations = locs.iter().map(convert::location).collect();
                }
            }
        });
        let status = self.item_index.get(&id).map_or(ItemStatus::Pending, |&at| self.items[at].status);
        self.subagents_started(&native, fields.raw_input.as_ref(), status);
    }

    fn plan(&mut self, plan: &acp::Plan) {
        let steps = convert::plan_steps(plan);
        let done = !steps.is_empty() && steps.iter().all(|s| s.status == harness_contracts::StepStatus::Completed);
        let id = self.plan.as_ref().map(|p| p.id.clone()).unwrap_or_else(|| PlanId::new(new_id()));
        let artifact = PlanArtifact {
            id,
            thread_id: self.thread.id.clone(),
            status: if done { PlanStatus::Completed } else { PlanStatus::Active },
            body: PlanBody::TodoList { steps },
        };
        self.plan = Some(artifact.clone());
        self.emit(AdapterEvent::PlanUpdated { plan: artifact });
    }

    // ---- requests ----

    /// Something the agent waits on the user for: about a tool call
    /// (`native_tool_id`), with options to choose or questions to answer.
    pub fn open_request(&mut self, native_tool_id: Option<&str>, kind: RequestKind, title: String, detail: Option<String>, options: Vec<ApprovalOption>, questions: Vec<Question>) -> RequestId {
        let item_id = native_tool_id.and_then(|id| self.tool_items.get(id).cloned());
        // Its call's sender's (a sub-agent's request nests where its call is).
        self.parent = item_id.as_ref().and_then(|id| self.item_index.get(id)).and_then(|&at| self.items[at].parent_id.clone());
        self.close_streams();
        let request = RuntimeRequest {
            id: RequestId::new(new_id()),
            thread_id: self.thread.id.clone(),
            turn_id: self.running.front().cloned(),
            item_id,
            kind,
            status: RequestStatus::Pending,
            title,
            detail,
            options,
            questions,
            response: None,
        };
        let id = request.id.clone();
        self.requests.push(request.clone());
        self.emit(AdapterEvent::RequestUpdated { request });
        self.push_item(ItemBody::Request { request_id: id.clone() }, ItemStatus::Pending, None, None);
        self.parent = None;
        self.set_thread(|t| t.status = ThreadStatus::WaitingForInput);
        id
    }

    pub fn resolve_request(&mut self, id: &RequestId, response: harness_contracts::RequestResponse) {
        if let Some(request) = self.requests.iter_mut().find(|r| &r.id == id) {
            request.status = RequestStatus::Resolved;
            request.response = Some(response);
            let request = request.clone();
            self.emit(AdapterEvent::RequestUpdated { request });
        }
        if self.turn_running() && !self.requests.iter().any(|r| r.status == RequestStatus::Pending) {
            self.set_thread(|t| t.status = ThreadStatus::Running);
        }
    }

    /// The agent no longer asks `id` (it withdrew it).
    pub fn withdraw_request(&mut self, id: &RequestId) {
        if self.request(id).is_some_and(|r| r.status == RequestStatus::Pending) {
            self.set_request_status(id, RequestStatus::Cancelled);
            if self.turn_running() && !self.requests.iter().any(|r| r.status == RequestStatus::Pending) {
                self.set_thread(|t| t.status = ThreadStatus::Running);
            }
        }
    }

    fn set_request_status(&mut self, id: &RequestId, status: RequestStatus) {
        if let Some(request) = self.requests.iter_mut().find(|r| &r.id == id) {
            request.status = status;
            let request = request.clone();
            self.emit(AdapterEvent::RequestUpdated { request });
        }
    }

    pub fn request(&self, id: &RequestId) -> Option<&RuntimeRequest> {
        self.requests.iter().find(|r| &r.id == id)
    }
}

fn append_block(content: &mut Vec<ContentBlock>, block: ContentBlock) {
    if let (Some(ContentBlock::Text { text: last }), ContentBlock::Text { text }) = (content.last_mut(), &block) {
        last.push_str(text);
        return;
    }
    content.push(block);
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::{InstanceId, Piece, RuntimePolicy, ThreadId};

    fn state() -> SessionState {
        SessionState::new(Thread {
            id: ThreadId::new("t"),
            instance_id: InstanceId::new("i"),
            native_id: None,
            title: None,
            status: ThreadStatus::Idle,
            policy: RuntimePolicy::default(),
            model: None,
            controls: Default::default(),
            mcp_servers: Vec::new(),
        })
    }

    fn chunk(text: &str) -> acp::ContentChunk {
        acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(text)))
    }

    #[test]
    fn history_replay_builds_one_turn_per_user_message() {
        let mut s = state();
        s.apply(&acp::SessionUpdate::UserMessageChunk(chunk("q1")));
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("a1")));
        s.apply(&acp::SessionUpdate::UserMessageChunk(chunk("q2 ")));
        s.apply(&acp::SessionUpdate::UserMessageChunk(chunk("cont")));
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("a2")));

        assert_eq!(s.turns.len(), 2);
        assert!(s.turns.iter().all(|t| t.status == TurnStatus::Completed));
        let by_turn = |n: usize| s.items.iter().filter(|i| i.turn_id == s.turns[n].id).map(|i| &i.body).collect::<Vec<_>>();
        assert!(matches!(by_turn(0).as_slice(), [ItemBody::UserMessage { text, .. }, ItemBody::AssistantMessage { .. }] if text == "q1"));
        assert!(matches!(by_turn(1).as_slice(), [ItemBody::UserMessage { text, .. }, ItemBody::AssistantMessage { .. }] if text == "q2 cont"));
        assert!(!s.turn_running());
    }

    #[test]
    fn live_prompt_ignores_user_echo_and_closes_streams_on_end() {
        let mut s = state();
        let turn = s.begin_turn(&TurnInput::of("hello"));
        s.apply(&acp::SessionUpdate::UserMessageChunk(chunk("hello")));
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("hi")));
        s.end_turn(&turn, TurnStatus::Completed, None);
        assert_eq!(s.items.len(), 2);
        assert!(matches!(&s.items[1].body, ItemBody::AssistantMessage { streaming: false, .. }));
        assert_eq!(s.turns[0].status, TurnStatus::Completed);
    }

    #[test]
    fn turns_sent_while_one_runs_wait_and_start_in_order() {
        let mut s = state();
        let first = s.begin_turn(&TurnInput::of("one"));
        let second = s.begin_turn(&TurnInput::of("two"));
        assert_eq!(s.turns[1].status, TurnStatus::Queued);
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("for one")));
        let ask = s.open_request(None, RequestKind::Approval { action: harness_contracts::ToolAction::Tool }, "ok?".into(), None, Vec::new(), Vec::new());
        assert_eq!(s.request(&ask).and_then(|r| r.turn_id.clone()), Some(first.clone()), "asked in the running turn");
        assert_eq!(s.end_turn(&first, TurnStatus::Completed, None), vec![ask.clone()], "what the turn still asked ends with it");
        assert_eq!((s.turns[1].status, s.thread.status), (TurnStatus::Running, ThreadStatus::Running));
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("for two")));
        let of = |turn: &TurnId| s.items.iter().filter(|i| &i.turn_id == turn).map(|i| &i.body).collect::<Vec<_>>();
        assert!(matches!(of(&second).as_slice(), [ItemBody::UserMessage { text, .. }, ItemBody::AssistantMessage { .. }] if text == "two"));
        s.end_turn(&second, TurnStatus::Completed, None);
        assert_eq!(s.thread.status, ThreadStatus::Idle);
    }

    #[test]
    fn a_message_added_to_the_running_turn_lands_in_it() {
        let mut s = state();
        assert_eq!(s.inject(&TurnInput::of("late")), None, "nothing running");
        let turn = s.begin_turn(&TurnInput::of("start"));
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("working")));
        assert_eq!(s.inject(&TurnInput::of("also this")).map(|(t, _)| t), Some(turn));
        assert!(matches!(&s.items[1].body, ItemBody::AssistantMessage { streaming: false, .. }), "what streamed before it is done");
        assert!(matches!(&s.items[2].body, ItemBody::UserMessage { text, .. } if text == "also this"));
    }

    /// The bodies of each turn, in the turns' order, as their numbers have them.
    fn turns(s: &SessionState) -> Vec<(u32, TurnStatus, Vec<String>)> {
        let mut turns = s.turns.clone();
        turns.sort_by_key(|t| t.ordinal);
        let text = |b: &ItemBody| match b {
            ItemBody::UserMessage { text, .. } => format!("you: {text}"),
            ItemBody::AssistantMessage { content, .. } => content.iter().map(|c| if let ContentBlock::Text { text } = c { text.clone() } else { String::new() }).collect(),
            _ => String::new(),
        };
        turns.iter().map(|t| (t.ordinal, t.status, s.items.iter().filter(|i| i.turn_id == t.id).map(|i| text(&i.body)).collect())).collect()
    }

    /// A message added to a running turn, once the agent takes it, starts
    /// the next turn, as replaying the session's history numbers it: a
    /// tag on a turn names the same turn before and after a reload.
    #[test]
    fn a_message_the_agent_took_while_it_ran_starts_the_next_turn_as_a_replay_numbers_it() {
        let mut s = state();
        let first = s.begin_turn(&TurnInput::of("start"));
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("working")));
        let queued = s.begin_turn(&TurnInput::of("later"));
        let (_, item) = s.inject(&TurnInput::of("also this")).unwrap();
        // Before the agent says it took it: still the turn that runs.
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("still on the first")));
        let ask = s.open_request(None, RequestKind::Approval { action: harness_contracts::ToolAction::Tool }, "ok?".into(), None, Vec::new(), Vec::new());
        let went_on = s.steered(&item).unwrap();
        assert_eq!(s.active_turn(), Some(&went_on), "what the agent says now is the new turn's");
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("done both")));
        assert_eq!(s.request(&ask).and_then(|r| r.turn_id.clone()), Some(first.clone()), "asked before it took the message: the first turn's");
        assert_eq!(s.end_turn(&first, TurnStatus::Completed, None), vec![ask], "the prompt's end ends the turn it went on in, and what it asked");
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("and later")));
        s.end_turn(&queued, TurnStatus::Completed, None);
        let live = turns(&s);
        assert_eq!(
            live.iter().map(|(n, status, said)| (*n, *status, said.join(" | "))).collect::<Vec<_>>(),
            [(1, TurnStatus::Completed, "you: start | working | still on the first | ".to_owned()), (2, TurnStatus::Completed, "you: also this | done both".into()), (3, TurnStatus::Completed, "you: later | and later".into())],
            "(the request asked is an item of the turn it was asked in)"
        );
        // The same session replayed: the same turns, by the same numbers.
        let mut replayed = state();
        for (user, said) in [("start", "working still on the first"), ("also this", "done both"), ("later", "and later")] {
            replayed.apply(&acp::SessionUpdate::UserMessageChunk(chunk(user)));
            replayed.apply(&acp::SessionUpdate::AgentMessageChunk(chunk(said)));
        }
        let numbered = |t: &[(u32, TurnStatus, Vec<String>)]| t.iter().map(|(n, _, said)| (*n, said.first().cloned())).collect::<Vec<_>>();
        assert_eq!(numbered(&turns(&replayed)), numbered(&live));
    }

    /// Two messages of yours one after the other (a prompt stopped before
    /// any answer, then the next) are two turns when replayed, as they
    /// were live, when the agent says which message each chunk is of.
    #[test]
    fn messages_replayed_one_after_the_other_are_told_apart_by_their_ids() {
        let mut s = state();
        let of = |text: &str, id: &str| acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(text))).message_id(acp::MessageId::new(id));
        s.apply(&acp::SessionUpdate::UserMessageChunk(of("stopped", "m1")));
        s.apply(&acp::SessionUpdate::UserMessageChunk(of("next ", "m2")));
        s.apply(&acp::SessionUpdate::UserMessageChunk(of("one", "m2")));
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("answer")));
        assert_eq!(turns(&s).into_iter().map(|(_, _, said)| said).collect::<Vec<_>>(), [vec!["you: stopped".to_owned()], vec!["you: next one".into(), "answer".into()]]);
    }

    /// A pack's replay, as it ships, and whether its chunks are whole messages.
    fn shipped(pack: &str) -> (crate::pieces::Replay, Chunks) {
        let path = format!("{}/../../packs/harness/{pack}/pack.toml", env!("CARGO_MANIFEST_DIR"));
        let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let replay = &manifest["provides"]["settings"]["replay"];
        let whole = if replay.get("whole_chunks").and_then(toml::Value::as_bool).unwrap_or(false) { Chunks::Whole } else { Chunks::OfItsId };
        (crate::pieces::Replay::parse(replay["piece"].as_str().unwrap(), replay.get("listed").and_then(|l| l.as_str())).unwrap(), whole)
    }

    fn replaying(pack: &str) -> SessionState {
        let (replay, whole) = shipped(pack);
        let mut s = state();
        s.replays_by(Some(replay), whole);
        s
    }

    fn yours(s: &SessionState) -> Vec<(String, Vec<Piece>)> {
        s.items.iter().filter_map(|i| match &i.body { ItemBody::UserMessage { text, pieces } => Some((text.clone(), pieces.clone())), _ => None }).collect()
    }

    /// "see #flaky and @a.rs": two pieces.
    fn sent() -> (String, Vec<Piece>) {
        let link = |start, end, to| harness_contracts::Link { start, end, to, include: harness_contracts::Include::Auto };
        let flaky = Piece { links: vec![link(4, 10, harness_contracts::Linked::Tag { tag: harness_contracts::TagId::new("flaky") })], text: "#flaky\n- lines 1 to 2".into() };
        let file = Piece { links: vec![link(15, 20, harness_contracts::Linked::Thing { subject: harness_contracts::Subject::CodeFile { path: "a.rs".into() } })], text: "@a.rs\n- a.rs:\n  fn a() {}".into() };
        ("see #flaky and @a.rs".into(), vec![flaky, file])
    }

    /// Your messages as each agent replays them (Claude Code: chunks of one
    /// id, the words, every label, then every piece's context; Antigravity:
    /// one chunk, the pieces run on after the words): each read back to
    /// its words and pieces, once it's all come.
    #[test]
    fn a_replayed_message_of_yours_comes_back_to_its_words_and_pieces() {
        let (words, pieces) = sent();
        let of = |text: &str, id: &str| acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(text))).message_id(acp::MessageId::new(id));

        let mut s = replaying("claude-code");
        let mut chunks = vec![words.clone()];
        chunks.extend(pieces.iter().map(Piece::label));
        chunks.extend(pieces.iter().map(|p| format!("\n<context ref=\"{}\">\n{}\n</context>", p.label(), p.text)));
        for c in &chunks {
            s.apply(&acp::SessionUpdate::UserMessageChunk(of(c, "m1")));
        }
        assert!(yours(&s).is_empty(), "still coming: no item yet");
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("answer")));
        assert_eq!(yours(&s), [(words.clone(), pieces.clone())]);
        assert_eq!(turns(&s)[0].2.len(), 2, "yours, then the answer, in one turn");

        let mut s = replaying("antigravity-acp");
        let mut text = words.clone();
        pieces.iter().for_each(|p| text.push_str(&format!("Embedded resource ({}):\n{}", p.label(), p.text)));
        s.apply(&acp::SessionUpdate::UserMessageChunk(chunk(&text)));
        // Two of yours in a row (the first stopped before an answer): two messages.
        s.apply(&acp::SessionUpdate::UserMessageChunk(chunk("try again")));
        s.replayed();
        assert_eq!(yours(&s), [(words, pieces), ("try again".to_owned(), Vec::new())]);
    }

    /// A message you send, then its session's history replayed in a fresh
    /// one: the same words and pieces, whichever way the agent took them.
    #[test]
    fn what_you_send_comes_back_the_same_from_the_replay() {
        let (words, pieces) = sent();
        let input = TurnInput { pieces: pieces.clone(), ..TurnInput::of(words.clone()) };
        for sending in [crate::pieces::Sending::AsPieces, crate::pieces::Sending::InText] {
            let mut live = state();
            live.begin_turn(&input);
            // What the agent was sent, as our way replays it (an agent that
            // takes no pieces replays its one text).
            let mut replay = state();
            replay.replays_by(Some(crate::pieces::Replay::ours()), Chunks::OfItsId);
            let sent: String = crate::pieces::blocks(words.clone(), pieces.clone(), sending)
                .into_iter()
                .filter_map(|b| match b {
                    acp::ContentBlock::Text(t) => Some(t.text),
                    acp::ContentBlock::Resource(r) => match r.resource {
                        acp::EmbeddedResourceResource::TextResourceContents(c) => Some(crate::pieces::Replay::ours_written(&c.uri, &c.text)),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            replay.apply(&acp::SessionUpdate::UserMessageChunk(chunk(&sent)));
            replay.replayed();
            assert_eq!(yours(&replay), yours(&live), "{sending:?}");
        }
    }

    /// A long session: thousands of your messages each with a full piece,
    /// replayed; each is read back once, as it ends, and made once.
    #[test]
    fn a_long_replay_reads_each_message_back_once() {
        let link = harness_contracts::Link { start: 0, end: 6, to: harness_contracts::Linked::Tag { tag: harness_contracts::TagId::new("flaky") }, include: harness_contracts::Include::Whole };
        let piece = Piece { links: vec![link], text: "x".repeat(16_000) };
        let label = piece.label();
        let mut s = replaying("antigravity-acp");
        let mut events = s.subscribe();
        let started = std::time::Instant::now();
        for n in 0..3000 {
            s.apply(&acp::SessionUpdate::UserMessageChunk(chunk(&format!("#flaky {n}Embedded resource ({label}):\n{}", piece.text))));
            s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("ok")));
            // Each of yours told once, as it's made.
            let mut told = 0;
            while let Ok(event) = events.try_recv() {
                told += usize::from(matches!(event, AdapterEvent::ItemUpdated { item } if matches!(item.body, ItemBody::UserMessage { .. })));
            }
            assert_eq!(told, 1);
        }
        s.replayed();
        let took = started.elapsed();
        let read_back = s.items.iter().filter(|i| matches!(&i.body, ItemBody::UserMessage { pieces, .. } if pieces.len() == 1)).count();
        assert_eq!(read_back, 3000);
        assert!(took < std::time::Duration::from_secs(5), "{took:?} for 3000 messages of 16 KB");
    }

    /// What each agent really replayed of a message of two pieces
    /// (recorded: `fixtures/`), read back to the words and pieces sent.
    #[test]
    fn a_recorded_replay_reads_back_to_what_was_sent() {
        for (pack, fixture) in [("claude-code", include_str!("../fixtures/replay-claude-code-0.95.0.json")), ("antigravity-acp", include_str!("../fixtures/replay-antigravity-1.4.3.json"))] {
            let recorded: serde_json::Value = serde_json::from_str(fixture).unwrap();
            let mut s = replaying(pack);
            for update in recorded["replayed"].as_array().unwrap() {
                let chunk: acp::ContentChunk = serde_json::from_value(update.clone()).unwrap();
                s.apply(&acp::SessionUpdate::UserMessageChunk(chunk));
            }
            s.replayed();
            let sent: Vec<(String, String)> = recorded["pieces"].as_array().unwrap().iter().map(|p| (p["label"].as_str().unwrap().to_owned(), p["text"].as_str().unwrap().to_owned())).collect();
            let read = yours(&s);
            assert_eq!(read.len(), 1, "{pack}: one message");
            assert_eq!(read[0].0, recorded["words"].as_str().unwrap(), "{pack}");
            assert_eq!(read[0].1.iter().map(|p| (p.label(), p.text.clone())).collect::<Vec<_>>(), sent, "{pack}");
        }
    }
}
