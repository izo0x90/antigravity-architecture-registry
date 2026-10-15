//! Home: the servers it links to, their events through its socket, and
//! what it says when one stops answering. Three servers in one process:
//! two workers and a home (each with a project and the fake harness).

mod common;

use std::time::Duration;

use common::{COCKPIT_TOKEN, http, spawn_app};
use futures::StreamExt;
use harness_contracts::{ClientEvent, HomeEvent, LinkState, NotificationKind, ServerView};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn home_socket(addr: std::net::SocketAddr) -> Socket {
    let ticket = common::ticket(addr).await;
    tokio_tungstenite::connect_async(format!("ws://{addr}/ws/home?ticket={ticket}")).await.unwrap().0
}

/// The next of home's events that `want` takes, within a few seconds.
async fn until<T>(socket: &mut Socket, mut want: impl FnMut(HomeEvent) -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    loop {
        let Ok(message) = tokio::time::timeout_at(deadline, socket.next()).await else { panic!("home didn't say it in time; it said: {seen:#?}") };
        if let Message::Text(text) = message.expect("open").unwrap() {
            let event: HomeEvent = serde_json::from_str(&text).unwrap();
            seen.push(text.chars().take(160).collect::<String>());
            if let Some(found) = want(event) {
                return found;
            }
        }
    }
}

async fn servers(addr: std::net::SocketAddr) -> Vec<ServerView> {
    let (status, body) = http(addr, "GET", "/api/home/servers", None).await;
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

async fn add(home: std::net::SocketAddr, name: &str, at: &str) -> (u16, String) {
    http(home, "POST", "/api/home/servers", Some(&json!({ "name": name, "url": at, "secret": COCKPIT_TOKEN }).to_string())).await
}

#[tokio::test]
async fn home_follows_each_server_and_says_whose_events_are_whose() {
    let (worker, worker_state) = spawn_app().await;
    let (home, _) = spawn_app().await;
    let mut socket = home_socket(home).await;
    let first = until(&mut socket, |e| match e {
        HomeEvent::Servers { servers } => Some(servers),
        _ => None,
    })
    .await;
    assert_eq!(first.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["here"], "home's own, to start");
    let (status, body) = add(home, "w1", &format!("http://{worker}")).await;
    assert_eq!(status, 201, "{body}");
    assert!(!body.contains(COCKPIT_TOKEN), "never with its secret: {body}");
    // Linked, and its state (after Resync), in whichever order they come.
    let (mut linked, mut state) = (false, false);
    until(&mut socket, |e| {
        match e {
            HomeEvent::Servers { servers } => linked |= servers.iter().any(|s| s.name == "w1" && s.link == LinkState::Connected),
            HomeEvent::Event { server, event } if matches!(*event, ClientEvent::InstanceUpdated { .. }) => state |= server == "w1",
            _ => {}
        }
        (linked && state).then_some(())
    })
    .await;
    let sub = worker_state.harness.slots.create("sub", Some("reviewer".into()), harness_core::SlotSettings { instance: Some(harness_contracts::InstanceId::new(common::FAKE)), ..Default::default() }, harness_contracts::Starter::User).await.unwrap();
    until(&mut socket, |e| matches!(&e, HomeEvent::Event { server, event } if server == "w1" && matches!(&**event, ClientEvent::SlotUpdated { slot } if slot.id == sub)).then_some(())).await;
    // Its notifications come said as its.
    worker_state.notify.notify.message(Some(&sub), "from the worker").unwrap();
    let told = until(&mut socket, |e| match e {
        HomeEvent::Event { server, event } if server == "w1" => match *event {
            ClientEvent::NotificationUpdated { notification } => Some(notification),
            _ => None,
        },
        _ => None,
    })
    .await;
    assert_eq!((told.server.as_deref(), told.kind), (Some("w1"), NotificationKind::Message));
    // Disconnected: listed still, not followed; the worker runs on.
    assert_eq!(http(home, "POST", "/api/home/servers/w1/disconnect", None).await.0, 204);
    assert_eq!(servers(home).await.iter().find(|s| s.name == "w1").map(|s| s.link.clone()), Some(LinkState::Disconnected));
    assert_eq!(http(worker, "GET", "/health", None).await.0, 200);
    assert_eq!(http(home, "POST", "/api/home/servers/w1/connect", None).await.0, 204);
    assert_eq!(http(home, "POST", "/api/home/servers/nope/connect", None).await.0, 404);
}

#[tokio::test]
async fn a_server_that_doesnt_answer_is_said_lost_once_until_removed() {
    let (home, home_state) = spawn_app().await;
    let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let (status, body) = add(home, "gone", &format!("http://{gone}")).await;
    assert_eq!(status, 201, "{body}");
    let mut lost = Vec::new();
    for _ in 0..100 {
        lost = home_state.notify.notify.open().into_iter().filter(|n| n.kind == NotificationKind::ServerLost).collect();
        if !lost.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(lost.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["lost:gone:1"], "said once while it keeps trying");
    assert!(matches!(servers(home).await.iter().find(|s| s.name == "gone").map(|s| &s.link), Some(LinkState::Lost { .. } | LinkState::Connecting)));
    let (status, body) = add(home, "gone", &format!("http://{gone}")).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(http(home, "DELETE", "/api/home/servers/gone", None).await.0, 204);
    assert!(!home_state.notify.notify.open().iter().any(|n| n.kind == NotificationKind::ServerLost), "removed: no longer lost");
    assert_eq!(http(home, "DELETE", "/api/home/servers/gone", None).await.0, 404);
    assert_eq!(servers(home).await.len(), 1);
}
