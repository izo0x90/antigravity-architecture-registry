//! Live description of a provider instance: status, auth, and the dynamic catalog
//! (models, commands, skills, modes). Clients render pickers and menus from this
//! only; it is refreshed by the driver and by catalog updates from live sessions.

use serde::{Deserialize, Serialize};

use crate::capabilities::Capabilities;
use crate::ids::{DriverKind, InstanceId};
use crate::policy::RuntimeMode;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshot {
    pub instance_id: InstanceId,
    pub driver: DriverKind,
    pub display_name: String,
    pub status: ProviderStatus,
    pub auth: AuthState,
    pub capabilities: Capabilities,
    pub models: Vec<ModelOption>,
    pub commands: Vec<SlashCommand>,
    pub skills: Vec<Skill>,
    /// Runtime modes this instance can honor.
    pub runtime_modes: Vec<RuntimeMode>,
    pub supports_plan_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ProviderStatus {
    Ready,
    Starting,
    NeedsAuth,
    Unavailable { reason: String },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthState {
    pub status: AuthStatus,
    pub account: Option<String>,
    pub methods: Vec<AuthMethod>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    Unknown,
    SignedOut,
    SignedIn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub is_default: bool,
    /// Per-model settings (effort, thinking, ...) the client renders generically.
    pub options: Vec<OptionDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionDescriptor {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub kind: OptionKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum OptionKind {
    Select { choices: Vec<OptionChoice>, current: Option<String> },
    Boolean { current: Option<bool> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionChoice {
    pub value: String,
    pub label: String,
}

/// A command invoked by sending `/name args` as a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub input_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: Option<String>,
    pub path: Option<String>,
}
