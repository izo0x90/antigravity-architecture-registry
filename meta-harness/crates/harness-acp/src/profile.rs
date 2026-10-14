//! One agent in one project: its settings put to use. Everything the
//! adapter does differently per agent goes through here, so an agent is
//! data (its pack's settings), never code.

use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1 as acp;
use harness_contracts::{
    Account, ApprovalCapabilities, ApprovalOption, AuthStatus, Capabilities, InteractionMode, Launch,
    PlanningCapabilities, RuntimeMode, RuntimePolicy, SessionCapabilities, SignIn, StreamingCapabilities,
    ToolCapabilities, ToolKind, TurnCapabilities,
};

use crate::convert;
use crate::settings::{AcpSettings, Places};
use crate::workspace::WorkspaceFs;

pub struct Profile {
    pub settings: AcpSettings,
    /// The project its sessions, checks and logins run in.
    pub project: PathBuf,
    pub places: Places,
}

/// What the agent's `initialize` answer says it does beyond the protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Declared {
    pub queueing: bool,
    pub steering: bool,
    /// It sends who it's signed in as.
    pub sign_in: bool,
}

impl Profile {
    pub fn new(settings: AcpSettings, project: &Path, launch: Option<&Launch>) -> Self {
        let places = Places::new(&settings, project, launch);
        Self { settings, project: project.to_path_buf(), places }
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
    pub fn approval_options(&self, native: &[acp::PermissionOption]) -> Vec<ApprovalOption> {
        native
            .iter()
            .map(|option| {
                let mut option = convert::approval_option(option);
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
        self.settings.client_fs.then(|| WorkspaceFs::new(vec![cwd.to_path_buf()]))
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
