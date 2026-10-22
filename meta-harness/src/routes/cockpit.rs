//! What the cockpit itself uses: the launch link, its config and saved
//! state, and the agents' way to drive it.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post, put},
};

use crate::modules::CockpitState;
use crate::server::{StateFailed, with_state};
use crate::cockpit::{Needs, can};

pub(crate) fn routes() -> Router<CockpitState> {
    Router::new()
        .route("/api/ui/config", get(ui_config_handler))
        .route("/api/ui/state", get(get_ui_state).put(put_ui_state))
        .route("/api/ui/drafts", get(get_drafts))
        .route("/api/ui/drafts/{slot}", put(put_draft))
        .route("/api/ui/shell", get(get_ui_shell).put(put_ui_shell))
        .route("/cockpit", get(crate::cockpit::launch))
        .route("/ws/cockpit", get(crate::remote::socket))
        .route("/api/cockpit/ticket", post(ticket))
}

/// The user's UI config file: `$NAH_UI_CONFIG`, else `~/.config/nah/ui.toml`.
fn ui_config_path() -> Option<std::path::PathBuf> {
    match std::env::var_os("NAH_UI_CONFIG") {
        Some(path) => Some(path.into()),
        None => std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".config/nah/ui.toml")),
    }
}

/// The cockpit's state as it last saved it (`null` before it ever has).
async fn get_ui_state(_: Needs<can::ViewRead>, State(state): State<CockpitState>) -> Result<Json<Option<serde_json::Value>>, StateFailed> {
    Ok(Json(with_state(&state.state, |db| db.ui_state()).await?))
}

/// The cockpit saves its state (only the cockpit's user can).
async fn put_ui_state(_: Needs<can::SessionsControl>, State(state): State<CockpitState>, Json(body): Json<serde_json::Value>) -> Result<StatusCode, StateFailed> {
    with_state(&state.state, move |db| db.set_ui_state(&body)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Each slot's unsent chat text, as the cockpit last saved it (its own
/// record: a layout reset doesn't lose it).
async fn get_drafts(_: Needs<can::ViewRead>, State(state): State<CockpitState>) -> Result<Json<std::collections::BTreeMap<String, String>>, StateFailed> {
    Ok(Json(with_state(&state.state, |db| db.drafts()).await?))
}

#[derive(serde::Deserialize)]
struct Draft {
    text: String,
}

/// The cockpit saves a slot's unsent text (empty: none).
async fn put_draft(_: Needs<can::SessionsControl>, State(state): State<CockpitState>, Path(slot): Path<String>, Json(body): Json<Draft>) -> Result<StatusCode, StateFailed> {
    with_state(&state.state, move |db| db.set_draft(&slot, &body.text)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The cockpit's own state on home: which server it shows, its settings
/// (each server keeps its own desk's layout).
async fn get_ui_shell(_: Needs<can::ViewRead>, State(state): State<CockpitState>) -> Result<Json<Option<serde_json::Value>>, StateFailed> {
    Ok(Json(with_state(&state.state, |db| db.ui_shell()).await?))
}

async fn put_ui_shell(_: Needs<can::SessionsControl>, State(state): State<CockpitState>, Json(body): Json<serde_json::Value>) -> Result<StatusCode, StateFailed> {
    with_state(&state.state, move |db| db.set_ui_shell(&body)).await?;
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
