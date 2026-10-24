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
        .route("/api/ui/fonts/{name}", get(font_face))
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
async fn get_drafts(_: Needs<can::ViewRead>, State(state): State<CockpitState>) -> Result<Json<std::collections::BTreeMap<String, harness_contracts::Message>>, StateFailed> {
    Ok(Json(with_state(&state.state, |db| db.drafts()).await?))
}

/// The cockpit saves a slot's unsent message (its text and links; empty: none).
async fn put_draft(_: Needs<can::SessionsControl>, State(state): State<CockpitState>, Path(slot): Path<String>, Json(body): Json<harness_contracts::Message>) -> Result<StatusCode, StateFailed> {
    with_state(&state.state, move |db| db.set_draft(&slot, &body)).await?;
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

/// A font face that can't be served: why, as the pack engine says.
struct FaceFailed(nah_packs::PackError);

impl axum::response::IntoResponse for FaceFailed {
    fn into_response(self) -> axum::response::Response {
        use nah_packs::PackError as E;
        let status = match &self.0 {
            E::NotFound(_) | E::NotInstalled(_) => StatusCode::NOT_FOUND,
            // Asked of a pack that isn't a font.
            E::NotA { .. } => StatusCode::BAD_REQUEST,
            // There, but not as this server's packs say it should be.
            E::Outdated { .. } => StatusCode::CONFLICT,
            // The server's own packs (a manifest, the lock, an install).
            E::Broken { .. } | E::Manifest { .. } | E::Config(_) | E::Lock(_) | E::Install { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(serde_json::json!({ "error": self.0.to_string() }))).into_response()
    }
}

/// An installed font pack's face, for the cockpit to draw its text in. The
/// file as `ServeFile` serves it (its type, its last change: asked again,
/// it's "not modified" unless the pack was installed again since), checked
/// with the browser each time it's asked.
async fn font_face(_: Needs<can::ViewRead>, State(state): State<CockpitState>, Path(name): Path<String>, request: axum::extract::Request) -> Result<axum::response::Response, FaceFailed> {
    use axum::response::IntoResponse;
    use tower::ServiceExt;
    let at = state.packs_at.clone();
    let file = tokio::task::spawn_blocking(move || {
        let catalog = crate::packs::catalog(&at)?;
        let lock = nah_packs::Lock::read(&catalog.lock_path)?;
        nah_packs::font_face(&catalog, &lock, &name)
    })
    .await
    .map_err(|e| FaceFailed(nah_packs::PackError::Config(e.to_string())))?
    .map_err(FaceFailed)?;
    let Ok(served) = tower_http::services::ServeFile::new(file).oneshot(request).await;
    let mut served = served.map(axum::body::Body::new).into_response();
    served.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-cache"));
    Ok(served)
}

/// A one-time ticket to open a socket with (`/ws/events`, `/ws/cockpit`).
async fn ticket(_: Needs<can::ViewRead>, State(tickets): State<std::sync::Arc<crate::cockpit::Tickets>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ticket": tickets.issue() }))
}
