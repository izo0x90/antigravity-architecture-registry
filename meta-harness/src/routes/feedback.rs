//! Developer feedback from the cockpit's inspector.

use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
use serde::{Deserialize, Serialize};

use crate::server::AppState;
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

async fn dev_feedback_handler(
    _: Needs<can::SessionsControl>,
    Json(mut item): Json<DevFeedbackItem>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if item.id.is_empty() {
        item.id = format!("fb_{}", uuid::Uuid::new_v4().simple());
    }
    if item.timestamp.is_empty() {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        item.timestamp = format!("{secs}");
    }

    let feedback_file = std::path::Path::new("dev_feedback.json");
    let mut list: Vec<DevFeedbackItem> = if feedback_file.exists() {
        let content = tokio::fs::read_to_string(feedback_file).await.unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    };

    list.push(item.clone());

    let formatted = serde_json::to_string_pretty(&list).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    tokio::fs::write(feedback_file, formatted).await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    Ok(Json(serde_json::json!({
        "status": "saved",
        "id": item.id,
        "total_feedback_count": list.len()
    })))
}

async fn get_dev_feedback_handler(_: Needs<can::ViewRead>) -> Result<impl IntoResponse, (StatusCode, String)> {
    let feedback_file = std::path::Path::new("dev_feedback.json");
    let list: Vec<DevFeedbackItem> = if feedback_file.exists() {
        let content = tokio::fs::read_to_string(feedback_file).await.unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(Json(list))
}

async fn update_dev_feedback_handler(
    _: Needs<can::SessionsControl>,
    Json(item): Json<DevFeedbackItem>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let feedback_file = std::path::Path::new("dev_feedback.json");
    let mut list: Vec<DevFeedbackItem> = if feedback_file.exists() {
        let content = tokio::fs::read_to_string(feedback_file).await.unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut found = false;
    for existing in list.iter_mut() {
        if existing.id == item.id {
            *existing = item.clone();
            found = true;
            break;
        }
    }

    if !found {
        list.push(item.clone());
    }

    let formatted = serde_json::to_string_pretty(&list).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    tokio::fs::write(feedback_file, formatted).await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    Ok(Json(item))
}
