//! Tags, for the cockpit: list them, read one (what each subject says
//! now), tag a subject (a new tag when the name is new), take one off,
//! rename, recolour or note one, delete one; a tag by its id (or its
//! name). Reading needs `view.read`; changing them `tags.edit` (agents
//! change them through their tools, as their role allows). What the user
//! does is theirs: `by` is "user" (as the activity kept names them).

use std::sync::Arc;

/// Who the cockpit's changes are by: the user.
use harness_contracts::USER;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use harness_contracts::{Subject, Tag, TagRead};
use nah_tags::{Edit, TagError, Tags};
use serde::Deserialize;

use crate::cockpit::{Needs, can};
use crate::modules::TagsState;

pub(crate) fn routes() -> Router<TagsState> {
    Router::new()
        .route("/api/tags", get(list).post(tag))
        .route("/api/tags/{tag}", get(read).delete(delete))
        .route("/api/tags/{tag}/edit", post(edit))
        .route("/api/tags/{tag}/untag", post(untag))
}

/// A tag's change refused, said as the rest of the API says it.
struct Refused(TagError);

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        let status = match self.0 {
            TagError::NotFound(_) => StatusCode::NOT_FOUND,
            TagError::Taken(_) | TagError::NotOn(_) => StatusCode::CONFLICT,
            TagError::BadName(_) | TagError::BadColour | TagError::Subject(_) | TagError::TooMany => StatusCode::BAD_REQUEST,
            TagError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(serde_json::json!({ "error": self.0.to_string() }))).into_response()
    }
}

/// Tags are kept in the state database, and owners say what subjects say
/// (a terminal's lines): off the async threads.
async fn blocking<T: Send + 'static>(tags: &Arc<Tags>, f: impl FnOnce(&Tags) -> Result<T, TagError> + Send + 'static) -> Result<T, Refused> {
    let tags = tags.clone();
    tokio::task::spawn_blocking(move || f(&tags)).await.map_err(|e| Refused(TagError::Store(e.to_string())))?.map_err(Refused)
}

async fn list(_: Needs<can::ViewRead>, State(state): State<TagsState>) -> Json<Vec<Tag>> {
    Json(state.tags.list())
}

async fn read(_: Needs<can::ViewRead>, State(state): State<TagsState>, Path(tag): Path<String>) -> Result<Json<TagRead>, Refused> {
    blocking(&state.tags, move |t| t.read(&tag)).await.map(Json)
}


#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tagging {
    /// A tag's id, or a name (a new tag when there's none of that name).
    tag: String,
    subject: Subject,
}

async fn tag(_: Needs<can::TagsEdit>, State(state): State<TagsState>, Json(body): Json<Tagging>) -> Result<Json<Tag>, Refused> {
    blocking(&state.tags, move |t| t.tag(&body.tag, body.subject, USER)).await.map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Untagging {
    subject: Subject,
}

async fn untag(_: Needs<can::TagsEdit>, State(state): State<TagsState>, Path(tag): Path<String>, Json(body): Json<Untagging>) -> Result<Json<Tag>, Refused> {
    blocking(&state.tags, move |t| t.untag(&tag, &body.subject, USER)).await.map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Editing {
    name: Option<String>,
    colour: Option<u8>,
    note: Option<String>,
}

async fn edit(_: Needs<can::TagsEdit>, State(state): State<TagsState>, Path(tag): Path<String>, Json(body): Json<Editing>) -> Result<Json<Tag>, Refused> {
    blocking(&state.tags, move |t| t.edit(&tag, Edit { name: body.name, colour: body.colour, note: body.note }, USER)).await.map(Json)
}

async fn delete(_: Needs<can::TagsEdit>, State(state): State<TagsState>, Path(tag): Path<String>) -> Result<StatusCode, Refused> {
    blocking(&state.tags, move |t| t.delete(&tag, USER)).await.map(|()| StatusCode::NO_CONTENT)
}
