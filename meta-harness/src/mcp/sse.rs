use std::collections::HashMap;
use std::sync::Arc;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use futures::Stream;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::mcp::protocol::JsonRpcRequest;
use crate::mcp::service::McpService;

pub type SseSender = tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>;

#[derive(Clone, Default)]
pub struct SseSessionManager {
    sessions: Arc<Mutex<HashMap<String, SseSender>>>,
}

impl SseSessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, session_id: String, sender: SseSender) {
        self.sessions.lock().await.insert(session_id, sender);
    }

    pub async fn unregister(&self, session_id: &str) {
        self.sessions.lock().await.remove(session_id);
    }

    pub async fn send_to(&self, session_id: &str, event: Event) -> bool {
        let guard = self.sessions.lock().await;
        if let Some(tx) = guard.get(session_id) {
            tx.send(Ok(event)).await.is_ok()
        } else {
            false
        }
    }
}

#[derive(Clone)]
pub struct McpServerState {
    pub service: Arc<McpService>,
    pub sessions: Arc<SseSessionManager>,
}

impl McpServerState {
    pub fn new(service: Arc<McpService>) -> Self {
        Self {
            service,
            sessions: Arc::new(SseSessionManager::new()),
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct McpMessagesQuery {
    pub session_id: Option<String>,
}

pub async fn mcp_sse_handler(
    State(mcp_state): State<McpServerState>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let session_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::mpsc::channel(64);

    let endpoint_url = format!("/api/mcp/messages?session_id={}", session_id);
    let init_event = Event::default().event("endpoint").data(endpoint_url);
    let _ = tx.send(Ok(init_event)).await;

    mcp_state.sessions.register(session_id.clone(), tx).await;

    let stream = ReceiverStream::new(rx);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub async fn mcp_messages_handler(
    State(mcp_state): State<McpServerState>,
    Query(query): Query<McpMessagesQuery>,
    Json(rpc_req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let has_id = rpc_req.id.is_some();
    let response = mcp_state.service.handle_request(rpc_req).await;

    if let Some(ref sid) = query.session_id
        && let Ok(json_str) = serde_json::to_string(&response)
    {
        let event = Event::default().event("message").data(json_str);
        let delivered = mcp_state.sessions.send_to(sid, event).await;
        if !delivered {
            tracing::warn!("Failed to deliver MCP message to session: {}", sid);
        }
    }

    if has_id {
        (StatusCode::OK, Json(response)).into_response()
    } else {
        StatusCode::NO_CONTENT.into_response()
    }
}
