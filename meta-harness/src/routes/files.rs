//! Reading files of the repository NAH works on.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::modules::ProjectState;
use crate::cockpit::{Needs, can};

pub(crate) fn routes() -> Router<ProjectState> {
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
    State(state): State<ProjectState>,
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
    // The server's own: its state and the harnesses' kept logins, however
    // they're reached (a link included).
    let asked = resolved.components().fold(std::path::PathBuf::new(), |mut at, part| {
        match part {
            std::path::Component::ParentDir => {
                at.pop();
            }
            std::path::Component::CurDir => {}
            part => at.push(part),
        }
        at
    });
    let private = arch_service::private_paths(&root);
    if private.iter().any(|p| asked.starts_with(p) || canonical.starts_with(p) || p.canonicalize().is_ok_and(|p| canonical.starts_with(p))) {
        return Err((StatusCode::FORBIDDEN, "That's the server's own (its state, the harnesses' logins): it isn't served".to_string()));
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
