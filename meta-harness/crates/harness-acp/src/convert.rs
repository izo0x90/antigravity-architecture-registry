//! Pure conversions from ACP schema types to harness contracts.

use agent_client_protocol::schema::v1 as acp;
use harness_contracts::{
    ConfigCategory, ConfigChoice, ConfigControl, ConfigKind, ContentBlock, Diff,
    FileLocation, ItemStatus, ModeOption, ModelOption, PlanStep, SlashCommand, StepStatus, SubagentStatus, ToolKind, TurnStatus, Usage,
};

pub fn content_block(block: &acp::ContentBlock) -> ContentBlock {
    match block {
        acp::ContentBlock::Text(t) => ContentBlock::Text { text: t.text.clone() },
        acp::ContentBlock::Image(i) => ContentBlock::Image {
            mime_type: i.mime_type.clone(),
            data_base64: Some(i.data.clone()),
            uri: i.uri.clone(),
        },
        acp::ContentBlock::ResourceLink(l) => ContentBlock::ResourceLink { uri: l.uri.clone(), name: Some(l.name.clone()) },
        acp::ContentBlock::Resource(r) => match &r.resource {
            acp::EmbeddedResourceResource::TextResourceContents(t) => {
                ContentBlock::Resource { uri: t.uri.clone(), text: Some(t.text.clone()) }
            }
            acp::EmbeddedResourceResource::BlobResourceContents(b) => ContentBlock::Resource { uri: b.uri.clone(), text: None },
            _ => ContentBlock::Unsupported,
        },
        _ => ContentBlock::Unsupported,
    }
}

pub fn tool_content(content: &acp::ToolCallContent) -> ContentBlock {
    match content {
        acp::ToolCallContent::Content(c) => content_block(&c.content),
        acp::ToolCallContent::Diff(d) => ContentBlock::Diff(Diff {
            path: d.path.display().to_string(),
            old_text: d.old_text.clone(),
            new_text: d.new_text.clone(),
        }),
        acp::ToolCallContent::Terminal(t) => ContentBlock::Text { text: format!("[terminal {}]", t.terminal_id) },
        _ => ContentBlock::Unsupported,
    }
}

pub fn tool_kind(kind: &acp::ToolKind) -> ToolKind {
    match kind {
        acp::ToolKind::Read => ToolKind::Read,
        acp::ToolKind::Edit => ToolKind::Edit,
        acp::ToolKind::Delete => ToolKind::Delete,
        acp::ToolKind::Move => ToolKind::Move,
        acp::ToolKind::Search => ToolKind::Search,
        acp::ToolKind::Execute => ToolKind::Execute,
        acp::ToolKind::Think => ToolKind::Think,
        acp::ToolKind::Fetch => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

pub fn tool_status(status: &acp::ToolCallStatus) -> ItemStatus {
    match status {
        acp::ToolCallStatus::Pending => ItemStatus::Pending,
        acp::ToolCallStatus::InProgress => ItemStatus::InProgress,
        acp::ToolCallStatus::Completed => ItemStatus::Completed,
        acp::ToolCallStatus::Failed => ItemStatus::Failed,
        _ => ItemStatus::InProgress,
    }
}

pub fn location(loc: &acp::ToolCallLocation) -> FileLocation {
    FileLocation { path: loc.path.display().to_string(), line: loc.line }
}

pub fn plan_steps(plan: &acp::Plan) -> Vec<PlanStep> {
    plan.entries
        .iter()
        .map(|e| PlanStep {
            text: e.content.clone(),
            status: match e.status {
                acp::PlanEntryStatus::Completed => StepStatus::Completed,
                acp::PlanEntryStatus::InProgress => StepStatus::InProgress,
                _ => StepStatus::Pending,
            },
        })
        .collect()
}

pub fn commands(update: &acp::AvailableCommandsUpdate) -> Vec<SlashCommand> {
    update
        .available_commands
        .iter()
        .map(|c| SlashCommand {
            name: c.name.clone(),
            description: c.description.clone(),
            input_hint: c.input.as_ref().and_then(|input| match input {
                acp::AvailableCommandInput::Unstructured(u) => Some(u.hint.clone()),
                _ => None,
            }),
        })
        .collect()
}

pub fn usage(update: &acp::UsageUpdate) -> Usage {
    Usage { context_used_tokens: Some(update.used), context_window_tokens: Some(update.size), ..Usage::default() }
}

pub fn stop_reason(reason: &acp::StopReason) -> TurnStatus {
    match reason {
        acp::StopReason::Cancelled => TurnStatus::Cancelled,
        acp::StopReason::Refusal => TurnStatus::Failed,
        _ => TurnStatus::Completed,
    }
}

/// Default mapping of native permission options; flavors may override labels/warnings.
pub fn modes(state: &acp::SessionModeState) -> (Vec<ModeOption>, Option<String>) {
    let modes = state
        .available_modes
        .iter()
        .map(|m| ModeOption { id: m.id.to_string(), name: m.name.clone(), description: m.description.clone() })
        .collect();
    (modes, Some(state.current_mode_id.to_string()))
}

fn category(category: Option<&acp::SessionConfigOptionCategory>) -> ConfigCategory {
    match category {
        Some(acp::SessionConfigOptionCategory::Mode) => ConfigCategory::Mode,
        Some(acp::SessionConfigOptionCategory::Model) => ConfigCategory::Model,
        Some(acp::SessionConfigOptionCategory::ModelConfig) => ConfigCategory::ModelConfig,
        Some(acp::SessionConfigOptionCategory::ThoughtLevel) => ConfigCategory::ThoughtLevel,
        _ => ConfigCategory::Other,
    }
}

fn choice(option: &acp::SessionConfigSelectOption, group: Option<&str>) -> ConfigChoice {
    ConfigChoice {
        value: option.value.to_string(),
        name: option.name.clone(),
        description: option.description.clone(),
        group: group.map(str::to_owned),
    }
}

pub fn config_controls(options: &[acp::SessionConfigOption]) -> Vec<ConfigControl> {
    options
        .iter()
        .filter_map(|o| {
            let kind = match &o.kind {
                acp::SessionConfigKind::Select(select) => {
                    let choices = match &select.options {
                        acp::SessionConfigSelectOptions::Ungrouped(list) => list.iter().map(|c| choice(c, None)).collect(),
                        acp::SessionConfigSelectOptions::Grouped(groups) => groups
                            .iter()
                            .flat_map(|g| g.options.iter().map(|c| choice(c, Some(&g.name))))
                            .collect(),
                        _ => Vec::new(),
                    };
                    ConfigKind::Select { choices, current: select.current_value.to_string() }
                }
                acp::SessionConfigKind::Boolean(b) => ConfigKind::Boolean { current: b.current_value },
                _ => return None,
            };
            Some(ConfigControl {
                id: o.id.to_string(),
                name: o.name.clone(),
                description: o.description.clone(),
                category: category(o.category.as_ref()),
                kind,
            })
        })
        .collect()
}

/// The model catalog implied by the model-category config option.
pub fn models_from_controls(controls: &[ConfigControl]) -> Option<Vec<ModelOption>> {
    controls.iter().find(|c| c.category == ConfigCategory::Model).and_then(|c| match &c.kind {
        ConfigKind::Select { choices, current } => Some(
            choices
                .iter()
                .map(|ch| ModelOption {
                    id: ch.value.clone(),
                    name: ch.name.clone(),
                    description: ch.description.clone(),
                    is_default: &ch.value == current,
                })
                .collect(),
        ),
        ConfigKind::Boolean { .. } => None,
    })
}

/// A `session/update` as it's read: the session it's under, and what it
/// says. Read from the message as it came (not copied first); kinds the
/// ACP schema we build on doesn't have yet (the sub-agents draft) are read
/// here too, so its format has one owner.
pub fn session_update(params: serde_json::Value) -> Option<(String, Update)> {
    let serde_json::Value::Object(mut params) = params else { return None };
    let session = params.get("sessionId").and_then(serde_json::Value::as_str)?.to_owned();
    let update = params.remove("update")?;
    let text = |key: &str| update.get(key).and_then(serde_json::Value::as_str).map(str::to_owned);
    let read = match update.get("sessionUpdate").and_then(serde_json::Value::as_str).unwrap_or_default() {
        "subagent_update" => Update::Subagent(SubagentUpdate {
            session: text("sessionId")?,
            title: text("title"),
            description: text("description"),
            status: update.get("state").map(subagent_status),
            stop: update.get("capabilities").map(|c| if c.get("cancel").is_some() { Stop::Alone } else { Stop::WithTheTurn }),
        }),
        "session_message" => Update::Message(SessionMessage {
            from: text("senderSessionId"),
            to: text("recipientSessionId"),
            text: update.get("content").and_then(serde_json::Value::as_array).into_iter().flatten().filter_map(|b| b.get("text").and_then(serde_json::Value::as_str)).collect(),
        }),
        // Its parts as they stream: the whole one comes as `session_message`.
        "session_message_chunk" => return None,
        kind => {
            let kind = kind.to_owned();
            match serde_json::from_value::<acp::SessionUpdate>(update) {
                Ok(update) => Update::Typed(update),
                Err(e) => Update::Unread { kind, why: e.to_string() },
            }
        }
    };
    Some((session, read))
}

/// What a `session/update` says.
#[derive(Debug)]
#[allow(clippy::large_enum_variant, reason = "made and taken at once, per update: a box would cost an allocation each")]
pub enum Update {
    Typed(acp::SessionUpdate),
    /// A sub-agent announced, or its state changed.
    Subagent(SubagentUpdate),
    /// A message between the agent and a sub-agent (its task, its answer).
    Message(SessionMessage),
    /// Of a kind it can't read (`why`).
    Unread { kind: String, why: String },
}

#[derive(Debug, Default)]
pub struct SubagentUpdate {
    /// Its own session.
    pub session: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<SubagentStatus>,
    /// How it can be stopped (none: not said).
    pub stop: Option<Stop>,
}

/// How a sub-agent can be stopped, as its harness says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// On its own, the turn going on.
    Alone,
    /// Only with the turn.
    WithTheTurn,
}

#[derive(Debug, Default)]
pub struct SessionMessage {
    pub from: Option<String>,
    pub to: Option<String>,
    pub text: String,
}

/// A sub-agent's state as the draft says it (`running`, `requires_action`,
/// `idle` with how it stopped, `unknown`).
fn subagent_status(state: &serde_json::Value) -> SubagentStatus {
    match (state.get("state").and_then(serde_json::Value::as_str), state.get("stopReason").and_then(serde_json::Value::as_str)) {
        (Some("running"), _) => SubagentStatus::Running,
        (Some("requires_action"), _) => SubagentStatus::Waiting,
        (Some("idle"), Some("end_turn")) => SubagentStatus::Completed,
        (Some("idle"), Some("cancelled")) => SubagentStatus::Cancelled,
        (Some("idle"), Some("error" | "refusal")) => SubagentStatus::Failed,
        _ => SubagentStatus::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reasons_map_to_turn_status() {
        assert_eq!(stop_reason(&acp::StopReason::EndTurn), TurnStatus::Completed);
        assert_eq!(stop_reason(&acp::StopReason::Cancelled), TurnStatus::Cancelled);
        assert_eq!(stop_reason(&acp::StopReason::Refusal), TurnStatus::Failed);
    }
}
