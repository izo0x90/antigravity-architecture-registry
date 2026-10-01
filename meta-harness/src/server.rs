use std::sync::Arc;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::adapter::AntigravityAdapter;
use crate::driver::AntigravityDriver;
use crate::protocol::*;

#[derive(Clone)]
pub struct AppState {
    pub adapter: Arc<AntigravityAdapter>,
    pub driver: Arc<tokio::sync::Mutex<AntigravityDriver>>,
    pub mcp_state: crate::mcp::McpServerState,
}

impl AppState {
    pub fn new(
        adapter: Arc<AntigravityAdapter>,
        driver: Arc<tokio::sync::Mutex<AntigravityDriver>>,
    ) -> Self {
        let repo = Arc::new(crate::registry::FsJsonRepository::from_architecture(
            harness_protocol::arch::SystemArchitecture::default(),
            "test_architecture.json",
        ));
        let registry = Arc::new(crate::registry::RegistryService::new(repo));
        let mcp_service = Arc::new(crate::mcp::McpService::new(
            registry,
            Some(adapter.event_sender().clone()),
        ));
        let mcp_state = crate::mcp::McpServerState::new(mcp_service);
        Self {
            adapter,
            driver,
            mcp_state,
        }
    }

    pub fn with_mcp_state(
        adapter: Arc<AntigravityAdapter>,
        driver: Arc<tokio::sync::Mutex<AntigravityDriver>>,
        mcp_state: crate::mcp::McpServerState,
    ) -> Self {
        Self {
            adapter,
            driver,
            mcp_state,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendTurnRequest {
    pub prompt: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SteerRequest {
    pub runtime_mode: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub decision: String,
}

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

pub fn create_router(state: AppState) -> Router {
    let static_dir = std::env::var("STATIC_DIR").unwrap_or_else(|_| "static".to_string());
    let mut router = Router::new()
        .route("/health", get(health_check))
        .route("/api/driver/snapshot", get(get_driver_snapshot))
        .route("/api/driver/refresh_models", post(refresh_models))
        .route("/api/sessions/start", post(start_session))
        .route("/api/sessions/{thread_id}/turn", post(send_turn))
        .route("/api/sessions/{thread_id}/steer", post(steer_session))
        .route("/api/sessions/{thread_id}/cancel", post(cancel_session))
        .route("/api/sessions/{thread_id}/stop", post(stop_session))
        .route("/api/sessions/{thread_id}/approvals/{request_id}", post(resolve_approval))
        .route("/api/sessions/{thread_id}/requests/{request_id}/respond", post(resolve_approval))
        .route("/api/fs/read", get(fs_read_handler))
        .route("/api/dev/feedback", get(get_dev_feedback_handler).post(dev_feedback_handler).put(update_dev_feedback_handler))
        .route("/api/mcp/sse", get(mcp_sse_proxy).post(mcp_messages_proxy))
        .route("/api/mcp/messages", post(mcp_messages_proxy))
        .route("/ws/events", get(ws_events_handler));

    if std::path::Path::new(&static_dir).exists() {
        router = router.fallback_service(ServeDir::new(static_dir));
    }

    router
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

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

async fn get_driver_snapshot(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let driver = state.driver.lock().await;
    let mut snap = driver.get_snapshot();
    if snap.models.is_empty() {
        snap.models = crate::driver::official_antigravity_models();
    }
    if snap.slash_commands.is_empty() {
        snap.slash_commands = crate::driver::official_antigravity_commands();
    }
    Ok(Json(snap))
}

async fn refresh_models(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mut driver = state.driver.lock().await;
    driver.refresh_models().map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    let mut snap = driver.get_snapshot();
    if snap.models.is_empty() {
        snap.models = crate::driver::official_antigravity_models();
    }
    if snap.slash_commands.is_empty() {
        snap.slash_commands = crate::driver::official_antigravity_commands();
    }
    Ok(Json(snap))
}

async fn start_session(
    State(state): State<AppState>,
    Json(input): Json<StartSessionInput>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let session = state.adapter.start_session(input).await.map_err(|e| {
        tracing::error!("start_session failed: {}", e);
        (StatusCode::BAD_REQUEST, e.to_string())
    })?;
    Ok(Json(session))
}

async fn send_turn(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Json(req): Json<SendTurnRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let result = state
        .adapter
        .send_turn(SendTurnInput {
            thread_id: thread_id.clone(),
            input: req.prompt,
            model: None,
        })
        .await
        .map_err(|e| {
            tracing::error!("send_turn failed for thread {}: {}", thread_id, e);
            let code = match &e {
                crate::error::HarnessError::Validation(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (code, e.to_string())
        })?;
    Ok(Json(result))
}

async fn steer_session(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Json(req): Json<SteerRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let result = state
        .adapter
        .steer_session(&thread_id, req.model, req.runtime_mode)
        .await
        .map_err(|e| {
            tracing::error!("steer_session failed for thread {}: {}", thread_id, e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;
    Ok(Json(result))
}

async fn cancel_session(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    state
        .adapter
        .interrupt_turn(&thread_id)
        .await
        .map_err(|e| {
            tracing::error!("cancel_session failed for thread {}: {}", thread_id, e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;
    Ok(Json(serde_json::json!({ "status": "cancelled" })))
}

async fn stop_session(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    state.adapter.stop_session(&thread_id).await.map_err(|e| {
        tracing::error!("stop_session failed for thread {}: {}", thread_id, e);
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    Ok(Json(serde_json::json!({ "status": "stopped" })))
}

async fn resolve_approval(
    State(state): State<AppState>,
    Path((thread_id, request_id)): Path<(String, String)>,
    Json(req): Json<ApprovalRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    state
        .adapter
        .respond_to_request(&thread_id, &request_id, &req.decision)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "resolved" })))
}

async fn ws_events_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_events(socket, state.adapter))
}

async fn handle_ws_events(socket: WebSocket, adapter: Arc<AntigravityAdapter>) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut rx = adapter.subscribe();

    // Spawn task to forward broadcast runtime events to websocket client
    let mut send_task = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let Ok(json_str) = serde_json::to_string(&event) else {
                continue;
            };
            if ws_sender.send(Message::Text(json_str.into())).await.is_err() {
                break;
            }
        }
    });

    // Handle incoming client messages (e.g. ping/pong, heartbeats)
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                Message::Close(_) => break,
                Message::Ping(_) => (),
                _ => (),
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    }
}

async fn fs_read_handler(
    Query(query): Query<FsReadQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let clean_path = query.path.trim().trim_start_matches("file://");
    let target = std::path::Path::new(clean_path);
    let resolved = if target.is_absolute() {
        target.to_path_buf()
    } else {
        let cur = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let direct = cur.join(target);
        if direct.exists() {
            direct
        } else if let Some(parent) = cur.parent() {
            let parent_target = parent.join(target);
            if parent_target.exists() {
                parent_target
            } else {
                direct
            }
        } else {
            direct
        }
    };

    let canonical = match resolved.canonicalize() {
        Ok(c) => c,
        Err(e) => return Err((StatusCode::NOT_FOUND, format!("Path not found: {e}"))),
    };

    if !canonical.is_file() {
        return Err((StatusCode::BAD_REQUEST, "Target is not a regular file".to_string()));
    }

    let content = tokio::fs::read_to_string(&canonical).await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to read file: {e}"))
    })?;

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

