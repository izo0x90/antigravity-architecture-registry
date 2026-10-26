//! The sub-agents an agent starts, one model whichever way its harness
//! tells of them: each an item where it started (its steps under it, when
//! the harness says which are its) and a `Subagent` (who it is, what it
//! was asked, how it's doing, whether it can be stopped alone).
//!
//! - Told we show them (`subagents.declare`): the harness announces each
//!   with its own session (`subagent_update`: its title, task, state, what
//!   can be done), sends its steps under that session, and the messages
//!   between it and its parent (`session_message`: its prompt, its answer).
//! - Only by the call that starts them (`subagents.started_by`): who each
//!   is, from the call's input; how it's doing, from the turn.

use agent_client_protocol::schema::v1 as acp;
use harness_contracts::{AdapterEvent, ItemBody, ItemId, ItemStatus, Subagent, SubagentId, SubagentStatus};
use serde_json::Value;

use super::SessionState;
use crate::settings::StartedBy;

impl SessionState {
    /// An update the ACP schema we build on can't read, or one under a
    /// sub-agent's own session (raw: `session`'s `update`), taken; false:
    /// it's the agent's own, for `apply`.
    pub fn take_raw(&mut self, session: &str, update: &Value) -> bool {
        let child = self.child(session);
        match update.get("sessionUpdate").and_then(Value::as_str).unwrap_or_default() {
            "subagent_update" => self.subagent_update(update),
            // As the agent's own session has them (its sub-agent's has the same).
            "session_message" if !child => self.session_message(update),
            "session_message" | "session_message_chunk" => {}
            // (One of a kind it can't read: nothing of it shown.)
            _ if child => {
                if let Ok(update) = serde_json::from_value::<acp::SessionUpdate>(update.clone()) {
                    self.apply_child(session, &update);
                }
            }
            _ => return false,
        }
        true
    }

    /// The sub-agent whose own session `session` is, if it's one.
    pub fn child(&self, session: &str) -> bool {
        self.children.contains_key(session)
    }

    /// Sub-agent `id`, when it can be stopped alone: its session.
    pub fn cancellable(&self, id: &SubagentId) -> Option<String> {
        self.children.iter().find(|(_, at)| self.subagents[**at].id == *id && self.subagents[**at].can_cancel).map(|(s, _)| s.clone())
    }

    /// How the agent tells of its sub-agents by the calls that start them.
    pub fn subagents_started_by(&mut self, started_by: Option<StartedBy>) {
        self.started_by = started_by;
    }

    /// A sub-agent announced, or its state changed (`subagent_update`).
    pub fn subagent_update(&mut self, update: &Value) {
        let Some(session) = update.get("sessionId").and_then(Value::as_str).map(str::to_owned) else { return };
        let at = match self.children.get(&session) {
            Some(at) => *at,
            None => {
                let title = update.get("title").and_then(Value::as_str).unwrap_or("sub-agent").to_owned();
                let at = self.start_subagent(SubagentId::new(session.clone()), title, None, None);
                self.children.insert(session, at);
                at
            }
        };
        let text = |key: &str| update.get(key).and_then(Value::as_str).map(str::to_owned);
        let status = update.get("state").map(subagent_status);
        let can_cancel = update.get("capabilities").map(|c| c.get("cancel").is_some());
        let (title, description) = (text("title"), text("description"));
        self.change_subagent(at, |s| {
            if let Some(title) = title {
                s.title = title;
            }
            if description.is_some() {
                s.description = description;
            }
            if let Some(status) = status {
                s.status = status;
            }
            if let Some(can) = can_cancel {
                s.can_cancel = can;
            }
        });
    }

    /// A message between the agent and one of its sub-agents, seen in the
    /// agent's own session (`session_message`): the sub-agent's prompt.
    /// (Its answer is its own last message, under it already.)
    pub fn session_message(&mut self, message: &Value) {
        let to = message.get("recipientSessionId").and_then(Value::as_str);
        let Some(&at) = to.and_then(|s| self.children.get(s)) else { return };
        let text: String = message.get("content").and_then(Value::as_array).into_iter().flatten().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("");
        self.change_subagent(at, |s| s.prompt = Some(text));
    }

    /// An update from sub-agent `session`'s own session: its step, under
    /// its item; what it's doing now, from its latest call.
    pub fn apply_child(&mut self, session: &str, update: &acp::SessionUpdate) {
        let Some(&at) = self.children.get(session) else { return };
        let previous = self.parent.replace(self.subagents[at].item_id.clone());
        self.apply_as_sent(update);
        self.parent = previous;
        if let acp::SessionUpdate::ToolCall(call) = update {
            let doing = call.title.clone();
            self.change_subagent(at, |s| s.doing = Some(doing));
        }
    }

    /// A call (`native`) whose input lists sub-agents it starts
    /// (`started_by`): kept while it's asked for, each started once it goes
    /// ahead (`status`), under where it is; dropped when it doesn't.
    pub(super) fn subagents_started(&mut self, native: &str, input: Option<&Value>, status: ItemStatus) {
        let Some(by) = self.started_by.clone() else { return };
        if let Some(list) = input.and_then(|i| i.pointer(&by.list)).and_then(Value::as_array).filter(|l| !l.is_empty()) {
            self.starting.entry(native.to_owned()).or_insert_with(|| list.clone());
        }
        match status {
            ItemStatus::Pending => {}
            ItemStatus::Failed | ItemStatus::Cancelled => {
                self.starting.remove(native);
            }
            ItemStatus::InProgress | ItemStatus::Completed => {
                for (n, one) in self.starting.remove(native).unwrap_or_default().iter().enumerate() {
                    let read = |pointer: &Option<String>| pointer.as_deref().and_then(|p| one.pointer(p)).and_then(Value::as_str).map(str::to_owned);
                    let title = one.pointer(&by.title).and_then(Value::as_str).unwrap_or("sub-agent").to_owned();
                    let at = self.start_subagent(SubagentId::new(format!("{native}/{n}")), title, read(&by.kind), read(&by.prompt));
                    let task = self.subagents[at].prompt.as_deref().map(first_line);
                    self.change_subagent(at, |s| s.description = task);
                }
            }
        }
    }

    /// The turn ended: what said nothing of its own end has ended with it
    /// (its harness doesn't say how).
    pub(super) fn subagents_end_with_the_turn(&mut self) {
        let ended: Vec<usize> = (0..self.subagents.len()).filter(|&at| self.subagents[at].status.working() && !self.children.values().any(|c| *c == at)).collect();
        for at in ended {
            self.change_subagent(at, |s| s.status = SubagentStatus::Idle);
        }
    }

    /// Its item, where the agent is now; and it, running.
    fn start_subagent(&mut self, id: SubagentId, title: String, kind: Option<String>, prompt: Option<String>) -> usize {
        let item_id: ItemId = self.push_item(ItemBody::Subagent { subagent_id: id.clone() }, ItemStatus::InProgress, Some(title.clone()), None);
        self.subagents.push(Subagent { id, thread_id: self.thread.id.clone(), item_id, title, kind, description: None, prompt, doing: None, status: SubagentStatus::Running, can_cancel: false });
        let at = self.subagents.len() - 1;
        self.emit(AdapterEvent::SubagentUpdated { subagent: self.subagents[at].clone() });
        at
    }

    /// Sub-agent `at` changed: told, and its item says whether it's at work.
    fn change_subagent(&mut self, at: usize, change: impl FnOnce(&mut Subagent)) {
        let before = self.subagents[at].clone();
        change(&mut self.subagents[at]);
        let now = self.subagents[at].clone();
        if now == before {
            return;
        }
        if now.status != before.status || now.title != before.title {
            let status = match now.status {
                SubagentStatus::Running | SubagentStatus::Waiting => ItemStatus::InProgress,
                SubagentStatus::Failed => ItemStatus::Failed,
                SubagentStatus::Cancelled => ItemStatus::Cancelled,
                SubagentStatus::Idle | SubagentStatus::Completed => ItemStatus::Completed,
            };
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

/// A sub-agent's state as the ACP draft says it (`running`,
/// `requires_action`, `idle` with how it stopped, `unknown`).
fn subagent_status(state: &Value) -> SubagentStatus {
    match (state.get("state").and_then(Value::as_str), state.get("stopReason").and_then(Value::as_str)) {
        (Some("running"), _) => SubagentStatus::Running,
        (Some("requires_action"), _) => SubagentStatus::Waiting,
        (Some("idle"), Some("end_turn")) => SubagentStatus::Completed,
        (Some("idle"), Some("cancelled")) => SubagentStatus::Cancelled,
        (Some("idle"), Some("error" | "refusal")) => SubagentStatus::Failed,
        _ => SubagentStatus::Idle,
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    match line.char_indices().nth(120) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use harness_contracts::{InstanceId, RuntimePolicy, Thread, ThreadId, ThreadStatus, TurnInput, TurnStatus};

    use super::*;

    /// A pack's settings as it ships.
    fn shipped(pack: &str) -> crate::settings::AcpSettings {
        let path = format!("{}/../../packs/harness/{pack}/pack.toml", env!("CARGO_MANIFEST_DIR"));
        let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let settings = serde_json::to_value(&manifest["provides"]["settings"]).unwrap();
        crate::settings::AcpSettings::parse(&settings).unwrap()
    }

    /// A session as `pack`'s adapter keeps it, a turn begun, and what was
    /// recorded (`fixture`) applied as the adapter applies it: what it
    /// takes raw first, the rest as typed.
    fn replayed(pack: &str, fixture: &str) -> SessionState {
        let settings = shipped(pack);
        let thread = Thread { id: ThreadId::new("t"), instance_id: InstanceId::new("i"), native_id: None, title: None, status: ThreadStatus::Idle, policy: RuntimePolicy::default(), model: None, controls: Default::default(), mcp_servers: Vec::new() };
        let mut state = SessionState::new(thread, settings.parent_call.clone());
        state.subagents_started_by(settings.subagents.started_by.clone());
        let turn = state.begin_turn(&TurnInput::of("go"));
        let recorded: Value = serde_json::from_str(fixture).unwrap();
        for one in recorded["updates"].as_array().unwrap() {
            let session = one["sessionId"].as_str().unwrap();
            if !state.take_raw(session, &one["update"]) {
                state.apply(&serde_json::from_value(one["update"].clone()).unwrap());
            }
        }
        state.end_turn(&turn, TurnStatus::Completed, None);
        state
    }

    fn item<'a>(state: &'a SessionState, id: &ItemId) -> &'a harness_contracts::TurnItem {
        &state.items[state.item_index[id]]
    }

    /// Claude Code told we show sub-agents: its one sub-agent announced
    /// (its type, its task, that it can be stopped alone), its prompt, its
    /// step under it, what it was doing, its end (stopped alone); the
    /// agent's own answer after it isn't its.
    #[test]
    fn a_sub_agent_claude_code_announces_is_one_with_its_steps_under_it() {
        let state = replayed("claude-code", include_str!("../../fixtures/subagent-cancelled-claude-code-0.95.0.json"));
        let [one] = state.subagents.as_slice() else { panic!("{:?}", state.subagents) };
        assert_eq!((one.title.as_str(), one.description.as_deref(), one.can_cancel, one.status), ("general-purpose", Some("Run sleep 90"), true, SubagentStatus::Cancelled));
        assert!(one.prompt.as_deref().is_some_and(|p| p.starts_with("Your only job: run the shell command `sleep 90`")), "{:?}", one.prompt);
        assert_eq!(one.doing.as_deref(), Some("sleep 90"));
        assert_eq!(item(&state, &one.item_id).status, ItemStatus::Cancelled);
        let under: Vec<&harness_contracts::TurnItem> = state.items.iter().filter(|i| i.parent_id.as_ref() == Some(&one.item_id)).collect();
        assert!(under.iter().any(|i| i.title.as_deref() == Some("sleep 90")), "its call, under it: {under:?}");
        let answer = state.items.iter().rev().find(|i| matches!(&i.body, ItemBody::AssistantMessage { .. })).unwrap();
        assert!(answer.parent_id.is_none() && format!("{:?}", answer.body).contains("parent done"), "the agent's own: {answer:?}");
        assert_eq!(state.cancellable(&one.id).as_deref(), Some(one.id.as_str()), "stopped alone by its own session");
    }

    /// Antigravity tells of a sub-agent only by the call that starts it
    /// (once allowed): who it is, from the call's input; it ends with the
    /// turn (its harness says nothing of its end); its steps can't be told
    /// apart, and it can't be stopped alone.
    #[test]
    fn a_sub_agent_antigravity_starts_is_one_from_its_start_call() {
        let state = replayed("antigravity-acp", include_str!("../../fixtures/subagent-antigravity-1.4.3.json"));
        let [one] = state.subagents.as_slice() else { panic!("{:?}", state.subagents) };
        assert_eq!((one.title.as_str(), one.kind.as_deref(), one.can_cancel, one.status), ("File Counter", Some("research"), false, SubagentStatus::Idle));
        assert!(one.prompt.as_deref().is_some_and(|p| p.starts_with("Count the .py files")) && one.description.as_deref() == one.prompt.as_deref(), "{one:?}");
        assert!(state.items.iter().all(|i| i.parent_id.as_ref() != Some(&one.item_id)), "nothing said to be its");
        assert_eq!(state.cancellable(&one.id), None);
        // The call that asked is answered, not failed.
        let asked = state.items.iter().find(|i| i.title.as_deref().is_some_and(|t| t.contains("invoke_subagent"))).unwrap();
        assert_eq!(asked.status, ItemStatus::Completed);
    }

    /// Asked for, then refused: never started.
    #[test]
    fn a_start_call_refused_starts_nothing() {
        let settings = shipped("antigravity-acp");
        let thread = Thread { id: ThreadId::new("t"), instance_id: InstanceId::new("i"), native_id: None, title: None, status: ThreadStatus::Idle, policy: RuntimePolicy::default(), model: None, controls: Default::default(), mcp_servers: Vec::new() };
        let mut state = SessionState::new(thread, None);
        state.subagents_started_by(settings.subagents.started_by.clone());
        state.begin_turn(&TurnInput::of("go"));
        let input = serde_json::json!({ "Subagents": [{ "Role": "Helper", "Prompt": "x", "TypeName": "research" }] });
        state.apply(&acp::SessionUpdate::ToolCall(acp::ToolCall::new("c1", "Run invoke_subagent?").raw_input(input).status(acp::ToolCallStatus::Pending)));
        assert!(state.subagents.is_empty(), "asked: not yet");
        state.apply(&acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new("c1", acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Failed))));
        state.apply(&acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new("c1", acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed))));
        assert!(state.subagents.is_empty(), "refused: never");
    }
}
