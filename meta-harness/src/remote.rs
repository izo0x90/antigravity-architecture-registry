//! Agents driving the cockpit: the open cockpit connects here while the
//! user lets agents drive it, and the MCP cockpit tools' calls go to it and
//! back. Only the cockpit can connect: the socket takes a one-time ticket
//! that only the cockpit's session gets (proxies may drop cookies from a
//! socket's request, so the cookie is checked on the ticket's). The calls
//! themselves are the cockpit's (its `remote` module): read the screen, list
//! and run actions, click, type, press keys. The newest cockpit to connect
//! is the one driven.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::cockpit::{Needs, can};

/// How long a call waits for the cockpit.
const ANSWER_WITHIN: Duration = Duration::from_secs(20);
/// How long a ticket stays good.
const TICKET_FOR: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct Remote {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The cockpit driven now: its connection number, and where calls go.
    link: Option<(u64, mpsc::UnboundedSender<String>)>,
    next: u64,
    /// Calls waiting for answers, with the connection they went to.
    waiting: HashMap<u64, (u64, oneshot::Sender<Result<Value, String>>)>,
    /// Tickets given out and not used, with when they were.
    tickets: HashMap<String, std::time::Instant>,
}

#[derive(Deserialize)]
struct Answer {
    id: u64,
    #[serde(default)]
    ok: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

impl Remote {
    /// A ticket to connect with (once, within a minute).
    pub fn ticket(&self) -> String {
        let ticket = uuid::Uuid::new_v4().simple().to_string();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.tickets.retain(|_, at| at.elapsed() < TICKET_FOR);
        inner.tickets.insert(ticket.clone(), std::time::Instant::now());
        ticket
    }

    /// Uses a ticket up: whether it was good.
    fn admits(&self, ticket: &str) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.tickets.remove(ticket).is_some_and(|at| at.elapsed() < TICKET_FOR)
    }

    /// Sends a call (`{"op": ..., ...}`) to the cockpit; its answer.
    pub async fn call(&self, call: Value) -> Result<Value, String> {
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let Some((link, to)) = inner.link.clone() else {
                return Err("no cockpit is letting agents drive it: the user turns it on in the cockpit (Space a d, \"Let agents drive the cockpit\")".into());
            };
            inner.next += 1;
            let id = inner.next;
            if to.send(json!({ "id": id, "call": call }).to_string()).is_err() {
                return Err("the cockpit just went away".into());
            }
            inner.waiting.insert(id, (link, tx));
            id
        };
        match tokio::time::timeout(ANSWER_WITHIN, rx).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err("the cockpit went away before answering".into()),
            Err(_) => {
                self.inner.lock().unwrap_or_else(|e| e.into_inner()).waiting.remove(&id);
                Err(format!("the cockpit didn't answer within {}s (is its tab in the background?)", ANSWER_WITHIN.as_secs()))
            }
        }
    }

    /// A cockpit connected: it's the one driven until it goes (or a newer one comes).
    async fn serve(&self, socket: WebSocket) {
        let (mut sink, mut incoming) = socket.split();
        let (tx, mut outgoing) = mpsc::unbounded_channel::<String>();
        let link = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.next += 1;
            let link = inner.next;
            inner.link = Some((link, tx));
            link
        };
        loop {
            tokio::select! {
                call = outgoing.recv() => match call {
                    Some(text) => if sink.send(Message::Text(text.into())).await.is_err() { break },
                    None => break,
                },
                message = incoming.next() => match message {
                    Some(Ok(Message::Text(text))) => self.answered(&text),
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                },
            }
        }
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.link.as_ref().is_some_and(|(l, _)| *l == link) {
            inner.link = None;
        }
        // What went to it won't be answered now.
        inner.waiting.retain(|_, (to, _)| *to != link);
    }

    fn answered(&self, text: &str) {
        let Ok(answer) = serde_json::from_str::<Answer>(text) else {
            tracing::warn!("cockpit socket: not an answer: {text}");
            return;
        };
        let waiting = self.inner.lock().unwrap_or_else(|e| e.into_inner()).waiting.remove(&answer.id);
        if let Some((_, tx)) = waiting {
            let _ = tx.send(match (answer.ok, answer.error) {
                (_, Some(error)) => Err(error),
                (ok, None) => Ok(ok.unwrap_or(Value::Null)),
            });
        }
    }
}

/// `POST /api/cockpit/ticket`: a ticket for the cockpit's socket (the cockpit's session only).
pub async fn ticket(_: Needs<can::SessionsControl>, State(remote): State<Arc<Remote>>) -> Json<Value> {
    Json(json!({ "ticket": remote.ticket() }))
}

#[derive(Deserialize)]
pub struct Ticket {
    #[serde(default)]
    ticket: String,
}

/// `GET /ws/cockpit?ticket=`: the cockpit's side.
pub async fn socket(Query(q): Query<Ticket>, ws: WebSocketUpgrade, State(remote): State<Arc<Remote>>) -> axum::response::Response {
    if !remote.admits(&q.ticket) {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "only the cockpit connects here (with a ticket from /api/cockpit/ticket)" }))).into_response();
    }
    ws.on_upgrade(move |socket| async move { remote.serve(socket).await }).into_response()
}

/// A read as text: the mode, the columns, then what's drawn, row by row
/// (top to bottom, left to right), each piece with where it is.
pub fn screen_text(read: &Value) -> String {
    let mut out = format!("mode: {}", read["mode"].as_str().unwrap_or("?"));
    if let Some(leader) = read["leader"].as_str() {
        out.push_str(&format!(" (leader keys so far: {leader:?})"));
    }
    if read["palette"] == true {
        out.push_str(", palette open");
    }
    if let Some(note) = read["note"].as_str() {
        out.push_str(&format!("\nnote: {note}"));
    }
    for column in read["columns"].as_array().into_iter().flatten() {
        let focus = if column["focused"] == true { " (focused)" } else { "" };
        out.push_str(&format!("\ncolumn {}: {}{focus}", column["column"], column["view"].as_str().unwrap_or("?")));
    }
    out.push_str("\n\nOn screen (y: text @x ...):");
    let mut texts: Vec<(f64, f64, &str)> = read["texts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| (t["y"].as_f64().unwrap_or(0.0), t["x"].as_f64().unwrap_or(0.0), t["text"].as_str().unwrap_or("")))
        .collect();
    texts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    // Text within a few points vertically is one row.
    let mut row_y = f64::NEG_INFINITY;
    for (y, x, text) in texts {
        if y - row_y > 4.0 {
            row_y = y;
            out.push_str(&format!("\n{y:>5}:"));
        }
        out.push_str(&format!(" {} @{x}", text.replace('\n', " ⏎ ")));
    }
    out
}

/// The actions as text: one a line, with keys, what it needs, and whether it applies now.
pub fn actions_text(actions: &Value) -> String {
    let mut out = String::from("Run one with cockpit_run (id, and value if it needs one). Not available now: marked (n/a).\n");
    for a in actions.as_array().into_iter().flatten() {
        let keys = a["keys"].as_str().filter(|k| !k.is_empty()).map(|k| format!(" [{k}]")).unwrap_or_default();
        let na = if a["available"] == false { " (n/a)" } else { "" };
        out.push_str(&format!("\n{} — {}{keys}{na}", a["id"].as_str().unwrap_or("?"), a["label"].as_str().unwrap_or("")));
        match a["needs"].as_str() {
            Some("nothing") | None => {}
            Some(needs) => {
                out.push_str(&format!(" · needs {needs}"));
                let choices: Vec<String> = a["choices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| {
                        let current = if c["current"] == true { "*" } else { "" };
                        format!("{}{current} ({})", c["value"].as_str().unwrap_or(""), c["label"].as_str().unwrap_or(""))
                    })
                    .collect();
                if !choices.is_empty() {
                    out.push_str(&format!(": {}", choices.join(", ")));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_is_rows_top_to_bottom() {
        let read = json!({
            "mode": "Normal", "note": null, "leader": null, "palette": false,
            "columns": [{ "column": 1, "view": "chat · the agent's session", "focused": true }],
            "texts": [
                { "text": "Send", "x": 500.0, "y": 1200.0 },
                { "text": "Harness:", "x": 17.0, "y": 104.0 },
                { "text": "Antigravity", "x": 71.0, "y": 106.0 },
            ]
        });
        let text = screen_text(&read);
        assert!(text.contains("column 1: chat · the agent's session (focused)"), "{text}");
        assert!(text.contains("  104: Harness: @17 Antigravity @71\n 1200: Send @500"), "{text}");
    }

    #[test]
    fn a_ticket_is_good_once() {
        let remote = Remote::default();
        let ticket = remote.ticket();
        assert!(remote.admits(&ticket));
        assert!(!remote.admits(&ticket), "used up");
        assert!(!remote.admits("made-up"));
    }

    #[tokio::test]
    async fn without_a_cockpit_letting_agents_drive_a_call_says_how_to_allow_it() {
        let err = Remote::default().call(json!({ "op": "read" })).await.unwrap_err();
        assert!(err.contains("Let agents drive the cockpit"), "{err}");
    }
}
