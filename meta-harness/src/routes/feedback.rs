//! Developer feedback from the cockpit's inspector.

use axum::{Json, Router, extract::State, response::IntoResponse, routing::get};
use serde::{Deserialize, Serialize};

use crate::server::{AppState, StateFailed, with_state};
use crate::cockpit::{Needs, can};

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/dev/feedback", get(get_dev_feedback_handler).post(dev_feedback_handler).put(update_dev_feedback_handler))
}

fn default_status() -> String {
    "open".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevFeedbackItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub timestamp: String,
    pub target: serde_json::Value,
    pub category: String,
    pub comment: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub context: Option<serde_json::Value>,
}

/// Keeps a new item: it gets an id and a time if it came without.
async fn dev_feedback_handler(_: Needs<can::SessionsControl>, State(state): State<AppState>, Json(mut item): Json<DevFeedbackItem>) -> Result<impl IntoResponse, StateFailed> {
    if item.id.is_empty() {
        item.id = format!("fb_{}", uuid::Uuid::new_v4().simple());
    }
    if item.timestamp.is_empty() {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        item.timestamp = format!("{secs}");
    }
    let (id, value) = (item.id.clone(), as_value(&item));
    let total = with_state(&state.state, move |db| db.put_feedback(&id, &value)).await?;
    Ok(Json(serde_json::json!({ "status": "saved", "id": item.id, "total_feedback_count": total })))
}

/// Every item, oldest first.
async fn get_dev_feedback_handler(_: Needs<can::ViewRead>, State(state): State<AppState>) -> Result<impl IntoResponse, StateFailed> {
    Ok(Json(with_state(&state.state, |db| db.feedback()).await?))
}

/// Replaces the item with its id (or adds it).
async fn update_dev_feedback_handler(_: Needs<can::SessionsControl>, State(state): State<AppState>, Json(item): Json<DevFeedbackItem>) -> Result<impl IntoResponse, StateFailed> {
    let (id, value) = (item.id.clone(), as_value(&item));
    with_state(&state.state, move |db| db.put_feedback(&id, &value)).await?;
    Ok(Json(item))
}

fn as_value(item: &DevFeedbackItem) -> serde_json::Value {
    serde_json::to_value(item).unwrap_or_default()
}
