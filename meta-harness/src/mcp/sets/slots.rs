//! The lead's sub-slots: list and read them, start and close them, send
//! them work; a sub-slot's report back to the lead; any agent telling the
//! user something.

use std::sync::Arc;
use std::time::Duration;

use arch_graph::tree::TreeConfig;
use arch_service::ServiceError;
use harness_contracts::{InstanceId, ModelSelection, RuntimeMode, RuntimePolicy, Slot, SlotId, Starter};
use harness_core::{Delivery, Open, Origin, Reported, Sent, SlotManager, SlotSettings};
use nah_access::Capability;
use serde::Deserialize;
use serde_json::{Value, json};

use super::schema::tool;
use super::{ToolDef, ToolSet};
use crate::mcp::McpCaller;
use crate::mcp::text::parse;

pub(crate) struct Slots {
    pub(crate) slots: Arc<SlotManager>,
    pub(crate) notify: Arc<nah_notify::NotifyHub>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotArgs {
    slot: String,
    #[serde(default)]
    last: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartArgs {
    harness: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArgs {
    slot: String,
    text: String,
    #[serde(default)]
    add_to_running: bool,
    #[serde(default)]
    wait_secs: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextArgs {
    text: String,
}

/// The longest a send waits for its answer.
const MAX_WAIT_SECS: u64 = 600;

fn failed(e: harness_core::HarnessError) -> ServiceError {
    ServiceError::InvalidOp(e.to_string())
}

impl Slots {
    pub(crate) fn schemas(_trees: &TreeConfig) -> Vec<ToolDef> {
        let slot = json!({ "type": "string", "description": "A sub-slot: its id or its name (slot_list shows both)" });
        vec![
            tool(Capability::SlotsObserve, "slot_list", "Your sub-slots: each one's name and id, harness, model, what it's doing (running, idle, waiting on the user, failed, not running) and its sessions.", json!({ "type": "object", "properties": {} })),
            tool(
                Capability::SlotsObserve,
                "slot_read",
                "What a sub-slot is doing and its last messages (prompts and its answers).",
                json!({ "type": "object", "properties": { "slot": slot, "last": { "type": "integer", "description": "How many messages (default 10)" } }, "required": ["slot"] }),
            ),
            tool(
                Capability::SlotsManage,
                "slot_start",
                "Start a sub-slot: an agent of its own on a harness (and model), working under you. It can read the IDE, the graph and the code, and report back to you; it can't drive the user's view or edit plans. Its own harness tools (editing files, running commands) follow its mode and the user's approvals.",
                json!({ "type": "object", "properties": {
                    "harness": { "type": "string", "description": "The harness instance, e.g. claude-code or antigravity" },
                    "name": { "type": "string" },
                    "model": { "type": "string", "description": "A model the harness offers" },
                    "mode": { "type": "string", "enum": ["approval-required", "auto-accept-edits", "auto", "full-access"], "description": "How much it may do without asking (default: approval-required)" }
                }, "required": ["harness"] }),
            ),
            tool(Capability::SlotsManage, "slot_close", "Close a sub-slot and its sessions.", json!({ "type": "object", "properties": { "slot": slot }, "required": ["slot"] })),
            tool(
                Capability::SlotsMessage,
                "slot_send",
                "Send a sub-slot a message: it's queued after what it's doing (or added to its running turn), marked as from you. With wait_secs, waits that long for its answer.",
                json!({ "type": "object", "properties": {
                    "slot": slot,
                    "text": { "type": "string" },
                    "add_to_running": { "type": "boolean", "description": "Into its running turn, when its harness takes that" },
                    "wait_secs": { "type": "integer", "description": "Wait up to this long for its answer (at most 600)" }
                }, "required": ["slot", "text"] }),
            ),
            tool(
                Capability::SlotsReport,
                "tell_lead",
                "Report to the lead: your message is queued for it (never cutting into what it's doing). If no lead session is running, it's kept and delivered when one starts. The user sees it too.",
                json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }),
            ),
            tool(
                Capability::UserNotify,
                "tell_user",
                "Tell the user something they should know (a result, a blocker, a decision you need them to look at). It shows in their notifications, named as from you. For a question you're waiting on, ask in your reply instead.",
                json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }),
            ),
        ]
    }

    /// A sub-slot by id or name.
    fn find(&self, which: &str) -> Result<Slot, ServiceError> {
        let all = self.slots.list();
        if let Some(slot) = all.iter().find(|s| s.id.as_str() == which) {
            return Ok(slot.clone());
        }
        let named: Vec<&Slot> = all.iter().filter(|s| s.name == which).collect();
        match named.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(ServiceError::NotFound(format!("slot {which:?} (slot_list shows them)"))),
            _ => Err(ServiceError::InvalidOp(format!("several slots are named {which:?}: use its id"))),
        }
    }

    /// A sub-slot the lead may act on: not itself, not a queue worker.
    fn sub(&self, which: &str) -> Result<Slot, ServiceError> {
        let slot = self.find(which)?;
        match slot.role.as_str() {
            "sub" => Ok(slot),
            "lead" => Err(ServiceError::InvalidOp("that's you: send to a sub-slot".into())),
            other => Err(ServiceError::InvalidOp(format!("{} is a {other} slot, not one of yours", slot.name))),
        }
    }

    fn describe(&self, slot: &Slot) -> String {
        let state = self.slots.read(&slot.id, 0).map(|r| r.state.word()).unwrap_or("unknown");
        let harness = slot.instance_id.as_ref().map_or("no harness", |i| i.as_str());
        let model = slot.model.as_ref().map(|m| format!(", {}", m.model)).unwrap_or_default();
        format!("{} (id {}): {harness}{model}; {state}; {} session(s)", slot.name, slot.id, slot.sessions.len())
    }
}

fn mode(name: &str) -> Result<RuntimeMode, ServiceError> {
    serde_json::from_value(Value::String(name.into())).map_err(|_| ServiceError::InvalidOp(format!("mode {name:?}: use approval-required, auto-accept-edits, auto or full-access")))
}

#[async_trait::async_trait]
impl ToolSet for Slots {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value, caller: &McpCaller) -> Result<String, ServiceError> {
        match name {
            "slot_list" => {
                let subs: Vec<String> = self.slots.list().iter().filter(|s| s.role == "sub").map(|s| format!("- {}", self.describe(s))).collect();
                Ok(if subs.is_empty() {
                    "No sub-slots.\n\nNext: slot_start one on a harness to give it work.".into()
                } else {
                    format!("{}\n\nNext: slot_read one to see what it's doing, or slot_send it work.", subs.join("\n"))
                })
            }
            "slot_read" => {
                let a: SlotArgs = parse(args)?;
                let slot = self.sub(&a.slot)?;
                let report = self.slots.read(&slot.id, a.last.unwrap_or(10)).map_err(failed)?;
                let mut out = format!("{} is {}.\n", slot.name, report.state.word());
                for (who, text) in &report.messages {
                    out.push_str(&format!("\n[{who}] {text}\n"));
                }
                if report.messages.is_empty() {
                    out.push_str("\nNo messages yet.\n");
                }
                out.push_str("\nNext: slot_send it more, or wait and read again.");
                Ok(out)
            }
            "slot_start" => {
                let a: StartArgs = parse(args)?;
                let instance = InstanceId::new(a.harness);
                let policy = RuntimePolicy { runtime_mode: a.mode.as_deref().map(mode).transpose()?.unwrap_or_default(), ..RuntimePolicy::default() };
                let model = a.model.map(|model| ModelSelection { instance_id: instance.clone(), model, options: Vec::new() });
                let id = self.slots.create("sub", a.name, SlotSettings { instance: Some(instance), policy, model }, Starter::Lead).await.map_err(failed)?;
                let opened = self.slots.open_session(&id, Open::New { resume: None }).await;
                let slot = self.slots.get(&id).ok_or_else(|| ServiceError::NotFound(format!("slot {id}")))?;
                match opened {
                    Ok(o) => Ok(format!("Started {}.{}\n\nNext: slot_send it work.", self.describe(&slot), o.note.map(|n| format!(" {n}")).unwrap_or_default())),
                    Err(e) => Ok(format!("Made {}, but its session didn't open: {e}\n\nNext: fix that and slot_send it (that tries again), or slot_close it.", slot.name)),
                }
            }
            "slot_close" => {
                let a: SlotArgs = parse(args)?;
                let slot = self.sub(&a.slot)?;
                self.slots.close(&slot.id).await.map_err(failed)?;
                Ok(format!("Closed {}.", slot.name))
            }
            "slot_send" => {
                let a: SendArgs = parse(args)?;
                let slot = self.sub(&a.slot)?;
                let delivery = if a.add_to_running { Delivery::AddToRunning } else { Delivery::Queue };
                let wait = a.wait_secs.map(|s| Duration::from_secs(s.min(MAX_WAIT_SECS)));
                Ok(match self.slots.send(&slot.id, a.text, delivery, Origin::Lead, wait).await.map_err(failed)? {
                    Sent::Started { .. } => format!("Sent to {}.\n\nNext: slot_read it later for its answer.", slot.name),
                    Sent::Replied { text, .. } => format!("{} answered:\n\n{text}", slot.name),
                    Sent::StillWorking { .. } => format!("{} is still working on it.\n\nNext: slot_read it later for its answer.", slot.name),
                })
            }
            "tell_lead" => {
                let a: TextArgs = parse(args)?;
                let from: SlotId = caller.slot.clone().ok_or_else(|| ServiceError::InvalidOp("only a slot's agent reports to the lead".into()))?;
                let reported = self.slots.report(&from, a.text.clone()).await.map_err(failed)?;
                let to_user = match self.notify.message(Some(&from), &a.text) {
                    Ok(_) => "",
                    Err(e) => {
                        tracing::warn!("a report couldn't be shown to the user: {e}");
                        " (It couldn't be shown to the user: their notifications failed.)"
                    }
                };
                Ok(match reported {
                    Reported::Delivered => format!("Delivered: the lead gets it after what it's doing.{to_user}"),
                    Reported::Held => format!("Kept: no lead session is running; it's delivered when one starts.{to_user}"),
                })
            }
            "tell_user" => {
                let a: TextArgs = parse(args)?;
                self.notify.message(caller.slot.as_ref(), &a.text).map_err(|e| ServiceError::InvalidOp(e.to_string()))?;
                Ok("The user has it, in their notifications.".into())
            }
            other => Err(ServiceError::NotFound(format!("tool '{other}'"))),
        }
    }
}
