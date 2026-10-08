//! The explainer's notes: questions and remarks the user pins to an item of
//! the explainer page (served from `static/explainer/`). Anyone local can
//! read them (the agent too); only the user, from the cockpit, writes them.
//! Kept in `.nah/explainer/notes.json` under the project.

use std::path::PathBuf;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::{Deserialize, Serialize};

use crate::cockpit::CockpitUser;
use crate::server::AppState;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Note {
    id: String,
    /// The explainer item it's on.
    item: String,
    /// "question" or "note".
    kind: String,
    text: String,
    /// Seconds since the Unix epoch.
    at: u64,
    #[serde(default)]
    resolved: bool,
}

#[derive(Deserialize)]
struct NewNote {
    item: String,
    kind: String,
    text: String,
}

#[derive(Deserialize)]
struct Change {
    resolved: Option<bool>,
    text: Option<String>,
}

/// One writer at a time (the file is read, changed and written whole).
static WRITING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/explainer/notes", get(list).post(add)).route("/api/explainer/notes/{id}", put(change).delete(remove))
}

fn file(state: &AppState) -> PathBuf {
    state.arch.root().join(".nah/explainer/notes.json")
}

async fn read(state: &AppState) -> Result<Vec<Note>, Response> {
    match tokio::fs::read_to_string(file(state)).await {
        Ok(text) => serde_json::from_str(&text).map_err(|e| failed(format!("the notes file can't be read: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(failed(e.to_string())),
    }
}

async fn write(state: &AppState, notes: &[Note]) -> Result<(), Response> {
    let path = file(state);
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| failed(e.to_string()))?;
    }
    let text = serde_json::to_string_pretty(notes).map_err(|e| failed(e.to_string()))? + "\n";
    tokio::fs::write(path, text).await.map_err(|e| failed(e.to_string()))
}

fn failed(error: String) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": error }))).into_response()
}

fn refused(error: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": error }))).into_response()
}

async fn list(State(state): State<AppState>) -> Response {
    match read(&state).await {
        Ok(notes) => Json(notes).into_response(),
        Err(e) => e,
    }
}

async fn add(_: CockpitUser, State(state): State<AppState>, Json(new): Json<NewNote>) -> Response {
    if new.text.trim().is_empty() || new.item.trim().is_empty() {
        return refused("a note needs an item and some text");
    }
    if !matches!(new.kind.as_str(), "question" | "note") {
        return refused("a note is a question or a note");
    }
    let _one = WRITING.lock().await;
    let mut notes = match read(&state).await {
        Ok(n) => n,
        Err(e) => return e,
    };
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let note = Note { id: format!("n_{}", uuid::Uuid::new_v4().simple()), item: new.item, kind: new.kind, text: new.text.trim().to_owned(), at, resolved: false };
    notes.push(note.clone());
    match write(&state, &notes).await {
        Ok(()) => (StatusCode::CREATED, Json(note)).into_response(),
        Err(e) => e,
    }
}

async fn change(_: CockpitUser, State(state): State<AppState>, Path(id): Path<String>, Json(change): Json<Change>) -> Response {
    let _one = WRITING.lock().await;
    let mut notes = match read(&state).await {
        Ok(n) => n,
        Err(e) => return e,
    };
    let Some(note) = notes.iter_mut().find(|n| n.id == id) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "no such note" }))).into_response();
    };
    if let Some(resolved) = change.resolved {
        note.resolved = resolved;
    }
    if let Some(text) = change.text.filter(|t| !t.trim().is_empty()) {
        note.text = text.trim().to_owned();
    }
    let note = note.clone();
    match write(&state, &notes).await {
        Ok(()) => Json(note).into_response(),
        Err(e) => e,
    }
}

async fn remove(_: CockpitUser, State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let _one = WRITING.lock().await;
    let mut notes = match read(&state).await {
        Ok(n) => n,
        Err(e) => return e,
    };
    let before = notes.len();
    notes.retain(|n| n.id != id);
    if notes.len() == before {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "no such note" }))).into_response();
    }
    match write(&state, &notes).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e,
    }
}
