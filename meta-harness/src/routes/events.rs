//! The event stream: the current state replayed, then every change.

use std::sync::Arc;

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use futures::{SinkExt, StreamExt};
use harness_core::EventHub;

use crate::server::AppState;

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/ws/events", get(ws_events_handler))
}

async fn ws_events_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| stream_events(socket, state.hub))
}

/// Replays current state, then streams `ClientEvent`s as JSON text frames.
async fn stream_events(socket: WebSocket, hub: Arc<EventHub>) {
    let (mut sink, mut incoming) = socket.split();
    let (replay, mut live) = hub.subscribe();
    for event in replay {
        let Ok(text) = serde_json::to_string(&event) else { continue };
        if sink.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }
    loop {
        tokio::select! {
            event = live.recv() => match event {
                Ok(event) => {
                    let Ok(text) = serde_json::to_string(&event) else { continue };
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
            msg = incoming.next() => if matches!(msg, None | Some(Err(_)) | Some(Ok(Message::Close(_)))) {
                return;
            },
        }
    }
}
