//! The harnesses: their instances (refresh, saved sessions, sign-in) and
//! threads (steering, stopping, answering requests). Routes address
//! instances and threads by id and never know which harness is behind them.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use harness_contracts::{InstanceId, NativeId, RequestId, RequestResponse, SteerInput, ThreadId};
use harness_core::HarnessError;
use serde::Deserialize;

use crate::server::ApiResult;
use crate::modules::HarnessState;
use crate::cockpit::{Needs, can};

pub(crate) fn routes() -> Router<HarnessState> {
    Router::new()
        .route("/api/instances", get(list_instances))
        .route("/api/instances/{id}/refresh", post(refresh_instance))
        .route("/api/instances/{id}/sessions", get(list_saved_sessions))
        .route("/api/instances/{id}/sessions/{native_id}", delete(delete_saved_session))
        .route("/api/instances/{id}/auth/{method}", post(begin_auth))
        .route("/api/instances/{id}/auth/complete", post(complete_auth))
        .route("/api/instances/{id}/auth/cancel", post(cancel_auth))
        .route("/api/instances/{id}/auth/logout", post(logout))
        .route("/api/threads/{thread}/steer", post(steer))
        .route("/api/threads/{thread}/interrupt", post(interrupt))
        .route("/api/threads/{thread}/subagents/{subagent}/cancel", post(cancel_subagent))
        .route("/api/threads/{thread}/requests/{request}", post(respond_request))
}

// ---- instances ----

async fn list_instances(_: Needs<can::ViewRead>, State(state): State<HarnessState>) -> Json<Vec<harness_contracts::ProviderSnapshot>> {
    Json(state.registry.snapshots())
}

/// Re-probes an instance, first looking its harness's launch up again: a
/// pack installed or fixed since makes the instance anew with it.
async fn refresh_instance(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    let id = InstanceId::new(id);
    let root = state.root.clone();
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
    _: Needs<can::ViewRead>,
    State(state): State<HarnessState>,
    Path(id): Path<String>,
    Query(query): Query<SavedSessionsQuery>,
) -> ApiResult<Json<harness_contracts::SavedSessionPage>> {
    let catalog = state.registry.get(&InstanceId::new(id))?.sessions().ok_or(HarnessError::Unsupported { operation: "saved sessions" })?;
    Ok(Json(catalog.list(query.cwd.as_deref().unwrap_or("."), query.cursor.as_deref()).await?))
}

async fn delete_saved_session(
    _: Needs<can::SessionsControl>,
    State(state): State<HarnessState>,
    Path((id, native_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let catalog = state.registry.get(&InstanceId::new(id))?.sessions().ok_or(HarnessError::Unsupported { operation: "saved sessions" })?;
    catalog.delete(&NativeId::new(native_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct CompleteAuthRequest {
    /// The sign-in it's for.
    pub id: u64,
    pub response: String,
}

/// Which sign-in to stop.
#[derive(Deserialize)]
struct CancelAuthRequest {
    pub id: u64,
}

fn auth_controller(state: &HarnessState, id: String) -> ApiResult<Arc<dyn harness_core::AuthController>> {
    Ok(state.registry.get(&InstanceId::new(id))?.auth().ok_or(HarnessError::Unsupported { operation: "login" })?)
}

/// What's to be done to sign in (also in the instance's snapshot), or
/// nothing: it's signed in.
async fn begin_auth(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path((id, method)): Path<(String, String)>) -> ApiResult<Json<Option<harness_contracts::SignInWaiting>>> {
    Ok(Json(auth_controller(&state, id)?.begin(&method).await?))
}

async fn complete_auth(
    _: Needs<can::SessionsControl>,
    State(state): State<HarnessState>,
    Path(id): Path<String>,
    Json(body): Json<CompleteAuthRequest>,
) -> ApiResult<StatusCode> {
    auth_controller(&state, id)?.complete(body.id, &body.response).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn cancel_auth(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path(id): Path<String>, Json(body): Json<CancelAuthRequest>) -> ApiResult<StatusCode> {
    auth_controller(&state, id)?.cancel(body.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn logout(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    auth_controller(&state, id)?.logout().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- threads (sessions open, close and take prompts by slot: see `slots`) ----

async fn steer(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path(thread): Path<String>, Json(input): Json<SteerInput>) -> ApiResult<StatusCode> {
    state.sessions.get(&ThreadId::new(thread))?.steer(input).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Stops one sub-agent's work, the turn going on (when its harness lets it).
async fn cancel_subagent(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path((thread, subagent)): Path<(String, String)>) -> ApiResult<StatusCode> {
    state.sessions.get(&ThreadId::new(thread))?.cancel_subagent(&harness_contracts::SubagentId::new(subagent)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn interrupt(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path(thread): Path<String>) -> ApiResult<StatusCode> {
    state.sessions.get(&ThreadId::new(thread))?.interrupt().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Answering an agent's permission request is the user's call.
async fn respond_request(
    _: Needs<can::RequestsAnswer>,
    State(state): State<HarnessState>,
    Path((thread, request)): Path<(String, String)>,
    Json(response): Json<RequestResponse>,
) -> ApiResult<StatusCode> {
    state.sessions.get(&ThreadId::new(thread))?.respond(&RequestId::new(request), response).await?;
    Ok(StatusCode::NO_CONTENT)
}
