//! Home's pass-through: `/s/{server}/...` reaches that server, streamed,
//! with the server's secret and without the cookie; sockets are bridged.
//! A worker and a home with the fake harness, and a server that echoes
//! what it got.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::routing::{get, post};
use common::{COCKPIT_TOKEN, http, spawn_app};
use futures::StreamExt;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BIG: usize = 8 * 1024 * 1024;

/// A server that says what it was sent: the headers, a body's length; and
/// that sends a big body, or breaks off halfway.
async fn echo() -> SocketAddr {
    let app = Router::new()
        .route(
            "/headers",
            get(|headers: axum::http::HeaderMap| async move {
                let all: serde_json::Map<String, serde_json::Value> = headers.iter().map(|(k, v)| (k.to_string(), json!(v.to_str().unwrap_or("")))).collect();
                axum::Json(all)
            }),
        )
        .route(
            "/len",
            post(|body: Body| async move {
                let mut stream = body.into_data_stream();
                let mut n = 0usize;
                while let Some(chunk) = stream.next().await {
                    n += chunk.unwrap().len();
                }
                n.to_string()
            }),
        )
        .route("/big", get(|| async { Body::from_stream(futures::stream::iter((0..BIG / 65536).map(|_| Ok::<_, std::io::Error>(vec![b'x'; 65536])))) }))
        .route(
            "/drop",
            get(|| async {
                let chunks = futures::stream::iter([Ok(vec![b'x'; 1024]), Err(std::io::Error::other("the worker went away"))]);
                Body::from_stream(chunks)
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

async fn add(home: SocketAddr, name: &str, at: SocketAddr) {
    let (status, body) = http(home, "POST", "/api/home/servers", Some(&json!({ "name": name, "url": format!("http://{at}"), "secret": COCKPIT_TOKEN }).to_string())).await;
    assert_eq!(status, 201, "{body}");
}

/// A raw request through home, with the cockpit's cookie and more.
async fn raw(home: SocketAddr, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>, bool) {
    let mut stream = tokio::net::TcpStream::connect(home).await.unwrap();
    let head = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nCookie: nah_cockpit={COCKPIT_TOKEN}\r\nX-Kept: yes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    let mut answer = Vec::new();
    let whole = tokio::time::timeout(Duration::from_secs(30), stream.read_to_end(&mut answer)).await.unwrap().is_ok();
    let split = answer.windows(4).position(|w| w == b"\r\n\r\n").map_or(answer.len(), |p| p + 4);
    let status = String::from_utf8_lossy(&answer[..split]).split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    (status, answer[split..].to_vec(), whole)
}

#[tokio::test]
async fn a_request_and_a_socket_go_through_to_the_server_named() {
    let (worker, _) = spawn_app().await;
    let (home, _) = spawn_app().await;
    add(home, "w1", worker).await;
    let (status, body) = http(home, "GET", "/s/w1/api/slots", None).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"id\":\"lead\""), "the worker's slots: {body}");
    let (status, body) = http(home, "POST", "/s/w1/api/slots", Some(r#"{"instanceId":"fake"}"#)).await;
    assert_eq!(status, 200, "a change goes through too: {body}");
    assert_eq!(http(home, "GET", "/s/nope/api/slots", None).await.0, 404);
    assert_eq!(common::http_anonymous(home, "GET", "/s/w1/api/slots", None).await.0, 403, "only the user, through home");
    // Its event socket, through home (home's ticket; the worker gets home's secret).
    let ticket = common::ticket(home).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{home}/s/w1/ws/events?ticket={ticket}")).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(10), socket.next()).await.unwrap().unwrap().unwrap();
    assert!(first.to_text().unwrap().contains("instance_updated"), "the worker's replay: {first}");
}

#[tokio::test]
async fn what_reaches_the_server_is_the_request_with_its_secret_not_the_cookie_and_big_bodies_stream() {
    let (home, _) = spawn_app().await;
    let echoed = echo().await;
    add(home, "echo", echoed).await;
    let (status, body, _) = raw(home, "GET", "/s/echo/headers", b"").await;
    assert_eq!(status, 200);
    let got: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(got["authorization"], format!("Bearer {COCKPIT_TOKEN}"));
    assert_eq!(got["x-kept"], "yes");
    assert!(got.get("cookie").is_none() && got.get("connection").is_none(), "{got}");
    let big = vec![b'y'; BIG];
    let (status, body, _) = raw(home, "POST", "/s/echo/len", &big).await;
    assert_eq!((status, String::from_utf8_lossy(&body).to_string()), (200, BIG.to_string()), "sent through whole");
    let (status, body, _) = raw(home, "GET", "/s/echo/big", b"").await;
    assert_eq!(status, 200);
    assert!(body.len() >= BIG, "received whole (chunked framing aside): {}", body.len());
}

#[tokio::test]
async fn a_server_that_breaks_off_or_isnt_there_is_said_and_home_carries_on() {
    let (home, _) = spawn_app().await;
    let echoed = echo().await;
    add(home, "echo", echoed).await;
    // Broken off: said as home's 502 (it broke before its answer began),
    // or its answer cut short; never a hang.
    let (status, body, _) = raw(home, "GET", "/s/echo/drop", b"").await;
    assert!(status == 502 || (status == 200 && body.len() < 4096), "{status}: {}", String::from_utf8_lossy(&body));
    let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    add(home, "gone", gone).await;
    let (status, body) = http(home, "GET", "/s/gone/health", None).await;
    assert_eq!(status, 502, "{body}");
    assert!(body.contains("gone doesn't answer"), "{body}");
    assert_eq!(http(home, "GET", "/health", None).await.0, 200);
}
