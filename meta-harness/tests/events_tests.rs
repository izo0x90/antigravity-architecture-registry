//! The event stream never has a gap: a client that falls behind is told to
//! start over and gets the whole state again.

mod common;

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use harness_contracts::ClientEvent;
use harness_core::EventHub;
use tokio_tungstenite::tungstenite::Message;

async fn next(ws: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin), within: Duration) -> Option<ClientEvent> {
    loop {
        match tokio::time::timeout(within, ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => return Some(serde_json::from_str(&text).unwrap()),
            Ok(Some(Ok(_))) => continue,
            _ => return None,
        }
    }
}

#[tokio::test]
async fn a_client_that_falls_behind_gets_resync_and_the_whole_state_never_a_gap() {
    let hub = Arc::new(EventHub::with_capacity(4));
    let (addr, state) = common::spawn_app_with_hub(common::fixture_architecture(), hub.clone()).await;
    let mut ws = common::events_socket(addr).await;
    while next(&mut ws, Duration::from_millis(300)).await.is_some() {}

    // Far more changes than the stream holds, before it gets to send any.
    let mut snapshot = state.harness.registry.snapshots().into_iter().next().expect("the fake harness");
    for n in 0..200 {
        snapshot.display_name = format!("change {n}");
        hub.publish(ClientEvent::InstanceUpdated { snapshot: snapshot.clone() });
    }

    let mut resynced = false;
    let mut last = None;
    while let Some(event) = next(&mut ws, Duration::from_millis(500)).await {
        match event {
            ClientEvent::Resync => resynced = true,
            ClientEvent::InstanceUpdated { snapshot } => last = Some(snapshot.display_name),
            _ => {}
        }
    }
    assert!(resynced, "it was told to start over");
    assert_eq!(last.as_deref(), Some("change 199"), "and ends with the latest state");
}
