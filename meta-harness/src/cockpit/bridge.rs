use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use crate::adapter::AntigravityAdapter;
use crate::protocol::{
    ProviderRuntimeEvent, SendTurnInput, StartSessionInput, StreamKind,
};

#[derive(Debug, Clone)]
pub enum AppCommand {
    StartSession {
        thread_id: String,
        cwd: String,
        model: String,
        mode: String,
    },
    SendPrompt {
        thread_id: String,
        prompt: String,
    },
    SteerSession {
        thread_id: String,
        model: Option<String>,
        mode: Option<String>,
    },
    CancelTurn {
        thread_id: String,
    },
    StopSession {
        thread_id: String,
    },
    ApproveTool {
        thread_id: String,
        request_id: String,
        decision: String,
    },
    ReadFile {
        path: String,
        line: Option<usize>,
    },
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    SessionStarted {
        session_id: String,
        model: String,
        mode: String,
    },
    TurnStarted {
        turn_id: String,
    },
    ContentDelta {
        delta: String,
        is_thought: bool,
    },
    TaskUpdated {
        task_id: String,
        title: String,
        status: String,
    },
    ApprovalRequested {
        request_id: String,
        title: String,
        details: String,
        options: Vec<(String, String)>,
    },
    TurnCompleted {
        turn_id: String,
    },
    FileLoaded {
        path: String,
        content: String,
        lines: usize,
        highlight_line: Option<usize>,
    },
    Toast(String),
    Error(String),
    ArchitectureFocused {
        root_id: String,
        depth: usize,
        direction: String,
        affected_components: Vec<String>,
    },
    ArchitectureUpdated {
        component_id: String,
        status: String,
    },
}

pub struct CockpitBridge {
    pub cmd_tx: UnboundedSender<AppCommand>,
    pub event_rx: UnboundedReceiver<AppEvent>,
}

impl CockpitBridge {
    pub fn spawn(egui_ctx: egui::Context) -> Self {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<AppCommand>();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();

        let event_sender = event_tx.clone();
        let ctx = egui_ctx.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build Tokio runtime for Cockpit Bridge");

            rt.block_on(async move {
                let adapter = Arc::new(AntigravityAdapter::new());

                let mut event_subscriber = adapter.subscribe();
                let sub_event_tx = event_sender.clone();
                let sub_ctx = ctx.clone();

                // Forward adapter events to egui
                tokio::spawn(async move {
                    while let Ok(ev) = event_subscriber.recv().await {
                        let app_event = match ev {
                            ProviderRuntimeEvent::TurnStarted { turn_id, .. } => {
                                Some(AppEvent::TurnStarted { turn_id })
                            }
                            ProviderRuntimeEvent::TurnCompleted { turn_id, .. } => {
                                Some(AppEvent::TurnCompleted { turn_id })
                            }
                            ProviderRuntimeEvent::ContentDelta { payload, .. } => {
                                let is_thought = payload.stream_kind == StreamKind::ReasoningText;
                                Some(AppEvent::ContentDelta {
                                    delta: payload.delta,
                                    is_thought,
                                })
                            }
                            ProviderRuntimeEvent::TaskStarted { payload, .. }
                            | ProviderRuntimeEvent::TaskProgress { payload, .. }
                            | ProviderRuntimeEvent::TaskUpdated { payload, .. }
                            | ProviderRuntimeEvent::TaskCompleted { payload, .. } => {
                                Some(AppEvent::TaskUpdated {
                                    task_id: payload.task_id,
                                    title: payload.title,
                                    status: payload.status,
                                })
                            }
                            ProviderRuntimeEvent::RequestOpened { request_id, payload } => {
                                let title = payload.tool_call.title.unwrap_or_else(|| "Tool Approval Required".to_string());
                                let details = payload.tool_call.raw_output.unwrap_or_default();
                                let mut opts = Vec::new();
                                for o in payload.options {
                                    opts.push((o.decision, o.label));
                                }
                                if opts.is_empty() {
                                    opts.push(("accept".to_string(), "Approve".to_string()));
                                    opts.push(("reject".to_string(), "Reject".to_string()));
                                }
                                Some(AppEvent::ApprovalRequested {
                                    request_id,
                                    title,
                                    details,
                                    options: opts,
                                })
                            }
                            ProviderRuntimeEvent::ArchitectureFocused {
                                root_id,
                                depth,
                                direction,
                                affected_components,
                            } => Some(AppEvent::ArchitectureFocused {
                                root_id,
                                depth,
                                direction,
                                affected_components,
                            }),
                            ProviderRuntimeEvent::ArchitectureUpdated {
                                component_id,
                                status,
                            } => Some(AppEvent::ArchitectureUpdated {
                                component_id,
                                status,
                            }),
                            _ => None,
                        };

                        if let Some(app_ev) = app_event {
                            let _ = sub_event_tx.send(app_ev);
                            sub_ctx.request_repaint();
                        }
                    }
                });

                // Listen for UI commands
                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        AppCommand::StartSession { thread_id, cwd, model, mode } => {
                            let ad = adapter.clone();
                            let tx = event_sender.clone();
                            let ctx_repaint = ctx.clone();
                            tokio::spawn(async move {
                                let input = StartSessionInput {
                                    thread_id,
                                    cwd,
                                    runtime_mode: mode,
                                    model: Some(model),
                                    ..Default::default()
                                };
                                match ad.start_session(input).await {
                                    Ok(state) => {
                                        let _ = tx.send(AppEvent::SessionStarted {
                                            session_id: state.session_id,
                                            model: state.model,
                                            mode: state.runtime_mode,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = tx.send(AppEvent::Error(format!("Failed to start session: {e}")));
                                    }
                                }
                                ctx_repaint.request_repaint();
                            });
                        }
                        AppCommand::SendPrompt { thread_id, prompt } => {
                            let ad = adapter.clone();
                            let tx = event_sender.clone();
                            let ctx_repaint = ctx.clone();
                            tokio::spawn(async move {
                                let input = SendTurnInput {
                                    thread_id,
                                    input: prompt,
                                    model: None,
                                };
                                if let Err(e) = ad.send_turn(input).await {
                                    let _ = tx.send(AppEvent::Error(format!("Turn prompt failed: {e}")));
                                    ctx_repaint.request_repaint();
                                }
                            });
                        }
                        AppCommand::SteerSession { thread_id, model, mode } => {
                            let ad = adapter.clone();
                            let tx = event_sender.clone();
                            let ctx_repaint = ctx.clone();
                            tokio::spawn(async move {
                                match ad.steer_session(&thread_id, model.clone(), mode.clone()).await {
                                    Ok(_) => {
                                        let desc = format!("Steered: model={:?}, mode={:?}", model, mode);
                                        let _ = tx.send(AppEvent::Toast(desc));
                                    }
                                    Err(e) => {
                                        let _ = tx.send(AppEvent::Error(format!("Steering failed: {e}")));
                                    }
                                }
                                ctx_repaint.request_repaint();
                            });
                        }
                        AppCommand::CancelTurn { thread_id } => {
                            let ad = adapter.clone();
                            let tx = event_sender.clone();
                            let ctx_repaint = ctx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = ad.interrupt_turn(&thread_id).await {
                                    let _ = tx.send(AppEvent::Error(format!("Cancel failed: {e}")));
                                } else {
                                    let _ = tx.send(AppEvent::Toast("Turn cancelled".to_string()));
                                }
                                ctx_repaint.request_repaint();
                            });
                        }
                        AppCommand::StopSession { thread_id } => {
                            let ad = adapter.clone();
                            let tx = event_sender.clone();
                            let ctx_repaint = ctx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = ad.stop_session(&thread_id).await {
                                    let _ = tx.send(AppEvent::Error(format!("Stop session failed: {e}")));
                                } else {
                                    let _ = tx.send(AppEvent::Toast("Session stopped".to_string()));
                                }
                                ctx_repaint.request_repaint();
                            });
                        }
                        AppCommand::ApproveTool { thread_id, request_id, decision } => {
                            let ad = adapter.clone();
                            let tx = event_sender.clone();
                            let ctx_repaint = ctx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = ad.respond_to_request(&thread_id, &request_id, &decision).await {
                                    let _ = tx.send(AppEvent::Error(format!("Approval failed: {e}")));
                                }
                                ctx_repaint.request_repaint();
                            });
                        }
                        AppCommand::ReadFile { path, line } => {
                            let tx = event_sender.clone();
                            let ctx_repaint = ctx.clone();
                            tokio::spawn(async move {
                                match tokio::fs::read_to_string(&path).await {
                                    Ok(content) => {
                                        let lines = content.lines().count();
                                        let _ = tx.send(AppEvent::FileLoaded {
                                            path,
                                            content,
                                            lines,
                                            highlight_line: line,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = tx.send(AppEvent::Error(format!("Failed to read {path}: {e}")));
                                    }
                                }
                                ctx_repaint.request_repaint();
                            });
                        }
                    }
                }
            });
        });

        Self { cmd_tx, event_rx }
    }
}
