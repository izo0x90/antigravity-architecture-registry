//! Discussions, for the cockpit: list them (or those on a thing), read
//! one, start one on a thing (asking the lead, another agent slot, or no
//! one but you), reply; settle, reopen, make one a change request, delete
//! one (the user's alone: `plans.approve`). Reading needs `view.read`,
//! starting and replying `discuss`. Agents answer through their tools.
//! What the user does is theirs: `by` is "user".

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use harness_contracts::{Discussion, DiscussionId, DiscussionKind, Subject};
use nah_discussions::{DiscussionError, Discussions, USER};
use serde::Deserialize;

use crate::cockpit::{Needs, can};
use crate::modules::DiscussionsState;

pub(crate) fn routes() -> Router<DiscussionsState> {
    Router::new()
        .route("/api/discussions", get(list).post(start))
        .route("/api/discussions/on", post(on))
        .route("/api/discussions/{id}", get(read).delete(delete))
        .route("/api/discussions/{id}/on", get(on_now))
        .route("/api/discussions/{id}/reply", post(reply))
        .route("/api/discussions/{id}/settle", post(settle))
        .route("/api/discussions/{id}/reopen", post(reopen))
        .route("/api/discussions/{id}/change", post(change))
}

/// A discussion's change refused, said as the rest of the API says it.
struct Refused(DiscussionError);

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        let status = match self.0 {
            DiscussionError::NotFound(_) => StatusCode::NOT_FOUND,
            DiscussionError::Forbidden(_) => StatusCode::FORBIDDEN,
            DiscussionError::Invalid(_) | DiscussionError::Subject(_) => StatusCode::BAD_REQUEST,
            DiscussionError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(serde_json::json!({ "error": self.0.to_string() }))).into_response()
    }
}

/// Discussions are kept in the state database and files, and owners say
/// where things are now: off the async threads.
async fn blocking<T: Send + 'static>(discussions: &Arc<Discussions>, f: impl FnOnce(&Discussions) -> Result<T, DiscussionError> + Send + 'static) -> Result<T, Refused> {
    let discussions = discussions.clone();
    tokio::task::spawn_blocking(move || f(&discussions)).await.map_err(|e| Refused(DiscussionError::Store(e.to_string())))?.map_err(Refused)
}

async fn list(_: Needs<can::ViewRead>, State(state): State<DiscussionsState>) -> Result<Json<Vec<Discussion>>, Refused> {
    blocking(&state.discussions, |d| Ok(d.list())).await.map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct On {
    subject: Subject,
}

/// The discussions on a thing (or on something it's in).
async fn on(_: Needs<can::ViewRead>, State(state): State<DiscussionsState>, Json(body): Json<On>) -> Result<Json<Vec<Discussion>>, Refused> {
    blocking(&state.discussions, move |d| Ok(d.on(&body.subject))).await.map(Json)
}

/// What the discussion is on says, as it is now (or as it was kept).
async fn on_now(_: Needs<can::ViewRead>, State(state): State<DiscussionsState>, Path(id): Path<String>) -> Result<Json<harness_contracts::OnNow>, Refused> {
    let things = state.things.clone();
    blocking(&state.discussions, move |d| {
        let d = d.get(&DiscussionId::new(id))?;
        let (held, text) = things.glimpse(&d.on.subject, d.kept.as_ref(), ON_NOW_MOST);
        Ok(harness_contracts::OnNow { what: d.on.what, held, lines: text.as_ref().map_or(0, |(_, n)| *n), text: text.map(|(t, _)| t) })
    })
    .await
    .map(Json)
}

/// How much of what a discussion's thing says the cockpit is given (about).
const ON_NOW_MOST: usize = 4000;

async fn read(_: Needs<can::ViewRead>, State(state): State<DiscussionsState>, Path(id): Path<String>) -> Result<Json<Discussion>, Refused> {
    blocking(&state.discussions, move |d| d.get(&DiscussionId::new(id))).await.map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Starting {
    subject: Subject,
    #[serde(default)]
    kind: DiscussionKind,
    text: String,
    /// The agent slot asked, by its id or name (none: the lead).
    #[serde(default)]
    asked: Option<String>,
}

async fn start(_: Needs<can::Discuss>, State(state): State<DiscussionsState>, Json(body): Json<Starting>) -> Result<Json<Discussion>, Refused> {
    let asked = crate::discussing::asked(&state.slots, body.asked.as_deref(), USER).map_err(|e| Refused(DiscussionError::Invalid(e)))?;
    blocking(&state.discussions, move |d| d.start(&body.subject, body.kind, &body.text, USER, Some(&asked))).await.map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Saying {
    #[serde(default)]
    text: String,
}

async fn reply(_: Needs<can::Discuss>, State(state): State<DiscussionsState>, Path(id): Path<String>, Json(body): Json<Saying>) -> Result<Json<Discussion>, Refused> {
    blocking(&state.discussions, move |d| d.reply(&DiscussionId::new(id), &body.text, USER)).await.map(Json)
}

async fn settle(_: Needs<can::PlansApprove>, State(state): State<DiscussionsState>, Path(id): Path<String>, Json(body): Json<Saying>) -> Result<Json<Discussion>, Refused> {
    blocking(&state.discussions, move |d| d.settle(&DiscussionId::new(id), &body.text, USER)).await.map(Json)
}

async fn reopen(_: Needs<can::PlansApprove>, State(state): State<DiscussionsState>, Path(id): Path<String>, Json(body): Json<Saying>) -> Result<Json<Discussion>, Refused> {
    blocking(&state.discussions, move |d| d.reopen(&DiscussionId::new(id), &body.text, USER)).await.map(Json)
}

/// Made a change request (one answered: open again, `text` saying why).
async fn change(_: Needs<can::PlansApprove>, State(state): State<DiscussionsState>, Path(id): Path<String>, Json(body): Json<Saying>) -> Result<Json<Discussion>, Refused> {
    blocking(&state.discussions, move |d| d.ask_for_change(&DiscussionId::new(id), &body.text, USER)).await.map(Json)
}

async fn delete(_: Needs<can::PlansApprove>, State(state): State<DiscussionsState>, Path(id): Path<String>) -> Result<StatusCode, Refused> {
    blocking(&state.discussions, move |d| d.delete(&DiscussionId::new(id), USER)).await.map(|()| StatusCode::NO_CONTENT)
}
