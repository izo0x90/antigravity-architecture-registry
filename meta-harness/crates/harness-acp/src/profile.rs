//! One agent in one project: its settings put to use. Everything the
//! adapter does differently per agent goes through here, so an agent is
//! data (its pack's settings), never code.

use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1 as acp;
use harness_contracts::{
    Account, ApprovalCapabilities, ApprovalOption, AuthStatus, RequestKind, Capabilities, InteractionMode, Launch,
    PlanningCapabilities, RuntimeMode, RuntimePolicy, SessionCapabilities, SignIn, StreamingCapabilities, SubagentCapabilities,
    ToolCapabilities, ToolKind, TurnCapabilities,
};

use crate::settings::{AcpSettings, Places};
use crate::workspace::WorkspaceFs;

pub struct Profile {
    pub settings: AcpSettings,
    /// The project its sessions, checks and logins run in.
    pub project: PathBuf,
    pub places: Places,
    /// The folder the project keeps its home in (none: its own folders):
    /// its login is there.
    pub kept_home: Option<PathBuf>,
    /// What of the project its file access never reaches (the server's own,
    /// harnesses' kept homes included).
    pub private: Vec<PathBuf>,
    /// Its settings' replay, read once (checked when they were read).
    replay: Option<crate::pieces::Replay>,
}

/// What the agent's `initialize` answer says it does beyond the protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Declared {
    pub queueing: bool,
    pub steering: bool,
    /// It takes a message's pieces as pieces (embedded resources).
    pub pieces: bool,
    /// It sends who it's signed in as.
    pub sign_in: bool,
}

impl Profile {
    pub fn new(settings: AcpSettings, project: &Path, launch: Option<&Launch>) -> Self {
        let places = Places::new(&settings, project, launch);
        let replay = settings.replay.as_ref().and_then(|r| crate::pieces::Replay::parse(&r.piece, r.listed.as_deref()).ok());
        Self { settings, project: project.to_path_buf(), places, kept_home: None, private: Vec::new(), replay }
    }

    /// With its home kept in `dir` (by the project).
    pub fn keeping_home(mut self, dir: Option<PathBuf>) -> Self {
        self.kept_home = dir;
        self
    }

    /// With `private` out of its file access's reach.
    pub fn keeping_out(mut self, private: Vec<PathBuf>) -> Self {
        self.private = private;
        self
    }

    /// Where its home is, if its pack says, and whether the project keeps it.
    pub fn home(&self) -> Option<harness_contracts::HarnessHome> {
        let home = self.settings.home.as_ref()?;
        Some(match &self.kept_home {
            Some(kept) => harness_contracts::HarnessHome { path: kept.display().to_string(), kept: true },
            None => harness_contracts::HarnessHome { path: self.places.path(&format!("{{{}}}", home.dir)).display().to_string(), kept: false },
        })
    }

    /// What it can do before it's asked (`initialize` fills in the rest).
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            sessions: SessionCapabilities { runtime_mode_switch_in_session: true, model_switch_in_session: true, ..Default::default() },
            turns: TurnCapabilities { interrupt: true, ..Default::default() },
            streaming: StreamingCapabilities { assistant_text: true, reasoning: true, tool_output: true },
            tools: ToolCapabilities { mcp_http: true, client_file_system: self.settings.client_fs, ..Default::default() },
            approvals: ApprovalCapabilities { command_approval: true, file_change_approval: true, live_only: true },
            planning: PlanningCapabilities { todo_list: true, structured_questions: self.settings.advertise.elicitation_form, ..Default::default() },
            // Told of by the call that starts them, or announced with their
            // own steps and state.
            subagents: SubagentCapabilities { supported: self.settings.subagents.declare || self.settings.subagents.started_by.is_some(), lifecycle_events: self.settings.subagents.declare },
            ..Default::default()
        }
    }

    pub fn runtime_modes(&self) -> Vec<RuntimeMode> {
        self.settings.modes.offered()
    }

    pub fn supports_plan_mode(&self) -> bool {
        self.settings.modes.plan.is_some()
    }

    /// Where a session works: the folder asked for, else the project.
    pub fn cwd(&self, policy: &RuntimePolicy) -> PathBuf {
        policy.cwd.as_ref().map_or_else(|| self.project.clone(), PathBuf::from)
    }

    /// Its mode for a policy, if the session has it: planning when asked
    /// for and it plans, else the runtime mode's.
    pub fn native_mode(&self, policy: &RuntimePolicy, available: &[String]) -> Option<String> {
        let modes = &self.settings.modes;
        let plan = (policy.interaction_mode == InteractionMode::Plan).then_some(modes.plan.as_deref()).flatten();
        plan.or_else(|| modes.native(policy.runtime_mode)).filter(|m| available.iter().any(|a| a == m)).map(str::to_owned)
    }

    /// The policy after the agent switched to one of its modes.
    pub fn policy_for(&self, native: &str, mut policy: RuntimePolicy) -> RuntimePolicy {
        if self.settings.modes.plan.as_deref() == Some(native) {
            policy.interaction_mode = InteractionMode::Plan;
        } else if let Some(mode) = self.settings.modes.ours(native) {
            policy.runtime_mode = mode;
            policy.interaction_mode = InteractionMode::Default;
        }
        policy
    }

    /// The agent's options for a permission request, with their warnings.
    /// A permission request's options as `asked` (an approval's allow and
    /// deny, with this harness's warnings; a question's answers).
    pub fn approval_options(&self, asked: RequestKind, native: &[acp::PermissionOption]) -> Vec<ApprovalOption> {
        crate::classify::options(asked, native)
            .into_iter()
            .map(|mut option| {
                option.warning = self.settings.approval_warnings.get(&option.decision).cloned();
                option
            })
            .collect()
    }

    /// Its "allow once" option when the runtime mode lets a tool of `kind`
    /// run without asking.
    pub fn auto_approve(&self, policy: &RuntimePolicy, kind: &ToolKind, native: &[acp::PermissionOption]) -> Option<String> {
        let allowed = match policy.runtime_mode {
            RuntimeMode::FullAccess | RuntimeMode::Auto => true,
            RuntimeMode::AutoAcceptEdits => matches!(kind, ToolKind::Edit | ToolKind::Delete | ToolKind::Move | ToolKind::Read),
            RuntimeMode::ApprovalRequired => false,
        };
        if !allowed {
            return None;
        }
        native.iter().find(|o| matches!(o.kind, acp::PermissionOptionKind::AllowOnce)).map(|o| o.option_id.to_string())
    }

    /// What we tell it we can do. Files only when it's given them.
    pub fn client_capabilities(&self, files: bool) -> acp::ClientCapabilities {
        let advertise = &self.settings.advertise;
        let mut caps = acp::ClientCapabilities::new();
        if files {
            caps = caps.fs(acp::FileSystemCapabilities::new().read_text_file(true).write_text_file(true));
        }
        if advertise.auth_terminal {
            caps = caps.auth(acp::AuthCapabilities::new().terminal(true));
        }
        if advertise.elicitation_form {
            caps = caps.elicitation(acp::ElicitationCapabilities::new().form(acp::ElicitationFormCapabilities::new()));
        }
        if !advertise.meta.is_empty() {
            caps = caps.meta(advertise.meta.clone());
        }
        caps
    }

    /// The `initialize` we send: our capabilities, and that we show its
    /// sub-agents when its pack says it tells of them so (a capability the
    /// ACP schema we build on doesn't have yet: added to what it writes).
    pub fn initialize(&self, files: bool) -> Result<agent_client_protocol::UntypedMessage, agent_client_protocol::Error> {
        let request = acp::InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1).client_capabilities(self.client_capabilities(files));
        let mut params = serde_json::to_value(request).map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;
        if self.settings.subagents.declare
            && let Some(caps) = params.get_mut("clientCapabilities").and_then(serde_json::Value::as_object_mut)
        {
            caps.insert("subagents".into(), serde_json::json!({}));
        }
        agent_client_protocol::UntypedMessage::new("initialize", params)
    }

    /// How a replayed message's pieces are read back: as its agent writes
    /// them, when it takes pieces (its pack says how, else they're shown
    /// as they come), or as we write them for one that takes none.
    pub fn replayed_pieces(&self, declared: &Declared) -> Option<crate::pieces::Replay> {
        match declared.pieces {
            true => self.replay.clone(),
            false => Some(crate::pieces::Replay::ours()),
        }
    }

    /// What a chunk of a message of yours it replays is.
    pub fn chunks(&self) -> crate::state::Chunks {
        match self.settings.replay.as_ref().is_some_and(|r| r.whole_chunks) {
            true => crate::state::Chunks::Whole,
            false => crate::state::Chunks::OfItsId,
        }
    }

    pub fn session_meta(&self) -> Option<acp::Meta> {
        (!self.settings.session_meta.is_empty()).then(|| self.settings.session_meta.clone())
    }

    pub fn declared(&self, init: &acp::InitializeResponse) -> Declared {
        let json = serde_json::to_value(init).unwrap_or_default();
        let says = |pointer: Option<&str>| pointer.and_then(|p| json.pointer(p)).is_some_and(|v| !matches!(v, serde_json::Value::Bool(false) | serde_json::Value::Null));
        let sign_in = self.settings.sign_in.as_ref();
        Declared {
            queueing: says(self.settings.queueing.as_deref()),
            steering: says(self.settings.steering.as_deref()),
            pieces: init.agent_capabilities.prompt_capabilities.embedded_context,
            sign_in: sign_in.is_some_and(|s| s.declared.is_none() || says(s.declared.as_deref())),
        }
    }

    /// Who it says it's signed in as, if `method` is how it says so.
    pub fn sign_in(&self, method: &str, params: &serde_json::Value) -> Option<SignIn> {
        let settings = self.settings.sign_in.as_ref().filter(|s| s.notification == method)?;
        let text = |pointer: &Option<String>| pointer.as_deref().and_then(|p| params.pointer(p)).and_then(serde_json::Value::as_str).map(str::to_owned);
        let status = text(&Some(settings.status.clone()))?;
        if settings.signed_out.contains(&status) {
            return Some(SignIn { status: AuthStatus::SignedOut, account: None });
        }
        let account = Account {
            label: text(&settings.label).unwrap_or(status),
            email: text(&settings.email),
            plan: text(&settings.plan),
            organization: text(&settings.organization),
        };
        Some(SignIn { status: AuthStatus::SignedIn, account: Some(account) })
    }

    /// File access for a session working in `cwd`, when it's given it.
    pub fn workspace(&self, cwd: &Path) -> Option<WorkspaceFs> {
        self.settings.client_fs.then(|| WorkspaceFs::new(vec![cwd.to_path_buf()], self.private.iter().chain(&self.kept_home).cloned().collect()))
    }

    /// The suffix of a question's own answer box, if its forms have them.
    pub fn custom_suffix(&self) -> Option<&str> {
        self.settings.questions.custom_suffix.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Modes, SignInSettings};

    fn profile(settings: AcpSettings) -> Profile {
        Profile::new(settings, Path::new("/p"), None)
    }

    #[test]
    fn planning_and_runtime_modes_map_to_the_agents_and_back() {
        let modes = Modes { approval_required: Some("default".into()), full_access: Some("bypass".into()), plan: Some("plan".into()), ..Modes::default() };
        let p = profile(AcpSettings { modes, ..AcpSettings::default() });
        let available: Vec<String> = ["default", "bypass", "plan"].map(String::from).to_vec();
        let plan = RuntimePolicy { interaction_mode: InteractionMode::Plan, runtime_mode: RuntimeMode::FullAccess, ..RuntimePolicy::default() };
        assert_eq!(p.native_mode(&plan, &available).as_deref(), Some("plan"));
        let full = RuntimePolicy { runtime_mode: RuntimeMode::FullAccess, ..RuntimePolicy::default() };
        assert_eq!(p.native_mode(&full, &available).as_deref(), Some("bypass"));
        assert_eq!(p.native_mode(&full, &["default".into()]), None, "a mode the session lacks isn't asked for");
        assert_eq!(p.policy_for("plan", full.clone()).interaction_mode, InteractionMode::Plan);
        let back = p.policy_for("default", plan);
        assert_eq!((back.runtime_mode, back.interaction_mode), (RuntimeMode::ApprovalRequired, InteractionMode::Default));
    }

    #[test]
    fn sign_in_reads_its_notification_by_pointers() {
        let sign_in = SignInSettings {
            notification: "_auth/status_update".into(),
            declared: Some("/agentCapabilities/_meta/authStatus".into()),
            wait_s: 1,
            status: "/authStatus/kind".into(),
            signed_out: vec!["none".into()],
            label: Some("/authStatus/label".into()),
            email: Some("/authStatus/account/email".into()),
            plan: Some("/authStatus/account/plan".into()),
            organization: None,
        };
        let p = profile(AcpSettings { sign_in: Some(sign_in), ..AcpSettings::default() });
        let account = serde_json::json!({ "authStatus": { "kind": "account", "label": "Claude Max", "account": { "plan": "max", "email": "a@b.c" } } });
        let signed = p.sign_in("_auth/status_update", &account).unwrap();
        assert_eq!(signed.status, AuthStatus::SignedIn);
        assert_eq!(signed.account.unwrap(), Account { label: "Claude Max".into(), email: Some("a@b.c".into()), plan: Some("max".into()), organization: None });
        let out = p.sign_in("_auth/status_update", &serde_json::json!({ "authStatus": { "kind": "none" } })).unwrap();
        assert_eq!(out, SignIn { status: AuthStatus::SignedOut, account: None });
        assert_eq!(p.sign_in("_other", &account), None);
    }
}
