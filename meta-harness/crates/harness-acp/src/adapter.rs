//! `AcpAdapter`: opens sessions on any ACP agent, the way its profile says.
//! One agent connection per session.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{Agent, ByteStreams, ConnectionTo, UntypedMessage};
use async_trait::async_trait;
use harness_contracts::{
    ApprovalDecision, ApprovalOption, Capabilities, InstanceId, McpServer, McpTransport, NativeId, RequestKind,
    RequestResponse, SessionConfig, Thread, ThreadStatus,
};
use harness_core::{HarnessError, ProviderAdapter, Result, SessionRuntime};

use crate::convert;
use crate::control::AcpDeclaredCapabilities;
use crate::launch::AcpLauncher;
use crate::profile::Profile;
use crate::questions::Form;
use crate::session::{AcpSession, Link, Shared, acp_error, lock};
use crate::state::SessionState;
use crate::workspace::WorkspaceFs;

pub struct AcpAdapter {
    instance_id: InstanceId,
    profile: Arc<Profile>,
    launcher: Arc<dyn AcpLauncher>,
    /// Told when a session can't open for not being signed in.
    on_auth: crate::auth::AuthChanged,
}

impl AcpAdapter {
    pub fn new(instance_id: InstanceId, profile: Arc<Profile>, launcher: Arc<dyn AcpLauncher>, on_auth: crate::auth::AuthChanged) -> Self {
        Self { instance_id, profile, launcher, on_auth }
    }

    async fn open(&self, config: SessionConfig) -> Result<Box<dyn SessionRuntime>> {
        let cwd = self.profile.cwd(&config.policy);
        let mut policy = config.policy.clone();
        policy.cwd = Some(cwd.display().to_string());
        let config = SessionConfig { policy, ..config };
        let transport = self.launcher.launch(&config).await?;
        // An agent that starts a login itself when it isn't signed in: its
        // link, said while the session opens, means it isn't.
        let pattern = self.profile.settings.login.on_session.then(|| self.profile.settings.login.link_pattern()).flatten();
        let said = transport.said;
        let asks_login = async move {
            match pattern {
                Some(pattern) => crate::launch::says(said, Some(pattern)).await,
                None => std::future::pending().await,
            }
        };
        let thread = Thread {
            id: config.thread_id.clone(),
            instance_id: self.instance_id.clone(),
            native_id: None,
            title: None,
            status: ThreadStatus::Starting,
            policy: config.policy.clone(),
            model: config.model.clone(),
            controls: Default::default(),
            mcp_servers: Vec::new(),
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(SessionState::new(thread, self.profile.parent_call())),
            pending: Mutex::new(HashMap::new()),
            policy: Mutex::new(config.policy.clone()),
            profile: self.profile.clone(),
            waiting: Mutex::new(VecDeque::new()),
        });

        let files = self.profile.workspace(&cwd);
        let client = self.profile.client_capabilities(files.is_some());
        let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
        let streams = ByteStreams::new(transport.writer, transport.reader);
        let connection = tokio::spawn(run_connection(streams, shared.clone(), files, handle_tx));
        let cx = handle_rx.await.map_err(|_| HarnessError::Provider { reason: "agent connection failed to start".into() })?;

        let init = cx.send_request(acp::InitializeRequest::new(ProtocolVersion::V1).client_capabilities(client)).block_task().await.map_err(acp_error)?;
        let declared = AcpDeclaredCapabilities::of(&init, &self.profile);
        // Before a replay: how its messages' pieces are read back.
        lock(&shared.state).replays_by(self.profile.replayed_pieces(&declared.declared), self.profile.chunks());
        let agent = &init.agent_capabilities;
        let mcp: Vec<acp::McpServer> = config.mcp_servers.iter().map(|s| mcp_server(s, agent.mcp_capabilities.sse)).collect();
        let meta = self.profile.session_meta();
        let opening = async { match &config.resume {
            // Prefer load: it replays the history into the stream.
            Some(id) if agent.load_session => {
                let native = acp::SessionId::new(id.as_str().to_owned());
                let loaded = cx
                    .send_request(acp::LoadSessionRequest::new(native.clone(), cwd).mcp_servers(mcp).meta(meta))
                    .block_task()
                    .await
                    .map_err(acp_error)?;
                Ok((native, loaded.modes, loaded.config_options))
            }
            Some(id) if agent.session_capabilities.resume.is_some() => {
                let native = acp::SessionId::new(id.as_str().to_owned());
                let resumed = cx
                    .send_request(acp::ResumeSessionRequest::new(native.clone(), cwd).mcp_servers(mcp).meta(meta))
                    .block_task()
                    .await
                    .map_err(acp_error)?;
                Ok((native, resumed.modes, resumed.config_options))
            }
            Some(_) => Err(HarnessError::Unsupported { operation: "resuming sessions" }),
            None => {
                let created = cx.send_request(acp::NewSessionRequest::new(cwd).mcp_servers(mcp).meta(meta)).block_task().await.map_err(acp_error)?;
                Ok((created.session_id, created.modes, created.config_options))
            }
        } };
        let (native_id, modes, config_options) = tokio::select! {
            opened = opening => opened?,
            Some(_) = asks_login => return Err(HarnessError::AuthRequired { reason: NOT_SIGNED_IN.into() }),
        };

        {
            let mut state = lock(&shared.state);
            state.replayed();
            if let Some(controls) = config_options.as_deref() {
                state.set_config(convert::config_controls(controls));
            }
            let (modes, current_mode) = modes.as_ref().map(convert::modes).unwrap_or_default();
            state.set_thread(|t| {
                t.native_id = Some(NativeId::new(native_id.to_string()));
                t.status = ThreadStatus::Idle;
                t.controls.modes = modes;
                t.controls.current_mode = current_mode;
                // Shown to every client: no session's key in it.
                t.mcp_servers = config.mcp_servers.iter().map(harness_contracts::McpServer::redacted).collect();
            });
        }

        let session = AcpSession {
            thread_id: config.thread_id.clone(),
            link: Arc::new(Link { cx, native_id, shared, sending: if declared.declared.pieces { crate::pieces::Sending::AsPieces } else { crate::pieces::Sending::InText } }),
            declared: declared.declared,
            supports_close: agent.session_capabilities.close.is_some(),
            connection,
            _guard: transport.guard,
        };
        session.apply_policy(config.policy).await?;
        if let Some(model) = config.model {
            let options = model.options.clone();
            session.steer(harness_contracts::SteerInput { model: Some(model), config: options, ..Default::default() }).await?;
        }
        Ok(Box::new(session))
    }
}

/// Why a session didn't open: the agent wants a login first.
const NOT_SIGNED_IN: &str = "sign in, then try again";

/// A tool server as the agent takes it: an SSE one as HTTP when the agent
/// doesn't say it takes SSE.
fn mcp_server(server: &McpServer, sse: bool) -> acp::McpServer {
    match &server.transport {
        McpTransport::Sse { url, headers } if sse => acp::McpServer::Sse(
            acp::McpServerSse::new(server.name.clone(), url.clone())
                .headers(headers.iter().map(|(k, v)| acp::HttpHeader::new(k.clone(), v.clone())).collect()),
        ),
        McpTransport::Http { url, headers } | McpTransport::Sse { url, headers } => acp::McpServer::Http(
            acp::McpServerHttp::new(server.name.clone(), url.clone())
                .headers(headers.iter().map(|(k, v)| acp::HttpHeader::new(k.clone(), v.clone())).collect()),
        ),
        McpTransport::Stdio { command, args, env } => acp::McpServer::Stdio(
            acp::McpServerStdio::new(server.name.clone(), PathBuf::from(command))
                .args(args.clone())
                .env(env.iter().map(|(k, v)| acp::EnvVariable::new(k.clone(), v.clone())).collect()),
        ),
    }
}

fn selected(option_id: String) -> acp::RequestPermissionResponse {
    acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(option_id)))
}

/// The choices on a question form.
const SUBMIT: &str = "submit";
const DECLINE: &str = "decline";

fn form_options(submit: &str) -> Vec<ApprovalOption> {
    vec![
        ApprovalOption { id: SUBMIT.into(), decision: ApprovalDecision::Accept, label: submit.into(), warning: None },
        ApprovalOption { id: DECLINE.into(), decision: ApprovalDecision::Decline, label: "Skip".into(), warning: None },
    ]
}

/// Opens a request and waits for the user's response (None: cancelled,
/// e.g. its turn ended).
async fn ask(shared: &Shared, open: impl FnOnce(&mut SessionState) -> harness_contracts::RequestId) -> Option<RequestResponse> {
    let rx = {
        let mut state = lock(&shared.state);
        let id = open(&mut state);
        let (tx, rx) = tokio::sync::oneshot::channel();
        lock(&shared.pending).insert(id, tx);
        rx
    };
    rx.await.ok()
}

/// The answer to a question form: its content, declined, or cancelled.
fn elicitation_answer(form: Option<&Form>, response: Option<RequestResponse>) -> acp::CreateElicitationResponse {
    let action = match response {
        None => acp::ElicitationAction::Cancel,
        Some(r) if r.option_id.as_deref() == Some(DECLINE) => acp::ElicitationAction::Decline,
        Some(r) => {
            let content = form.map(|f| f.content(&r.answers)).filter(|c| !c.is_empty());
            acp::ElicitationAction::Accept(acp::ElicitationAcceptAction::new().content(content))
        }
    };
    acp::CreateElicitationResponse::new(action)
}

type Streams = ByteStreams<Box<dyn futures::AsyncWrite + Send + Unpin>, Box<dyn futures::AsyncRead + Send + Unpin>>;

/// Registers the client-side handlers and runs the connection until the
/// task is aborted, handing the connection out through `handle_tx`.
async fn run_connection(transport: Streams, shared: Arc<Shared>, files: Option<WorkspaceFs>, handle_tx: tokio::sync::oneshot::Sender<ConnectionTo<Agent>>) {
    let (on_update, on_permission, on_question, on_other) = (shared.clone(), shared.clone(), shared.clone(), shared.clone());
    let (on_read, on_write) = (files.clone(), files);
    let result = agent_client_protocol::Client
        .builder()
        .name("meta-harness")
        .on_receive_notification(
            async move |n: acp::SessionNotification, _cx| {
                let mut state = lock(&on_update.state);
                if let acp::SessionUpdate::CurrentModeUpdate(update) = &n.update {
                    let mut policy = lock(&on_update.policy);
                    *policy = on_update.profile.policy_for(&update.current_mode_id.to_string(), policy.clone());
                    let policy = policy.clone();
                    state.set_thread(|t| t.policy = policy);
                }
                state.apply(&n.update);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |req: acp::RequestPermissionRequest, responder, cx: ConnectionTo<Agent>| {
                let shared = on_permission.clone();
                let native_tool = req.tool_call.tool_call_id.to_string();
                let policy = lock(&shared.policy).clone();
                // What's asked first: only an approval may be answered for the user.
                let (asked, auto) = {
                    let mut state = lock(&shared.state);
                    state.tool_call_update(&req.tool_call);
                    let kind = state.tool_kind(&native_tool);
                    let asked = crate::classify::permission(&shared.profile.settings.questions, &native_tool, &kind, &req.options);
                    let auto = if matches!(asked, RequestKind::Approval { .. }) { shared.profile.auto_approve(&policy, &kind, &req.options) } else { None };
                    (asked, auto)
                };
                if let Some(option) = auto {
                    return responder.respond(selected(option));
                }
                cx.spawn(async move {
                    let response = ask(&shared, |state| {
                        let title = req.tool_call.fields.title.clone().unwrap_or_else(|| "Permission requested".into());
                        state.open_request(Some(&native_tool), asked, title, None, shared.profile.approval_options(asked, &req.options), Vec::new())
                    })
                    .await;
                    match response.and_then(|r| r.option_id) {
                        Some(option) => responder.respond(selected(option)),
                        None => responder.respond(acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Cancelled)),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: acp::CreateElicitationRequest, responder, cx: ConnectionTo<Agent>| {
                let shared = on_question.clone();
                cx.spawn(async move {
                    let tool = |scope: &acp::ElicitationScope| match scope {
                        acp::ElicitationScope::Session(s) => s.tool_call_id.as_ref().map(ToString::to_string),
                        _ => None,
                    };
                    match &req.mode {
                        acp::ElicitationMode::Form(form_mode) => {
                            let form = Form::new(&form_mode.requested_schema, &req.message, shared.profile.custom_suffix());
                            let tool = tool(&form_mode.scope);
                            let questions = form.questions.clone();
                            let response = ask(&shared, |state| state.open_request(tool.as_deref(), RequestKind::Question, req.message.clone(), None, form_options("Answer"), questions)).await;
                            responder.respond(elicitation_answer(Some(&form), response))
                        }
                        acp::ElicitationMode::Url(url_mode) => {
                            let tool = tool(&url_mode.scope);
                            let response = ask(&shared, |state| state.open_request(tool.as_deref(), RequestKind::Link, req.message.clone(), Some(url_mode.url.clone()), form_options("Done"), Vec::new())).await;
                            responder.respond(elicitation_answer(None, response))
                        }
                        _ => responder.respond(acp::CreateElicitationResponse::new(acp::ElicitationAction::Decline)),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        // Files are read and written off the connection's loop.
        .on_receive_request(
            async move |req: acp::ReadTextFileRequest, responder, cx: ConnectionTo<Agent>| {
                let Some(fs) = on_read.clone() else {
                    return responder.respond_with_error(agent_client_protocol::Error::method_not_found());
                };
                cx.spawn(async move {
                    let read = tokio::task::spawn_blocking(move || fs.read_text_file(&req.path.display().to_string(), req.line, req.limit)).await;
                    match read.map_err(|e| HarnessError::Provider { reason: e.to_string() }).and_then(|r| r) {
                        Ok(content) => responder.respond(acp::ReadTextFileResponse::new(content)),
                        Err(e) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(e.to_string())),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: acp::WriteTextFileRequest, responder, cx: ConnectionTo<Agent>| {
                let Some(fs) = on_write.clone() else {
                    return responder.respond_with_error(agent_client_protocol::Error::method_not_found());
                };
                cx.spawn(async move {
                    let written = tokio::task::spawn_blocking(move || fs.write_text_file(&req.path.display().to_string(), &req.content)).await;
                    match written.map_err(|e| HarnessError::Provider { reason: e.to_string() }).and_then(|r| r) {
                        Ok(()) => responder.respond(acp::WriteTextFileResponse::new()),
                        Err(e) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(e.to_string())),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        // Last: the agent's own notifications (who it's signed in as).
        .on_receive_notification(
            async move |n: UntypedMessage, _cx| {
                if let Some(signed) = on_other.profile.sign_in(&n.method, &n.params) {
                    lock(&on_other.state).sign_in(signed);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
            let _ = handle_tx.send(cx);
            std::future::pending::<agent_client_protocol::Result<()>>().await
        })
        .await;
    if let Err(err) = result {
        tracing::warn!(target: "harness_acp", "ACP connection ended: {err}");
    }
    lock(&shared.waiting).clear();
    lock(&shared.pending).clear();
    let mut state = lock(&shared.state);
    state.end_all(harness_contracts::Failure { message: "agent connection closed".into(), retryable: true });
    state.set_thread(|t| t.status = ThreadStatus::Closed);
}

#[async_trait]
impl ProviderAdapter for AcpAdapter {
    fn capabilities(&self) -> Capabilities {
        self.profile.capabilities()
    }

    /// A session that can't open for not being signed in (however the agent
    /// says so) says the instance isn't.
    async fn open_session(&self, config: SessionConfig) -> Result<Box<dyn SessionRuntime>> {
        let opened = self.open(config).await;
        if let Err(HarnessError::AuthRequired { .. }) = &opened {
            (self.on_auth)(crate::auth::AuthEvent::Status(Some(false)));
        }
        opened
    }
}
