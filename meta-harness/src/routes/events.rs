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
use harness_contracts::ClientEvent;
use harness_core::EventHub;

use crate::server::AppState;

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/ws/events", get(ws_events_handler))
}

async fn ws_events_handler(_: crate::cockpit::Ticketed, ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| stream_events(socket, state.hub))
}

/// Replays current state, then streams `ClientEvent`s as JSON text frames.
/// A client that falls behind gets `Resync`, then the whole state again:
/// never a stream with events missing.
async fn stream_events(socket: WebSocket, hub: Arc<EventHub>) {
    let (mut sink, mut incoming) = socket.split();
    let (replay, mut live) = hub.subscribe();
    if send_all(&mut sink, replay).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            event = live.recv() => match event {
                Ok(event) => {
                    if send_all(&mut sink, [event]).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::info!("an event client fell {missed} events behind: sending it everything again");
                    let (replay, fresh) = hub.subscribe();
                    live = fresh;
                    if send_all(&mut sink, std::iter::once(ClientEvent::Resync).chain(replay)).await.is_err() {
                        return;
                    }
                }
                Err(_) => return,
            },
            msg = incoming.next() => if matches!(msg, None | Some(Err(_)) | Some(Ok(Message::Close(_)))) {
                return;
            },
        }
    }
}

async fn send_all(sink: &mut futures::stream::SplitSink<WebSocket, Message>, events: impl IntoIterator<Item = ClientEvent>) -> Result<(), axum::Error> {
    for event in events {
        let Ok(text) = serde_json::to_string(&event) else { continue };
        sink.send(Message::Text(text.into())).await?;
    }
    Ok(())
}
