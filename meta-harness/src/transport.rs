use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use futures::future::BoxFuture;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, ChildStderr};
use tokio::sync::{broadcast, oneshot, Mutex};

use crate::adapter::AcpRuntime;
use crate::error::HarnessError;
use crate::fs_proxy::{
    read_client_text_file, write_client_text_file, ReadTextFileRequest, WriteTextFileRequest,
};
use crate::protocol::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

pub type PermissionHandler =
    Arc<dyn Fn(PermissionRequest) -> BoxFuture<'static, PermissionOutcome> + Send + Sync>;

pub type PendingRequests =
    Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, HarnessError>>>>>;

pub struct JsonRpcTransport {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending_requests: PendingRequests,
    next_id: AtomicU64,
    events_tx: broadcast::Sender<NativeEvent>,
    permission_handler: Arc<std::sync::RwLock<Option<PermissionHandler>>>,
    allowed_roots: Arc<Vec<PathBuf>>,
}

impl JsonRpcTransport {
    pub fn new(
        stdin: ChildStdin,
        stdout: ChildStdout,
        stderr: Option<ChildStderr>,
        allowed_roots: Vec<PathBuf>,
        on_auth_url: Option<Arc<dyn Fn(String) + Send + Sync>>,
    ) -> Arc<Self> {
        let (events_tx, _) = broadcast::channel(2048);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let shared_stdin = Arc::new(Mutex::new(Some(stdin)));
        let perm_handler = Arc::new(std::sync::RwLock::new(None));
        let roots = Arc::new(allowed_roots);

        let transport = Arc::new(Self {
            stdin: shared_stdin.clone(),
            pending_requests: pending.clone(),
            next_id: AtomicU64::new(1),
            events_tx: events_tx.clone(),
            permission_handler: perm_handler.clone(),
            allowed_roots: roots.clone(),
        });

        // Spawn stdout listener loop
        let pending_for_stdout = pending.clone();
        let stdin_for_stdout = shared_stdin.clone();
        let events_for_stdout = events_tx.clone();
        let perm_for_stdout = perm_handler.clone();
        let roots_for_stdout = roots.clone();
        let auth_cb_for_stdout = on_auth_url.clone();

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                    tracing::debug!(target: "subprocess_stdout_raw", "{}", trimmed);
                    // Check if stdout line contains an auth URL before discarding
                    if let Some(ref auth_cb) = auth_cb_for_stdout
                        && let Some(url) = crate::driver::detect_auth_url(trimmed)
                    {
                        auth_cb(url);
                    }
                    continue;
                };

                // Case 1: Inbound Response to our request
                if let Some(id_val) = val.get("id") && (val.get("result").is_some() || val.get("error").is_some()) {
                    if let Some(id) = id_val.as_u64() {
                        let mut guard = pending_for_stdout.lock().await;
                        if let Some(sender) = guard.remove(&id) {
                            if let Some(err) = val.get("error") && !err.is_null() {
                                let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("RPC Error");
                                tracing::error!(
                                    target: "subprocess_rpc",
                                    "RPC error response received from child for req_id {}: {:?}",
                                    id,
                                    err
                                );
                                let _ = sender.send(Err(HarnessError::ProviderDriver {
                                    detail: msg.to_string(),
                                }));
                            } else {
                                let result = val.get("result").cloned().unwrap_or(serde_json::Value::Null);
                                let _ = sender.send(Ok(result));
                            }
                        }
                    }
                    continue;
                }

                // Case 2: Inbound Reverse-RPC Request from server
                if let Some(method) = val.get("method").and_then(|m| m.as_str()) && let Some(id) = val.get("id") {
                    let id_clone = id.clone();
                    let stdin_ref = stdin_for_stdout.clone();
                    let perm_ref = perm_for_stdout.clone();
                    let roots_ref = roots_for_stdout.clone();
                    let params = val.get("params").cloned().unwrap_or(serde_json::Value::Null);
                    let method_owned = method.to_string();

                    tokio::spawn(async move {
                        let response_val = match method_owned.as_str() {
                            "session/request_permission" => {
                                let handler_opt = {
                                    let guard = perm_ref.read().unwrap();
                                    guard.clone()
                                };
                                if let Some(handler) = handler_opt {
                                    match serde_json::from_value::<PermissionRequest>(params) {
                                        Ok(req) => {
                                            tracing::info!(target: "subprocess_rpc", "Received session/request_permission for tool: {}", req.tool_call.kind);
                                            let outcome = handler(req).await;
                                            tracing::info!(target: "subprocess_rpc", "Resolved session/request_permission with outcome: {:?}", outcome);
                                            serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": id_clone,
                                                "result": serde_json::json!({
                                                    "outcome": outcome
                                                }),
                                            })
                                        }
                                        Err(e) => {
                                            serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": id_clone,
                                                "error": { "code": -32602, "message": format!("Invalid permission request params: {}", e) }
                                            })
                                        }
                                    }
                                } else {
                                    serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": id_clone,
                                        "error": { "code": -32603, "message": "No permission handler installed" }
                                    })
                                }
                            }
                            "fs/read_text_file" => {
                                match serde_json::from_value::<ReadTextFileRequest>(params) {
                                    Ok(req) => match read_client_text_file(&roots_ref, &req) {
                                        Ok(res) => serde_json::json!({
                                            "jsonrpc": "2.0",
                                            "id": id_clone,
                                            "result": res,
                                        }),
                                        Err(e) => {
                                            tracing::error!(target: "subprocess_rpc", "fs/read_text_file error: {}", e);
                                            serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": id_clone,
                                                "error": { "code": -32602, "message": e.to_string() }
                                            })
                                        }
                                    },
                                    Err(e) => {
                                        tracing::error!(target: "subprocess_rpc", "fs/read_text_file invalid params: {}", e);
                                        serde_json::json!({
                                            "jsonrpc": "2.0",
                                            "id": id_clone,
                                            "error": { "code": -32602, "message": e.to_string() }
                                        })
                                    }
                                }
                            }
                            "fs/write_text_file" => {
                                match serde_json::from_value::<WriteTextFileRequest>(params) {
                                    Ok(req) => match write_client_text_file(&roots_ref, &req) {
                                        Ok(res) => serde_json::json!({
                                            "jsonrpc": "2.0",
                                            "id": id_clone,
                                            "result": res,
                                        }),
                                        Err(e) => {
                                            tracing::error!(target: "subprocess_rpc", "fs/write_text_file error: {}", e);
                                            serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": id_clone,
                                                "error": { "code": -32602, "message": e.to_string() }
                                            })
                                        }
                                    },
                                    Err(e) => {
                                        tracing::error!(target: "subprocess_rpc", "fs/write_text_file invalid params: {}", e);
                                        serde_json::json!({
                                            "jsonrpc": "2.0",
                                            "id": id_clone,
                                            "error": { "code": -32602, "message": e.to_string() }
                                        })
                                    }
                                }
                            }
                            unknown_method => {
                                tracing::warn!(target: "subprocess_rpc", "Unhandled reverse-RPC method from child: {}", unknown_method);
                                serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id_clone,
                                    "error": { "code": -32601, "message": "Method not found" }
                                })
                            }
                        };

                        let mut line_buf = serde_json::to_string(&response_val).unwrap_or_default();
                        line_buf.push('\n');
                        let mut guard = stdin_ref.lock().await;
                        if let Some(ref mut sin) = *guard {
                            if let Err(e) = sin.write_all(line_buf.as_bytes()).await {
                                tracing::error!(target: "subprocess_rpc", "Failed to write reverse-RPC response to child stdin: {}", e);
                            }
                            if let Err(e) = sin.flush().await {
                                tracing::error!(target: "subprocess_rpc", "Failed to flush reverse-RPC response to child stdin: {}", e);
                            }
                        }
                    });
                    continue;
                }

                // Case 3: Inbound Notification from server
                if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
                    let params = val.get("params").cloned().unwrap_or(serde_json::Value::Null);
                    if method == "session/update" {
                        let update_val = if let Some(up) = params.get("update") {
                            up.clone()
                        } else if let Some(ev) = params.get("event") {
                            ev.clone()
                        } else {
                            params.clone()
                        };

                        if let Ok(evt) = serde_json::from_value::<NativeEvent>(update_val.clone()) {
                            let _ = events_for_stdout.send(evt);
                        } else {
                            let tag = update_val
                                .get("sessionUpdate")
                                .or_else(|| update_val.get("type"))
                                .or_else(|| update_val.get("_tag"))
                                .and_then(|t| t.as_str());
                            match tag {
                                Some("agent_message_chunk") | Some("text_delta") | Some("content_delta") => {
                                    let delta = update_val
                                        .get("content")
                                        .and_then(|c| c.get("text"))
                                        .or_else(|| update_val.get("delta"))
                                        .or_else(|| update_val.get("text"))
                                        .and_then(|d| d.as_str())
                                        .unwrap_or("");
                                    let _ = events_for_stdout.send(NativeEvent::ContentDelta {
                                        text: delta.to_string(),
                                    });
                                }
                                Some("agent_thought_chunk") | Some("thought_delta") | Some("thought_chunk") => {
                                    let delta = update_val
                                        .get("content")
                                        .and_then(|c| c.get("text"))
                                        .or_else(|| update_val.get("delta"))
                                        .or_else(|| update_val.get("text"))
                                        .and_then(|d| d.as_str())
                                        .unwrap_or("");
                                    let _ = events_for_stdout.send(NativeEvent::ThoughtDelta {
                                        text: delta.to_string(),
                                    });
                                }
                                Some("tool_call") | Some("tool_call_update") => {
                                    if let Ok(tool_call) = serde_json::from_value::<ToolCallState>(update_val.clone()) {
                                        let _ = events_for_stdout.send(NativeEvent::ToolCallUpdated { tool_call });
                                    }
                                }
                                Some("available_commands_update") => {
                                    let cmds = update_val
                                        .get("availableCommands")
                                        .and_then(|c| c.as_array())
                                        .map(|arr| {
                                            arr.iter()
                                                .filter_map(|cmd| cmd.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    let _ = events_for_stdout.send(NativeEvent::AvailableCommandsUpdated { commands: cmds });
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }

            // Loop exited due to EOF or stream termination: drain all pending requests to avoid hanging callers
            {
                let mut guard = pending_for_stdout.lock().await;
                for (_id, sender) in guard.drain() {
                    let _ = sender.send(Err(HarnessError::ProviderDriver {
                        detail: "Subprocess exited or closed stdout".to_string(),
                    }));
                }
            }
            let _ = events_for_stdout.send(NativeEvent::ConnectionTerminated {
                detail: "Process terminated or disconnected".to_string(),
            });
            {
                let mut stdin_guard = stdin_for_stdout.lock().await;
                *stdin_guard = None;
            }
        });

        // Spawn stderr listener loop
        if let Some(err) = stderr {
            let auth_cb_for_stderr = on_auth_url;
            tokio::spawn(async move {
                let mut reader = BufReader::new(err).lines();
                loop {
                    match reader.next_line().await {
                        Ok(Some(line)) => {
                            let trimmed = line.trim();
                            if !trimmed.is_empty() {
                                tracing::warn!(target: "subprocess_stderr", "{}", trimmed);
                                if let Some(ref auth_cb) = auth_cb_for_stderr
                                    && let Some(url) = crate::driver::detect_auth_url(trimmed)
                                {
                                    auth_cb(url);
                                }
                            }
                        }
                        Ok(None) => {
                            tracing::debug!(target: "subprocess_stderr", "Subprocess stderr reached EOF");
                            break;
                        }
                        Err(e) => {
                            tracing::error!(target: "subprocess_stderr", "Error reading subprocess stderr: {}", e);
                            break;
                        }
                    }
                }
            });
        }

        transport
    }

    pub fn allowed_roots(&self) -> &[PathBuf] {
        &self.allowed_roots
    }

    pub async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, HarnessError> {
        let req_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.pending_requests.lock().await;
            guard.insert(req_id, tx);
        }

        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params,
        });

        let mut line = serde_json::to_string(&payload).map_err(|e| HarnessError::Validation(e.to_string()))?;
        line.push('\n');

        let write_result = async {
            let mut stdin_guard = self.stdin.lock().await;
            let stdin = stdin_guard.as_mut().ok_or_else(|| HarnessError::ProviderDriver {
                detail: "Child stdin closed".to_string(),
            })?;
            stdin.write_all(line.as_bytes()).await.map_err(HarnessError::Io)?;
            stdin.flush().await.map_err(HarnessError::Io)?;
            Ok::<(), HarnessError>(())
        }.await;

        if let Err(e) = write_result {
            let mut guard = self.pending_requests.lock().await;
            guard.remove(&req_id);
            return Err(e);
        }

        let response = rx.await.map_err(|_| {
            tracing::error!(target: "subprocess_rpc", "Process exited or dropped request without response (req_id={}, method={})", req_id, method);
            HarnessError::ProviderDriver {
                detail: "Process exited or dropped request without response".to_string(),
            }
        })?;
        if let Err(ref e) = response {
            tracing::error!(target: "subprocess_rpc", "Request '{}' failed: {}", method, e);
        }
        response
    }

    pub async fn send_notification(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), HarnessError> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let mut line = serde_json::to_string(&payload).map_err(|e| HarnessError::Validation(e.to_string()))?;
        line.push('\n');

        let mut stdin_guard = self.stdin.lock().await;
        let stdin = stdin_guard.as_mut().ok_or_else(|| HarnessError::ProviderDriver {
            detail: "Child stdin closed".to_string(),
        })?;
        stdin.write_all(line.as_bytes()).await.map_err(HarnessError::Io)?;
        stdin.flush().await.map_err(HarnessError::Io)?;
        Ok(())
    }
}

pub struct NativeAcpRuntime {
    transport: Arc<JsonRpcTransport>,
    session_id: Arc<tokio::sync::RwLock<Option<String>>>,
    auth_method_id: String,
    api_key: Option<String>,
    mcp_servers: Vec<serde_json::Value>,
    _handle: Option<Arc<dyn std::any::Any + Send + Sync>>,
}

impl NativeAcpRuntime {
    pub fn new(
        transport: Arc<JsonRpcTransport>,
        auth_method_id: String,
        api_key: Option<String>,
    ) -> Self {
        Self {
            transport,
            session_id: Arc::new(tokio::sync::RwLock::new(None)),
            auth_method_id,
            api_key,
            mcp_servers: vec![],
            _handle: None,
        }
    }

    pub fn with_handle(
        transport: Arc<JsonRpcTransport>,
        auth_method_id: String,
        api_key: Option<String>,
        handle: Arc<dyn std::any::Any + Send + Sync>,
        mcp_servers: Vec<serde_json::Value>,
    ) -> Self {
        Self {
            transport,
            session_id: Arc::new(tokio::sync::RwLock::new(None)),
            auth_method_id,
            api_key,
            mcp_servers,
            _handle: Some(handle),
        }
    }
}

#[async_trait::async_trait]
impl AcpRuntime for NativeAcpRuntime {
    async fn start(&self) -> Result<String, HarnessError> {
        // Step 1: Handshake initialize
        self.transport
            .send_request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": {
                        "fs": { "readTextFile": true, "writeTextFile": true }
                    },
                    "clientInfo": {
                        "name": "meta-harness",
                        "version": "0.1.0"
                    }
                }),
            )
            .await?;

        // Step 2: Authenticate
        let mut auth_params = serde_json::json!({
            "methodId": self.auth_method_id,
        });
        if let Some(ref key) = self.api_key {
            auth_params["apiKey"] = serde_json::Value::String(key.clone());
        }
        self.transport.send_request("authenticate", auth_params).await?;

        // Step 3: session/new
        let cwd = self
            .transport
            .allowed_roots
            .first()
            .and_then(|p| p.canonicalize().ok().or_else(|| Some(p.clone())))
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| ".".to_string())
            });

        let res = self
            .transport
            .send_request(
                "session/new",
                serde_json::json!({
                    "cwd": cwd,
                    "mcpServers": self.mcp_servers
                }),
            )
            .await?;

        let session_id = res
            .get("sessionId")
            .and_then(|s| s.as_str())
            .ok_or_else(|| HarnessError::ProviderDriver {
                detail: "Server failed to return sessionId from session/new".to_string(),
            })?
            .to_string();

        *self.session_id.write().await = Some(session_id.clone());
        Ok(session_id)
    }

    async fn set_model(&self, model: &str) -> Result<(), HarnessError> {
        let sid = self.session_id.read().await.clone().unwrap_or_default();
        self.transport
            .send_request(
                "session/set_model",
                serde_json::json!({
                    "sessionId": sid,
                    "modelId": model,
                    "model": model,
                }),
            )
            .await?;
        Ok(())
    }

    async fn set_mode(&self, mode: &str) -> Result<(), HarnessError> {
        let sid = self.session_id.read().await.clone().unwrap_or_default();
        self.transport
            .send_request(
                "session/set_mode",
                serde_json::json!({
                    "sessionId": sid,
                    "modeId": mode,
                    "mode": mode,
                }),
            )
            .await?;
        Ok(())
    }

    async fn prompt(&self, text: &str) -> Result<PromptResponse, HarnessError> {
        let sid = self.session_id.read().await.clone().unwrap_or_default();
        let res = self
            .transport
            .send_request(
                "session/prompt",
                serde_json::json!({
                    "sessionId": sid,
                    "prompt": [
                        {
                            "type": "text",
                            "text": text
                        }
                    ]
                }),
            )
            .await?;

        let stop_reason = res
            .get("stopReason")
            .or_else(|| res.get("stop_reason"))
            .and_then(|s| s.as_str())
            .unwrap_or("end_turn")
            .to_string();

        Ok(PromptResponse { stop_reason })
    }

    async fn cancel(&self) -> Result<(), HarnessError> {
        let sid = self.session_id.read().await.clone().unwrap_or_default();
        self.transport
            .send_notification(
                "session/cancel",
                serde_json::json!({
                    "sessionId": sid,
                }),
            )
            .await?;
        Ok(())
    }

    async fn drain_events(&self) -> Result<(), HarnessError> {
        tokio::task::yield_now().await;
        Ok(())
    }

    fn subscribe_events(&self) -> broadcast::Receiver<NativeEvent> {
        self.transport.events_tx.subscribe()
    }

    fn register_permission_handler(
        &self,
        handler: Arc<dyn Fn(PermissionRequest) -> BoxFuture<'static, PermissionOutcome> + Send + Sync>,
    ) {
        let mut guard = self.transport.permission_handler.write().unwrap();
        *guard = Some(handler);
    }
}
