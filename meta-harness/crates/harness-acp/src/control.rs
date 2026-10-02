//! Short-lived control connections: launch the agent, `initialize`, run one
//! operation, close. Used for probing, auth, and saved-session management.

use std::sync::Arc;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{Agent, ByteStreams, ConnectionTo};
use harness_contracts::{AuthMethod, RuntimePolicy, SessionConfig, ThreadId};
use harness_core::{HarnessError, Result};

use crate::launch::AcpLauncher;
use crate::session::acp_error;

pub(crate) async fn with_connection<T: Send + 'static>(
    launcher: &Arc<dyn AcpLauncher>,
    cwd: &str,
    op: impl AsyncFnOnce(ConnectionTo<Agent>, acp::InitializeResponse) -> agent_client_protocol::Result<T> + Send + 'static,
) -> Result<T> {
    let config = SessionConfig {
        thread_id: ThreadId::new("control"),
        policy: RuntimePolicy { cwd: Some(cwd.to_owned()), ..RuntimePolicy::default() },
        model: None,
        resume: None,
        mcp_servers: Vec::new(),
    };
    let transport = launcher.launch(&config).await?;
    let streams = ByteStreams::new(transport.writer, transport.reader);
    let (tx, rx) = tokio::sync::oneshot::channel();
    agent_client_protocol::Client
        .builder()
        .name("meta-harness-control")
        .connect_with(streams, async move |cx: ConnectionTo<Agent>| {
            let init = cx.send_request(acp::InitializeRequest::new(ProtocolVersion::V1)).block_task().await?;
            let _ = tx.send(op(cx, init).await);
            Ok(())
        })
        .await
        .map_err(acp_error)?;
    drop(transport.guard);
    rx.await
        .map_err(|_| HarnessError::Provider { reason: "agent control connection closed".into() })?
        .map_err(acp_error)
}

/// What an agent reports about itself in `initialize`.
#[derive(Debug, Clone)]
pub struct AcpProbe {
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
    pub auth_methods: Vec<AuthMethod>,
    /// Capabilities the agent declares, mapped onto the contracts.
    pub capabilities: AcpDeclaredCapabilities,
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
}

impl AcpDeclaredCapabilities {
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
    }
}

pub async fn probe(launcher: &Arc<dyn AcpLauncher>) -> Result<AcpProbe> {
    with_connection(launcher, ".", async move |_cx, init| {
        let auth_methods = init
            .auth_methods
            .iter()
            .filter_map(|m| match m {
                acp::AuthMethod::Agent(a) => {
                    Some(AuthMethod { id: a.id.to_string(), name: a.name.clone(), description: a.description.clone() })
                }
                _ => None,
            })
            .collect();
        let caps = &init.agent_capabilities;
        let sessions = &caps.session_capabilities;
        Ok(AcpProbe {
            agent_name: init.agent_info.as_ref().map(|i| i.name.clone()),
            agent_version: init.agent_info.as_ref().map(|i| i.version.clone()),
            auth_methods,
            capabilities: AcpDeclaredCapabilities {
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
            },
        })
    })
    .await
}
