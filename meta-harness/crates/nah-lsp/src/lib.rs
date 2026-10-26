//! A generic language server client: one
//! server process per language, over stdio (JSON-RPC, LSP; what servers
//! send is read with lsp-types). It starts in the background and never makes
//! a caller wait for it: until the server is ready (its `$/progress` has
//! ended and, if it says it reports its status, it says it's quiescent),
//! asking it anything is refused at once. A server not ready within its time
//! limit shows as failed until it is. A crashed or hung server is started
//! again with backoff; its stderr is drained while it runs; dropping the
//! client shuts it down. Callers speak in bytes: positions are turned into
//! the server's encoding (UTF-8 when it offers it, else UTF-16) and back,
//! line by line.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How often waits look whether the client is being stopped.
const TICK: Duration = Duration::from_millis(50);

/// How to start one language server, and what to send it.
#[derive(Clone, Debug)]
pub struct ServerSpec {
    /// The server's pack, for messages.
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The project it serves (also its working folder).
    pub root: PathBuf,
    pub initialization_options: Value,
    /// Its settings: pushed after `initialized`, and the answer when it asks
    /// for a section (`workspace/configuration`).
    pub settings: Value,
    /// The requests it must answer (by method); one it doesn't offer fails it.
    pub requires: Vec<String>,
    pub ready_timeout: Duration,
    /// How long it must look ready (initialized, no progress open, quiet)
    /// before it counts as ready: a server announces its loading only after
    /// `initialized`.
    pub settle: Duration,
    pub request_timeout: Duration,
    /// Requests sent together before waiting for answers.
    pub in_flight: usize,
    /// Wait before starting a crashed server again: the first, then doubled
    /// up to the second (back to the first after a run that got ready).
    pub backoff: (Duration, Duration),
    /// On shutdown: how long it has to answer, then to exit.
    pub shutdown: Duration,
}

/// Where a server is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerState {
    Starting,
    Ready {
        version: Option<String>,
    },
    /// It ended or hung; started again after a wait.
    Restarting {
        reason: String,
    },
    /// Not ready within its time limit (it may still get there), or it
    /// can't serve (then it's stopped).
    Failed {
        reason: String,
    },
    Stopped,
}

/// A changed file, as `workspace/didChangeWatchedFiles` says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Created = 1,
    Changed = 2,
    Deleted = 3,
}

/// What a name refers to: a file and a byte range in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub file: PathBuf,
    pub start: usize,
    pub end: usize,
}

/// How a place uses what's named there, as the server says
/// (`textDocument/documentHighlight`'s kinds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// What a definition request came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Definition {
    At(Place),
    /// The server has no definition there.
    Nowhere,
    /// It named a place that can't be turned into bytes (counted).
    Unreadable,
}

impl Definition {
    #[cfg(test)]
    pub fn place(&self) -> Option<&Place> {
        match self {
            Definition::At(p) => Some(p),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LspError {
    #[error("the language server isn't ready yet")]
    NotReady,
    /// It answered with an error (it was busy, the content changed): ask again.
    #[error("the language server refused: {0}")]
    Refused(String),
    #[error("the language server ended or didn't answer: {0}")]
    Lost(String),
}

/// What the client and its threads share.
struct Shared {
    spec: ServerSpec,
    state: Mutex<State>,
    changed: Condvar,
    stop: AtomicBool,
    restarts: AtomicUsize,
    dropped: AtomicUsize,
}

struct State {
    server: ServerState,
    initialized: bool,
    open_progress: HashSet<String>,
    /// Whether it said it reports its status (then it's ready only once it
    /// says it's quiescent).
    reports_status: bool,
    quiescent: Option<bool>,
    /// Failed only for being late: it turns ready when it gets there.
    late: bool,
    /// Since when it has looked ready, without a break.
    eligible_since: Option<Instant>,
    /// This run got ready at least once.
    got_ready: bool,
    version: Option<String>,
    encoding: Option<String>,
    /// It answers `textDocument/documentHighlight`.
    highlights: bool,
    /// The files it asked to hear about (`workspace/didChangeWatchedFiles`
    /// registrations), by registration.
    watched: HashMap<String, Vec<Watch>>,
    /// What goes to the server, in order: a writer thread of its own
    /// drains it, so sending never waits on the server (it may be busy
    /// writing to us while its input is full).
    outbox: Option<Sender<Vec<u8>>>,
    pid: Option<u32>,
    next_id: i64,
    waiting: HashMap<i64, Sender<Value>>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Ready when initialized, no progress is open and, if it reports its
    /// status, it's quiescent, and it has looked so for the settle time.
    fn settle(&self, s: &mut State) {
        let quiet = if s.reports_status { s.quiescent == Some(true) } else { s.quiescent != Some(false) };
        let eligible = s.initialized && s.open_progress.is_empty() && quiet;
        s.eligible_since = match (eligible, s.eligible_since) {
            (true, Some(since)) => Some(since),
            (true, None) => Some(Instant::now()),
            (false, _) => None,
        };
        let ready = s.eligible_since.is_some_and(|since| since.elapsed() >= self.spec.settle);
        let next = match (&s.server, ready) {
            (ServerState::Starting | ServerState::Ready { .. }, true) => ServerState::Ready { version: s.version.clone() },
            (ServerState::Failed { .. }, true) if s.late => ServerState::Ready { version: s.version.clone() },
            (ServerState::Ready { .. }, false) => ServerState::Starting,
            (other, _) => other.clone(),
        };
        if matches!(next, ServerState::Ready { .. }) {
            s.late = false;
            s.got_ready = true;
        }
        if next != s.server {
            s.server = next;
            self.changed.notify_all();
        }
    }

    /// Waits up to `limit`, waking early when stopped; false if stopped.
    fn pause(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        let mut s = self.lock();
        while !self.stop.load(Ordering::SeqCst) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            s = self.changed.wait_timeout(s, left.min(TICK)).unwrap_or_else(|e| e.into_inner()).0;
        }
        false
    }
}

fn frame(msg: &Value) -> Vec<u8> {
    let body = msg.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend(body.as_bytes());
    out
}

/// Queues a message for the server (its writer thread sends it).
fn send(s: &mut State, msg: &Value) -> Result<(), LspError> {
    let outbox = s.outbox.as_ref().ok_or_else(|| LspError::Lost("no server running".into()))?;
    outbox.send(frame(msg)).map_err(|_| LspError::Lost("the server's input closed".into()))
}

/// Writes what's queued to the server's input until the queue closes or
/// the server stops reading.
fn write_messages(mut stdin: ChildStdin, queue: Receiver<Vec<u8>>) {
    for message in queue {
        if stdin.write_all(&message).and_then(|()| stdin.flush()).is_err() {
            return;
        }
    }
}

/// A request's id; its answer arrives on the returned channel.
fn request(shared: &Shared, method: &str, params: Value) -> Result<(i64, Receiver<Value>), LspError> {
    let mut s = shared.lock();
    s.next_id += 1;
    let id = s.next_id;
    let (tx, rx) = channel();
    s.waiting.insert(id, tx);
    send(&mut s, &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
    Ok((id, rx))
}

fn notify(shared: &Shared, method: &str, params: Value) -> Result<(), LspError> {
    send(&mut shared.lock(), &json!({"jsonrpc": "2.0", "method": method, "params": params}))
}

/// Waits for an answer up to `limit`, giving up early when stopped.
fn answer(shared: &Shared, rx: &Receiver<Value>, limit: Duration) -> Option<Value> {
    let deadline = Instant::now() + limit;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || shared.stop.load(Ordering::SeqCst) {
            return None;
        }
        match rx.recv_timeout(left.min(TICK)) {
            Ok(v) => return Some(v),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// The value at a dotted `section` of `settings` (None: all of them).
fn section<'a>(settings: &'a Value, section: Option<&str>) -> &'a Value {
    let Some(section) = section else { return settings };
    section.split('.').try_fold(settings, |v, key| v.get(key)).unwrap_or(&Value::Null)
}

/// Reads the server's messages until it ends.
fn read_messages(shared: &Shared, stdout: impl Read) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':')
                && k.eq_ignore_ascii_case("content-length")
            {
                length = v.trim().parse::<usize>().ok();
            }
        }
        let Some(length) = length else { return };
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(msg) = serde_json::from_slice::<Value>(&body) else { continue };
        let mut s = shared.lock();
        match (msg.get("id"), msg.get("method").and_then(Value::as_str)) {
            // The server asks for settings: each section asked, from the pack's.
            (Some(id), Some("workspace/configuration")) => {
                let items = msg["params"]["items"].as_array().cloned().unwrap_or_default();
                let answers: Vec<Value> = items.iter().map(|i| section(&shared.spec.settings, i["section"].as_str()).clone()).collect();
                let _ = send(&mut s, &json!({"jsonrpc": "2.0", "id": id, "result": answers}));
            }
            // The files it wants to hear about: kept, so changes to them
            // (a manifest, its own config) are sent too.
            (Some(id), Some("client/registerCapability")) => {
                for r in msg["params"]["registrations"].as_array().into_iter().flatten() {
                    if r["method"] == "workspace/didChangeWatchedFiles" {
                        let watches = r["registerOptions"]["watchers"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|w| Watch::of(&w["globPattern"]))
                            .collect();
                        s.watched.insert(r["id"].as_str().unwrap_or_default().to_owned(), watches);
                    }
                }
                let _ = send(&mut s, &json!({"jsonrpc": "2.0", "id": id, "result": null}));
            }
            (Some(id), Some("client/unregisterCapability")) => {
                for u in msg["params"]["unregisterations"].as_array().into_iter().flatten() {
                    s.watched.remove(u["id"].as_str().unwrap_or_default());
                }
                let _ = send(&mut s, &json!({"jsonrpc": "2.0", "id": id, "result": null}));
            }
            // Any other request from the server: answered with nothing.
            (Some(id), Some(_)) => {
                let _ = send(&mut s, &json!({"jsonrpc": "2.0", "id": id, "result": null}));
            }
            (Some(id), None) => {
                if let Some(tx) = id.as_i64().and_then(|id| s.waiting.remove(&id)) {
                    let _ = tx.send(msg);
                }
            }
            (None, Some("$/progress")) => {
                if let Ok(progress) = serde_json::from_value::<lsp_types::ProgressParams>(msg["params"].clone()) {
                    let token = match progress.token {
                        lsp_types::NumberOrString::Number(n) => n.to_string(),
                        lsp_types::NumberOrString::String(t) => t,
                    };
                    let lsp_types::ProgressParamsValue::WorkDone(work) = progress.value;
                    match work {
                        lsp_types::WorkDoneProgress::Begin(_) => {
                            s.open_progress.insert(token);
                        }
                        lsp_types::WorkDoneProgress::End(_) => {
                            s.open_progress.remove(&token);
                        }
                        lsp_types::WorkDoneProgress::Report(_) => {}
                    }
                }
                shared.settle(&mut s);
            }
            (None, Some("experimental/serverStatus")) => {
                s.quiescent = msg["params"]["quiescent"].as_bool();
                shared.settle(&mut s);
            }
            _ => {}
        }
    }
}

/// Whether the server offers what `method` asks for (`textDocument/definition`:
/// `definitionProvider`), by the protocol's naming.
fn offers(capabilities: &Value, method: &str) -> bool {
    let name = method.rsplit('/').next().unwrap_or(method);
    match capabilities.get(format!("{name}Provider")) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(_) => true,
    }
}

/// One run of the server: start, initialize, then wait until it ends.
/// Returns why it ended, and whether to start it again.
fn run_once(shared: &Arc<Shared>) -> (String, bool) {
    let spec = &shared.spec;
    let mut child: Child = match Command::new(&spec.program)
        .args(&spec.args)
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&spec.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return (format!("{} couldn't start: {e}", spec.program), true),
    };
    let (stdin, stdout, stderr) =
        (child.stdin.take().expect("piped"), child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
    let (outbox, queue) = channel();
    let writer = std::thread::spawn(move || write_messages(stdin, queue));
    {
        let mut s = shared.lock();
        s.outbox = Some(outbox);
        s.pid = Some(child.id());
        s.initialized = false;
        s.watched.clear();
        s.open_progress.clear();
        s.quiescent = None;
        s.reports_status = false;
        s.got_ready = false;
        s.eligible_since = None;
    }
    // stderr is drained so a chatty server never blocks on it.
    let drain = std::thread::spawn(move || {
        let _ = std::io::copy(&mut BufReader::new(stderr), &mut std::io::sink());
    });
    let reader = {
        let shared = shared.clone();
        std::thread::spawn(move || read_messages(&shared, stdout))
    };
    let end = |child: &mut Child, shared: &Shared| {
        let _ = child.kill();
        let _ = child.wait();
        let mut s = shared.lock();
        s.outbox = None;
        s.pid = None;
        s.initialized = false;
        s.waiting.clear();
    };
    let root = file_uri(&spec.root);
    let init = request(
        shared,
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": root,
            "workspaceFolders": [{"uri": root, "name": spec.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()}],
            "initializationOptions": spec.initialization_options,
            "capabilities": {
                "general": {"positionEncodings": ["utf-8", "utf-16"]},
                "window": {"workDoneProgress": true},
                "workspace": {"didChangeWatchedFiles": {"dynamicRegistration": true}, "configuration": true, "workspaceFolders": true},
                "textDocument": {"definition": {"linkSupport": true}, "documentHighlight": {}},
                "experimental": {"serverStatusNotification": true},
            },
        }),
    );
    let answered = init.ok().and_then(|(_, rx)| answer(shared, &rx, spec.ready_timeout));
    let result = match answered {
        Some(a) if a.get("result").is_some() => a["result"].clone(),
        Some(a) => {
            end(&mut child, shared);
            return (format!("initialize failed: {}", a["error"]), true);
        }
        None if shared.stop.load(Ordering::SeqCst) => {
            end(&mut child, shared);
            return ("stopped".into(), false);
        }
        None => {
            end(&mut child, shared);
            return (format!("didn't answer initialize within {}s", spec.ready_timeout.as_secs()), true);
        }
    };
    let capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);
    // What it must answer: one it doesn't offer, it never will.
    if let Some(missing) = spec.requires.iter().find(|m| !offers(&capabilities, m)) {
        end(&mut child, shared);
        return (format!("it doesn't offer {missing}"), false);
    }
    let parsed: Option<lsp_types::InitializeResult> = serde_json::from_value(result.clone()).ok();
    {
        let mut s = shared.lock();
        s.encoding =
            Some(parsed.as_ref().and_then(|r| r.capabilities.position_encoding.as_ref()).map_or("utf-16", |e| e.as_str()).to_owned());
        s.version = parsed.as_ref().and_then(|r| r.server_info.as_ref()).and_then(|i| i.version.clone());
        s.reports_status = capabilities.pointer("/experimental/serverStatusNotification") == Some(&Value::Bool(true));
        s.highlights = offers(&capabilities, "textDocument/documentHighlight");
    }
    let _ = notify(shared, "initialized", json!({}));
    let _ = notify(shared, "workspace/didChangeConfiguration", json!({"settings": spec.settings}));
    {
        let mut s = shared.lock();
        s.initialized = true;
        shared.settle(&mut s);
    }
    // Ready once it has looked ready for the settle time; not ready within
    // its time limit: failed, until it is.
    let readiness = {
        let shared = shared.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + shared.spec.ready_timeout;
            let mut s = shared.lock();
            loop {
                if shared.stop.load(Ordering::SeqCst) || !s.initialized {
                    return;
                }
                shared.settle(&mut s);
                if !s.got_ready && !s.late && Instant::now() >= deadline {
                    s.late = true;
                    s.server = ServerState::Failed {
                        reason: format!("not ready within {}s (still loading)", shared.spec.ready_timeout.as_secs()),
                    };
                    shared.changed.notify_all();
                }
                s = shared.changed.wait_timeout(s, TICK).unwrap_or_else(|e| e.into_inner()).0;
            }
        })
    };
    let status = child.wait();
    let _ = reader.join();
    let _ = drain.join();
    // The queue's last sender goes with the run: the writer ends.
    shared.lock().outbox = None;
    let _ = writer.join();
    {
        let mut s = shared.lock();
        s.outbox = None;
        s.pid = None;
        s.initialized = false;
        // Whoever waits for an answer gets none.
        s.waiting.clear();
        shared.changed.notify_all();
    }
    let _ = readiness.join();
    let reason = shared.lock().server.clone();
    match (reason, status) {
        (ServerState::Restarting { reason }, _) => (reason, true),
        (_, Ok(st)) => (format!("it ended ({st})"), true),
        (_, Err(e)) => (format!("it ended: {e}"), true),
    }
}

/// Starts the server, and again (after a wait) each time it ends, until
/// stopped or it can't serve.
fn supervise(shared: Arc<Shared>) {
    let mut wait = shared.spec.backoff.0;
    loop {
        let (reason, again) = run_once(&shared);
        if shared.stop.load(Ordering::SeqCst) {
            let mut s = shared.lock();
            s.server = ServerState::Stopped;
            shared.changed.notify_all();
            return;
        }
        if !again {
            tracing::warn!("language server {}: {reason}", shared.spec.name);
            let mut s = shared.lock();
            s.server = ServerState::Failed { reason };
            s.late = false;
            shared.changed.notify_all();
            return;
        }
        // A run that got ready was healthy: the wait starts over.
        if shared.lock().got_ready {
            wait = shared.spec.backoff.0;
        }
        tracing::warn!("language server {} {reason}; starting it again in {wait:?}", shared.spec.name);
        {
            let mut s = shared.lock();
            s.server = ServerState::Restarting { reason };
            shared.changed.notify_all();
        }
        if !shared.pause(wait) {
            let mut s = shared.lock();
            s.server = ServerState::Stopped;
            shared.changed.notify_all();
            return;
        }
        wait = (wait * 2).min(shared.spec.backoff.1);
        shared.restarts.fetch_add(1, Ordering::SeqCst);
        let mut s = shared.lock();
        s.server = ServerState::Starting;
    }
}

/// A running language server.
pub struct Client {
    shared: Arc<Shared>,
    supervisor: Option<JoinHandle<()>>,
}

impl Client {
    /// Starts `spec`'s server in the background; returns at once.
    pub fn start(spec: ServerSpec) -> Self {
        let shared = Arc::new(Shared {
            spec,
            state: Mutex::new(State {
                server: ServerState::Starting,
                initialized: false,
                open_progress: HashSet::new(),
                reports_status: false,
                quiescent: None,
                late: false,
                eligible_since: None,
                got_ready: false,
                version: None,
                encoding: None,
                highlights: false,
                watched: HashMap::new(),
                outbox: None,
                pid: None,
                next_id: 0,
                waiting: HashMap::new(),
            }),
            changed: Condvar::new(),
            stop: AtomicBool::new(false),
            restarts: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
        });
        let supervisor = {
            let shared = shared.clone();
            std::thread::Builder::new().name(format!("lsp-{}", shared.spec.name)).spawn(move || supervise(shared)).ok()
        };
        Self { shared, supervisor }
    }

    pub fn state(&self) -> ServerState {
        self.shared.lock().server.clone()
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.state(), ServerState::Ready { .. })
    }

    /// Waits up to `timeout` for the server to be ready.
    #[cfg(test)]
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut s = self.shared.lock();
        loop {
            if matches!(s.server, ServerState::Ready { .. }) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            s = self.shared.changed.wait_timeout(s, left.min(TICK)).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// The position encoding the server chose.
    pub fn encoding(&self) -> Option<String> {
        self.shared.lock().encoding.clone()
    }

    /// Times the server was started again after ending.
    #[cfg(test)]
    pub fn restarts(&self) -> usize {
        self.shared.restarts.load(Ordering::SeqCst)
    }

    /// Positions the server gave that couldn't be turned into bytes.
    pub fn dropped_positions(&self) -> usize {
        self.shared.dropped.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub fn pid(&self) -> Option<u32> {
        self.shared.lock().pid
    }

    /// Whether the server asked to hear of changes to `path` (beyond its
    /// language's own files).
    pub fn watches(&self, path: &Path) -> bool {
        self.shared.lock().watched.values().flatten().any(|w| w.matches(path))
    }

    /// Tells the server these files changed on disk (once it's initialized:
    /// before that, it reads them as it loads).
    pub fn changed(&self, files: &[(PathBuf, Change)]) {
        if files.is_empty() || !self.shared.lock().initialized {
            return;
        }
        let changes: Vec<Value> = files.iter().map(|(p, c)| json!({"uri": file_uri(p), "type": *c as u8})).collect();
        let _ = notify(&self.shared, "workspace/didChangeWatchedFiles", json!({"changes": changes}));
    }

    /// A hung server: its waiting requests are dropped and it's ended, to
    /// be started again.
    fn hung(&self, ids: &[i64], why: &str) {
        let pid = {
            let mut s = self.shared.lock();
            for id in ids {
                s.waiting.remove(id);
            }
            s.server = ServerState::Restarting { reason: why.to_owned() };
            self.shared.changed.notify_all();
            s.pid
        };
        if let Some(pid) = pid {
            kill(pid);
        }
    }

    /// What each name at byte offsets `at` in `file` (whose text is `text`)
    /// refers to, asked together in batches of `in_flight`. Refused at once
    /// when the server isn't ready; an error answer refuses the batch (ask
    /// again later); no answer in time ends the server, to be started again.
    pub fn definitions(&self, file: &Path, text: &str, at: &[usize]) -> Result<Vec<Definition>, LspError> {
        let encoding = self.encoding().unwrap_or_else(|| "utf-16".into());
        let mut texts: HashMap<PathBuf, Option<String>> = HashMap::new();
        texts.insert(file.to_path_buf(), Some(text.to_owned()));
        self.ask_each("textDocument/definition", file, text, at, |result| self.place(result, &encoding, &mut texts))
    }

    /// Whether the server says how each place uses what's named there
    /// (`textDocument/documentHighlight`).
    pub fn answers_highlights(&self) -> bool {
        self.shared.lock().highlights
    }

    /// How each place in `file` (bytes into `text`) uses what's named
    /// there: read, written, or the server doesn't say.
    pub fn accesses(&self, file: &Path, text: &str, at: &[usize]) -> Result<Vec<Option<Access>>, LspError> {
        let encoding = self.encoding().unwrap_or_else(|| "utf-16".into());
        let lines = Lines::new(text);
        let mut asked = at.iter();
        let dropped = &self.shared.dropped;
        self.ask_each("textDocument/documentHighlight", file, text, at, |result| {
            let offset = asked.next().copied().unwrap_or_default();
            // An answer that can't be read, and places that can't be
            // converted, are counted (never silently dropped).
            let highlights: Vec<lsp_types::DocumentHighlight> =
                match serde_json::from_value::<Option<Vec<lsp_types::DocumentHighlight>>>(result.clone()) {
                    Ok(h) => h.unwrap_or_default(),
                    Err(_) => {
                        dropped.fetch_add(1, Ordering::SeqCst);
                        Vec::new()
                    }
                };
            let byte = |p: lsp_types::Position| lines.byte(text, p.line as usize, p.character as usize, &encoding);
            let readable: Vec<(usize, usize, Option<lsp_types::DocumentHighlightKind>)> = highlights
                .into_iter()
                .filter_map(|h| match (byte(h.range.start), byte(h.range.end)) {
                    (Some(start), Some(end)) => Some((start, end, h.kind)),
                    _ => {
                        dropped.fetch_add(1, Ordering::SeqCst);
                        None
                    }
                })
                .collect();
            readable.into_iter().find(|(start, end, _)| *start <= offset && offset < *end).and_then(|(_, _, kind)| match kind {
                Some(lsp_types::DocumentHighlightKind::READ) => Some(Access::Read),
                Some(lsp_types::DocumentHighlightKind::WRITE) => Some(Access::Write),
                _ => None,
            })
        })
    }

    /// Asks `method` at each place in `file`, a few at a time, and reads
    /// each answer with `read`.
    fn ask_each<T>(
        &self,
        method: &str,
        file: &Path,
        text: &str,
        at: &[usize],
        mut read: impl FnMut(&Value) -> T,
    ) -> Result<Vec<T>, LspError> {
        if !self.is_ready() {
            return Err(LspError::NotReady);
        }
        let encoding = self.encoding().unwrap_or_else(|| "utf-16".into());
        let lines = Lines::new(text);
        let uri = file_uri(file);
        let mut out = Vec::with_capacity(at.len());
        for chunk in at.chunks(self.shared.spec.in_flight.max(1)) {
            let mut asked = Vec::with_capacity(chunk.len());
            for &offset in chunk {
                let position = lines.position(text, offset, &encoding);
                asked.push(request(&self.shared, method, json!({"textDocument": {"uri": uri}, "position": position}))?);
            }
            let deadline = Instant::now() + self.shared.spec.request_timeout;
            let mut refused = None;
            for (i, (_, rx)) in asked.iter().enumerate() {
                let left = deadline.saturating_duration_since(Instant::now());
                let Some(answer) = answer(&self.shared, rx, left) else {
                    let ids: Vec<i64> = asked[i..].iter().map(|(id, _)| *id).collect();
                    let why = format!("{} didn't answer within {}s", self.shared.spec.name, self.shared.spec.request_timeout.as_secs());
                    self.hung(&ids, &why);
                    return Err(LspError::Lost(why));
                };
                if let Some(error) = answer.get("error") {
                    refused.get_or_insert_with(|| error.to_string());
                    continue;
                }
                out.push(read(&answer["result"]));
            }
            if let Some(why) = refused {
                return Err(LspError::Refused(why));
            }
        }
        Ok(out)
    }

    /// The first place a definition answer names, in bytes.
    fn place(&self, result: &Value, encoding: &str, texts: &mut HashMap<PathBuf, Option<String>>) -> Definition {
        let Ok(response) = serde_json::from_value::<Option<lsp_types::GotoDefinitionResponse>>(result.clone()) else {
            return self.unreadable();
        };
        let first = match response {
            None => None,
            Some(lsp_types::GotoDefinitionResponse::Scalar(l)) => Some((l.uri, l.range)),
            Some(lsp_types::GotoDefinitionResponse::Array(list)) => list.into_iter().next().map(|l| (l.uri, l.range)),
            Some(lsp_types::GotoDefinitionResponse::Link(list)) => {
                list.into_iter().next().map(|l| (l.target_uri, l.target_selection_range))
            }
        };
        let Some((uri, range)) = first else { return Definition::Nowhere };
        let Some(path) = path_of(uri.as_str()) else { return self.unreadable() };
        let Some(text) = texts.entry(path.clone()).or_insert_with(|| std::fs::read_to_string(&path).ok()).as_deref() else {
            return self.unreadable();
        };
        let lines = Lines::new(text);
        let at = |p: lsp_types::Position| lines.byte(text, p.line as usize, p.character as usize, encoding);
        match (at(range.start), at(range.end)) {
            (Some(start), Some(end)) => Definition::At(Place { file: path, start, end }),
            _ => self.unreadable(),
        }
    }

    fn unreadable(&self) -> Definition {
        self.shared.dropped.fetch_add(1, Ordering::SeqCst);
        Definition::Unreadable
    }
}

/// Ends process `pid` at once.
fn kill(pid: u32) {
    // SAFETY: a plain signal to a process this client started.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

impl Drop for Client {
    /// Asks a running server to shut down, then ends it; a server still
    /// starting is ended at once.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.changed.notify_all();
        let initialized = self.shared.lock().initialized;
        if initialized && let Ok((_, rx)) = request(&self.shared, "shutdown", Value::Null) {
            let _ = rx.recv_timeout(self.shared.spec.shutdown);
            let _ = notify(&self.shared, "exit", Value::Null);
            let deadline = Instant::now() + self.shared.spec.shutdown;
            while self.shared.lock().pid.is_some() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if let Some(pid) = self.shared.lock().pid {
            kill(pid);
        }
        if let Some(t) = self.supervisor.take() {
            let _ = t.join();
        }
    }
}

/// Byte offsets of line starts.
struct Lines(Vec<usize>);

impl Lines {
    fn new(text: &str) -> Self {
        Self(std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect())
    }

    /// The LSP position of byte `offset`.
    fn position(&self, text: &str, offset: usize, encoding: &str) -> Value {
        let line = self.0.partition_point(|&s| s <= offset).saturating_sub(1);
        let start = self.0[line];
        let before = text.get(start..offset).unwrap_or("");
        let character = if encoding == "utf-8" { before.len() } else { before.encode_utf16().count() };
        json!({"line": line, "character": character})
    }

    /// The byte offset of `character` (in `encoding`) on `line`.
    fn byte(&self, text: &str, line: usize, character: usize, encoding: &str) -> Option<usize> {
        let start = *self.0.get(line)?;
        let end = self.0.get(line + 1).copied().unwrap_or(text.len());
        let line_text = &text[start..end];
        let offset = if encoding == "utf-8" {
            (character <= line_text.len() && line_text.is_char_boundary(character)).then_some(character)?
        } else {
            let mut units = 0;
            let mut found = None;
            for (i, c) in line_text.char_indices() {
                if units == character {
                    found = Some(i);
                    break;
                }
                units += c.len_utf16();
            }
            found.or((units == character).then_some(line_text.len()))?
        };
        Some(start + offset)
    }
}

/// One file pattern a server watches: a glob, against the whole path or
/// under a base folder.
#[derive(Debug)]
struct Watch {
    base: Option<PathBuf>,
    glob: globset::GlobMatcher,
}

impl Watch {
    /// A `GlobPattern`: a string, or a base folder and a pattern under it.
    fn of(pattern: &Value) -> Option<Self> {
        let (base, text) = match pattern {
            Value::String(text) => (None, text.as_str()),
            Value::Object(o) => {
                let base = o.get("baseUri").and_then(|b| b.as_str().or_else(|| b["uri"].as_str())).and_then(path_of)?;
                (Some(base), o.get("pattern")?.as_str()?)
            }
            _ => return None,
        };
        let glob = globset::GlobBuilder::new(text).literal_separator(true).build().ok()?.compile_matcher();
        Some(Self { base, glob })
    }

    fn matches(&self, path: &Path) -> bool {
        match &self.base {
            Some(base) => path.strip_prefix(base).is_ok_and(|rel| self.glob.is_match(rel)),
            None => self.glob.is_match(path),
        }
    }
}

/// `file://` and the path, percent-encoded where it must be.
fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

#[cfg(test)]
mod client_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_and_uris_round_trip() {
        let text = "fn a() {}\nlet é = b();\n";
        let lines = Lines::new(text);
        let b = text.find("b()").unwrap();
        for encoding in ["utf-8", "utf-16"] {
            let p = lines.position(text, b, encoding);
            assert_eq!(
                lines.byte(text, p["line"].as_u64().unwrap() as usize, p["character"].as_u64().unwrap() as usize, encoding),
                Some(b)
            );
        }
        let path = Path::new("/tmp/a dir/é.rs");
        assert_eq!(path_of(&file_uri(path)).unwrap(), path);
    }

    #[test]
    fn the_files_a_server_registers_to_watch_are_matched_as_it_asks() {
        // rust-analyzer's own registrations: globs over the whole path, and
        // a pattern under a base folder.
        let manifest = Watch::of(&json!("**/Cargo.{toml,lock}")).unwrap();
        assert!(manifest.matches(Path::new("/repo/Cargo.toml")) && manifest.matches(Path::new("/repo/crates/a/Cargo.lock")));
        assert!(!manifest.matches(Path::new("/repo/src/lib.rs")));
        let sources = Watch::of(&json!({"baseUri": file_uri(Path::new("/repo")), "pattern": "**/*.rs"})).unwrap();
        assert!(sources.matches(Path::new("/repo/src/a/b.rs")) && !sources.matches(Path::new("/elsewhere/b.rs")));
        assert!(Watch::of(&json!(7)).is_none());
    }
}
