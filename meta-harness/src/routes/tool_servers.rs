//! The tool servers every session gets: ours and the project's (`.nah/mcp.toml`).

use axum::{Json, Router, extract::State, routing::get};
use harness_core::HarnessError;

use crate::cockpit::{Needs, can};
use crate::modules::HarnessState;
use crate::server::ApiResult;

pub(crate) fn routes() -> Router<HarnessState> {
    Router::new().route("/api/mcp/servers", get(get_tool_servers).put(put_tool_servers))
}

async fn get_tool_servers(_: Needs<can::ViewRead>, State(state): State<HarnessState>) -> ApiResult<Json<harness_contracts::ToolServers>> {
    let tools = state.tool_servers.clone();
    let current =
        tokio::task::spawn_blocking(move || tools.current()).await.map_err(|e| HarnessError::Provider { reason: e.to_string() })?;
    Ok(Json(current.map_err(|reason| HarnessError::InvalidConfig { reason })?))
}

/// Replaces the project's list (only the cockpit's user can); new sessions
/// get it.
async fn put_tool_servers(
    _: Needs<can::SessionsControl>,
    State(state): State<HarnessState>,
    Json(project): Json<Vec<harness_contracts::McpServer>>,
) -> ApiResult<Json<harness_contracts::ToolServers>> {
    let tools = state.tool_servers.clone();
    let current = tokio::task::spawn_blocking(move || tools.file.write(&project).and_then(|()| tools.current()))
        .await
        .map_err(|e| HarnessError::Provider { reason: e.to_string() })?;
    Ok(Json(current.map_err(|reason| HarnessError::InvalidConfig { reason })?))
}
