//! Pure conversions from ACP schema types to harness contracts.

use agent_client_protocol::schema::v1 as acp;
use harness_contracts::{
    ApprovalDecision, ApprovalOption, ConfigCategory, ConfigChoice, ConfigControl, ConfigKind, ContentBlock, Diff,
    FileLocation, ItemStatus, ModeOption, ModelOption, PlanStep, SlashCommand, StepStatus, ToolKind, TurnStatus, Usage,
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
pub fn approval_option(option: &acp::PermissionOption) -> ApprovalOption {
    let decision = match option.kind {
        acp::PermissionOptionKind::AllowOnce => ApprovalDecision::Accept,
        acp::PermissionOptionKind::AllowAlways => ApprovalDecision::AcceptForSession,
        acp::PermissionOptionKind::RejectOnce | acp::PermissionOptionKind::RejectAlways => ApprovalDecision::Decline,
        _ => ApprovalDecision::Decline,
    };
    ApprovalOption { id: option.option_id.to_string(), decision, label: option.name.clone(), warning: None }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_kinds_map_to_decisions() {
        let opt = |kind| acp::PermissionOption::new("id", "label", kind);
        assert_eq!(approval_option(&opt(acp::PermissionOptionKind::AllowOnce)).decision, ApprovalDecision::Accept);
        assert_eq!(approval_option(&opt(acp::PermissionOptionKind::AllowAlways)).decision, ApprovalDecision::AcceptForSession);
        assert_eq!(approval_option(&opt(acp::PermissionOptionKind::RejectOnce)).decision, ApprovalDecision::Decline);
    }

    #[test]
    fn stop_reasons_map_to_turn_status() {
        assert_eq!(stop_reason(&acp::StopReason::EndTurn), TurnStatus::Completed);
        assert_eq!(stop_reason(&acp::StopReason::Cancelled), TurnStatus::Cancelled);
        assert_eq!(stop_reason(&acp::StopReason::Refusal), TurnStatus::Failed);
    }
}
