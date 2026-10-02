//! App-level runtime policy and model selection. Adapters map these to native
//! modes/models; clients never send harness-native mode strings.

use serde::{Deserialize, Serialize};

use crate::ids::InstanceId;

/// How much the agent may do without asking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeMode {
    #[default]
    ApprovalRequired,
    AutoAcceptEdits,
    Auto,
    FullAccess,
}

/// Whether the agent should act, or plan and wait for approval.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InteractionMode {
    #[default]
    Default,
    Plan,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePolicy {
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: InteractionMode,
    pub cwd: Option<String>,
    pub reasoning_effort: Option<String>,
}

/// Which model to use, with values for the options the model advertises
/// (see `ModelOption::options`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    pub instance_id: InstanceId,
    pub model: String,
    #[serde(default)]
    pub options: Vec<OptionValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionValue {
    pub id: String,
    pub value: serde_json::Value,
}
