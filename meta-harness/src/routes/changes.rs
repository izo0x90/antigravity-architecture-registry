//! The project's changes (git): what a comparison changes, one file's
//! changes, which components they touch, the timeline. Reading only; the
//! comparison is the caller's (`from`, `to`: HEAD, index, files,
//! rev:<name>, snapshot:<id>; the uncommitted changes when left out).

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use harness_contracts::changes::{ChangeList, Comparison, FileChanges, Group, Point};
use nah_changes::{Changes, ChangesError};
use serde::Deserialize;

use crate::cockpit::{Needs, can};
use crate::modules::ProjectState;

pub(crate) fn routes() -> Router<ProjectState> {
    Router::new().route("/api/changes", get(list)).route("/api/changes/file", get(file))
}

/// A changes error as an HTTP response: `{ "error": "..." }`.
pub(crate) struct ChangesFailed(ChangesError);

impl IntoResponse for ChangesFailed {
    fn into_response(self) -> Response {
        let status = match self.0 {
            ChangesError::NotARepo(_) => StatusCode::NOT_FOUND,
            ChangesError::BadPoint(_) => StatusCode::BAD_REQUEST,
            ChangesError::Git(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(serde_json::json!({ "error": self.0.to_string() }))).into_response()
    }
}

type ChangesResult<T> = Result<T, ChangesFailed>;

/// The project's changes, if it's a git repository.
pub(crate) fn changes_of(state: &ProjectState) -> ChangesResult<Arc<Changes>> {
    state.changes.clone().ok_or_else(|| ChangesFailed(ChangesError::NotARepo(state.arch.root().display().to_string())))
}

/// Runs `f` on the changes off the async threads.
pub(crate) async fn blocking<T: Send + 'static>(changes: Arc<Changes>, f: impl FnOnce(&Changes) -> Result<T, ChangesError> + Send + 'static) -> ChangesResult<T> {
    tokio::task::spawn_blocking(move || f(&changes)).await.map_err(|e| ChangesFailed(ChangesError::Git(e.to_string())))?.map_err(ChangesFailed)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Compared {
    #[serde(default)]
    from: Option<Point>,
    #[serde(default)]
    to: Option<Point>,
}

impl Compared {
    pub(crate) fn comparison(&self) -> Comparison {
        let default = Comparison::default();
        Comparison { from: self.from.clone().unwrap_or(default.from), to: self.to.clone().unwrap_or(default.to) }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    #[serde(flatten)]
    compared: Compared,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
}

/// The files a comparison changes (a page), with their counts.
async fn list(_: Needs<can::ViewRead>, State(state): State<ProjectState>, Query(q): Query<ListQuery>) -> ChangesResult<Json<ChangeList>> {
    let comparison = q.compared.comparison();
    let (offset, limit) = (q.offset, q.limit.unwrap_or(nah_changes::PAGE));
    Ok(Json(blocking(changes_of(&state)?, move |c| c.list(&comparison, offset, limit)).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileQuery {
    #[serde(flatten)]
    compared: Compared,
    path: String,
    group: Group,
    #[serde(default)]
    context: Option<u32>,
}

/// One file's changes.
async fn file(_: Needs<can::ViewRead>, State(state): State<ProjectState>, Query(q): Query<FileQuery>) -> ChangesResult<Json<FileChanges>> {
    let comparison = q.compared.comparison();
    let context = q.context.unwrap_or(nah_changes::CONTEXT);
    Ok(Json(blocking(changes_of(&state)?, move |c| c.file(&comparison, &q.path, q.group, context)).await?))
}
