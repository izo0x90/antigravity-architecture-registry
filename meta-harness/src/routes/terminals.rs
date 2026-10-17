//! Terminals, for the cockpit: list them, open one, start one's shell
//! again, close one; and each one's socket, where its screen goes out and
//! what's typed comes in, in the order typed. All of it the user's (agents
//! read and type through their tools, as their role allows).

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, State, ws::Message, ws::WebSocket, ws::WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use futures::{SinkExt, StreamExt};
use harness_contracts::{FromTerminal, ReaderScreen, Row, Terminal, TerminalId, ToTerminal};

use crate::cockpit::{Needs, Ticketed, can};
use crate::modules::TerminalsState;
use crate::terminals::{Open, TerminalError, Terminals};

pub(crate) fn routes() -> Router<TerminalsState> {
    Router::new()
        .route("/api/terminals", get(list).post(open))
        .route("/api/terminals/{id}", delete(close))
        .route("/api/terminals/{id}/restart", post(restart))
        .route("/ws/terminals/{id}", get(terminal_socket))
}

/// At most this often, a reader gets what changed (changes in between are
/// sent together).
const FRAME: Duration = Duration::from_millis(16);

impl IntoResponse for TerminalError {
    fn into_response(self) -> Response {
        let status = match self {
            TerminalError::NotFound(_) => StatusCode::NOT_FOUND,
            TerminalError::Ended(_) => StatusCode::CONFLICT,
            TerminalError::Start { .. } | TerminalError::Io(_) | TerminalError::Store(_) | TerminalError::Dropped(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(serde_json::json!({ "error": self.to_string() }))).into_response()
    }
}

async fn list(_: Needs<can::ViewRead>, State(state): State<TerminalsState>) -> Json<Vec<Terminal>> {
    Json(state.terminals.list())
}

async fn open(_: Needs<can::ShellDrive>, State(state): State<TerminalsState>) -> Result<Json<Terminal>, TerminalError> {
    let terminals = state.terminals.clone();
    blocking(move || terminals.open()).await.map(Json)
}

async fn restart(_: Needs<can::ShellDrive>, State(state): State<TerminalsState>, Path(id): Path<String>) -> Result<Json<Terminal>, TerminalError> {
    let terminals = state.terminals.clone();
    blocking(move || terminals.restart(&TerminalId::new(id))).await.map(Json)
}

async fn close(_: Needs<can::ShellDrive>, State(state): State<TerminalsState>, Path(id): Path<String>) -> Result<StatusCode, TerminalError> {
    let terminals = state.terminals.clone();
    blocking(move || terminals.close(&TerminalId::new(id))).await.map(|()| StatusCode::NO_CONTENT)
}

/// Starting and ending shells, and writing to them, can block: off the
/// async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, TerminalError> + Send + 'static) -> Result<T, TerminalError> {
    tokio::task::spawn_blocking(f).await?
}

async fn terminal_socket(_: Ticketed, ws: WebSocketUpgrade, State(state): State<TerminalsState>, Path(id): Path<String>) -> Response {
    match state.terminals.get(&TerminalId::new(id)) {
        Ok(open) => {
            let terminals = state.terminals.clone();
            ws.on_upgrade(move |socket| async move { state.stopping.or_stop(serve(socket, terminals, open)).await }).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// One reader: the whole screen first, then what changed (at most every
/// frame); what it sends is done in the order sent. Reading the terminal
/// takes its lock (while its output is fed): off the async threads.
async fn serve(socket: WebSocket, terminals: Arc<Terminals>, open: Arc<Open>) {
    let (mut sink, mut incoming) = socket.split();
    let viewer = open.viewer();
    let mut changed = open.watch();
    let Ok(first) = read(&open, |o| o.screen(None)).await else { return };
    let (mut seen, mut title, mut started) = (first.seq, first.title.clone(), first.shell);
    // What the reader has of the screen: rows that scroll off are sent
    // again only when they changed after it was sent them.
    let mut has = ReaderScreen::default();
    has.take(&first);
    if send(&mut sink, &FromTerminal::Screen { update: first }).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            woke = changed.changed() => {
                if woke.is_err() {
                    break;
                }
                tokio::time::sleep(FRAME).await;
                changed.borrow_and_update();
                // A new start of its shell: its screen whole, numbered anew
                // (seen by what's read: the start it says is the one read).
                let known = started;
                let Ok(update) = read(&open, move |o| {
                    let update = o.screen(Some(seen));
                    if update.shell == known { update } else { o.screen(None) }
                })
                .await
                else {
                    break;
                };
                started = update.shell;
                if update.full || update.seq != seen || update.title != title {
                    (seen, title) = (update.seq, update.title.clone());
                    let (from, gone) = has.take(&update);
                    if let Some(changed) = changed_since(&open, from, gone).await
                        && send(&mut sink, &changed).await.is_err()
                    {
                        break;
                    }
                    if send(&mut sink, &FromTerminal::Screen { update }).await.is_err() {
                        break;
                    }
                }
            }
            message = incoming.next() => {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    Some(Ok(_)) => continue,
                };
                let Ok(said) = serde_json::from_str::<ToTerminal>(&text) else {
                    tracing::warn!("A terminal's reader sent something unreadable: {text}");
                    continue;
                };
                if let Some(answer) = said_to(&terminals, &open, viewer, said).await
                    && send(&mut sink, &answer).await.is_err()
                {
                    break;
                }
            }
        }
    }
    terminals.shown(&open, viewer, None);
}

/// `f` on the terminal, on a thread that may block.
async fn read<T: Send + 'static>(open: &Arc<Open>, f: impl FnOnce(&Arc<Open>) -> T + Send + 'static) -> Result<T, tokio::task::JoinError> {
    let open = open.clone();
    tokio::task::spawn_blocking(move || f(&open)).await
}

/// Of the rows that scrolled off the reader's screen (`gone`, from line
/// `from`, as it had them), those that changed before they went.
async fn changed_since(open: &Arc<Open>, from: u64, gone: Vec<Row>) -> Option<FromTerminal> {
    if gone.is_empty() {
        return None;
    }
    let count = gone.len() as u32;
    let (start, now) = read(open, move |o| o.scrollback(from, count)).await.ok()?;
    let lines: Vec<(u64, Row)> = (start..).zip(now).filter(|(n, row)| n.checked_sub(from).and_then(|i| gone.get(i as usize)) != Some(row)).collect();
    (!lines.is_empty()).then_some(FromTerminal::ScrolledOff { lines })
}

/// Does what a reader sent; what it answers, if anything.
async fn said_to(terminals: &Arc<Terminals>, open: &Arc<Open>, viewer: u64, said: ToTerminal) -> Option<FromTerminal> {
    match said {
        ToTerminal::Input { input } => {
            // Awaited before the next is read: typed in the order sent.
            if let Ok(Err(e)) = read(open, move |o| o.input(&input)).await {
                tracing::debug!("Typing into a terminal: {e}");
            }
            None
        }
        ToTerminal::Size { size } => {
            let terminals = terminals.clone();
            let _ = read(open, move |o| terminals.shown(o, viewer, Some(size))).await;
            None
        }
        ToTerminal::Lines { from, count } => {
            let (from, rows) = read(open, move |o| o.scrollback(from, count.min(1000))).await.ok()?;
            Some(FromTerminal::Lines { from, rows })
        }
        ToTerminal::Search { query } => {
            let asked = query.clone();
            let found = read(open, move |o| o.search(&asked)).await.ok()?;
            Some(FromTerminal::Found { query, found })
        }
        ToTerminal::CommandOutput { command } => {
            let text = read(open, move |o| o.command_output(command, u64::MAX)).await.ok()?.map(|(text, _)| text);
            Some(FromTerminal::CommandOutput { command, text })
        }
    }
}

async fn send(sink: &mut futures::stream::SplitSink<WebSocket, Message>, message: &FromTerminal) -> Result<(), axum::Error> {
    let text = serde_json::to_string(message).map_err(axum::Error::new)?;
    sink.send(Message::Text(text.into())).await
}
