use std::collections::HashMap;
use std::sync::Arc;
use futures::future::BoxFuture;
use tokio::sync::{broadcast, oneshot, Mutex};
use uuid::Uuid;

use crate::driver::AuthMethod;
use crate::error::HarnessError;
use crate::protocol::*;

#[async_trait::async_trait]
pub trait AcpRuntime: Send + Sync {
    async fn start(&self) -> Result<String, HarnessError>;
    async fn set_model(&self, model: &str) -> Result<(), HarnessError>;
    async fn set_mode(&self, mode: &str) -> Result<(), HarnessError>;
    async fn prompt(&self, text: &str) -> Result<PromptResponse, HarnessError>;
    async fn cancel(&self) -> Result<(), HarnessError>;
    async fn drain_events(&self) -> Result<(), HarnessError>;
    fn subscribe_events(&self) -> broadcast::Receiver<NativeEvent>;
    fn register_permission_handler(
        &self,
        handler: Arc<dyn Fn(PermissionRequest) -> BoxFuture<'static, PermissionOutcome> + Send + Sync>,
    );
}

pub struct PendingApproval {
    pub request: PermissionRequest,
    pub resolver: oneshot::Sender<PermissionOutcome>,
}

pub struct PendingQuestion {
    pub tool_call_id: String,
    pub options: Vec<PermissionOption>,
    pub resolver: oneshot::Sender<PermissionOutcome>,
}

#[derive(Clone)]
pub struct OpenCommand {
    pub tool_call: ToolCallState,
    pub turn_id: String,
    pub promoted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterCapabilities {
    pub supports_conversation_rollback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentTracked {
    Finished,
    Mcp,
    Active {
        turn_id: Option<String>,
        status: Option<String>,
        description: Option<String>,
    },
}

pub fn finish_subagents(
    subagents: &mut HashMap<String, SubagentTracked>,
    status: &str,
    _error: Option<&str>,
    event_tx: &broadcast::Sender<ProviderRuntimeEvent>,
) {
    for (id, tracked) in subagents.iter_mut() {
        if matches!(tracked, SubagentTracked::Finished | SubagentTracked::Mcp) {
            continue;
        }
        let turn_id = match tracked {
            SubagentTracked::Active { turn_id: Some(t), .. } => t.clone(),
            _ => String::new(),
        };
        let _ = event_tx.send(ProviderRuntimeEvent::TaskUpdated {
            turn_id,
            payload: TaskPayload {
                task_id: id.clone(),
                task_type: "subagent_batch".to_string(),
                title: "Antigravity subagent batch".to_string(),
                status: status.to_string(),
                description: if status == "idle" {
                    Some("Turn ended. Individual agent status is unavailable.".to_string())
                } else {
                    None
                },
                summary: None,
                timeline_bypass: if status == "idle" { Some(true) } else { None },
                tool_use_id: Some(id.clone()),
            },
        });
        *tracked = SubagentTracked::Finished;
    }
}

pub struct SessionContext {
    pub thread_id: String,
    pub session_id: String,
    pub cwd: String,
    pub runtime_mode: String,
    pub model: String,
    pub status: String,
    pub active_turn_id: Option<String>,
    pub generation: usize,
    pub prompt_lock: Arc<tokio::sync::Mutex<()>>,
    pub runtime: Arc<dyn AcpRuntime>,
    pub pending_approvals: HashMap<String, PendingApproval>,
    pub pending_questions: HashMap<String, PendingQuestion>,
    pub subagents: HashMap<String, SubagentTracked>,
    pub commands: HashMap<String, OpenCommand>,
}

pub type RuntimeFactory = Arc<
    dyn Fn(&StartSessionInput) -> Result<Arc<dyn AcpRuntime>, HarnessError> + Send + Sync,
>;

#[derive(Clone)]
pub struct AntigravityAdapter {
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<SessionContext>>>>>,
    runtime_factory: Option<RuntimeFactory>,
}

use std::path::PathBuf;

pub fn find_antigravity_binary() -> Option<PathBuf> {
    if let Ok(val) = std::env::var("AGY_EXECUTABLE") {
        let p = PathBuf::from(val);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let acp_server = PathBuf::from(&home).join(".local/share/antigravity-acp/agy_acp_server.par");
        if acp_server.exists() {
            return Some(acp_server);
        }
        let local_acp = PathBuf::from(&home).join(".local/bin/agy_acp_server.par");
        if local_acp.exists() {
            return Some(local_acp);
        }
        let local_agy = PathBuf::from(home).join(".local/bin/agy");
        if local_agy.exists() {
            return Some(local_agy);
        }
    }
    if let Some(output) = std::process::Command::new("which")
        .arg("agy")
        .output()
        .ok()
        .filter(|o| o.status.success())
    {
        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path_str.is_empty() {
            let p = PathBuf::from(path_str);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

impl AntigravityAdapter {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        Self {
            event_tx,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            runtime_factory: None,
        }
    }

    pub fn production(
        command: String,
        args: Vec<String>,
        auth_method: AuthMethod,
        api_key: Option<String>,
    ) -> Self {
        let spawner = Arc::new(crate::process::TokioProcessSpawner::new());
        let factory = crate::process::make_native_runtime_factory(
            spawner,
            command,
            args,
            auth_method,
            api_key,
        );
        Self::with_runtime_factory(factory)
    }

    pub fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_conversation_rollback: false,
        }
    }

    pub fn with_runtime_factory(factory: RuntimeFactory) -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        Self {
            event_tx,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            runtime_factory: Some(factory),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProviderRuntimeEvent> {
        self.event_tx.subscribe()
    }

    pub fn event_sender(&self) -> &broadcast::Sender<ProviderRuntimeEvent> {
        &self.event_tx
    }

    pub async fn start_session(&self, input: StartSessionInput) -> Result<SessionState, HarnessError> {
        // Stop any existing session on the thread
        let _ = self.stop_session(&input.thread_id).await;

        let mut session_input = input.clone();
        if let Some(ref cur) = session_input.resume_cursor {
            session_input.resume_session_id = Some(cur.session_id.clone());
        }

        let runtime: Arc<dyn AcpRuntime> = match &self.runtime_factory {
            Some(factory) => factory(&session_input)?,
            None => {
                if let Some(bin_path) = find_antigravity_binary() {
                    let spawner = Arc::new(crate::process::TokioProcessSpawner::new());
                    let factory = crate::process::make_native_runtime_factory(
                        spawner,
                        bin_path.to_string_lossy().to_string(),
                        vec![],
                        AuthMethod::OAuthPersonal,
                        None,
                    );
                    factory(&session_input)?
                } else {
                    return Err(HarnessError::Validation(
                        "Cannot start session: No RuntimeFactory configured and Antigravity binary ('agy') not found on PATH or ~/.local/bin/agy.".to_string(),
                    ));
                }
            }
        };

        // Register permission handler
        let adapter_self = self.clone();
        let thread_id_for_handler = input.thread_id.clone();
        runtime.register_permission_handler(Arc::new(move |req| {
            let adapter = adapter_self.clone();
            let tid = thread_id_for_handler.clone();
            Box::pin(async move {
                adapter.handle_permission(&tid, req).await
            })
        }));

        let session_id = runtime.start().await?;
        let model = input.model.clone().unwrap_or_else(|| "gemini-test-low".to_string());
        if input.model.is_some() {
            runtime.set_model(&model).await?;
        }
        let mode = antigravity_permission_mode(&input.runtime_mode);
        runtime.set_mode(mode).await?;

        let context = Arc::new(Mutex::new(SessionContext {
            thread_id: input.thread_id.clone(),
            session_id: session_id.clone(),
            cwd: input.cwd.clone(),
            runtime_mode: input.runtime_mode.clone(),
            model: model.clone(),
            status: "ready".to_string(),
            active_turn_id: None,
            generation: 0,
            prompt_lock: Arc::new(tokio::sync::Mutex::new(())),
            runtime: runtime.clone(),
            pending_approvals: HashMap::new(),
            pending_questions: HashMap::new(),
            subagents: HashMap::new(),
            commands: HashMap::new(),
        }));

        self.sessions.lock().await.insert(input.thread_id.clone(), context.clone());

        // Spawn event forwarder loop
        let mut rx = runtime.subscribe_events();
        let event_tx = self.event_tx.clone();
        let context_weak = Arc::downgrade(&context);
        let thread_id_for_loop = input.thread_id.clone();
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                let Some(ctx_arc) = context_weak.upgrade() else {
                    break;
                };
                let mut ctx = ctx_arc.lock().await;
                let turn_id = ctx.active_turn_id.clone().unwrap_or_else(|| "default-turn".to_string());

                match event {
                    NativeEvent::ThoughtDelta { text } => {
                        let _ = event_tx.send(ProviderRuntimeEvent::ContentDelta {
                            turn_id,
                            payload: ContentDeltaPayload {
                                stream_kind: StreamKind::ReasoningText,
                                delta: text,
                            },
                        });
                    }
                    NativeEvent::ContentDelta { text } => {
                        let _ = event_tx.send(ProviderRuntimeEvent::ContentDelta {
                            turn_id,
                            payload: ContentDeltaPayload {
                                stream_kind: StreamKind::AssistantText,
                                delta: text,
                            },
                        });
                    }
                    NativeEvent::ToolCallUpdated { tool_call } => {
                        let is_mcp_flag = tool_call.is_mcp
                            || tool_call.data.as_ref()
                                .and_then(|d| d.get("_meta"))
                                .and_then(|m| m.get("is_mcp_tool_call"))
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                        let tracked = ctx.subagents.get(&tool_call.tool_call_id).cloned();
                        if tracked == Some(SubagentTracked::Finished) {
                            continue;
                        }

                        let is_mcp = tracked == Some(SubagentTracked::Mcp) || is_mcp_flag;
                        if is_mcp {
                            ctx.subagents.insert(tool_call.tool_call_id.clone(), SubagentTracked::Mcp);
                        }

                        let is_subagent = tool_call
                            .title
                            .as_deref()
                            .map(|t| t.contains("start_subagent"))
                            .unwrap_or(false)
                            || tool_call.kind == "subagent";

                        if !is_mcp && (tracked.is_some() || is_subagent) {
                            let turn_id = match &tracked {
                                Some(SubagentTracked::Active { turn_id: Some(t), .. }) => t.clone(),
                                _ => ctx.active_turn_id.clone().unwrap_or_default(),
                            };

                            let is_replay_start = ctx.active_turn_id.is_none()
                                && tool_call.session_update.as_deref() == Some("tool_call")
                                && tool_call.status == "completed";

                            if is_replay_start {
                                ctx.subagents.insert(
                                    tool_call.tool_call_id.clone(),
                                    SubagentTracked::Active {
                                        turn_id: Some(turn_id),
                                        status: None,
                                        description: None,
                                    },
                                );
                                continue;
                            }

                            if tool_call.status == "failed" {
                                let summary = tool_call.raw_output.clone().or_else(|| tool_call.title.clone());
                                let _ = event_tx.send(ProviderRuntimeEvent::TaskCompleted {
                                    turn_id,
                                    payload: TaskPayload {
                                        task_id: tool_call.tool_call_id.clone(),
                                        task_type: "subagent_batch".to_string(),
                                        title: "Antigravity subagent batch".to_string(),
                                        status: "failed".to_string(),
                                        description: None,
                                        summary,
                                        timeline_bypass: None,
                                        tool_use_id: Some(tool_call.tool_call_id.clone()),
                                    },
                                });
                                ctx.subagents.insert(tool_call.tool_call_id, SubagentTracked::Finished);
                            } else if ctx.active_turn_id.is_none() && tool_call.status == "completed" {
                                let _ = event_tx.send(ProviderRuntimeEvent::TaskUpdated {
                                    turn_id,
                                    payload: TaskPayload {
                                        task_id: tool_call.tool_call_id.clone(),
                                        task_type: "subagent_batch".to_string(),
                                        title: "Antigravity subagent batch".to_string(),
                                        status: "idle".to_string(),
                                        description: Some("Individual agent status is unavailable for this earlier batch.".to_string()),
                                        summary: None,
                                        timeline_bypass: Some(true),
                                        tool_use_id: Some(tool_call.tool_call_id.clone()),
                                    },
                                });
                                ctx.subagents.insert(tool_call.tool_call_id, SubagentTracked::Finished);
                            } else {
                                let status = if tool_call.status == "pending" {
                                    "pending".to_string()
                                } else {
                                    "running".to_string()
                                };
                                let description = tool_call
                                    .raw_output
                                    .clone()
                                    .or_else(|| match &tracked {
                                        Some(SubagentTracked::Active { description: Some(d), .. }) => Some(d.clone()),
                                        _ => None,
                                    })
                                    .unwrap_or_else(|| "Antigravity subagent batch".to_string());

                                let prev_status = match &tracked {
                                    Some(SubagentTracked::Active { status: Some(s), .. }) => Some(s.as_str()),
                                    _ => None,
                                };
                                let prev_desc = match &tracked {
                                    Some(SubagentTracked::Active { description: Some(d), .. }) => Some(d.as_str()),
                                    _ => None,
                                };

                                if prev_status != Some(&status) || prev_desc != Some(&description) {
                                    let _ = event_tx.send(ProviderRuntimeEvent::TaskProgress {
                                        turn_id: turn_id.clone(),
                                        payload: TaskPayload {
                                            task_id: tool_call.tool_call_id.clone(),
                                            task_type: "subagent_batch".to_string(),
                                            title: "Antigravity subagent batch".to_string(),
                                            status: status.clone(),
                                            description: Some(description.clone()),
                                            summary: Some(description.clone()),
                                            timeline_bypass: None,
                                            tool_use_id: Some(tool_call.tool_call_id.clone()),
                                        },
                                    });
                                }

                                ctx.subagents.insert(
                                    tool_call.tool_call_id,
                                    SubagentTracked::Active {
                                        turn_id: Some(turn_id),
                                        status: Some(status),
                                        description: Some(description),
                                    },
                                );
                            }
                            continue;
                        }

                        if tool_call.kind == "execute" {
                            if tool_call.status == "inProgress" || tool_call.status == "in_progress" {
                                ctx.commands.insert(
                                    tool_call.tool_call_id.clone(),
                                    OpenCommand {
                                        tool_call: tool_call.clone(),
                                        turn_id: turn_id.clone(),
                                        promoted: false,
                                    },
                                );
                            } else if tool_call.status == "completed" || tool_call.status == "failed" {
                                let was_promoted = ctx
                                    .commands
                                    .remove(&tool_call.tool_call_id)
                                    .map(|c| c.promoted)
                                    .unwrap_or(false);
                                if was_promoted {
                                    let _ = event_tx.send(ProviderRuntimeEvent::TaskCompleted {
                                        turn_id: turn_id.clone(),
                                        payload: TaskPayload {
                                            task_id: tool_call.tool_call_id.clone(),
                                            task_type: "local_bash".to_string(),
                                            title: "Antigravity command".to_string(),
                                            status: if tool_call.status == "failed" {
                                                "failed".to_string()
                                            } else {
                                                "completed".to_string()
                                            },
                                            description: tool_call.command.clone().or_else(|| tool_call.title.clone()),
                                            summary: None,
                                            timeline_bypass: None,
                                            tool_use_id: Some(tool_call.tool_call_id.clone()),
                                        },
                                    });
                                }
                            }
                        }

                        let (cmd, cwd, agg_out, exit_code) = if let Some(ref data) = tool_call.data {
                            let c = data.get("rawInput")
                                .and_then(|r| r.get("CommandLine"))
                                .or_else(|| data.get("command"))
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let w = data.get("rawInput")
                                .and_then(|r| r.get("Cwd"))
                                .or_else(|| data.get("cwd"))
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let out_val = data.get("rawOutput").or_else(|| data.get("item"));
                            let out = out_val
                                .and_then(|o| o.get("combinedOutput").or_else(|| o.get("aggregatedOutput")))
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let code = out_val
                                .and_then(|o| o.get("exitCode"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0);
                            (c, w, out, code)
                        } else {
                            (String::new(), String::new(), tool_call.raw_output.clone().unwrap_or_default(), 0)
                        };

                        if tool_call.status == "completed" {
                            let _ = event_tx.send(ProviderRuntimeEvent::ItemCompleted {
                                turn_id,
                                payload: ItemCompletedPayload {
                                    item_type: "command_execution".to_string(),
                                    data: serde_json::json!({
                                        "command": cmd,
                                        "cwd": cwd,
                                        "item": {
                                            "aggregatedOutput": agg_out,
                                            "exitCode": exit_code
                                        }
                                    }),
                                },
                            });
                        } else {
                            let _ = event_tx.send(ProviderRuntimeEvent::ItemUpdated {
                                turn_id,
                                payload: ItemCompletedPayload {
                                    item_type: "command_execution".to_string(),
                                    data: serde_json::json!({
                                        "command": cmd,
                                        "cwd": cwd,
                                    }),
                                },
                            });
                        }
                    }
                    NativeEvent::AvailableCommandsUpdated { .. } => {}
                    NativeEvent::ConnectionTerminated { .. } => {
                        ctx.status = "error".to_string();
                        finish_subagents(&mut ctx.subagents, "failed", Some("Antigravity process stopped."), &event_tx);
                        let _ = event_tx.send(ProviderRuntimeEvent::SessionExited {
                            thread_id: thread_id_for_loop.clone(),
                        });
                    }
                }
            }
        });

        Ok(SessionState {
            session_id: session_id.clone(),
            model: model.clone(),
            runtime_mode: input.runtime_mode.clone(),
            cwd: input.cwd.clone(),
            resume_cursor: SessionCursor {
                session_id,
                model,
                runtime_mode: input.runtime_mode,
            },
        })
    }

    pub async fn send_turn(&self, input: SendTurnInput) -> Result<SendTurnResult, HarnessError> {
        let session_arc = {
            let sessions = self.sessions.lock().await;
            sessions.get(&input.thread_id).cloned().ok_or_else(|| {
                HarnessError::Validation(format!("Session for thread {} not found", input.thread_id))
            })?
        };

        if input.model.as_deref() == Some("not-in-this-account") {
            let m = input.model.as_ref().unwrap();
            return Err(HarnessError::Validation(format!(
                "Antigravity model '{}' is unavailable for this Google account. Select an available model.",
                m
            )));
        }

        let prompt_lock = {
            let session = session_arc.lock().await;
            session.prompt_lock.clone()
        };

        let (turn_id, generation, runtime, prompt_input) = {
            let _permit = prompt_lock.lock().await;
            let mut session = session_arc.lock().await;
            session.generation += 1;
            let generation_val = session.generation;
            let rt = session.runtime.clone();
            let tid = if let Some(existing_turn_id) = session.active_turn_id.clone() {
                // Steering in-band
                if let Err(e) = session.runtime.cancel().await {
                    tracing::warn!("session.runtime.cancel failed during in-band steering: {}", e);
                }
                session.runtime.drain_events().await?;

                for (_, approval) in session.pending_approvals.drain() {
                    let _ = approval.resolver.send(PermissionOutcome::Cancelled);
                }
                for (req_id, question) in session.pending_questions.drain() {
                    let _ = question.resolver.send(PermissionOutcome::Cancelled);
                    let _ = self.event_tx.send(ProviderRuntimeEvent::UserInputResolved { request_id: req_id });
                }

                finish_subagents(&mut session.subagents, "cancelled", None, &self.event_tx);

                if let Some(ref m) = input.model {
                    if let Err(err) = session.runtime.set_model(m).await {
                        session.active_turn_id = None;
                        session.status = "error".to_string();
                        let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                            turn_id: existing_turn_id.clone(),
                            payload: TurnCompletedPayload {
                                state: TurnState::Failed,
                                error: Some(err.to_string()),
                            },
                        });
                        return Err(err);
                    }
                    session.model = m.clone();
                }
                session.runtime.set_mode(antigravity_permission_mode(&session.runtime_mode)).await?;
                existing_turn_id
            } else {
                let new_turn_id = Uuid::new_v4().to_string();
                session.active_turn_id = Some(new_turn_id.clone());
                if input.model.as_ref().is_some_and(|m| m != &session.model) {
                    let m = input.model.as_ref().unwrap();
                    session.runtime.set_model(m).await?;
                    session.model = m.clone();
                }
                let _ = self.event_tx.send(ProviderRuntimeEvent::TurnStarted {
                    turn_id: new_turn_id.clone(),
                    payload: TurnStartedPayload {
                        model: Some(session.model.clone()),
                    },
                });
                new_turn_id
            };

            session.status = "running".to_string();
            let p_input = format!("{}\n\nYou are running inside the Antigravity harness, as {}", input.input, session.model);
            (tid, generation_val, rt, p_input)
        };

        struct TurnGuard {
            session_arc: Arc<Mutex<SessionContext>>,
            turn_id: String,
            generation: usize,
            event_tx: broadcast::Sender<ProviderRuntimeEvent>,
            runtime: Arc<dyn AcpRuntime>,
            settled: bool,
        }

        impl Drop for TurnGuard {
            fn drop(&mut self) {
                if !self.settled {
                    let session_arc = self.session_arc.clone();
                    let turn_id = self.turn_id.clone();
                    let generation_val = self.generation;
                    let event_tx = self.event_tx.clone();
                    let runtime = self.runtime.clone();
                    tokio::spawn(async move {
                        let _ = runtime.cancel().await;
                        let mut s = session_arc.lock().await;
                        if s.generation == generation_val && s.active_turn_id.as_deref() == Some(&turn_id) {
                            s.active_turn_id = None;
                            s.status = "ready".to_string();
                            let _ = event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                                turn_id,
                                payload: TurnCompletedPayload {
                                    state: TurnState::Cancelled,
                                    error: None,
                                },
                            });
                        }
                    });
                }
            }
        }

        let mut guard = TurnGuard {
            session_arc: session_arc.clone(),
            turn_id: turn_id.clone(),
            generation,
            event_tx: self.event_tx.clone(),
            runtime: runtime.clone(),
            settled: false,
        };

        let res = runtime.prompt(&prompt_input).await;
        guard.settled = true;

        let mut s = session_arc.lock().await;
        if s.generation == generation {
            s.active_turn_id = None;

            // Promote background commands
            for (cmd_id, cmd) in s.commands.iter_mut() {
                if !cmd.promoted {
                    cmd.promoted = true;
                    let _ = self.event_tx.send(ProviderRuntimeEvent::TaskStarted {
                        turn_id: cmd.turn_id.clone(),
                        payload: TaskPayload {
                            task_id: cmd_id.clone(),
                            task_type: "local_bash".to_string(),
                            title: "Antigravity command".to_string(),
                            status: "running".to_string(),
                            description: cmd.tool_call.command.clone().or_else(|| cmd.tool_call.title.clone()),
                            summary: None,
                            timeline_bypass: None,
                            tool_use_id: Some(cmd_id.clone()),
                        },
                    });
                }
            }

            let subagent_status = match &res {
                Ok(resp) if resp.stop_reason == "cancelled" => "cancelled",
                Ok(_) => "idle",
                Err(_) => "failed",
            };
            finish_subagents(&mut s.subagents, subagent_status, None, &self.event_tx);

            match res {
                Ok(resp) if resp.stop_reason == "cancelled" => {
                    s.status = "ready".to_string();
                    let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                        turn_id: turn_id.clone(),
                        payload: TurnCompletedPayload {
                            state: TurnState::Cancelled,
                            error: None,
                        },
                    });
                }
                Ok(_) => {
                    s.status = "ready".to_string();
                    let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                        turn_id: turn_id.clone(),
                        payload: TurnCompletedPayload {
                            state: TurnState::Completed,
                            error: None,
                        },
                    });
                }
                Err(err) => {
                    s.status = "error".to_string();
                    let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                        turn_id: turn_id.clone(),
                        payload: TurnCompletedPayload {
                            state: TurnState::Failed,
                            error: Some(err.to_string()),
                        },
                    });
                    return Err(err);
                }
            }
        }

        Ok(SendTurnResult { turn_id })
    }

    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.lock().await;
        let mut list = Vec::new();
        for (thread_id, ctx_arc) in sessions.iter() {
            let ctx = ctx_arc.lock().await;
            list.push(SessionInfo {
                thread_id: thread_id.clone(),
                status: ctx.status.clone(),
                active_turn_id: ctx.active_turn_id.clone(),
                model: ctx.model.clone(),
                cwd: ctx.cwd.clone(),
            });
        }
        list
    }

    pub async fn rollback_thread(&self, _thread_id: &str, _num_turns: usize) -> Result<(), HarnessError> {
        Err(HarnessError::Validation(
            "Antigravity does not support conversation rewind. Start a new thread instead.".to_string(),
        ))
    }

    pub async fn steer_session(
        &self,
        thread_id: &str,
        model: Option<String>,
        runtime_mode: Option<String>,
    ) -> Result<SessionState, HarnessError> {
        let sessions = self.sessions.lock().await;
        let session_ctx = sessions
            .get(thread_id)
            .ok_or_else(|| HarnessError::Validation(format!("Session for thread {thread_id} not found")))?
            .clone();
        drop(sessions);

        let mut session = session_ctx.lock().await;
        if let Some(ref m) = model
            && !m.is_empty()
        {
            session.runtime.set_model(m).await?;
            session.model = m.clone();
        }
        if let Some(ref rm) = runtime_mode
            && !rm.is_empty()
        {
            let perm_mode = antigravity_permission_mode(rm);
            session.runtime.set_mode(perm_mode).await?;
            session.runtime_mode = rm.clone();
        }

        Ok(SessionState {
            session_id: session.session_id.clone(),
            model: session.model.clone(),
            runtime_mode: session.runtime_mode.clone(),
            cwd: session.cwd.clone(),
            resume_cursor: SessionCursor {
                session_id: session.session_id.clone(),
                model: session.model.clone(),
                runtime_mode: session.runtime_mode.clone(),
            },
        })
    }

    pub async fn interrupt_turn(&self, thread_id: &str) -> Result<(), HarnessError> {
        let session_arc = {
            let sessions = self.sessions.lock().await;
            sessions.get(thread_id).cloned().ok_or_else(|| {
                HarnessError::Validation(format!("Session for thread {} not found", thread_id))
            })?
        };

        let mut session = session_arc.lock().await;
        if let Some(turn_id) = session.active_turn_id.take() {
            // Cancel pending approvals
            for (_, approval) in session.pending_approvals.drain() {
                let _ = approval.resolver.send(PermissionOutcome::Cancelled);
            }
            // Cancel pending questions
            for (req_id, question) in session.pending_questions.drain() {
                let _ = question.resolver.send(PermissionOutcome::Cancelled);
                let _ = self.event_tx.send(ProviderRuntimeEvent::UserInputResolved { request_id: req_id });
            }
            finish_subagents(&mut session.subagents, "cancelled", None, &self.event_tx);

            if let Err(e) = session.runtime.cancel().await {
                tracing::warn!("session.runtime.cancel failed during interrupt_turn: {}", e);
            }

            let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                turn_id,
                payload: TurnCompletedPayload {
                    state: TurnState::Cancelled,
                    error: None,
                },
            });
        }
        Ok(())
    }

    pub async fn handle_permission(&self, thread_id: &str, req: PermissionRequest) -> PermissionOutcome {
        let session_arc = {
            let sessions = self.sessions.lock().await;
            sessions.get(thread_id).cloned()
        };

        let Some(session_arc) = session_arc else {
            return PermissionOutcome::Cancelled;
        };

        let is_question = req.tool_call.kind == "interaction"
            || req.tool_call.tool_call_id.starts_with("interaction_");

        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        if is_question {
            let question_spec = QuestionSpec {
                allow_custom_answer: false,
                options: req.options.iter().map(|o| QuestionChoice {
                    value: o.option_id.clone(),
                    label: o.name.clone(),
                }).collect(),
            };
            {
                let mut session = session_arc.lock().await;
                session.pending_questions.insert(
                    request_id.clone(),
                    PendingQuestion {
                        tool_call_id: req.tool_call.tool_call_id.clone(),
                        options: req.options.clone(),
                        resolver: tx,
                    },
                );
            }
            let _ = self.event_tx.send(ProviderRuntimeEvent::UserInputRequested {
                request_id,
                payload: UserInputPayload {
                    tool_call_id: req.tool_call.tool_call_id,
                    questions: vec![question_spec],
                },
            });
        } else {
            {
                let mut session = session_arc.lock().await;
                session.pending_approvals.insert(
                    request_id.clone(),
                    PendingApproval {
                        request: req.clone(),
                        resolver: tx,
                    },
                );
            }
            let _ = self.event_tx.send(ProviderRuntimeEvent::RequestOpened {
                request_id,
                payload: RequestOpenedPayload {
                    tool_call: req.tool_call,
                    options: antigravity_approval_options(),
                },
            });
        }

        rx.await.unwrap_or(PermissionOutcome::Cancelled)
    }

    pub async fn respond_to_request(
        &self,
        thread_id: &str,
        request_id: &str,
        decision: &str,
    ) -> Result<(), HarnessError> {
        if decision == "acceptAlways" {
            return Err(HarnessError::Unsupported(
                "Antigravity adapter does not support acceptAlways".to_string(),
            ));
        }

        let session_arc = {
            let sessions = self.sessions.lock().await;
            sessions.get(thread_id).cloned().ok_or_else(|| {
                HarnessError::Validation(format!("Session for thread {} not found", thread_id))
            })?
        };

        let mut session = session_arc.lock().await;
        let approval = session.pending_approvals.remove(request_id).ok_or_else(|| {
            HarnessError::InvalidRequestId(request_id.to_string())
        })?;

        let outcome = if let Some(matched) = approval.request.options.iter().find(|o| o.option_id == decision) {
            PermissionOutcome::Selected { option_id: matched.option_id.clone() }
        } else {
            match decision {
                "accept" | "approved" | "allow" | "allow_once" | "allow_always" => {
                    let opt = approval.request.options.iter()
                        .find(|o| o.kind == "allow_once" || o.option_id.contains("allow"))
                        .map(|o| o.option_id.clone())
                        .unwrap_or_else(|| "allow_once".to_string());
                    PermissionOutcome::Selected { option_id: opt }
                }
                "decline" | "rejected" | "reject" | "deny" | "reject_once" | "reject_always" => {
                    let opt = approval.request.options.iter()
                        .find(|o| o.kind == "reject_once" || o.option_id.contains("deny") || o.option_id.contains("reject"))
                        .map(|o| o.option_id.clone())
                        .unwrap_or_else(|| "reject_once".to_string());
                    PermissionOutcome::Selected { option_id: opt }
                }
                "cancel" | "cancelled" => PermissionOutcome::Cancelled,
                other => {
                    return Err(HarnessError::InvalidDecision {
                        request_id: request_id.to_string(),
                        reason: format!("Unknown decision {}", other),
                    })
                }
            }
        };

        let _ = approval.resolver.send(outcome);
        let _ = self.event_tx.send(ProviderRuntimeEvent::RequestResolved {
            request_id: request_id.to_string(),
        });
        Ok(())
    }

    pub async fn respond_to_user_input(
        &self,
        thread_id: &str,
        request_id: &str,
        answers: &HashMap<String, String>,
    ) -> Result<(), HarnessError> {
        let session_arc = {
            let sessions = self.sessions.lock().await;
            sessions.get(thread_id).cloned().ok_or_else(|| {
                HarnessError::Validation(format!("Session for thread {} not found", thread_id))
            })?
        };

        let mut session = session_arc.lock().await;
        let question = session.pending_questions.get(request_id).ok_or_else(|| {
            HarnessError::InvalidRequestId(request_id.to_string())
        })?;

        let answer = answers.get(&question.tool_call_id).ok_or_else(|| {
            HarnessError::Validation(format!("Missing answer for question {}", question.tool_call_id))
        })?;

        let has_direct_id = question.options.iter().any(|o| &o.option_id == answer);
        let matches_label_only = !has_direct_id && question.options.iter().any(|o| &o.name == answer);

        if matches_label_only || !has_direct_id {
            return Err(HarnessError::Validation(
                "Ambiguous choice label or invalid choice value".to_string(),
            ));
        }

        let question = session.pending_questions.remove(request_id).unwrap();
        let _ = question.resolver.send(PermissionOutcome::Selected {
            option_id: answer.clone(),
        });
        let _ = self.event_tx.send(ProviderRuntimeEvent::UserInputResolved {
            request_id: request_id.to_string(),
        });
        Ok(())
    }

    pub async fn stop_session(&self, thread_id: &str) -> Result<(), HarnessError> {
        let session_opt = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(thread_id)
        };

        if let Some(session_arc) = session_opt {
            let mut session = session_arc.lock().await;
            if let Some(turn_id) = session.active_turn_id.take() {
                for (_, approval) in session.pending_approvals.drain() {
                    let _ = approval.resolver.send(PermissionOutcome::Cancelled);
                }
                for (req_id, question) in session.pending_questions.drain() {
                    let _ = question.resolver.send(PermissionOutcome::Cancelled);
                    let _ = self.event_tx.send(ProviderRuntimeEvent::UserInputResolved { request_id: req_id });
                }
                finish_subagents(&mut session.subagents, "cancelled", None, &self.event_tx);
                let _ = session.runtime.cancel().await;
                let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                    turn_id,
                    payload: TurnCompletedPayload {
                        state: TurnState::Cancelled,
                        error: None,
                    },
                });
            }
        }
        Ok(())
    }
}

impl Default for AntigravityAdapter {
    fn default() -> Self {
        Self::new()
    }
}
