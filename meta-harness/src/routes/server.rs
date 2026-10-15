//! Every server's own: being asked to stop (the cockpit asks, through
//! home for the others), which stops it the way Ctrl-C does.

use axum::{Router, extract::State, http::StatusCode, routing::post};

use crate::cockpit::{Needs, can};
use crate::modules::ServerState;

pub(crate) fn routes() -> Router<ServerState> {
    Router::new().route("/api/server/shutdown", post(shutdown))
}

/// Stops the server: what it's doing ends cleanly (agents it started stop
/// with it); the answer comes first.
async fn shutdown(_: Needs<can::SessionsControl>, State(state): State<ServerState>) -> StatusCode {
    tracing::info!("asked to stop");
    state.shutdown.ask();
    StatusCode::ACCEPTED
}
