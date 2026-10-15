//! What the cockpit itself uses: the launch link, health, its config and
//! saved state, and the agents' way to drive it.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};

use crate::server::{AppState, StateFailed, with_state};
use crate::cockpit::{Needs, can};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/ui/config", get(ui_config_handler))
        .route("/api/ui/state", get(get_ui_state).put(put_ui_state))
        .route("/cockpit", get(crate::cockpit::launch))
        .route("/ws/cockpit", get(crate::remote::socket))
        .route("/api/cockpit/ticket", post(ticket))
}

async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// The user's UI config file: `$NAH_UI_CONFIG`, else `~/.config/nah/ui.toml`.
fn ui_config_path() -> Option<std::path::PathBuf> {
    match std::env::var_os("NAH_UI_CONFIG") {
        Some(path) => Some(path.into()),
        None => std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".config/nah/ui.toml")),
    }
}

/// The cockpit's state as it last saved it (`null` before it ever has).
async fn get_ui_state(_: Needs<can::ViewRead>, State(state): State<AppState>) -> Result<Json<Option<serde_json::Value>>, StateFailed> {
    Ok(Json(with_state(&state.state, |db| db.ui_state()).await?))
}

/// The cockpit saves its state (only the cockpit's user can).
async fn put_ui_state(_: Needs<can::SessionsControl>, State(state): State<AppState>, Json(body): Json<serde_json::Value>) -> Result<StatusCode, StateFailed> {
    with_state(&state.state, move |db| db.set_ui_state(&body)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Serves the user's UI config text (merged over the UI's defaults client-side);
/// `text` is null when there is none.
async fn ui_config_handler(_: Needs<can::ViewRead>) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let Some(path) = ui_config_path() else {
        return Ok(Json(serde_json::json!({ "text": null })));
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => Ok(Json(serde_json::json!({ "text": text }))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Json(serde_json::json!({ "text": null }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": format!("{}: {e}", path.display()) })))),
    }
}

/// A one-time ticket to open a socket with (`/ws/events`, `/ws/cockpit`).
async fn ticket(_: Needs<can::ViewRead>, State(tickets): State<std::sync::Arc<crate::cockpit::Tickets>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ticket": tickets.issue() }))
}
