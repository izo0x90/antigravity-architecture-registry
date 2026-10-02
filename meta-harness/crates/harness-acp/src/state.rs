//! Per-session state: folds ACP updates into harness entities and emits each change
//! as an `AdapterEvent`. Synchronous and connection-free, so it is unit-testable.
//! Emission happens under the state lock, which lets `events()` subscribe and take a
//! replay snapshot atomically.

use std::collections::HashMap;

use agent_client_protocol::schema::v1 as acp;
use harness_contracts::{
    AdapterEvent, ApprovalOption, CatalogUpdate, ContentBlock, Failure, ItemBody, ItemId, ItemStatus,
    NativeId, PlanArtifact, PlanBody, PlanId, PlanStatus, RequestId, RequestKind, RequestStatus,
    RuntimeRequest, Thread, ThreadStatus, ToolKind, Turn, TurnId, TurnItem, TurnStatus, Usage,
};
use tokio::sync::broadcast;

use crate::convert;

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
    /// Item currently receiving streamed text, closed by any other item kind.
    open_message: Option<ItemId>,
    open_reasoning: Option<ItemId>,
    requests: Vec<RuntimeRequest>,
    plan: Option<PlanArtifact>,
    usage: Option<Usage>,
    catalog: CatalogUpdate,
    current_turn: Option<TurnId>,
    /// Turn that replayed history (no prompt running) attaches to.
    replay_turn: Option<TurnId>,
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
            open_message: None,
            open_reasoning: None,
            requests: Vec::new(),
            plan: None,
            usage: None,
            catalog: CatalogUpdate::default(),
            current_turn: None,
            replay_turn: None,
            events,
        }
    }

    /// Latest state of every entity, in an order clients can apply directly.
    pub fn replay(&self) -> Vec<AdapterEvent> {
        let mut out = vec![AdapterEvent::ThreadUpdated { thread: self.thread.clone() }];
        out.extend(self.turns.iter().map(|t| AdapterEvent::TurnUpdated { turn: t.clone() }));
        out.extend(self.items.iter().map(|i| AdapterEvent::ItemUpdated { item: i.clone() }));
        out.extend(self.requests.iter().map(|r| AdapterEvent::RequestUpdated { request: r.clone() }));
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
            self.emit(AdapterEvent::CatalogUpdated { update: CatalogUpdate { commands: None, models: Some(models) } });
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
        self.current_turn.is_some()
    }

    pub fn begin_turn(&mut self, prompt: &str) -> TurnId {
        self.close_streams();
        let turn_id = self.start_turn_record(TurnStatus::Running);
        self.current_turn = Some(turn_id.clone());
        self.replay_turn = None;
        self.push_item(ItemBody::UserMessage { text: prompt.to_owned() }, ItemStatus::Completed, None, None);
        self.set_thread(|t| t.status = ThreadStatus::Running);
        turn_id
    }

    pub fn end_turn(&mut self, status: TurnStatus, failure: Option<Failure>) {
        self.close_streams();
        let pending: Vec<RequestId> =
            self.requests.iter().filter(|r| r.status == RequestStatus::Pending).map(|r| r.id.clone()).collect();
        for id in pending {
            self.set_request_status(&id, RequestStatus::Cancelled);
        }
        if let Some(turn_id) = self.current_turn.take()
            && let Some(turn) = self.turns.iter_mut().find(|t| t.id == turn_id)
        {
            turn.status = status;
            turn.failure = failure;
            turn.usage = self.usage.clone();
            let turn = turn.clone();
            self.emit(AdapterEvent::TurnUpdated { turn });
        }
        let next = if status == TurnStatus::Failed { ThreadStatus::Error } else { ThreadStatus::Idle };
        self.set_thread(|t| t.status = next);
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

    /// Turn for an incoming item: the running prompt, else the current replay turn
    /// (history arriving on load), else a new completed replay turn.
    fn item_turn(&mut self) -> TurnId {
        if let Some(id) = self.current_turn.clone().or_else(|| self.replay_turn.clone()) {
            return id;
        }
        let id = self.start_turn_record(TurnStatus::Completed);
        self.replay_turn = Some(id.clone());
        id
    }

    // ---- items ----

    fn push_item(&mut self, body: ItemBody, status: ItemStatus, title: Option<String>, native: Option<String>) -> ItemId {
        let turn_id = self.item_turn();
        let item = TurnItem {
            id: ItemId::new(new_id()),
            thread_id: self.thread.id.clone(),
            turn_id,
            native_id: native.map(NativeId::new),
            parent_id: None,
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

    fn close_streams(&mut self) {
        for id in [self.open_message.take(), self.open_reasoning.take()].into_iter().flatten() {
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

    pub fn apply(&mut self, update: &acp::SessionUpdate) {
        match update {
            acp::SessionUpdate::UserMessageChunk(chunk) => self.user_chunk(&chunk.content),
            acp::SessionUpdate::AgentMessageChunk(chunk) => self.message_chunk(&chunk.content),
            acp::SessionUpdate::AgentThoughtChunk(chunk) => self.thought_chunk(&chunk.content),
            acp::SessionUpdate::ToolCall(call) => self.tool_call(call),
            acp::SessionUpdate::ToolCallUpdate(update) => self.tool_call_update(update),
            acp::SessionUpdate::Plan(plan) => self.plan(plan),
            acp::SessionUpdate::AvailableCommandsUpdate(update) => {
                self.catalog.commands = Some(convert::commands(update));
                self.emit(AdapterEvent::CatalogUpdated { update: CatalogUpdate { commands: self.catalog.commands.clone(), models: None } });
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
                self.emit(AdapterEvent::UsageUpdated { thread_id: self.thread.id.clone(), turn_id: self.current_turn.clone(), usage });
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
    /// consecutive chunks extend the same message.
    fn user_chunk(&mut self, content: &acp::ContentBlock) {
        if self.current_turn.is_some() {
            return; // the live prompt is already recorded by begin_turn
        }
        let text = match convert::content_block(content) {
            ContentBlock::Text { text } => text,
            other => format!("{other:?}"),
        };
        if let Some(last) = self.items.last().map(|i| i.id.clone())
            && self.open_message.is_none()
            && self.open_reasoning.is_none()
            && matches!(self.items.last().map(|i| &i.body), Some(ItemBody::UserMessage { .. }))
        {
            self.update_item(&last, |item| {
                if let ItemBody::UserMessage { text: existing } = &mut item.body {
                    existing.push_str(&text);
                }
            });
            return;
        }
        self.close_streams();
        self.replay_turn = None;
        self.push_item(ItemBody::UserMessage { text }, ItemStatus::Completed, None, None);
    }

    fn message_chunk(&mut self, content: &acp::ContentBlock) {
        self.close_reasoning();
        let block = convert::content_block(content);
        match self.open_message.clone() {
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
                self.open_message = Some(id);
            }
        }
    }

    fn thought_chunk(&mut self, content: &acp::ContentBlock) {
        let text = match convert::content_block(content) {
            ContentBlock::Text { text } => text,
            _ => return,
        };
        match self.open_reasoning.clone() {
            Some(id) => self.update_item(&id, |item| {
                if let ItemBody::Reasoning { text: existing, .. } = &mut item.body {
                    existing.push_str(&text);
                }
            }),
            None => {
                let id = self.push_item(ItemBody::Reasoning { text, streaming: true }, ItemStatus::InProgress, None, None);
                self.open_reasoning = Some(id);
            }
        }
    }

    fn close_reasoning(&mut self) {
        if let Some(id) = self.open_reasoning.take() {
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
            self.tool_items.insert(native, id);
        }
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
                self.tool_items.insert(native, id.clone());
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

    pub fn open_request(
        &mut self,
        native_tool_id: &str,
        kind: RequestKind,
        title: String,
        options: Vec<ApprovalOption>,
    ) -> RequestId {
        self.close_streams();
        let item_id = self.tool_items.get(native_tool_id).cloned();
        let request = RuntimeRequest {
            id: RequestId::new(new_id()),
            thread_id: self.thread.id.clone(),
            turn_id: self.current_turn.clone(),
            item_id,
            kind,
            status: RequestStatus::Pending,
            title,
            detail: None,
            options,
            questions: Vec::new(),
            response: None,
        };
        let id = request.id.clone();
        self.requests.push(request.clone());
        self.emit(AdapterEvent::RequestUpdated { request });
        self.push_item(ItemBody::Request { request_id: id.clone() }, ItemStatus::Pending, None, None);
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
        if self.current_turn.is_some() {
            self.set_thread(|t| t.status = ThreadStatus::Running);
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

/// Request kind for a permission prompt about a tool of `kind`.
pub fn request_kind(kind: &ToolKind) -> RequestKind {
    match kind {
        ToolKind::Execute => RequestKind::Command,
        ToolKind::Edit | ToolKind::Delete | ToolKind::Move => RequestKind::FileChange,
        ToolKind::Read => RequestKind::FileRead,
        _ => RequestKind::Permission,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::{InstanceId, RuntimePolicy, ThreadId};

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
        assert!(matches!(by_turn(0).as_slice(), [ItemBody::UserMessage { text }, ItemBody::AssistantMessage { .. }] if text == "q1"));
        assert!(matches!(by_turn(1).as_slice(), [ItemBody::UserMessage { text }, ItemBody::AssistantMessage { .. }] if text == "q2 cont"));
        assert!(!s.turn_running());
    }

    #[test]
    fn live_prompt_ignores_user_echo_and_closes_streams_on_end() {
        let mut s = state();
        s.begin_turn("hello");
        s.apply(&acp::SessionUpdate::UserMessageChunk(chunk("hello")));
        s.apply(&acp::SessionUpdate::AgentMessageChunk(chunk("hi")));
        s.end_turn(TurnStatus::Completed, None);
        assert_eq!(s.items.len(), 2);
        assert!(matches!(&s.items[1].body, ItemBody::AssistantMessage { streaming: false, .. }));
        assert_eq!(s.turns[0].status, TurnStatus::Completed);
    }
}
