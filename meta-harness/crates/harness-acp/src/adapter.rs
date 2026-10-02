//! `AcpAdapter`: opens sessions on any ACP agent. One agent connection per session.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{Agent, ByteStreams, ConnectionTo};
use async_trait::async_trait;
use harness_contracts::{
    ApprovalCapabilities, Capabilities, InstanceId, McpServer, McpTransport, NativeId, PlanningCapabilities,
    SessionCapabilities, SessionConfig, StreamingCapabilities, Thread, ThreadStatus, ToolCapabilities,
    TurnCapabilities,
};
use harness_core::{HarnessError, ProviderAdapter, Result, SessionRuntime};

use crate::flavor::AcpFlavor;
use crate::launch::AcpLauncher;
use crate::session::{AcpSession, Shared, acp_error, lock};
use crate::state::{SessionState, request_kind};

pub struct AcpAdapter {
    instance_id: InstanceId,
    flavor: Arc<dyn AcpFlavor>,
    launcher: Arc<dyn AcpLauncher>,
}

impl AcpAdapter {
    pub fn new(instance_id: InstanceId, flavor: Arc<dyn AcpFlavor>, launcher: Arc<dyn AcpLauncher>) -> Self {
        Self { instance_id, flavor, launcher }
    }
}

/// What any ACP agent supports by protocol; flavors adjust from here.
pub fn acp_base_capabilities() -> Capabilities {
    Capabilities {
        sessions: SessionCapabilities { runtime_mode_switch_in_session: true, ..Default::default() },
        turns: TurnCapabilities { interrupt: true, ..Default::default() },
        streaming: StreamingCapabilities { assistant_text: true, reasoning: true, tool_output: true },
        tools: ToolCapabilities { mcp_tools: true, ..Default::default() },
        approvals: ApprovalCapabilities { command_approval: true, file_change_approval: true, live_only: true },
        planning: PlanningCapabilities { todo_list: true, ..Default::default() },
        ..Default::default()
    }
}

fn mcp_server(server: &McpServer) -> acp::McpServer {
    match &server.transport {
        McpTransport::Http { url, headers } => acp::McpServer::Http(
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

/// Registers the client-side handlers on a connection builder and runs it until the
/// task is aborted, handing the connection handle out through `handle_tx`.
async fn run_connection(
    transport: ByteStreams<Box<dyn futures::AsyncWrite + Send + Unpin>, Box<dyn futures::AsyncRead + Send + Unpin>>,
    shared: Arc<Shared>,
    handle_tx: tokio::sync::oneshot::Sender<ConnectionTo<Agent>>,
) {
    let on_update = shared.clone();
    let on_permission = shared.clone();
    let on_read = shared.flavor.client_fs();
    let on_write = shared.flavor.client_fs();
    let result = agent_client_protocol::Client
        .builder()
        .name("meta-harness")
        .on_receive_notification(
            async move |n: acp::SessionNotification, _cx| {
                let mut state = lock(&on_update.state);
                if let acp::SessionUpdate::CurrentModeUpdate(update) = &n.update
                    && let Some(mode) = on_update.flavor.app_mode(&update.current_mode_id.to_string())
                {
                    lock(&on_update.policy).runtime_mode = mode;
                    state.set_thread(|t| t.policy.runtime_mode = mode);
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
                let wait = {
                    let mut state = lock(&shared.state);
                    state.tool_call_update(&req.tool_call);
                    let kind = state.tool_kind(&native_tool);
                    match shared.flavor.auto_approve(&policy, &kind, &req.options) {
                        Some(option) => Err(option),
                        None => {
                            let title = req.tool_call.fields.title.clone().unwrap_or_else(|| "Permission requested".into());
                            let options = shared.flavor.approval_options(&req.options);
                            let id = state.open_request(&native_tool, request_kind(&kind), title, options);
                            let (tx, rx) = tokio::sync::oneshot::channel();
                            lock(&shared.pending).insert(id, tx);
                            Ok(rx)
                        }
                    }
                };
                match wait {
                    Err(auto) => responder.respond(selected(auto)),
                    Ok(rx) => cx.spawn(async move {
                        match rx.await {
                            Ok(Some(option)) => responder.respond(selected(option)),
                            _ => responder.respond(acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Cancelled)),
                        }
                    }),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: acp::ReadTextFileRequest, responder, cx: ConnectionTo<Agent>| {
                let Some(fs) = on_read.clone() else {
                    return responder.respond_with_error(agent_client_protocol::Error::method_not_found());
                };
                cx.spawn(async move {
                    match fs.read_text_file(&req.path.display().to_string(), req.line, req.limit).await {
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
                    match fs.write_text_file(&req.path.display().to_string(), &req.content).await {
                        Ok(()) => responder.respond(acp::WriteTextFileResponse::new()),
                        Err(e) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(e.to_string())),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
            let _ = handle_tx.send(cx);
            std::future::pending::<agent_client_protocol::Result<()>>().await
        })
        .await;
    if let Err(err) = result {
        tracing::warn!(target: "harness_acp", "ACP connection ended: {err}");
    }
    let mut state = lock(&shared.state);
    if state.turn_running() {
        state.end_turn(harness_contracts::TurnStatus::Failed, Some(harness_contracts::Failure { message: "agent connection closed".into(), retryable: true }));
    }
    state.set_thread(|t| t.status = ThreadStatus::Closed);
}

#[async_trait]
impl ProviderAdapter for AcpAdapter {
    fn capabilities(&self) -> Capabilities {
        self.flavor.capabilities(acp_base_capabilities())
    }

    async fn open_session(&self, config: SessionConfig) -> Result<Box<dyn SessionRuntime>> {
        let transport = self.launcher.launch(&config).await?;
        let thread = Thread {
            id: config.thread_id.clone(),
            instance_id: self.instance_id.clone(),
            native_id: None,
            title: None,
            status: ThreadStatus::Starting,
            policy: config.policy.clone(),
            model: config.model.clone(),
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(SessionState::new(thread)),
            pending: Mutex::new(HashMap::new()),
            policy: Mutex::new(config.policy.clone()),
            flavor: self.flavor.clone(),
        });

        let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
        let streams = ByteStreams::new(transport.writer, transport.reader);
        let connection = tokio::spawn(run_connection(streams, shared.clone(), handle_tx));
        let cx = handle_rx.await.map_err(|_| HarnessError::Provider { reason: "agent connection failed to start".into() })?;

        let mut client = acp::ClientCapabilities::new();
        if self.flavor.client_fs().is_some() {
            client = client.fs(acp::FileSystemCapabilities::new().read_text_file(true).write_text_file(true));
        }
        cx.send_request(acp::InitializeRequest::new(ProtocolVersion::V1).client_capabilities(client))
            .block_task()
            .await
            .map_err(acp_error)?;

        let cwd = PathBuf::from(config.policy.cwd.clone().unwrap_or_else(|| ".".into()));
        let mcp: Vec<acp::McpServer> = config.mcp_servers.iter().map(mcp_server).collect();
        let (native_id, modes, config_options) = match &config.resume {
            Some(id) => {
                let native = acp::SessionId::new(id.as_str().to_owned());
                let loaded = cx
                    .send_request(acp::LoadSessionRequest::new(native.clone(), cwd).mcp_servers(mcp))
                    .block_task()
                    .await
                    .map_err(acp_error)?;
                (native, loaded.modes, loaded.config_options)
            }
            None => {
                let created = cx
                    .send_request(acp::NewSessionRequest::new(cwd).mcp_servers(mcp))
                    .block_task()
                    .await
                    .map_err(acp_error)?;
                (created.session_id, created.modes, created.config_options)
            }
        };

        let modes: Vec<String> = modes.map(|m| m.available_modes.iter().map(|x| x.id.to_string()).collect()).unwrap_or_default();
        let model_option = config_options
            .unwrap_or_default()
            .iter()
            .find(|o| matches!(o.category, Some(acp::SessionConfigOptionCategory::Model)))
            .map(|o| o.id.clone());

        lock(&shared.state).set_thread(|t| {
            t.native_id = Some(NativeId::new(native_id.to_string()));
            t.status = ThreadStatus::Idle;
        });

        let session = AcpSession {
            thread_id: config.thread_id.clone(),
            native_id,
            cx,
            shared,
            modes,
            model_option,
            connection,
            _guard: transport.guard,
        };
        session.apply_policy(config.policy).await?;
        Ok(Box::new(session))
    }
}
