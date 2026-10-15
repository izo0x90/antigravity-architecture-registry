//! Reading files of the repository NAH works on.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::server::AppState;
use crate::cockpit::{Needs, can};

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/fs/read", get(fs_read_handler))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsReadQuery {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FsReadResponse {
    pub path: String,
    pub content: String,
    pub lines: usize,
}

async fn fs_read_handler(
    _: Needs<can::ViewRead>,
    State(state): State<AppState>,
    Query(query): Query<FsReadQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Only files inside the repository NAH works on, however they're named.
    let root = state.arch.root().canonicalize().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("repository root: {e}")))?;
    let clean_path = query.path.trim().trim_start_matches("file://");
    let target = std::path::Path::new(clean_path);
    let resolved = if target.is_absolute() { target.to_path_buf() } else { root.join(target) };
    let canonical = match resolved.canonicalize() {
        Ok(c) => c,
        Err(e) => return Err((StatusCode::NOT_FOUND, format!("Path not found: {e}"))),
    };
    if !canonical.starts_with(&root) {
        return Err((StatusCode::FORBIDDEN, "Only files inside the repository can be read".to_string()));
    }
    if !canonical.is_file() {
        return Err((StatusCode::BAD_REQUEST, "Target is not a regular file".to_string()));
    }
    let content = tokio::fs::read_to_string(&canonical).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to read file: {e}")))?;

    let lines = content.lines().count();
    Ok(Json(FsReadResponse {
        path: canonical.to_string_lossy().to_string(),
        content,
        lines,
    }))
}
