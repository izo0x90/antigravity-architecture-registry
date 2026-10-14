//! Short-lived control connections: start the agent in the project,
//! `initialize`, run one operation, close. Used for checks, logins and
//! saved sessions. Who the agent says it's signed in as, if it says so
//! while connected, is handed to the operation.

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{Agent, ByteStreams, ConnectionTo, UntypedMessage};
use harness_contracts::{AuthMethod, AuthMethodKind, RuntimePolicy, SessionConfig, SignIn, ThreadId};
use harness_core::{HarnessError, Result};
use tokio::sync::watch;

use crate::launch::AcpLauncher;
use crate::profile::{Declared, Profile};
use crate::session::acp_error;

/// What a control connection is given besides the agent.
pub(crate) struct Opened {
    pub init: acp::InitializeResponse,
    /// The latest sign-in it reported (none yet: `None`).
    pub sign_in: watch::Receiver<Option<SignIn>>,
}

pub(crate) async fn with_connection<T: Send + 'static>(
    launcher: &Arc<dyn AcpLauncher>,
    profile: &Arc<Profile>,
    op: impl AsyncFnOnce(ConnectionTo<Agent>, Opened) -> agent_client_protocol::Result<T> + Send + 'static,
) -> Result<T> {
    let config = SessionConfig {
        thread_id: ThreadId::new("control"),
        policy: RuntimePolicy { cwd: Some(profile.project.display().to_string()), ..RuntimePolicy::default() },
        model: None,
        resume: None,
        mcp_servers: Vec::new(),
    };
    let transport = launcher.launch(&config).await?;
    let streams = ByteStreams::new(transport.writer, transport.reader);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let (sign_in_tx, sign_in) = watch::channel(None);
    let reader = profile.clone();
    let client = profile.client_capabilities(false);
    agent_client_protocol::Client
        .builder()
        .name("meta-harness-control")
        .on_receive_notification(
            async move |n: UntypedMessage, _cx| {
                if let Some(signed) = reader.sign_in(&n.method, &n.params) {
                    sign_in_tx.send_replace(Some(signed));
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(streams, async move |cx: ConnectionTo<Agent>| {
            let init = cx.send_request(acp::InitializeRequest::new(ProtocolVersion::V1).client_capabilities(client)).block_task().await?;
            let _ = tx.send(op(cx, Opened { init, sign_in }).await);
            Ok(())
        })
        .await
        .map_err(acp_error)?;
    drop(transport.guard);
    rx.await.map_err(|_| HarnessError::Provider { reason: "agent control connection closed".into() })?.map_err(acp_error)
}

/// What an agent reports about itself in `initialize`.
#[derive(Debug, Clone)]
pub struct AcpProbe {
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
    pub auth_methods: Vec<AuthMethod>,
    /// How to run the logins that are its own commands, by method.
    pub logins: Vec<CommandLogin>,
    pub capabilities: AcpDeclaredCapabilities,
    /// Who it said it's signed in as, if it says (and did in time).
    pub sign_in: Option<SignIn>,
}

/// A login the agent runs as a command of its own: its program with
/// these arguments and variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLogin {
    pub method: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// The parts of `Capabilities` an agent declares in `initialize`.
#[derive(Debug, Clone, Default)]
pub struct AcpDeclaredCapabilities {
    pub load_session: bool,
    pub resume_session: bool,
    pub list_sessions: bool,
    pub delete_sessions: bool,
    pub close_sessions: bool,
    pub prompt_images: bool,
    pub prompt_audio: bool,
    pub prompt_embedded_context: bool,
    pub mcp_http: bool,
    pub mcp_sse: bool,
    pub declared: Declared,
}

impl AcpDeclaredCapabilities {
    pub fn of(init: &acp::InitializeResponse, profile: &Profile) -> Self {
        let caps = &init.agent_capabilities;
        let sessions = &caps.session_capabilities;
        Self {
            load_session: caps.load_session,
            resume_session: sessions.resume.is_some(),
            list_sessions: sessions.list.is_some(),
            delete_sessions: sessions.delete.is_some(),
            close_sessions: sessions.close.is_some(),
            prompt_images: caps.prompt_capabilities.image,
            prompt_audio: caps.prompt_capabilities.audio,
            prompt_embedded_context: caps.prompt_capabilities.embedded_context,
            mcp_http: caps.mcp_capabilities.http,
            mcp_sse: caps.mcp_capabilities.sse,
            declared: profile.declared(init),
        }
    }

    /// Overwrites the declared fields of `caps`, leaving the rest as configured.
    pub fn apply(&self, caps: &mut harness_contracts::Capabilities) {
        caps.sessions.resume = self.load_session || self.resume_session;
        caps.sessions.list_saved = self.list_sessions;
        caps.sessions.delete_saved = self.delete_sessions;
        caps.sessions.close = self.close_sessions;
        caps.prompts.images = self.prompt_images;
        caps.prompts.audio = self.prompt_audio;
        caps.prompts.embedded_context = self.prompt_embedded_context;
        caps.tools.mcp_http = self.mcp_http;
        caps.tools.mcp_sse = self.mcp_sse;
        // Queued here when the agent doesn't queue them itself.
        caps.turns.queued_messages = true;
        caps.turns.active_steering = self.declared.steering;
    }
}

/// The agent's login methods: ours to show, and how to run its commands.
fn auth_methods(init: &acp::InitializeResponse) -> (Vec<AuthMethod>, Vec<CommandLogin>) {
    let (mut methods, mut logins) = (Vec::new(), Vec::new());
    for method in &init.auth_methods {
        match method {
            acp::AuthMethod::Agent(a) => methods.push(AuthMethod { id: a.id.to_string(), name: a.name.clone(), description: a.description.clone(), kind: AuthMethodKind::Agent }),
            acp::AuthMethod::Terminal(t) => {
                methods.push(AuthMethod { id: t.id.to_string(), name: t.name.clone(), description: t.description.clone(), kind: AuthMethodKind::Command });
                let mut env: Vec<(String, String)> = t.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                env.sort();
                logins.push(CommandLogin { method: t.id.to_string(), args: t.args.clone(), env });
            }
            _ => {}
        }
    }
    (methods, logins)
}

pub async fn probe(launcher: &Arc<dyn AcpLauncher>, profile: &Arc<Profile>) -> Result<AcpProbe> {
    let profile_for = profile.clone();
    with_connection(launcher, profile, async move |_cx, mut opened| {
        let capabilities = AcpDeclaredCapabilities::of(&opened.init, &profile_for);
        // An agent that reports who it's signed in as does so shortly after
        // `initialize`: wait for it a while.
        if capabilities.declared.sign_in {
            let wait = Duration::from_secs(profile_for.settings.sign_in.as_ref().map_or(0, |s| s.wait_s));
            let _ = tokio::time::timeout(wait, opened.sign_in.wait_for(Option::is_some)).await;
        }
        let sign_in = opened.sign_in.borrow().clone();
        let (auth_methods, logins) = auth_methods(&opened.init);
        let info = opened.init.agent_info.as_ref();
        Ok(AcpProbe {
            agent_name: info.map(|i| i.name.clone()),
            agent_version: info.map(|i| i.version.clone()),
            auth_methods,
            logins,
            capabilities,
            sign_in,
        })
    })
    .await
}
