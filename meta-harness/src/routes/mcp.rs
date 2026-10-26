//! The agents' tools: the MCP endpoint (an event stream plus message posts).

use axum::{
    Router,
    extract::State,
    routing::{get, post},
};

use crate::modules::ProjectState;

pub(crate) fn routes() -> Router<ProjectState> {
    Router::new().route("/api/mcp/sse", get(mcp_sse_proxy).post(mcp_messages_proxy)).route("/api/mcp/messages", post(mcp_messages_proxy))
}

async fn mcp_sse_proxy(
    State(state): State<ProjectState>,
    query: axum::extract::Query<crate::mcp::sse::McpStreamQuery>,
) -> axum::response::sse::Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    crate::mcp::mcp_sse_handler(axum::extract::State(state.mcp_state), query).await
}

async fn mcp_messages_proxy(
    State(state): State<ProjectState>,
    query: axum::extract::Query<crate::mcp::McpMessagesQuery>,
    json: axum::Json<crate::mcp::JsonRpcRequest>,
) -> impl axum::response::IntoResponse {
    crate::mcp::mcp_messages_handler(axum::extract::State(state.mcp_state), query, json).await
}
