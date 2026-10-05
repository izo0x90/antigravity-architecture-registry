//! HTTP/WebSocket API. Harness routes address instances and threads by id and never
//! know which harness is behind them; everything else (files, feedback, the
//! architecture graph and its MCP endpoint) is app-level.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use futures::{SinkExt, StreamExt};
use harness_contracts::{
    InstanceId, ModelSelection, NativeId, RequestId, RequestResponse, RuntimePolicy, SteerInput, ThreadId, TurnInput,
};
use harness_core::{EventHub, HarnessError, InstanceRegistry, OpenThread, SessionManager};
use serde::{Deserialize, Serialize};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<InstanceRegistry>,
    pub sessions: Arc<SessionManager>,
    pub hub: Arc<EventHub>,
    pub mcp_state: crate::mcp::McpServerState,
    pub arch: Arc<arch_service::ArchitectureService>,
    pub cockpit: crate::cockpit::CockpitAuth,
    pub queue: Arc<crate::queue::QueueRunner>,
}

impl axum::extract::FromRef<AppState> for crate::cockpit::CockpitAuth {
    fn from_ref(state: &AppState) -> Self {
        state.cockpit.clone()
    }
}

/// `HarnessError` as an HTTP response: `{ "error": "..." }` with a matching status.
pub struct ApiError(HarnessError);

impl From<HarnessError> for ApiError {
    fn from(err: HarnessError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            HarnessError::NotFound { .. } => StatusCode::NOT_FOUND,
            HarnessError::InvalidConfig { .. } => StatusCode::BAD_REQUEST,
            HarnessError::Unsupported { .. } => StatusCode::NOT_IMPLEMENTED,
            HarnessError::AuthRequired { .. } => StatusCode::UNAUTHORIZED,
            HarnessError::Provider { .. } | HarnessError::Io(_) => StatusCode::BAD_GATEWAY,
        };
        (status, Json(serde_json::json!({ "error": self.0.to_string() }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Deserialize)]
pub struct FsReadQuery {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FsReadResponse {
    pub path: String,
    pub content: String,
    pub lines: usize,
}

fn default_status() -> String {
    "open".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevFeedbackItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub timestamp: String,
    pub target: serde_json::Value,
    pub category: String,
    pub comment: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub context: Option<serde_json::Value>,
}


/// Forwards the architecture service's events to every connected client.
pub fn bridge_architecture_events(arch: &arch_service::ArchitectureService, hub: &Arc<EventHub>) {
    let hub = hub.clone();
    arch.subscribe(move |event| hub.publish(harness_contracts::ClientEvent::Architecture { event: event.clone() }));
}

/// The work queues, publishing their progress to every client.
pub fn queue_runner(arch: &Arc<arch_service::ArchitectureService>, config: arch_service::QueueConfig, hub: &Arc<EventHub>) -> Arc<crate::queue::QueueRunner> {
    let hub = hub.clone();
    crate::queue::QueueRunner::new(arch.clone(), config, move |event| hub.publish(harness_contracts::ClientEvent::Architecture { event }))
}

pub fn create_router(state: AppState) -> Router {
    let static_dir = std::env::var("STATIC_DIR").unwrap_or_else(|_| "static".to_string());
    let mut router = Router::new()
        .route("/health", get(health_check))
        .route("/api/ui/config", get(ui_config_handler))
        .route("/cockpit", get(crate::cockpit::launch))
        .route("/api/instances", get(list_instances))
        .route("/api/instances/{id}/refresh", post(refresh_instance))
        .route("/api/instances/{id}/sessions", get(list_saved_sessions))
        .route("/api/instances/{id}/sessions/{native_id}", delete(delete_saved_session))
        .route("/api/instances/{id}/auth/{method}", post(begin_auth))
        .route("/api/instances/{id}/auth/complete", post(complete_auth))
        .route("/api/instances/{id}/auth/logout", post(logout))
        .route("/api/threads", post(open_thread))
        .route("/api/threads/{thread}", delete(close_thread))
        .route("/api/threads/{thread}/turns", post(start_turn))
        .route("/api/threads/{thread}/steer", post(steer))
        .route("/api/threads/{thread}/interrupt", post(interrupt))
        .route("/api/threads/{thread}/requests/{request}", post(respond_request))
        .route("/api/fs/read", get(fs_read_handler))
        .route("/api/dev/feedback", get(get_dev_feedback_handler).post(dev_feedback_handler).put(update_dev_feedback_handler))
        .route("/api/mcp/sse", get(mcp_sse_proxy).post(mcp_messages_proxy))
        .route("/api/mcp/messages", post(mcp_messages_proxy))
        .route("/ws/events", get(ws_events_handler))
        .merge(crate::arch::routes())
        .merge(crate::queue::routes());

    if std::path::Path::new(&static_dir).exists() {
        router = router.fallback_service(ServeDir::new(static_dir));
    }

    // Same-origin only: no CORS layer, so other sites can't call the API.
    router.layer(axum::middleware::map_response(json_errors)).layer(TraceLayer::new_for_http()).with_state(state)
}

/// Errors from extractors (bad JSON, bad paths) come back as plain text;
/// every API error is `{ "error": "..." }`.
async fn json_errors(response: Response) -> Response {
    let status = response.status();
    let is_json = response.headers().get(axum::http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("application/json"));
    if !(status.is_client_error() || status.is_server_error()) || is_json {
        return response;
    }
    let bytes = match axum::body::to_bytes(response.into_body(), 64 * 1024).await {
        Ok(b) => b,
        Err(e) => return (status, Json(serde_json::json!({ "error": format!("unreadable error body: {e}") }))).into_response(),
    };
    let text = String::from_utf8_lossy(&bytes).trim().to_owned();
    let message = if text.is_empty() { status.canonical_reason().unwrap_or("error").to_owned() } else { text };
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

// ---- instances ----

async fn list_instances(State(state): State<AppState>) -> Json<Vec<harness_contracts::ProviderSnapshot>> {
    Json(state.registry.snapshots())
}

async fn refresh_instance(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    state.registry.get(&InstanceId::new(id))?.refresh().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct SavedSessionsQuery {
    pub cwd: String,
    pub cursor: Option<String>,
}

async fn list_saved_sessions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<SavedSessionsQuery>,
) -> ApiResult<Json<harness_contracts::SavedSessionPage>> {
    let catalog = state.registry.get(&InstanceId::new(id))?.sessions().ok_or(HarnessError::Unsupported { operation: "saved sessions" })?;
    Ok(Json(catalog.list(&query.cwd, query.cursor.as_deref()).await?))
}

async fn delete_saved_session(
    State(state): State<AppState>,
    Path((id, native_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let catalog = state.registry.get(&InstanceId::new(id))?.sessions().ok_or(HarnessError::Unsupported { operation: "saved sessions" })?;
    catalog.delete(&NativeId::new(native_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthChallengeResponse {
    pub url: Option<String>,
    pub instructions: String,
}

#[derive(Debug, Deserialize)]
pub struct CompleteAuthRequest {
    pub response: String,
}

fn auth_controller(state: &AppState, id: String) -> ApiResult<Arc<dyn harness_core::AuthController>> {
    Ok(state.registry.get(&InstanceId::new(id))?.auth().ok_or(HarnessError::Unsupported { operation: "login" })?)
}

async fn begin_auth(State(state): State<AppState>, Path((id, method)): Path<(String, String)>) -> ApiResult<Json<AuthChallengeResponse>> {
    let challenge = auth_controller(&state, id)?.begin(&method).await?;
    Ok(Json(AuthChallengeResponse { url: challenge.url, instructions: challenge.instructions }))
}

async fn complete_auth(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CompleteAuthRequest>,
) -> ApiResult<StatusCode> {
    auth_controller(&state, id)?.complete(&body.response).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn logout(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    auth_controller(&state, id)?.logout().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- threads ----

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenThreadRequest {
    pub instance_id: InstanceId,
    #[serde(default)]
    pub policy: RuntimePolicy,
    pub model: Option<ModelSelection>,
    pub resume: Option<NativeId>,
    /// Tool servers to add for this thread, on top of the built-in ones.
    #[serde(default)]
    pub mcp_servers: Vec<harness_contracts::McpServer>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadOpened {
    pub thread_id: ThreadId,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStarted {
    pub turn_id: harness_contracts::TurnId,
}

async fn open_thread(State(state): State<AppState>, Json(body): Json<OpenThreadRequest>) -> ApiResult<Json<ThreadOpened>> {
    let mut policy = body.policy;
    if policy.cwd.is_none() {
        policy.cwd = std::env::current_dir().ok().map(|p| p.display().to_string());
    }
    let thread_id = state
        .sessions
        .open(OpenThread {
            instance_id: body.instance_id,
            policy,
            model: body.model,
            resume: body.resume,
            extra_mcp_servers: body.mcp_servers,
        })
        .await?;
    Ok(Json(ThreadOpened { thread_id }))
}

async fn close_thread(State(state): State<AppState>, Path(thread): Path<String>) -> ApiResult<StatusCode> {
    state.sessions.close(&ThreadId::new(thread)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_turn(
    State(state): State<AppState>,
    Path(thread): Path<String>,
    Json(input): Json<TurnInput>,
) -> ApiResult<Json<TurnStarted>> {
    let turn_id = state.sessions.get(&ThreadId::new(thread))?.start_turn(input).await?;
    Ok(Json(TurnStarted { turn_id }))
}

async fn steer(State(state): State<AppState>, Path(thread): Path<String>, Json(input): Json<SteerInput>) -> ApiResult<StatusCode> {
    state.sessions.get(&ThreadId::new(thread))?.steer(input).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn interrupt(State(state): State<AppState>, Path(thread): Path<String>) -> ApiResult<StatusCode> {
    state.sessions.get(&ThreadId::new(thread))?.interrupt().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Answering an agent's permission request is the user's call.
async fn respond_request(
    _: crate::cockpit::CockpitUser,
    State(state): State<AppState>,
    Path((thread, request)): Path<(String, String)>,
    Json(response): Json<RequestResponse>,
) -> ApiResult<StatusCode> {
    state.sessions.get(&ThreadId::new(thread))?.respond(&RequestId::new(request), response).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- event stream ----

async fn ws_events_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| stream_events(socket, state.hub))
}

/// Replays current state, then streams `ClientEvent`s as JSON text frames.
async fn stream_events(socket: WebSocket, hub: Arc<EventHub>) {
    let (mut sink, mut incoming) = socket.split();
    let (replay, mut live) = hub.subscribe();
    for event in replay {
        let Ok(text) = serde_json::to_string(&event) else { continue };
        if sink.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }
    loop {
        tokio::select! {
            event = live.recv() => match event {
                Ok(event) => {
                    let Ok(text) = serde_json::to_string(&event) else { continue };
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
            msg = incoming.next() => if matches!(msg, None | Some(Err(_)) | Some(Ok(Message::Close(_)))) {
                return;
            },
        }
    }
}

// ---- app-level: MCP, files, feedback ----

async fn mcp_sse_proxy(
    State(state): State<AppState>,
) -> axum::response::sse::Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    crate::mcp::mcp_sse_handler(axum::extract::State(state.mcp_state)).await
}

async fn mcp_messages_proxy(
    State(state): State<AppState>,
    query: axum::extract::Query<crate::mcp::McpMessagesQuery>,
    json: axum::Json<crate::mcp::JsonRpcRequest>,
) -> impl axum::response::IntoResponse {
    crate::mcp::mcp_messages_handler(axum::extract::State(state.mcp_state), query, json).await
}

async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// The user's UI config file: `$NAH_UI_CONFIG`, else `~/.config/nah/ui.toml`.
fn ui_config_path() -> Option<std::path::PathBuf> {
    match std::env::var_os("NAH_UI_CONFIG") {
        Some(path) => Some(path.into()),
        None => std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".config/nah/ui.toml")),
    }
}

/// Serves the user's UI config text (merged over the UI's defaults client-side);
/// `text` is null when there is none.
async fn ui_config_handler() -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let Some(path) = ui_config_path() else {
        return Ok(Json(serde_json::json!({ "text": null })));
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => Ok(Json(serde_json::json!({ "text": text }))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Json(serde_json::json!({ "text": null }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": format!("{}: {e}", path.display()) })))),
    }
}



async fn fs_read_handler(
    State(state): State<AppState>,
    Query(query): Query<FsReadQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Only files inside the repository NAH works on, however they're named.
    let root = state.arch.root().canonicalize().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("repository root: {e}")))?;
    let clean_path = query.path.trim().trim_start_matches("file://");
    let target = std::path::Path::new(clean_path);
    let resolved = if target.is_absolute() { target.to_path_buf() } else { root.join(target) };
    let canonical = match resolved.canonicalize() {
        Ok(c) => c,
        Err(e) => return Err((StatusCode::NOT_FOUND, format!("Path not found: {e}"))),
    };
    if !canonical.starts_with(&root) {
        return Err((StatusCode::FORBIDDEN, "Only files inside the repository can be read".to_string()));
    }
    if !canonical.is_file() {
        return Err((StatusCode::BAD_REQUEST, "Target is not a regular file".to_string()));
    }
    let content = tokio::fs::read_to_string(&canonical).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to read file: {e}")))?;

    let lines = content.lines().count();
    Ok(Json(FsReadResponse {
        path: canonical.to_string_lossy().to_string(),
        content,
        lines,
    }))
}

async fn dev_feedback_handler(
    Json(mut item): Json<DevFeedbackItem>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if item.id.is_empty() {
        item.id = format!("fb_{}", uuid::Uuid::new_v4().simple());
    }
    if item.timestamp.is_empty() {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        item.timestamp = format!("{secs}");
    }

    let feedback_file = std::path::Path::new("dev_feedback.json");
    let mut list: Vec<DevFeedbackItem> = if feedback_file.exists() {
        let content = tokio::fs::read_to_string(feedback_file).await.unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    };

    list.push(item.clone());

    let formatted = serde_json::to_string_pretty(&list).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    tokio::fs::write(feedback_file, formatted).await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    Ok(Json(serde_json::json!({
        "status": "saved",
        "id": item.id,
        "total_feedback_count": list.len()
    })))
}

async fn get_dev_feedback_handler() -> Result<impl IntoResponse, (StatusCode, String)> {
    let feedback_file = std::path::Path::new("dev_feedback.json");
    let list: Vec<DevFeedbackItem> = if feedback_file.exists() {
        let content = tokio::fs::read_to_string(feedback_file).await.unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(Json(list))
}

async fn update_dev_feedback_handler(
    Json(item): Json<DevFeedbackItem>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let feedback_file = std::path::Path::new("dev_feedback.json");
    let mut list: Vec<DevFeedbackItem> = if feedback_file.exists() {
        let content = tokio::fs::read_to_string(feedback_file).await.unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut found = false;
    for existing in list.iter_mut() {
        if existing.id == item.id {
            *existing = item.clone();
            found = true;
            break;
        }
    }

    if !found {
        list.push(item.clone());
    }

    let formatted = serde_json::to_string_pretty(&list).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    tokio::fs::write(feedback_file, formatted).await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    Ok(Json(item))
}

