//! Home's routes: the servers it links to (listed without their secrets,
//! added, removed, connected and disconnected) and its socket.

use axum::{
    Json, Router,
    extract::{Path, State, ws::WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use harness_contracts::ServerView;
use serde::Deserialize;

use crate::cockpit::{Needs, Ticketed, can};
use crate::modules::HomeState;
use crate::server::{StateFailed, with_state};

pub(crate) fn routes() -> Router<HomeState> {
    Router::new()
        .route("/api/home/servers", get(list).post(add))
        .route("/api/home/servers/{name}", axum::routing::delete(remove))
        .route("/api/home/servers/{name}/connect", post(connect))
        .route("/api/home/servers/{name}/disconnect", post(disconnect))
        .route("/ws/home", get(home_socket))
        .route("/s/{server}/ws/{*rest}", get(pass_socket))
        .route("/s/{server}/{*rest}", axum::routing::any(pass))
}

/// `/s/{server}/...`: that server's `/...` (it checks what the call needs).
async fn pass(_: Needs<can::ViewRead>, State(state): State<HomeState>, Path((server, rest)): Path<(String, String)>, request: axum::extract::Request) -> axum::response::Response {
    match crate::home::pass::entry(&state.home, &server) {
        Some(entry) => crate::home::pass::request(&state.home.client, &entry, &rest, request).await,
        None => (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({ "error": format!("home links to no server named {server:?}") }))).into_response(),
    }
}

/// `/s/{server}/ws/...`: that server's socket, bridged both ways.
async fn pass_socket(_: Ticketed, ws: WebSocketUpgrade, State(state): State<HomeState>, Path((server, rest)): Path<(String, String)>, uri: axum::http::Uri) -> axum::response::Response {
    match crate::home::pass::entry(&state.home, &server) {
        Some(entry) => {
            let query = uri.query().map(str::to_owned);
            ws.on_upgrade(move |socket| crate::home::pass::socket(socket, entry, format!("ws/{rest}"), query)).into_response()
        }
        None => (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({ "error": format!("home links to no server named {server:?}") }))).into_response(),
    }
}

async fn list(_: Needs<can::ViewRead>, State(state): State<HomeState>) -> Json<Vec<ServerView>> {
    Json(state.home.servers())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewServer {
    name: String,
    url: String,
    secret: String,
}

/// Adds a server (its secret is what `meta-harness token` prints there),
/// followed from now on.
async fn add(_: Needs<can::SessionsControl>, State(state): State<HomeState>, Json(body): Json<NewServer>) -> Result<(StatusCode, Json<Vec<ServerView>>), StateFailed> {
    let entry = nah_state::ServerEntry { name: body.name, url: body.url, secret: body.secret, added_at_ms: crate::home::now_ms() };
    let kept = entry.clone();
    with_state(&state.home.db, move |db| db.add_server(&kept)).await?;
    let added = with_state(&state.home.db, move |db| db.server(&entry.name)).await?;
    if let Some(entry) = added {
        state.home.links.add(entry);
    }
    Ok((StatusCode::CREATED, Json(state.home.servers())))
}

async fn remove(_: Needs<can::SessionsControl>, State(state): State<HomeState>, Path(name): Path<String>) -> Result<StatusCode, StateFailed> {
    let removed = {
        let name = name.clone();
        with_state(&state.home.db, move |db| db.remove_server(&name)).await?
    };
    state.home.links.remove(&name);
    Ok(if removed { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
}

async fn connect(_: Needs<can::SessionsControl>, State(state): State<HomeState>, Path(name): Path<String>) -> StatusCode {
    if state.home.links.connect(&name) { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND }
}

/// Stops following a server: it runs on, and stays listed.
async fn disconnect(_: Needs<can::SessionsControl>, State(state): State<HomeState>, Path(name): Path<String>) -> StatusCode {
    if state.home.links.disconnect(&name) { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND }
}

async fn home_socket(_: Ticketed, ws: WebSocketUpgrade, State(state): State<HomeState>) -> impl IntoResponse {
    let home = state.home.clone();
    ws.on_upgrade(move |socket| crate::home::socket::serve(socket, home))
}
