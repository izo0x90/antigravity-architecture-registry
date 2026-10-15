//! HTTP/WebSocket API. Harness routes address instances and threads by id and never
//! know which harness is behind them; everything else (files, feedback, the
//! architecture graph and its MCP endpoint) is app-level.

use std::sync::Arc;

use axum::{
    Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use harness_core::{EventHub, HarnessError, InstanceRegistry, SessionManager};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<InstanceRegistry>,
    pub sessions: Arc<SessionManager>,
    /// The slots: the lead, sub-slots, the queue's workers.
    pub slots: Arc<harness_core::SlotManager>,
    pub hub: Arc<EventHub>,
    pub mcp_state: crate::mcp::McpServerState,
    pub arch: Arc<arch_service::ArchitectureService>,
    pub cockpit: crate::cockpit::CockpitAuth,
    pub queue: Arc<crate::queue::QueueRunner>,
    /// The project's state database: kept across restarts.
    pub state: Arc<nah_state::StateDb>,
    /// The tool servers every session gets (ours and the project's).
    pub tool_servers: Arc<crate::tool_servers::ToolServerState>,
    /// The cockpit, for agents driving it.
    pub remote: Arc<crate::remote::Remote>,
    /// What each caller may do.
    pub access: Arc<nah_access::Access>,
    /// One-time tickets for the sockets.
    pub tickets: Arc<crate::cockpit::Tickets>,
    /// What needs the user, and what happened.
    pub notify: Arc<nah_notify::NotifyHub>,
}

impl axum::extract::FromRef<AppState> for Arc<crate::cockpit::Tickets> {
    fn from_ref(state: &AppState) -> Self {
        state.tickets.clone()
    }
}

impl axum::extract::FromRef<AppState> for Arc<nah_access::Access> {
    fn from_ref(state: &AppState) -> Self {
        state.access.clone()
    }
}

impl axum::extract::FromRef<AppState> for Arc<crate::remote::Remote> {
    fn from_ref(state: &AppState) -> Self {
        state.remote.clone()
    }
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
            HarnessError::Store { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            HarnessError::Conflict { .. } => StatusCode::CONFLICT,
        };
        (status, Json(serde_json::json!({ "error": self.0.to_string() }))).into_response()
    }
}

impl From<nah_notify::NotifyError> for ApiError {
    fn from(err: nah_notify::NotifyError) -> Self {
        use nah_notify::NotifyError as N;
        Self(match err {
            N::NotFound(id) => HarnessError::NotFound { what: format!("notification {id}") },
            N::Transition(e) => HarnessError::Conflict { reason: e.to_string() },
            N::Store(reason) => HarnessError::Store { reason },
        })
    }
}

pub(crate) type ApiResult<T> = Result<T, ApiError>;

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

/// Where the cockpit's page and its build are served from (`STATIC_DIR`).
pub fn static_dir() -> std::path::PathBuf {
    std::env::var_os("STATIC_DIR").map_or_else(|| "static".into(), std::path::PathBuf::from)
}

/// The cockpit's build, as `make ui` leaves it in the static folder.
pub const COCKPIT_BUILD: &str = "wasm/cockpit_ui_bg.wasm";
pub fn create_router(state: AppState) -> Router {
    let static_dir = static_dir();
    let mut router = Router::new()
        .merge(crate::routes::cockpit::routes())
        .merge(crate::routes::harness::routes())
        .merge(crate::routes::slots::routes())
        .merge(crate::routes::notifications::routes())
        .merge(crate::routes::files::routes())
        .merge(crate::routes::feedback::routes())
        .merge(crate::routes::tool_servers::routes())
        .merge(crate::routes::mcp::routes())
        .merge(crate::routes::events::routes())
        .merge(crate::arch::routes())
        .merge(crate::graph::routes())
        .merge(crate::queue::routes());

    if static_dir.exists() {
        router = router.fallback_service(ServeDir::new(static_dir));
    }

    // Same-origin only: no CORS layer, so other sites can't call the API.
    router
        .layer(axum::middleware::map_response(json_errors))
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
        .with_state(state)
}

/// A request's span: its method and path, never its query (sessions' keys
/// and sockets' tickets travel there).
pub fn request_span<B>(request: &axum::http::Request<B>) -> tracing::Span {
    tracing::info_span!("request", method = %request.method(), path = %request.uri().path())
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

/// Runs `f` on the state database off the async threads.
pub(crate) async fn with_state<T: Send + 'static>(
    db: &Arc<nah_state::StateDb>,
    f: impl FnOnce(&nah_state::StateDb) -> Result<T, nah_state::StateError> + Send + 'static,
) -> Result<T, StateFailed> {
    let db = db.clone();
    tokio::task::spawn_blocking(move || f(&db)).await.map_err(|e| StateFailed(nah_state::StateError::Failed(e.to_string())))?.map_err(StateFailed)
}

/// A state database error as an HTTP response: `{ "error": "..." }`.
pub struct StateFailed(pub nah_state::StateError);

impl IntoResponse for StateFailed {
    fn into_response(self) -> Response {
        let status = match self.0 {
            nah_state::StateError::Invalid(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(serde_json::json!({ "error": self.0.to_string() }))).into_response()
    }
}
