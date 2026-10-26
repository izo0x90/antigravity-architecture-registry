//! `/ws/home`: every server's events, each said by name (its replay first,
//! then what happens), and the list of servers whenever it changes. A
//! server whose events this socket falls behind on is sent its whole state
//! again (after `Resync`); the others carry on.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use harness_contracts::{ClientEvent, HomeEvent, Stamped};
use harness_core::EventHub;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use super::Home;

/// Events waiting to be sent, from every server.
const QUEUE: usize = 1024;

pub async fn serve(socket: WebSocket, home: Arc<Home>) {
    let (mut sink, mut incoming) = socket.split();
    let (tx, mut rx) = mpsc::channel::<HomeEvent>(QUEUE);
    let mut changed = home.links.watch();
    let mut following: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
    let mut servers = home.servers();
    follow_new(&home, &mut following, &tx);
    if send(&mut sink, &HomeEvent::Servers { servers: servers.clone() }).await.is_err() {
        return stop(following);
    }
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Some(event) => {
                    if send(&mut sink, &event).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            seen = changed.changed() => {
                if seen.is_err() {
                    break;
                }
                follow_new(&home, &mut following, &tx);
                let now = home.servers();
                if now != servers {
                    servers = now;
                    if send(&mut sink, &HomeEvent::Servers { servers: servers.clone() }).await.is_err() {
                        break;
                    }
                }
            }
            message = incoming.next() => match message {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    stop(following);
}

/// Follows the servers it doesn't yet (and stops following removed ones).
fn follow_new(home: &Home, following: &mut HashMap<String, tokio::task::AbortHandle>, tx: &mpsc::Sender<HomeEvent>) {
    let hubs = home.hubs();
    following.retain(|name, task| {
        let kept = hubs.iter().any(|(n, _)| n == name);
        if !kept {
            task.abort();
        }
        kept
    });
    for (name, hub) in hubs {
        following.entry(name.clone()).or_insert_with(|| tokio::spawn(forward(name, hub, tx.clone())).abort_handle());
    }
}

fn stop(following: HashMap<String, tokio::task::AbortHandle>) {
    for task in following.into_values() {
        task.abort();
    }
}

/// One server's events: `Resync`, its replay and `Replayed`, then live;
/// again after falling behind.
async fn forward(server: String, hub: Arc<EventHub>, tx: mpsc::Sender<HomeEvent>) {
    loop {
        let (replay, mut live) = hub.subscribe();
        let now = |event| Stamped { at_ms: harness_core::now_ms(), event };
        for event in std::iter::once(now(ClientEvent::Resync)).chain(replay).chain([now(ClientEvent::Replayed)]) {
            if tx.send(HomeEvent::Event { server: server.clone(), event: Box::new(event) }).await.is_err() {
                return;
            }
        }
        loop {
            match live.recv().await {
                Ok(event) => {
                    if tx.send(HomeEvent::Event { server: server.clone(), event: Box::new(event) }).await.is_err() {
                        return;
                    }
                }
                Err(RecvError::Lagged(_)) => break,
                Err(RecvError::Closed) => return,
            }
        }
    }
}

async fn send(sink: &mut futures::stream::SplitSink<WebSocket, Message>, event: &HomeEvent) -> Result<(), ()> {
    let text = serde_json::to_string(event).map_err(|_| ())?;
    sink.send(Message::Text(text.into())).await.map_err(|_| ())
}
