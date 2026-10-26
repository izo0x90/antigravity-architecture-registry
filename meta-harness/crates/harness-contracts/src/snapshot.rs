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
    /// The tool (MCP) servers its own configuration adds.
    #[serde(default)]
    pub tool_servers: Vec<ToolServerInfo>,
    /// What couldn't be read of its configuration (skills, tool servers),
    /// in words.
    #[serde(default)]
    pub problems: Vec<String>,
    /// Where it keeps its home (login, settings, sessions), when its pack
    /// says.
    #[serde(default)]
    pub home: Option<HarnessHome>,
}

impl ProviderSnapshot {
    /// A harness just made: starting, nothing known of it yet.
    pub fn new(instance_id: InstanceId, driver: DriverKind, display_name: impl Into<String>) -> Self {
        Self {
            instance_id,
            driver,
            display_name: display_name.into(),
            status: ProviderStatus::Starting,
            auth: AuthState { status: AuthStatus::Unknown, account: None, methods: Vec::new(), waiting: None, ended: None },
            capabilities: Capabilities::default(),
            models: Vec::new(),
            commands: Vec::new(),
            skills: Vec::new(),
            runtime_modes: Vec::new(),
            supports_plan_mode: false,
            tool_servers: Vec::new(),
            problems: Vec::new(),
            home: None,
        }
    }
}

/// A harness's home: the folder, and whether the project's settings put it
/// there (else it's the harness's own).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessHome {
    pub path: String,
    pub kept: bool,
}

/// A sign-in under way on the project's server, for any device to finish:
/// the page to open, what to paste back, until when it waits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInWaiting {
    /// Which sign-in (a new one replaces it): what's pasted back or
    /// cancelled names it.
    pub id: u64,
    /// The page to open; none while it's starting.
    pub url: Option<String>,
    /// What to do, in words.
    pub instructions: String,
    /// What it takes back, in words.
    pub expects: Option<String>,
    /// When it stops waiting (milliseconds since the epoch, the server's
    /// clock), if it says.
    pub until_ms: Option<u64>,
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
    pub account: Option<Account>,
    pub methods: Vec<AuthMethod>,
    /// A sign-in under way.
    #[serde(default)]
    pub waiting: Option<Box<SignInWaiting>>,
    /// How the last sign-in ended, once it has (none while one is under way).
    #[serde(default)]
    pub ended: Option<Box<SignInEnded>>,
}

/// How a sign-in ended, as the server knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInEnded {
    /// Which sign-in.
    pub id: u64,
    pub how: SignInEnd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum SignInEnd {
    SignedIn,
    /// It failed, saying why.
    Failed {
        reason: String,
    },
    /// Its time ran out.
    RanOut,
    /// Cancelled, or a new one started in its place.
    Stopped,
}

/// Who the harness is signed in as, as it reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// How it's signed in, in words ("Claude Max", "Anthropic API key").
    pub label: String,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub organization: Option<String>,
}

/// A sign-in as the harness reports it (status and account).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignIn {
    pub status: AuthStatus,
    pub account: Option<Account>,
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
    #[serde(default)]
    pub kind: AuthMethodKind,
}

/// How a login runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethodKind {
    /// The harness runs it itself (it may open a browser and wait).
    #[default]
    Agent,
    /// The server runs the harness's login command: it shows a link, and
    /// what the page gives back is pasted in.
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub is_default: bool,
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
    #[serde(default)]
    pub scope: Scope,
}

/// Where something a harness has comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// The user's own (every project).
    #[default]
    User,
    /// This project's.
    Project,
}

/// A tool (MCP) server the harness's own configuration gives its sessions
/// (on top of the ones the meta-harness passes it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolServerInfo {
    pub name: String,
    /// `http`, `sse` or `stdio`.
    pub transport: String,
    /// The configuration file that adds it.
    pub file: String,
    pub scope: Scope,
}
