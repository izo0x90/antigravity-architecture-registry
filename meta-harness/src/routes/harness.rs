//! The harnesses: their instances (refresh, saved sessions, sign-in) and
//! threads (open, turns, steering, answering requests). Routes address
//! instances and threads by id and never know which harness is behind them.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use harness_contracts::{InstanceId, ModelSelection, NativeId, RequestId, RequestResponse, RuntimePolicy, SteerInput, ThreadId, TurnInput};
use harness_core::{HarnessError, OpenThread};
use serde::{Deserialize, Serialize};

use crate::server::{ApiResult, AppState};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
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
        .route("/api/threads/{thread}/inject", post(inject))
        .route("/api/threads/{thread}/steer", post(steer))
        .route("/api/threads/{thread}/interrupt", post(interrupt))
        .route("/api/threads/{thread}/requests/{request}", post(respond_request))
}

// ---- instances ----

async fn list_instances(State(state): State<AppState>) -> Json<Vec<harness_contracts::ProviderSnapshot>> {
    Json(state.registry.snapshots())
}

/// Re-probes an instance, first looking its harness's launch up again: a
/// pack installed or fixed since makes the instance anew with it.
async fn refresh_instance(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    let id = InstanceId::new(id);
    let root = state.arch.root().to_path_buf();
    let launches = tokio::task::spawn_blocking(move || crate::packs::catalog(&root).map(|c| crate::packs::harness_launches(&c))).await;
    if let Ok(Ok(launches)) = launches
        && let Some(config) = state.registry.default_configs(&launches).into_iter().find(|c| c.id == id)
        && state.registry.reconfigure(config).await?
    {
        return Ok(StatusCode::NO_CONTENT);
    }
    state.registry.get(&id)?.refresh().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSessionsQuery {
    /// The folder (relative: in the project; none: the project).
    #[serde(default)]
    pub cwd: Option<String>,
    pub cursor: Option<String>,
}

async fn list_saved_sessions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<SavedSessionsQuery>,
) -> ApiResult<Json<harness_contracts::SavedSessionPage>> {
    let catalog = state.registry.get(&InstanceId::new(id))?.sessions().ok_or(HarnessError::Unsupported { operation: "saved sessions" })?;
    Ok(Json(catalog.list(query.cwd.as_deref().unwrap_or("."), query.cursor.as_deref()).await?))
}

async fn delete_saved_session(
    State(state): State<AppState>,
    Path((id, native_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let catalog = state.registry.get(&InstanceId::new(id))?.sessions().ok_or(HarnessError::Unsupported { operation: "saved sessions" })?;
    catalog.delete(&NativeId::new(native_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct CompleteAuthRequest {
    pub response: String,
}

fn auth_controller(state: &AppState, id: String) -> ApiResult<Arc<dyn harness_core::AuthController>> {
    Ok(state.registry.get(&InstanceId::new(id))?.auth().ok_or(HarnessError::Unsupported { operation: "login" })?)
}

async fn begin_auth(State(state): State<AppState>, Path((id, method)): Path<(String, String)>) -> ApiResult<Json<harness_core::AuthChallenge>> {
    Ok(Json(auth_controller(&state, id)?.begin(&method).await?))
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
    // No folder asked for: the harness works in the project. It gets the
    // project's tool servers, then any added for this thread.
    let mut extra = state.tool_servers.project().map_err(|reason| HarnessError::InvalidConfig { reason })?;
    extra.retain(|p| !body.mcp_servers.iter().any(|b| b.name == p.name));
    extra.extend(body.mcp_servers);
    let thread_id = state
        .sessions
        .open(OpenThread {
            instance_id: body.instance_id,
            policy: body.policy,
            model: body.model,
            resume: body.resume,
            extra_mcp_servers: extra,
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

/// Adds a message to the running turn (else it starts one).
async fn inject(
    State(state): State<AppState>,
    Path(thread): Path<String>,
    Json(input): Json<TurnInput>,
) -> ApiResult<Json<TurnStarted>> {
    let turn_id = state.sessions.get(&ThreadId::new(thread))?.inject(input).await?;
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
