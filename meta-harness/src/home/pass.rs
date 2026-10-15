//! The pass-through: `/s/{server}/...` is that server's `/...`, streamed
//! both ways without buffering. Home adds the server's secret and drops
//! the cookie and the hop-by-hop headers; the server then checks what the
//! call needs against Home. Its sockets (`/s/{server}/ws/...`) are bridged
//! both ways.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::ws::{Message as AxMessage, WebSocket};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, Uri, header};
use futures::{SinkExt, StreamExt};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use nah_state::ServerEntry;
use tokio_tungstenite::tungstenite::{Message as TMessage, client::IntoClientRequest};

pub type HttpClient = Client<HttpConnector, Body>;

pub fn client() -> HttpClient {
    Client::builder(TokioExecutor::new()).build_http()
}

/// Headers that are about one hop, not the request.
const HOP_BY_HOP: [HeaderName; 8] = [
    header::CONNECTION,
    HeaderName::from_static("keep-alive"),
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
    header::UPGRADE,
];

fn said(status: StatusCode, message: String) -> Response<Body> {
    let mut response = Response::new(Body::from(serde_json::json!({ "error": message }).to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

/// What's passed on: the request's headers but the cookie, the host, its
/// own credentials and the hop-by-hop ones; with the server's secret.
fn passed_headers(from: &HeaderMap, secret: &str) -> Result<HeaderMap, String> {
    let mut headers = from.clone();
    for name in HOP_BY_HOP.iter().chain(&[header::COOKIE, header::HOST, header::AUTHORIZATION]) {
        headers.remove(name);
    }
    headers.insert(header::AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {secret}")).map_err(|_| "its secret isn't a header value".to_owned())?);
    Ok(headers)
}

/// The path and query on the server, the cockpit's ticket for home left out.
fn upstream(entry: &ServerEntry, rest: &str, query: Option<&str>) -> String {
    let query = query.map(|q| q.split('&').filter(|p| !p.starts_with("ticket=")).collect::<Vec<_>>().join("&")).filter(|q| !q.is_empty());
    match query {
        Some(q) => format!("{}/{rest}?{q}", entry.url),
        None => format!("{}/{rest}", entry.url),
    }
}

/// A request, passed on to `entry` and its answer passed back, streamed.
pub async fn request(client: &HttpClient, entry: &ServerEntry, rest: &str, request: Request<Body>) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let url = upstream(entry, rest, parts.uri.query());
    let uri: Uri = match url.parse() {
        Ok(uri) => uri,
        Err(e) => return said(StatusCode::BAD_REQUEST, format!("{url}: {e}")),
    };
    let headers = match passed_headers(&parts.headers, &entry.secret) {
        Ok(headers) => headers,
        Err(e) => return said(StatusCode::INTERNAL_SERVER_ERROR, format!("{}: {e}", entry.name)),
    };
    let mut out = Request::new(body);
    *out.method_mut() = parts.method;
    *out.uri_mut() = uri;
    *out.headers_mut() = headers;
    match client.request(out).await {
        Ok(answer) => {
            let (mut parts, body) = answer.into_parts();
            for name in &HOP_BY_HOP {
                parts.headers.remove(name);
            }
            Response::from_parts(parts, Body::new(body))
        }
        Err(e) => said(StatusCode::BAD_GATEWAY, format!("{} doesn't answer: {e}", entry.name)),
    }
}

/// A socket bridged to the same socket on `entry`, both ways, until
/// either side ends it.
pub async fn socket(client: WebSocket, entry: ServerEntry, rest: String, query: Option<String>) {
    let url = upstream(&entry, &rest, query.as_deref()).replacen("http://", "ws://", 1);
    let Ok(mut request) = url.as_str().into_client_request() else { return };
    let Ok(bearer) = format!("Bearer {}", entry.secret).parse() else { return };
    request.headers_mut().insert(header::AUTHORIZATION, bearer);
    let (server, _) = match tokio_tungstenite::connect_async(request).await {
        Ok(connected) => connected,
        Err(e) => {
            tracing::warn!("{}: its socket {rest} didn't open: {e}", entry.name);
            let mut client = client;
            let _ = client.send(AxMessage::Close(None)).await;
            return;
        }
    };
    let (mut to_server, mut from_server) = server.split();
    let (mut to_client, mut from_client) = client.split();
    let up = async {
        while let Some(Ok(message)) = from_client.next().await {
            let message = match message {
                AxMessage::Text(t) => TMessage::Text(t.as_str().into()),
                AxMessage::Binary(b) => TMessage::Binary(b),
                AxMessage::Ping(p) => TMessage::Ping(p),
                AxMessage::Pong(p) => TMessage::Pong(p),
                AxMessage::Close(_) => break,
            };
            if to_server.send(message).await.is_err() {
                break;
            }
        }
        let _ = to_server.close().await;
    };
    let down = async {
        while let Some(Ok(message)) = from_server.next().await {
            let message = match message {
                TMessage::Text(t) => AxMessage::Text(t.as_str().into()),
                TMessage::Binary(b) => AxMessage::Binary(b),
                TMessage::Ping(p) => AxMessage::Ping(p),
                TMessage::Pong(p) => AxMessage::Pong(p),
                TMessage::Close(_) => break,
                TMessage::Frame(_) => continue,
            };
            if to_client.send(message).await.is_err() {
                break;
            }
        }
        let _ = to_client.close().await;
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
}

/// The server named `name`, if home links to it.
pub fn entry(home: &Arc<super::Home>, name: &str) -> Option<ServerEntry> {
    home.links.entry(name)
}
