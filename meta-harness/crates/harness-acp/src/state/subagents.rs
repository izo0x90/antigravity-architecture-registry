//! The sub-agents an agent starts, one model whichever way its harness
//! tells of them: each an item where it started (its steps under it, when
//! the harness says which are its) and a `Subagent` (who it is, what it
//! was asked, how it's doing, whether it can be stopped alone).
//!
//! - Told we show them (`subagents.declare`): the harness announces each
//!   with its own session (`subagent_update`: its title, task, state, what
//!   can be done), sends its steps under that session, and the messages
//!   between it and its parent (`session_message`: its task, its answer).
//!   One a sub-agent starts is announced under its parent's session, and
//!   sits under its parent.
//! - Only by the call that starts them (`subagents.started_by`): who each
//!   is, from the call's input; how it's doing, from the turn.

use harness_contracts::{AdapterEvent, ItemBody, ItemId, ItemStatus, Subagent, SubagentId, SubagentStatus, TurnId, TurnStatus};
use serde_json::Value;

use super::SessionState;
use crate::convert::{SessionMessage, SubagentUpdate};
use crate::settings::StartedBy;

/// A call that starts sub-agents, as it goes.
#[derive(Debug)]
pub(super) enum Starting {
    /// Asked for: what it starts, once it goes ahead.
    Asked(Vec<Value>),
    /// Gone ahead (they're started) or refused: a later update with its
    /// input again starts nothing.
    Done,
}

/// A told-of sub-agent's own session: which it is, and the session it
/// was announced on (its parent's).
#[derive(Debug)]
pub(super) struct Told {
    at: usize,
    parent: String,
}

/// Sets `field` to `value`: whether that changed it.
fn set<T: PartialEq>(field: &mut T, value: T) -> bool {
    let changed = *field != value;
    *field = value;
    changed
}

impl SessionState {
    /// The sub-agent whose own session `session` is, if it's one.
    pub(super) fn child(&self, session: &str) -> Option<usize> {
        self.children.get(session).map(|t| t.at)
    }

    /// Sub-agent `id`, when it can be stopped alone: its session.
    pub fn cancellable(&self, id: &SubagentId) -> Option<String> {
        self.children.iter().find(|(_, t)| self.subagents[t.at].id == *id && self.subagents[t.at].can_cancel).map(|(s, _)| s.clone())
    }

    /// How the agent tells of its sub-agents by the calls that start them.
    pub fn subagents_started_by(&mut self, started_by: Option<StartedBy>) {
        self.started_by = started_by;
    }

    /// A sub-agent announced, or its state changed (`subagent_update`),
    /// under session `on` (the agent's own, or a sub-agent's: one of its).
    pub(super) fn subagent_update(&mut self, on: &str, update: SubagentUpdate) {
        let SubagentUpdate { session, title, description, status, stop } = update;
        let can_cancel = stop.map(|s| s == crate::convert::Stop::Alone);
        let Some(at) = self.child(&session) else {
            // Announced: under its parent (the sub-agent whose session it
            // came on), in its turn; not said how it's doing: not at work.
            let parent = self.child(on);
            let title = title.unwrap_or_else(|| "sub-agent".into());
            let status = status.unwrap_or(SubagentStatus::Idle);
            let at = self.start_subagent(
                parent,
                SubagentId::new(session.clone()),
                Started { title, kind: None, description, prompt: None, status, can_cancel: can_cancel.unwrap_or(false) },
            );
            self.children.insert(session, Told { at, parent: on.to_owned() });
            return;
        };
        self.change_subagent(at, |s| {
            let mut changed = false;
            if let Some(title) = title {
                changed |= set(&mut s.title, title);
            }
            if description.is_some() {
                changed |= set(&mut s.description, description);
            }
            if let Some(status) = status {
                changed |= set(&mut s.status, status);
            }
            if let Some(can) = can_cancel {
                changed |= set(&mut s.can_cancel, can);
            }
            changed
        });
    }

    /// A message between a sub-agent and the agent that started it, on
    /// either's session: from it to its parent, its answer; to it, its
    /// task (the latest).
    pub(super) fn session_message(&mut self, message: SessionMessage) {
        let SessionMessage { from, to, text } = message;
        let answered =
            from.as_deref().and_then(|f| self.children.get(f)).filter(|t| to.as_deref() == Some(t.parent.as_str())).map(|t| t.at);
        if let Some(at) = answered {
            self.change_subagent(at, |s| set(&mut s.answer, Some(text)));
        } else if let Some(at) = to.as_deref().and_then(|s| self.child(s)) {
            self.change_subagent(at, |s| set(&mut s.prompt, Some(text)));
        }
    }

    /// An update from sub-agent `at`'s own session: its steps (what it
    /// says, thinks and calls), under its item, in its turn. Its session's
    /// own settings, usage, commands, plan and title are its, not the
    /// agent's: not shown.
    pub(super) fn apply_child(&mut self, at: usize, update: &agent_client_protocol::schema::v1::SessionUpdate) {
        use agent_client_protocol::schema::v1::SessionUpdate as U;
        if !matches!(update, U::AgentMessageChunk(_) | U::AgentThoughtChunk(_) | U::ToolCall(_) | U::ToolCallUpdate(_)) {
            return;
        }
        let item = self.subagents[at].item_id.clone();
        let turn = self.item_index.get(&item).map(|&i| self.items[i].turn_id.clone());
        let was = (self.parent.replace(item), std::mem::replace(&mut self.turn, turn));
        self.apply(update);
        (self.parent, self.turn) = was;
    }

    /// A call (`native`) whose input lists sub-agents it starts
    /// (`started_by`): kept while it's asked for, each started once it goes
    /// ahead (`status`), under where it is; never, when it doesn't. (Its
    /// input as text, as a replay has it, is read as what it says.)
    pub(super) fn subagents_started(&mut self, native: &str, input: Option<&Value>, status: ItemStatus) {
        let Some(by) = &self.started_by else { return };
        if !self.starting.contains_key(native) {
            let parsed;
            let input = match input {
                Some(Value::String(text)) => {
                    parsed = serde_json::from_str::<Value>(text).ok();
                    parsed.as_ref()
                }
                other => other,
            };
            if let Some(list) = input.and_then(|i| i.pointer(&by.list)).and_then(Value::as_array).filter(|l| !l.is_empty()) {
                self.starting.insert(native.to_owned(), Starting::Asked(list.clone()));
            }
        }
        match status {
            ItemStatus::Pending => {}
            ItemStatus::Failed | ItemStatus::Cancelled => {
                self.starting.insert(native.to_owned(), Starting::Done);
            }
            ItemStatus::InProgress | ItemStatus::Completed => {
                let Some(Starting::Asked(list)) = self.starting.insert(native.to_owned(), Starting::Done) else { return };
                let Some(by) = &self.started_by else { return };
                let read =
                    |one: &Value, pointer: Option<&str>| pointer.and_then(|p| one.pointer(p)).and_then(Value::as_str).map(str::to_owned);
                let started: Vec<Started> = list
                    .iter()
                    .map(|one| Started {
                        title: read(one, Some(&by.title)).unwrap_or_else(|| "sub-agent".into()),
                        kind: read(one, by.kind.as_deref()),
                        description: None,
                        prompt: read(one, by.prompt.as_deref()),
                        status: SubagentStatus::Running,
                        can_cancel: false,
                    })
                    .collect();
                for (n, one) in started.into_iter().enumerate() {
                    self.start_subagent(None, SubagentId::new(format!("{native}/{n}")), one);
                }
            }
        }
    }

    /// The turn ended (`status`): what said nothing of its own end ended
    /// with it, as it did (completed: its harness doesn't say how); the
    /// calls asked for and never answered are dropped.
    pub(super) fn subagents_end_with_the_turn(&mut self, status: TurnStatus) {
        let ended = match status {
            TurnStatus::Failed => SubagentStatus::Failed,
            TurnStatus::Cancelled => SubagentStatus::Cancelled,
            _ => SubagentStatus::Idle,
        };
        self.end_subagents(ended, false);
        self.starting.clear();
    }

    /// The agent went away: none is at work any more.
    pub(super) fn subagents_end_with_the_agent(&mut self) {
        self.end_subagents(SubagentStatus::Failed, true);
    }

    /// The history's replay is over: what it started that said nothing of
    /// its end has ended (a replay has no turn's end).
    pub(super) fn subagents_replayed(&mut self) {
        self.end_subagents(SubagentStatus::Idle, false);
    }

    /// Those at work end (`ended`): told-of ones too when `all`.
    fn end_subagents(&mut self, ended: SubagentStatus, all: bool) {
        let told: std::collections::HashSet<usize> = self.children.values().map(|t| t.at).collect();
        for at in 0..self.subagents.len() {
            if self.subagents[at].status.working() && (all || !told.contains(&at)) {
                self.change_subagent(at, |s| set(&mut s.status, ended));
            }
        }
    }

    /// Its item, where the agent is now (under `parent`: the sub-agent that
    /// started it, in its turn); and it.
    fn start_subagent(&mut self, parent: Option<usize>, id: SubagentId, one: Started) -> usize {
        let under = parent.map(|p| self.subagents[p].item_id.clone());
        let turn: Option<TurnId> = under.as_ref().and_then(|i| self.item_index.get(i)).map(|&i| self.items[i].turn_id.clone());
        let was = (std::mem::replace(&mut self.parent, under), std::mem::replace(&mut self.turn, turn));
        let item_id: ItemId =
            self.push_item(ItemBody::Subagent { subagent_id: id.clone() }, item_status(one.status), Some(one.title.clone()), None);
        (self.parent, self.turn) = was;
        let started_ms = harness_core::now_ms();
        let ended_ms = (!one.status.working()).then_some(started_ms);
        let Started { title, kind, description, prompt, status, can_cancel } = one;
        self.subagents.push(Subagent {
            id,
            thread_id: self.thread.id.clone(),
            item_id,
            title,
            kind,
            description,
            prompt,
            answer: None,
            status,
            started_ms,
            ended_ms,
            can_cancel,
        });
        let at = self.subagents.len() - 1;
        self.emit(AdapterEvent::SubagentUpdated { subagent: self.subagents[at].clone() });
        at
    }

    /// Sub-agent `at` changed (`change` says whether it did): told, its
    /// end stamped, and its item says whether it's at work.
    fn change_subagent(&mut self, at: usize, change: impl FnOnce(&mut Subagent) -> bool) {
        let (status, title) = (self.subagents[at].status, self.subagents[at].title.clone());
        if !change(&mut self.subagents[at]) {
            return;
        }
        let now = &mut self.subagents[at];
        if now.status != status {
            now.ended_ms = (!now.status.working()).then(harness_core::now_ms);
        }
        let now = now.clone();
        if now.status != status || now.title != title {
            let status = item_status(now.status);
            let title = now.title.clone();
            self.update_item(&now.item_id, |item| {
                item.status = status;
                item.title = Some(title);
            });
        }
        self.emit(AdapterEvent::SubagentUpdated { subagent: now });
    }

    /// Every sub-agent, as it is (for a late subscriber).
    pub(super) fn subagents_now(&self) -> impl Iterator<Item = AdapterEvent> + '_ {
        self.subagents.iter().map(|s| AdapterEvent::SubagentUpdated { subagent: s.clone() })
    }
}

/// A sub-agent as it starts.
struct Started {
    title: String,
    kind: Option<String>,
    description: Option<String>,
    prompt: Option<String>,
    status: SubagentStatus,
    can_cancel: bool,
}

/// Its item's status, as it's doing (the stream's word for it).
fn item_status(status: SubagentStatus) -> ItemStatus {
    match status {
        SubagentStatus::Running | SubagentStatus::Waiting => ItemStatus::InProgress,
        SubagentStatus::Failed => ItemStatus::Failed,
        SubagentStatus::Cancelled => ItemStatus::Cancelled,
        SubagentStatus::Idle | SubagentStatus::Completed => ItemStatus::Completed,
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::v1 as acp;
    use harness_contracts::{InstanceId, RuntimePolicy, Thread, ThreadId, ThreadStatus, TurnInput};

    use super::*;

    /// A pack's settings as it ships.
    fn shipped(pack: &str) -> crate::settings::AcpSettings {
        let path = format!("{}/../../packs/harness/{pack}/pack.toml", env!("CARGO_MANIFEST_DIR"));
        let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let settings = serde_json::to_value(&manifest["provides"]["settings"]).unwrap();
        crate::settings::AcpSettings::parse(&settings).unwrap()
    }

    fn thread() -> Thread {
        Thread {
            id: ThreadId::new("t"),
            instance_id: InstanceId::new("i"),
            native_id: None,
            title: None,
            status: ThreadStatus::Idle,
            policy: RuntimePolicy::default(),
            model: None,
            controls: Default::default(),
            mcp_servers: Vec::new(),
        }
    }

    /// A session as `pack`'s adapter keeps it.
    fn session(pack: &str) -> SessionState {
        let mut state = SessionState::new(thread());
        state.subagents_started_by(shipped(pack).subagents.started_by);
        state
    }

    /// `updates` (each a `session/update`'s params) applied as the adapter
    /// applies them.
    fn send(state: &mut SessionState, updates: &[Value]) {
        for one in updates {
            if let Some((session, update)) = crate::convert::session_update(one.clone()) {
                state.session_update(&session, update);
            }
        }
    }

    /// What was recorded (`fixture`) applied in a turn of `pack`'s.
    fn replayed(pack: &str, fixture: &str) -> SessionState {
        let mut state = session(pack);
        let turn = state.begin_turn(&TurnInput::of("go"));
        let recorded: Value = serde_json::from_str(fixture).unwrap();
        send(&mut state, recorded["updates"].as_array().unwrap());
        state.end_turn(&turn, TurnStatus::Completed, None);
        state
    }

    fn item<'a>(state: &'a SessionState, id: &ItemId) -> &'a harness_contracts::TurnItem {
        &state.items[state.item_index[id]]
    }

    const PARENT: &str = "parent";

    fn on(session: &str, update: Value) -> Value {
        serde_json::json!({ "sessionId": session, "update": update })
    }

    fn announced(child: &str, title: &str, state: Option<&str>) -> Value {
        let mut update =
            serde_json::json!({ "sessionUpdate": "subagent_update", "sessionId": child, "title": title, "capabilities": { "cancel": {} } });
        if let Some(state) = state {
            update["state"] = serde_json::json!({ "state": state });
        }
        update
    }

    fn says(text: &str) -> Value {
        serde_json::json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } })
    }

    fn message(from: &str, to: &str, text: &str) -> Value {
        serde_json::json!({ "sessionUpdate": "session_message", "senderSessionId": from, "recipientSessionId": to, "content": [{ "type": "text", "text": text }] })
    }

    /// Claude Code told we show sub-agents: its one sub-agent announced
    /// (its type, its task, that it can be stopped alone), its prompt, its
    /// step under it, its end (stopped alone); the agent's own answer after
    /// it isn't its; its session's settings aren't the agent's.
    #[test]
    fn a_sub_agent_claude_code_announces_is_one_with_its_steps_under_it() {
        let state = replayed("claude-code", include_str!("../../fixtures/subagent-cancelled-claude-code-0.95.0.json"));
        let [one] = state.subagents.as_slice() else { panic!("{:?}", state.subagents) };
        assert_eq!(
            (one.title.as_str(), one.description.as_deref(), one.can_cancel, one.status),
            ("general-purpose", Some("Run sleep 90"), true, SubagentStatus::Cancelled)
        );
        assert!(
            one.prompt.as_deref().is_some_and(|p| p.starts_with("Your only job: run the shell command `sleep 90`")),
            "{:?}",
            one.prompt
        );
        assert!(one.ended_ms.is_some_and(|e| e >= one.started_ms));
        assert_eq!(item(&state, &one.item_id).status, ItemStatus::Cancelled);
        let under: Vec<&harness_contracts::TurnItem> = state.items.iter().filter(|i| i.parent_id.as_ref() == Some(&one.item_id)).collect();
        assert!(under.iter().any(|i| i.title.as_deref() == Some("sleep 90")), "its call, under it: {under:?}");
        let answer = state.items.iter().rev().find(|i| matches!(&i.body, ItemBody::AssistantMessage { .. })).unwrap();
        assert!(answer.parent_id.is_none() && format!("{:?}", answer.body).contains("parent done"), "the agent's own: {answer:?}");
        assert_eq!(state.cancellable(&one.id).as_deref(), Some(one.id.as_str()), "stopped alone by its own session");
        assert!(state.thread.controls.config.is_empty(), "its session's settings aren't the agent's: {:?}", state.thread.controls.config);
    }

    /// Antigravity tells of a sub-agent only by the call that starts it
    /// (once allowed): who it is, from the call's input; it ends with the
    /// turn (its harness says nothing of its end); its steps can't be told
    /// apart, and it can't be stopped alone.
    #[test]
    fn a_sub_agent_antigravity_starts_is_one_from_its_start_call() {
        let state = replayed("antigravity-acp", include_str!("../../fixtures/subagent-antigravity-1.4.3.json"));
        let [one] = state.subagents.as_slice() else { panic!("{:?}", state.subagents) };
        assert_eq!(
            (one.title.as_str(), one.kind.as_deref(), one.can_cancel, one.status),
            ("File Counter", Some("research"), false, SubagentStatus::Idle)
        );
        assert!(one.prompt.as_deref().is_some_and(|p| p.starts_with("Count the .py files")) && one.description.is_none(), "{one:?}");
        assert!(state.items.iter().all(|i| i.parent_id.as_ref() != Some(&one.item_id)), "nothing said to be its");
        assert_eq!(state.cancellable(&one.id), None);
        // The call that asked is answered, not failed.
        let asked = state.items.iter().find(|i| i.title.as_deref().is_some_and(|t| t.contains("invoke_subagent"))).unwrap();
        assert_eq!(asked.status, ItemStatus::Completed);
    }

    /// Reopened, Antigravity replays the start call with its input as
    /// text: the sub-agent's there, ended (a replay has no turn's end).
    #[test]
    fn a_sub_agent_antigravity_replays_is_there_ended() {
        let mut state = session("antigravity-acp");
        let recorded: Value = serde_json::from_str(include_str!("../../fixtures/subagent-replayed-antigravity-1.4.3.json")).unwrap();
        send(&mut state, recorded["replay"].as_array().unwrap());
        state.replayed();
        // (Followed once it's open: told once, as it is, not as at work.)
        let told: Vec<SubagentStatus> = state
            .replay()
            .into_iter()
            .filter_map(|e| if let AdapterEvent::SubagentUpdated { subagent } = e { Some(subagent.status) } else { None })
            .collect();
        assert_eq!(told, [SubagentStatus::Idle]);
        let [one] = state.subagents.as_slice() else { panic!("{:?}", state.subagents) };
        assert_eq!((one.title.as_str(), one.status), ("File Counter", SubagentStatus::Idle));
        assert_eq!(item(&state, &one.item_id).status, ItemStatus::Completed);
    }

    /// Asked for, then refused: never started. Gone ahead, its input sent
    /// again: started once.
    #[test]
    fn a_start_call_starts_its_sub_agents_once_and_a_refused_one_none() {
        let mut state = session("antigravity-acp");
        state.begin_turn(&TurnInput::of("go"));
        let input = serde_json::json!({ "Subagents": [{ "Role": "Helper", "Prompt": "x", "TypeName": "research" }] });
        let call = |id: &str, status| {
            acp::SessionUpdate::ToolCall(acp::ToolCall::new(id.to_owned(), "Run invoke_subagent?").raw_input(input.clone()).status(status))
        };
        let update = |id: &str, status| {
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                id.to_owned(),
                acp::ToolCallUpdateFields::new().raw_input(input.clone()).status(status),
            ))
        };
        state.apply(&call("c1", acp::ToolCallStatus::Pending));
        assert!(state.subagents.is_empty(), "asked: not yet");
        state.apply(&update("c1", acp::ToolCallStatus::Failed));
        state.apply(&update("c1", acp::ToolCallStatus::Completed));
        assert!(state.subagents.is_empty(), "refused: never");
        state.apply(&call("c2", acp::ToolCallStatus::InProgress));
        state.apply(&update("c2", acp::ToolCallStatus::Completed));
        assert_eq!(state.subagents.len(), 1, "started once");
    }

    /// A sub-agent's steps are under its item, its streamed text apart from
    /// the agent's own; a request for its call nests where the call is; one
    /// it starts is under it, with its task; still at work after the turn,
    /// its steps stay in its turn; announced not saying how it's doing, it
    /// isn't at work; the agent gone, none is.
    #[test]
    fn a_sub_agents_steps_and_its_own_sub_agents_are_under_it() {
        let mut state = session("claude-code");
        let turn = state.begin_turn(&TurnInput::of("look around"));
        send(
            &mut state,
            &[
                on(PARENT, says("mine ")),
                on(PARENT, announced("a", "Explorer", Some("running"))),
                on("a", says("its ")),
                on(PARENT, says("too")),
                on("a", says("own")),
            ],
        );
        let a = state.subagents[0].item_id.clone();
        let said: Vec<(String, Option<&ItemId>)> = state
            .items
            .iter()
            .filter(|i| matches!(i.body, ItemBody::AssistantMessage { .. }))
            .map(|i| (i.body.said().unwrap_or_default(), i.parent_id.as_ref()))
            .collect();
        assert_eq!(said, [("mine too".to_owned(), None), ("its own".to_owned(), Some(&a))], "each sender's text in one message of its own");
        let (_, added) = state.inject(&TurnInput::of("also this")).unwrap();
        assert_eq!(state.items[state.item_index[&added]].parent_id, None, "your message is yours");
        let call = serde_json::json!({ "sessionUpdate": "tool_call", "toolCallId": "run-1", "title": "Run tests" });
        send(&mut state, &[on("a", call)]);
        let request =
            state.open_request(Some("run-1"), harness_contracts::RequestKind::Question, "Run them?".into(), None, Vec::new(), Vec::new());
        let asked = state.items.iter().find(|i| matches!(&i.body, ItemBody::Request { request_id } if *request_id == request)).unwrap();
        assert_eq!(asked.parent_id.as_ref(), Some(&a), "the request where its call is");
        // One it starts: under it, its task told on either session.
        send(
            &mut state,
            &[
                on("a", announced("b", "Reader", Some("running"))),
                on("a", message("a", "b", "read it")),
                on("b", message("a", "b", "read it")),
                on("b", message("b", "a", "read")),
            ],
        );
        let b = &state.subagents[1];
        assert_eq!(item(&state, &b.item_id).parent_id.as_ref(), Some(&a));
        assert_eq!((b.prompt.as_deref(), b.answer.as_deref()), (Some("read it"), Some("read")));
        state.end_turn(&turn, TurnStatus::Completed, None);
        // Still at work after the turn: its steps in its turn.
        let turns = state.turns.len();
        send(&mut state, &[on("b", says("later"))]);
        assert_eq!(state.turns.len(), turns, "no turn of its own");
        let later = state.items.last().unwrap();
        assert_eq!(
            (later.turn_id == item(&state, &state.subagents[1].item_id).turn_id, later.parent_id.as_ref()),
            (true, Some(&state.subagents[1].item_id))
        );
        // Announced not saying how it's doing.
        send(&mut state, &[on(PARENT, announced("c", "Resumed", None))]);
        assert!(!state.subagents[2].status.working());
        state.end_all(harness_contracts::Failure { message: "gone".into(), retryable: true });
        assert!(state.subagents.iter().all(|s| !s.status.working()), "the agent gone, none at work");
    }

    /// An update under a session that's neither the agent's own nor a
    /// sub-agent's it announced is nobody's: not taken as the agent's.
    #[test]
    fn an_update_from_an_unknown_session_isnt_the_agents() {
        let mut state = session("claude-code");
        state.set_thread(|t| t.native_id = Some(harness_contracts::NativeId::new(PARENT)));
        state.begin_turn(&TurnInput::of("go"));
        let mode = serde_json::json!({ "sessionUpdate": "current_mode_update", "currentModeId": "plan" });
        send(&mut state, &[on("stranger", says("not mine")), on("stranger", mode)]);
        assert!(state.items.iter().all(|i| i.body.said().as_deref() != Some("not mine")) && state.thread.controls.current_mode.is_none());
        send(&mut state, &[on(PARENT, says("mine"))]);
        assert!(state.items.iter().any(|i| i.body.said().as_deref() == Some("mine")));
    }

    /// A sub-agent's own settings, mode and title stay its own.
    #[test]
    fn a_sub_agents_session_settings_are_not_the_agents() {
        let mut state = session("claude-code");
        state.begin_turn(&TurnInput::of("go"));
        let config = serde_json::json!({ "sessionUpdate": "config_option_update", "configOptions": [{ "id": "mode", "name": "Mode", "type": "select", "currentValue": "manual", "options": [{ "value": "manual", "name": "Manual" }] }] });
        let mode = serde_json::json!({ "sessionUpdate": "current_mode_update", "currentModeId": "manual" });
        let title = serde_json::json!({ "sessionUpdate": "session_info_update", "title": "its" });
        send(&mut state, &[on(PARENT, announced("a", "Explorer", Some("running"))), on("a", config), on("a", mode), on("a", title)]);
        let controls = &state.thread.controls;
        assert!(controls.config.is_empty() && controls.current_mode.is_none() && state.thread.title.is_none(), "{:?}", state.thread);
    }
}
