//! Notifications, for the cockpit: what's open (or the latest), a slot's
//! updates seen, one put away. Answering a request stays the request's own
//! route; its notification ends when the request does.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use harness_contracts::{Notification, NotificationId, SlotId};
use serde::{Deserialize, Serialize};

use crate::cockpit::{Needs, can};
use crate::server::ApiResult;
use crate::modules::NotifyState;

pub(crate) fn routes() -> Router<NotifyState> {
    Router::new()
        .route("/api/notifications", get(list))
        .route("/api/notifications/seen", post(seen))
        .route("/api/notifications/{id}/dismiss", post(dismiss))
}

/// How many `recent` lists.
const RECENT: usize = 200;

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Which {
    #[default]
    Open,
    Recent,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    #[serde(default)]
    state: Which,
}

/// The open ones in the order they're gone through, or the latest in any
/// state, newest first.
async fn list(_: Needs<can::SessionsControl>, State(state): State<NotifyState>, Query(query): Query<ListQuery>) -> ApiResult<Json<Vec<Notification>>> {
    Ok(Json(match query.state {
        Which::Open => state.notify.open(),
        Which::Recent => blocking(move || state.notify.recent(RECENT)).await?,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Seen {
    slot: SlotId,
}

#[derive(Serialize)]
struct Dismissed {
    dismissed: usize,
}

/// The user saw the slot's chat: its updates are put away.
async fn seen(_: Needs<can::SessionsControl>, State(state): State<NotifyState>, Json(body): Json<Seen>) -> ApiResult<Json<Dismissed>> {
    let dismissed = blocking(move || state.notify.seen(&body.slot)).await?;
    Ok(Json(Dismissed { dismissed }))
}

async fn dismiss(_: Needs<can::SessionsControl>, State(state): State<NotifyState>, Path(id): Path<String>) -> ApiResult<Json<Notification>> {
    Ok(Json(blocking(move || state.notify.dismiss(&NotificationId::new(id))).await?))
}

/// The state database is written off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, nah_notify::NotifyError> + Send + 'static) -> ApiResult<T> {
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => Ok(result?),
        Err(e) => Err(harness_core::HarnessError::Store { reason: e.to_string() }.into()),
    }
}
