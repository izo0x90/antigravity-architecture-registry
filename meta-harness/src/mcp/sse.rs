use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::Stream;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::mcp::protocol::JsonRpcRequest;
use crate::mcp::service::{McpCaller, McpService};

pub type SseSender = tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>;

#[derive(Clone, Default)]
pub struct SseSessionManager {
    /// Each open stream: where its answers go, and who it is.
    sessions: Arc<Mutex<HashMap<String, (SseSender, McpCaller)>>>,
}

impl SseSessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, session_id: String, sender: SseSender, caller: McpCaller) {
        self.sessions.lock().await.insert(session_id, (sender, caller));
    }

    /// Who an open stream belongs to.
    pub async fn caller(&self, session_id: &str) -> Option<McpCaller> {
        self.sessions.lock().await.get(session_id).map(|(_, caller)| caller.clone())
    }

    /// Ends every open event stream (the server is stopping: an agent's
    /// open stream mustn't keep it running).
    pub async fn close_all(&self) {
        self.sessions.lock().await.clear();
    }

    pub async fn send_to(&self, session_id: &str, event: Event) -> bool {
        let guard = self.sessions.lock().await;
        if let Some((tx, _)) = guard.get(session_id) { tx.send(Ok(event)).await.is_ok() } else { false }
    }
}

/// Who holds an MCP key (a slot's session), if anyone.
pub type Callers = Arc<dyn Fn(&str) -> Option<McpCaller> + Send + Sync>;

#[derive(Clone)]
pub struct McpServerState {
    pub service: Arc<McpService>,
    pub sessions: Arc<SseSessionManager>,
    pub callers: Callers,
}

impl McpServerState {
    pub fn new(service: Arc<McpService>, callers: Callers) -> Self {
        Self { service, sessions: Arc::new(SseSessionManager::new()), callers }
    }

    /// The caller behind a key; no key, or one no session holds, is external.
    fn resolve(&self, key: Option<&str>) -> McpCaller {
        key.and_then(|k| (self.callers)(k)).unwrap_or_else(McpCaller::external)
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct McpMessagesQuery {
    pub session_id: Option<String>,
    /// The session's key, for agents that post without a stream.
    pub key: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct McpStreamQuery {
    /// The session's key: which slot is calling.
    pub key: Option<String>,
}

pub async fn mcp_sse_handler(
    State(mcp_state): State<McpServerState>,
    Query(query): Query<McpStreamQuery>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let caller = mcp_state.resolve(query.key.as_deref());
    let session_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::mpsc::channel(64);

    let endpoint_url = format!("/api/mcp/messages?session_id={}", session_id);
    let init_event = Event::default().event("endpoint").data(endpoint_url);
    let _ = tx.send(Ok(init_event)).await;

    mcp_state.sessions.register(session_id.clone(), tx, caller).await;

    // The session is forgotten when its stream ends (the client went away).
    let open = OpenStream { sessions: mcp_state.sessions.clone(), id: session_id };
    let stream = ReceiverStream::new(rx).map(move |event| {
        let _ = &open;
        event
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// An open event stream; dropping it forgets its session.
struct OpenStream {
    sessions: Arc<SseSessionManager>,
    id: String,
}

impl Drop for OpenStream {
    fn drop(&mut self) {
        let sessions = self.sessions.sessions.clone();
        let id = std::mem::take(&mut self.id);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                sessions.lock().await.remove(&id);
            });
        }
    }
}

pub async fn mcp_messages_handler(
    State(mcp_state): State<McpServerState>,
    Query(query): Query<McpMessagesQuery>,
    Json(rpc_req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let caller = match &query.session_id {
        Some(id) => mcp_state.sessions.caller(id).await,
        None => None,
    };
    let caller = caller.unwrap_or_else(|| mcp_state.resolve(query.key.as_deref()));
    let Some(response) = mcp_state.service.handle_request(rpc_req, &caller).await else {
        // A notification: taken, never answered.
        return StatusCode::ACCEPTED.into_response();
    };
    if let Some(ref sid) = query.session_id
        && let Ok(json_str) = serde_json::to_string(&response)
    {
        let event = Event::default().event("message").data(json_str);
        let delivered = mcp_state.sessions.send_to(sid, event).await;
        if !delivered {
            tracing::warn!("Failed to deliver MCP message to session: {}", sid);
        }
    }
    (StatusCode::OK, Json(response)).into_response()
}
