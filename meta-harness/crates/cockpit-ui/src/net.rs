use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ewebsock::{WsEvent, WsMessage, WsSender};
use harness_contracts::{
    ClientEvent, HomeEvent, InstanceId, ModelSelection, NativeId, NotificationId, RequestId, RequestResponse, RuntimePolicy, SavedSessionPage, SessionKey,
    SlotId, SteerInput, ThreadId,
};
use harness_contracts::home::HERE;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Event WebSocket to the meta-harness server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsState {
    Connecting,
    Open,
    Closed,
}

/// How to open a session in a slot: its harness and settings (set first),
/// and which session: a new one, a saved one, or one of the slot's own.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSlotSession {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<InstanceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<RuntimePolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<NativeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionKey>,
}

/// A session opened in a slot: its thread, and what to tell the user.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotOpened {
    pub thread_id: ThreadId,
    pub session: SessionKey,
    pub note: Option<String>,
    pub delivered_reports: usize,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SlotCreated {
    slot_id: SlotId,
}

#[derive(serde::Deserialize)]
struct UiConfigFile {
    text: Option<String>,
}

/// A sign-in started: what to do, the page to open (if any), and what to
/// paste back (if anything).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthChallenge {
    pub url: Option<String>,
    pub instructions: String,
    pub expects: Option<String>,
}

pub type NetResult<T> = Result<T, NetError>;

/// Why an answer couldn't be had: what the server said (or the network),
/// and the HTTP status when there was one.
#[derive(Clone, Debug, PartialEq)]
pub struct NetError {
    pub status: Option<u16>,
    pub message: String,
}

impl NetError {
    /// The network, or an answer that couldn't be read: no status.
    pub fn other(message: impl Into<String>) -> Self {
        Self { status: None, message: message.into() }
    }

    /// The server said it has no such thing (404).
    pub fn not_found(&self) -> bool {
        self.status == Some(404)
    }
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<NetError> for String {
    fn from(e: NetError) -> Self {
        e.message
    }
}

/// What sees each server event's text as it arrives.
type Arrival = Arc<dyn Fn(&str) + Send + Sync>;

pub struct NetClient {
    base_url: String,
    /// Before each path: `""` for home's own server, `/s/<name>` for the
    /// others (home passes those on).
    prefix: String,
    ws_url: String,
    ws_sender: Option<WsSender>,
    /// What the socket said, by which socket (one dropped may still speak).
    ws_events: Arc<Mutex<Vec<(u64, WsEvent)>>>,
    /// The socket opened last.
    ws_generation: u64,
    /// Sees each server event's text as it arrives, even while no frame
    /// runs (a hidden browser tab).
    on_arrival: Option<Arrival>,
    /// This server's events, as home's socket brought them, since the last drain.
    pub incoming: Arc<Mutex<Vec<ClientEvent>>>,
    /// What home's socket said (every server's events, and the servers),
    /// since the last drain: only the client that holds the socket.
    pub from_home: Arc<Mutex<Vec<HomeEvent>>>,
    pub ws_state: Arc<Mutex<WsState>>,
    /// Bumped on every HTTP request/response and WebSocket message (drives the activity LED).
    pub activity: Arc<AtomicU64>,
    pub last_error: Arc<Mutex<Option<String>>>,
    /// Reconnect delays (seconds): first try, and the cap it doubles up to.
    backoff: (f64, f64),
    /// When the next reconnect is due (app time, seconds), and the delay used.
    retry: Option<(f64, f64)>,
    /// The socket dropped once already: the next open is a reconnect.
    dropped: bool,
    /// Set when a dropped socket came back: views should refetch (events
    /// were missed meanwhile). Taken by the app.
    pub reconnected: bool,
    /// The event socket opens with a one-time ticket, asked for first.
    events_door: TicketDoor,
    /// Every call made (method and path), for tests to check.
    #[cfg(test)]
    pub sent: Arc<Mutex<Vec<String>>>,
}

/// A socket opened with a one-time ticket (the server checks the cockpit's
/// session on the ticket's request: proxies may drop cookies from a
/// socket's). Asks for one, then hands it over once.
#[derive(Default)]
pub struct TicketDoor {
    inbox: Arc<Mutex<Option<NetResult<String>>>>,
    asking: bool,
}

impl TicketDoor {
    /// Starts asking (unless it already is): where to hand the answer.
    pub fn start(&mut self) -> Option<impl FnOnce(NetResult<String>) + Send + 'static> {
        if self.asking {
            return None;
        }
        self.asking = true;
        let inbox = self.inbox.clone();
        Some(move |r| {
            if let Ok(mut slot) = inbox.lock() {
                *slot = Some(r);
            }
        })
    }

    /// The ticket (or why there's none), once it has come.
    pub fn take(&mut self) -> Option<NetResult<String>> {
        let answer = self.inbox.lock().ok().and_then(|mut slot| slot.take());
        if answer.is_some() {
            self.asking = false;
        }
        answer
    }

    pub fn asking(&self) -> bool {
        self.asking
    }
}

/// Error text from a non-2xx response: the server's `{"error": ...}` or the status.
/// (name, value) pairs as a query string.
fn query(pairs: Vec<(&'static str, String)>) -> String {
    pairs.iter().map(|(k, v)| format!("{k}={}", urlencoding::encode(v))).collect::<Vec<_>>().join("&")
}

/// A source as a query parameter's value.
fn source_param(source: &arch_graph::Source) -> String {
    urlencoding::encode(&String::from(source.clone())).into_owned()
}

fn http_error(response: &ehttp::Response) -> NetError {
    let server_message = serde_json::from_slice::<serde_json::Value>(&response.bytes)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned));
    let message = match server_message {
        Some(message) => message,
        None => format!("HTTP {}", response.status),
    };
    NetError { status: Some(response.status), message }
}

impl NetClient {
    /// Home's own server (the one serving the cockpit).
    pub fn new(host_origin: &str) -> Self {
        Self::for_server(host_origin, HERE)
    }

    /// Server `name`, through home at `host_origin`.
    pub fn for_server(host_origin: &str, name: &str) -> Self {
        let base_url = host_origin.trim_end_matches('/').to_string();
        let ws_url = if base_url.starts_with("https://") {
            base_url.replacen("https://", "wss://", 1) + "/ws/home"
        } else {
            base_url.replacen("http://", "ws://", 1) + "/ws/home"
        };
        let prefix = if name == HERE { String::new() } else { format!("/s/{name}") };

        Self {
            base_url,
            prefix,
            ws_url,
            ws_sender: None,
            ws_events: Arc::default(),
            ws_generation: 0,
            on_arrival: None,
            #[cfg(test)]
            sent: Arc::default(),
            incoming: Arc::new(Mutex::new(Vec::new())),
            from_home: Arc::default(),
            ws_state: Arc::new(Mutex::new(WsState::Closed)),
            activity: Arc::new(AtomicU64::new(0)),
            last_error: Arc::new(Mutex::new(None)),
            backoff: (0.5, 10.0),
            retry: None,
            dropped: false,
            reconnected: false,
            events_door: TicketDoor::default(),
        }
    }

    /// Reconnect delays from config (milliseconds).
    pub fn set_backoff(&mut self, min_ms: u64, max_ms: u64) {
        self.backoff = (min_ms as f64 / 1000.0, (max_ms.max(min_ms)) as f64 / 1000.0);
    }

    fn set_ws_state(&self, state: WsState) {
        if let Ok(mut s) = self.ws_state.lock() {
            *s = state;
        }
    }

    /// All HTTP goes through here so the activity LED sees requests and responses.
    fn fetch(&self, request: ehttp::Request, on_done: impl 'static + Send + FnOnce(ehttp::Result<ehttp::Response>)) {
        // Tests answer for the server themselves (what was asked is in
        // `sent`): nothing goes out, so no real answer races theirs.
        #[cfg(test)]
        let _ = (request, on_done);
        #[cfg(not(test))]
        {
            self.activity.fetch_add(1, Ordering::Relaxed);
            let activity = self.activity.clone();
            ehttp::fetch(request, move |result| {
                activity.fetch_add(1, Ordering::Relaxed);
                on_done(result);
            });
        }
    }

    /// Sends JSON (or no body for GET) and decodes a JSON reply into `T`
    /// (`()` accepts any 2xx with no body).
    fn call<T: DeserializeOwned + Send + 'static>(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        ctx: egui::Context,
        on_done: impl FnOnce(NetResult<T>) + Send + 'static,
    ) {
        #[cfg(test)]
        self.sent.lock().unwrap().push(format!("{method} {}{path}", self.prefix));
        let url = format!("{}{}{}", self.base_url, self.prefix, path);
        let mut request = match body {
            Some(json) => {
                let mut r = ehttp::Request::post(&url, serde_json::to_vec(&json).unwrap_or_default());
                r.method = method.to_owned();
                r.headers.insert("Content-Type", "application/json");
                r
            }
            None => {
                let mut r = ehttp::Request::get(&url);
                r.method = method.to_owned();
                r
            }
        };
        request.headers.insert("Accept", "application/json");
        self.fetch(request, move |result| {
            let outcome = match result {
                Ok(resp) if resp.ok => {
                    let bytes: &[u8] = if resp.bytes.is_empty() { b"null" } else { &resp.bytes };
                    serde_json::from_slice::<T>(bytes).map_err(|e| NetError::other(format!("Bad response: {e}")))
                }
                Ok(resp) => Err(http_error(&resp)),
                Err(e) => Err(NetError::other(format!("Network error: {e}"))),
            };
            on_done(outcome);
            ctx.request_repaint();
        });
    }

    /// Starts connecting the event socket: a ticket first, then the socket.
    pub fn connect_ws(&mut self, ctx: egui::Context) {
        if let Some(done) = self.events_door.start() {
            self.set_ws_state(WsState::Connecting);
            // Keep the delay: if this attempt fails too, the next waits longer.
            self.retry = self.retry.map(|(_, delay)| (f64::INFINITY, delay));
            self.cockpit_ticket(ctx, done);
        }
    }

    /// `f` sees each server event's text as it arrives.
    pub fn on_arrival(&mut self, f: impl Fn(&str) + Send + Sync + 'static) {
        self.on_arrival = Some(Arc::new(f));
    }

    fn open_ws(&mut self, ticket: &str, ctx: egui::Context) {
        self.ws_generation += 1;
        let (generation, queue, arrival) = (self.ws_generation, self.ws_events.clone(), self.on_arrival.clone());
        let handler = Box::new(move |event: WsEvent| {
            if let (WsEvent::Message(WsMessage::Text(text)), Some(arrival)) = (&event, &arrival) {
                arrival(text);
            }
            if let Ok(mut queue) = queue.lock() {
                queue.push((generation, event));
            }
            ctx.request_repaint();
            std::ops::ControlFlow::Continue(())
        });
        match ewebsock::ws_connect(format!("{}?ticket={ticket}", self.ws_url), ewebsock::Options::default(), handler) {
            Ok(sender) => self.ws_sender = Some(sender),
            Err(e) => {
                let now = self.retry.map_or(0.0, |(at, _)| if at.is_finite() { at } else { 0.0 });
                self.dropped_at(now, Some(format!("WebSocket connect error: {e}")));
            }
        }
    }

    /// Drains socket events; reconnects (with backoff) after the socket drops.
    pub fn poll_ws(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        match self.events_door.take() {
            Some(Ok(ticket)) => self.open_ws(&ticket, ctx.clone()),
            Some(Err(e)) => self.dropped_at(now, Some(format!("no ticket for the event socket: {}", e.message))),
            None => {}
        }
        let arrived = self.ws_events.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        // Only the socket open now (a dropped one may still be speaking).
        let current = self.ws_sender.is_some().then_some(self.ws_generation);
        for (_, event) in arrived.into_iter().filter(|(g, _)| Some(*g) == current) {
            self.on_ws_event(event, now);
        }
        if let Some((at, _)) = self.retry.filter(|(at, _)| at.is_finite()) {
            if now >= at && !self.events_door.asking() {
                self.connect_ws(ctx.clone());
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(at - now));
            }
        }
    }

    /// One socket event at app time `now`.
    pub fn on_ws_event(&mut self, event: WsEvent, now: f64) {
        match event {
            WsEvent::Message(WsMessage::Text(text)) => {
                self.activity.fetch_add(1, Ordering::Relaxed);
                match serde_json::from_str::<HomeEvent>(&text) {
                    Ok(parsed) => {
                        if let Ok(mut events) = self.from_home.lock() {
                            events.push(parsed);
                        }
                    }
                    Err(err) => {
                        #[cfg(target_arch = "wasm32")]
                        web_sys::console::warn_1(&format!("Unreadable server event: {err} | {text}").into());
                        #[cfg(not(target_arch = "wasm32"))]
                        eprintln!("Unreadable server event: {err} | {text}");
                    }
                }
            }
            WsEvent::Opened => {
                self.set_ws_state(WsState::Open);
                self.retry = None;
                if std::mem::take(&mut self.dropped) {
                    self.reconnected = true;
                }
            }
            WsEvent::Closed => self.dropped_at(now, None),
            WsEvent::Error(err) => self.dropped_at(now, Some(format!("WS Error: {err}"))),
            _ => {}
        }
    }

    /// The socket is gone: try again later, waiting longer each time.
    fn dropped_at(&mut self, now: f64, error: Option<String>) {
        self.set_ws_state(WsState::Closed);
        if let Some(error) = error
            && let Ok(mut e) = self.last_error.lock()
        {
            *e = Some(error);
        }
        self.ws_sender = None;
        self.dropped = true;
        let delay = match self.retry {
            Some((_, last)) => (last * 2.0).min(self.backoff.1),
            None => self.backoff.0,
        };
        self.retry = Some((now + delay, delay));
    }

    // ---- home: the cockpit's own state, the servers it links to ----

    /// The cockpit's own state as home keeps it (none before it ever saved).
    pub fn ui_shell(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<Option<serde_json::Value>>) + Send + 'static) {
        self.call("GET", "/api/ui/shell", None, ctx, on_done);
    }

    pub fn put_ui_shell(&self, state: serde_json::Value, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<()>("PUT", "/api/ui/shell", Some(state), ctx, on_done);
    }

    pub fn add_server(&self, name: &str, url: &str, secret: &str, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        let body = serde_json::json!({ "name": name, "url": url, "secret": secret });
        self.call::<serde_json::Value>("POST", "/api/home/servers", Some(body), ctx, move |r| on_done(r.map(|_| ())));
    }

    pub fn remove_server(&self, name: &str, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<()>("DELETE", &format!("/api/home/servers/{}", urlencoding::encode(name)), None, ctx, on_done);
    }

    /// Follows a server again (`follow`), or stops following it.
    pub fn server_link(&self, name: &str, follow: bool, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        let what = if follow { "connect" } else { "disconnect" };
        self.call::<()>("POST", &format!("/api/home/servers/{}/{what}", urlencoding::encode(name)), None, ctx, on_done);
    }

    /// Asks this server to stop.
    pub fn shutdown(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<serde_json::Value>("POST", "/api/server/shutdown", None, ctx, move |r| on_done(r.map(|_| ())));
    }

    // ---- notifications ----

    /// The user saw `slot`'s chat: its updates are put away.
    pub fn notifications_seen(&self, slot: &SlotId, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<serde_json::Value>("POST", "/api/notifications/seen", Some(serde_json::json!({ "slot": slot })), ctx, move |r| on_done(r.map(|_| ())));
    }

    pub fn dismiss_notification(&self, id: &NotificationId, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<serde_json::Value>("POST", &format!("/api/notifications/{}/dismiss", urlencoding::encode(id.as_str())), None, ctx, move |r| on_done(r.map(|_| ())));
    }

    // ---- slots ----

    /// Opens a session in a slot (setting its harness and settings first, if given).
    pub fn open_slot_session(&self, slot: &SlotId, body: &OpenSlotSession, ctx: egui::Context, on_done: impl FnOnce(NetResult<SlotOpened>) + Send + 'static) {
        let json = serde_json::to_value(body).unwrap_or_default();
        self.call::<SlotOpened>("POST", &format!("/api/slots/{slot}/sessions"), Some(json), ctx, on_done);
    }

    /// A prompt to a slot (its session resumed or opened first if it isn't
    /// running); `add_to_running`: into the turn it's running.
    pub fn prompt_slot(&self, slot: &SlotId, text: &str, add_to_running: bool, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        let json = serde_json::json!({ "text": text, "addToRunning": add_to_running });
        self.call::<serde_json::Value>("POST", &format!("/api/slots/{slot}/prompt"), Some(json), ctx, move |r| on_done(r.map(|_| ())));
    }

    pub fn close_slot_session(&self, slot: &SlotId, session: &SessionKey, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<()>("DELETE", &format!("/api/slots/{slot}/sessions/{session}"), None, ctx, on_done);
    }

    pub fn switch_slot_session(&self, slot: &SlotId, session: &SessionKey, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<()>("POST", &format!("/api/slots/{slot}/current"), Some(serde_json::json!({ "session": session })), ctx, on_done);
    }

    pub fn set_slot_harness(&self, slot: &SlotId, instance: &InstanceId, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<()>("POST", &format!("/api/slots/{slot}/harness"), Some(serde_json::json!({ "instanceId": instance })), ctx, on_done);
    }

    pub fn set_slot_settings(&self, slot: &SlotId, policy: &RuntimePolicy, model: Option<&ModelSelection>, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<()>("POST", &format!("/api/slots/{slot}/settings"), Some(serde_json::json!({ "policy": policy, "model": model })), ctx, on_done);
    }

    /// Starts a sub-slot on a harness.
    pub fn new_slot(&self, instance: &InstanceId, ctx: egui::Context, on_done: impl FnOnce(NetResult<SlotId>) + Send + 'static) {
        self.call::<SlotCreated>("POST", "/api/slots", Some(serde_json::json!({ "instanceId": instance })), ctx, move |r| on_done(r.map(|c| c.slot_id)));
    }

    pub fn close_slot(&self, slot: &SlotId, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<()>("DELETE", &format!("/api/slots/{slot}"), None, ctx, on_done);
    }

    // ---- threads ----

    pub fn steer(&self, thread: &ThreadId, input: &SteerInput, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        let json = serde_json::to_value(input).unwrap_or_default();
        self.call::<()>("POST", &format!("/api/threads/{thread}/steer"), Some(json), ctx, on_done);
    }

    pub fn interrupt(&self, thread: &ThreadId, ctx: egui::Context) {
        self.call::<()>("POST", &format!("/api/threads/{thread}/interrupt"), Some(serde_json::json!({})), ctx, |_| {});
    }

    pub fn respond(
        &self,
        thread: &ThreadId,
        request: &RequestId,
        response: &RequestResponse,
        ctx: egui::Context,
        on_done: impl FnOnce(NetResult<()>) + Send + 'static,
    ) {
        let json = serde_json::to_value(response).unwrap_or_default();
        self.call::<()>("POST", &format!("/api/threads/{thread}/requests/{request}"), Some(json), ctx, on_done);
    }

    /// The user's UI config file (TOML), if the server has one.
    pub fn ui_config(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<Option<String>>) + Send + 'static) {
        self.call::<UiConfigFile>("GET", "/api/ui/config", None, ctx, move |r| on_done(r.map(|f| f.text)));
    }

    /// The cockpit's state as it last saved it (none before it ever has).
    pub fn ui_state(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<Option<serde_json::Value>>) + Send + 'static) {
        self.call("GET", "/api/ui/state", None, ctx, on_done);
    }

    /// Saves the cockpit's state.
    pub fn save_ui_state(&self, state: serde_json::Value, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call("PUT", "/api/ui/state", Some(state), ctx, on_done);
    }

    // ---- instances ----

    /// The harness's saved sessions in the project.
    pub fn list_saved_sessions(&self, instance: &InstanceId, ctx: egui::Context, on_done: impl FnOnce(NetResult<SavedSessionPage>) + Send + 'static) {
        self.call("GET", &format!("/api/instances/{instance}/sessions"), None, ctx, on_done);
    }

    pub fn delete_saved_session(&self, instance: &InstanceId, native: &NativeId, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        let path = format!("/api/instances/{instance}/sessions/{}", urlencoding::encode(native.as_str()));
        self.call::<()>("DELETE", &path, None, ctx, on_done);
    }

    pub fn refresh_instance(&self, instance: &InstanceId, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<()>("POST", &format!("/api/instances/{instance}/refresh"), Some(serde_json::json!({})), ctx, on_done);
    }

    pub fn logout(&self, instance: &InstanceId, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        self.call::<()>("POST", &format!("/api/instances/{instance}/auth/logout"), Some(serde_json::json!({})), ctx, on_done);
    }

    pub fn begin_auth(&self, instance: &InstanceId, method: &str, ctx: egui::Context, on_done: impl FnOnce(NetResult<AuthChallenge>) + Send + 'static) {
        let path = format!("/api/instances/{instance}/auth/{}", urlencoding::encode(method));
        self.call("POST", &path, Some(serde_json::json!({})), ctx, on_done);
    }

    pub fn complete_auth(&self, instance: &InstanceId, response: &str, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        let path = format!("/api/instances/{instance}/auth/complete");
        self.call::<()>("POST", &path, Some(serde_json::json!({ "response": response })), ctx, on_done);
    }

    // ---- tool servers ----

    /// The tool servers every session gets: ours and the project's.
    /// A one-time ticket for the cockpit's socket (agents driving it).
    pub fn cockpit_ticket(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<String>) + Send + 'static) {
        #[derive(serde::Deserialize)]
        struct Ticket {
            ticket: String,
        }
        self.call::<Ticket>("POST", "/api/cockpit/ticket", Some(serde_json::json!({})), ctx, move |r| on_done(r.map(|t| t.ticket)));
    }

    pub fn tool_servers(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<harness_contracts::ToolServers>) + Send + 'static) {
        self.call("GET", "/api/mcp/servers", None, ctx, on_done);
    }

    /// Replaces the project's list.
    pub fn set_tool_servers(&self, project: &[harness_contracts::McpServer], ctx: egui::Context, on_done: impl FnOnce(NetResult<harness_contracts::ToolServers>) + Send + 'static) {
        let json = serde_json::to_value(project).unwrap_or_default();
        self.call("PUT", "/api/mcp/servers", Some(json), ctx, on_done);
    }

    // ---- architecture ----

    /// The component types and what each requires.
    pub fn arch_types(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::TypeSystem>) + Send + 'static) {
        self.call("GET", "/api/arch/types", None, ctx, on_done);
    }

    /// One of the three views as a laid-out tree.
    pub fn graph_tree(&self, request: &arch_graph::tree::TreeRequest, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::tree::Tree>) + Send + 'static) {
        self.call("GET", &format!("/api/graph/tree?{}", query(request.query_pairs())), None, ctx, on_done);
    }

    /// What opening `card` `levels` further changes in the tree `request`
    /// was answered with (the one held): new cards, every card's place, the
    /// lines, all laid out by the server.
    pub fn graph_expand(&self, request: &arch_graph::tree::TreeRequest, card: &str, levels: u32, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::tree::Expanded>) + Send + 'static) {
        let mut pairs = request.query_pairs();
        pairs.extend([("card", card.to_owned()), ("levels", levels.to_string())]);
        self.call("GET", &format!("/api/graph/expand?{}", query(pairs)), None, ctx, on_done);
    }

    /// Everything about one card (its component's detail).
    pub fn graph_card(&self, key: &str, source: &arch_graph::Source, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::ComponentDetail>) + Send + 'static) {
        self.call("GET", &format!("/api/graph/details/{}?source={}", urlencoding::encode(key), source_param(source)), None, ctx, on_done);
    }

    /// Symbols, files or lines matching `query` (in the code, or with a plan).
    /// Symbols, files or lines matching `query`; only in the files
    /// `changed` changed, when given.
    #[allow(clippy::too_many_arguments)]
    pub fn search(
        &self,
        query: &str,
        kind: arch_search::SearchKind,
        source: &arch_graph::Source,
        limit: usize,
        changed: Option<&harness_contracts::changes::Comparison>,
        ctx: egui::Context,
        on_done: impl FnOnce(NetResult<arch_search::Found>) + Send + 'static,
    ) {
        let changed = changed.map(|c| format!("&changed={}", urlencoding::encode(&c.range()))).unwrap_or_default();
        self.call("GET", &format!("/api/search?q={}&kind={}&limit={limit}&source={}{changed}", urlencoding::encode(query), kind.name(), source_param(source)), None, ctx, on_done);
    }

    /// A file's text and its analysis, in one answer.
    /// The files `comparison` changes (a page), with their counts.
    pub fn changes(&self, comparison: &harness_contracts::changes::Comparison, offset: usize, limit: usize, ctx: egui::Context, on_done: impl FnOnce(NetResult<harness_contracts::changes::ChangeList>) + Send + 'static) {
        let path = format!("/api/changes?from={}&to={}&offset={offset}&limit={limit}", urlencoding::encode(&comparison.from.to_string()), urlencoding::encode(&comparison.to.to_string()));
        self.call("GET", &path, None, ctx, on_done);
    }

    /// Stages (or, `stage` false, unstages) `path`, or one change in it
    /// (where it starts on each side, and the context it was shown with).
    pub fn change_stage(&self, stage: bool, path: &str, hunk: Option<(u32, u32, u32)>, ctx: egui::Context, on_done: impl FnOnce(NetResult<serde_json::Value>) + Send + 'static) {
        let hunk = hunk.map(|(old_start, new_start, context)| serde_json::json!({ "old_start": old_start, "new_start": new_start, "context": context }));
        let route = if stage { "/api/changes/stage" } else { "/api/changes/unstage" };
        self.call("POST", route, Some(serde_json::json!({ "path": path, "hunk": hunk })), ctx, on_done);
    }

    /// When the files changed and who was working, and each turn's changes.
    pub fn change_timeline(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<harness_contracts::changes::Timeline>) + Send + 'static) {
        self.call("GET", "/api/changes/timeline", None, ctx, on_done);
    }

    /// Which components `comparison` changed (where the index knows them).
    pub fn change_components(&self, comparison: &harness_contracts::changes::Comparison, ctx: egui::Context, on_done: impl FnOnce(NetResult<harness_contracts::changes::ComponentChangeList>) + Send + 'static) {
        let path = format!("/api/changes/components?from={}&to={}", urlencoding::encode(&comparison.from.to_string()), urlencoding::encode(&comparison.to.to_string()));
        self.call("GET", &path, None, ctx, on_done);
    }

    /// One file's changes in `comparison` (its `group` of them), with
    /// `context` unchanged lines around each.
    pub fn change_file(
        &self,
        comparison: &harness_contracts::changes::Comparison,
        path: &str,
        group: harness_contracts::changes::Group,
        context: u32,
        ctx: egui::Context,
        on_done: impl FnOnce(NetResult<harness_contracts::changes::FileChanges>) + Send + 'static,
    ) {
        let group = serde_json::to_value(group).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        let url = format!(
            "/api/changes/file?from={}&to={}&path={}&group={group}&context={context}",
            urlencoding::encode(&comparison.from.to_string()),
            urlencoding::encode(&comparison.to.to_string()),
            urlencoding::encode(path)
        );
        self.call("GET", &url, None, ctx, on_done);
    }

    pub fn code_analysis(&self, path: &str, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::code::CodeAnalysis>) + Send + 'static) {
        self.call("GET", &format!("/api/code/analysis?path={}", urlencoding::encode(path)), None, ctx, on_done);
    }

    /// The views' settings: descriptions, depths and the sizes trees are laid out with.
    pub fn graph_config(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::tree::TreeConfig>) + Send + 'static) {
        self.call("GET", "/api/graph/config", None, ctx, on_done);
    }

    pub fn arch_component(
        &self,
        id: &arch_graph::ComponentId,
        source: &arch_graph::Source,
        ctx: egui::Context,
        on_done: impl FnOnce(NetResult<arch_graph::ComponentDetail>) + Send + 'static,
    ) {
        self.call("GET", &format!("/api/arch/components/{}?source={}", urlencoding::encode(id.as_str()), source_param(source)), None, ctx, on_done);
    }

    /// The component enclosing `file:line`.
    pub fn arch_at(&self, file: &str, line: usize, source: &arch_graph::Source, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::Component>) + Send + 'static) {
        self.call("GET", &format!("/api/arch/at?file={}&line={line}&source={}", urlencoding::encode(file), source_param(source)), None, ctx, on_done);
    }

    /// Indexing state: running or not, progress, last run, stale files.
    pub fn arch_index_status(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::IndexStatus>) + Send + 'static) {
        self.call("GET", "/api/arch/index", None, ctx, on_done);
    }

    /// Reads the code again: what changed (or everything, from scratch, when
    /// `full`).
    pub fn arch_ingest(&self, full: bool, ctx: egui::Context, on_done: impl FnOnce(NetResult<serde_json::Value>) + Send + 'static) {
        self.call("POST", "/api/arch/ingest", Some(serde_json::json!({ "full": full })), ctx, on_done);
    }

    /// Reads these repo files again.
    pub fn arch_refresh(&self, paths: &[String], ctx: egui::Context, on_done: impl FnOnce(NetResult<serde_json::Value>) + Send + 'static) {
        self.call("POST", "/api/arch/refresh", Some(serde_json::json!({ "paths": paths })), ctx, on_done);
    }

    pub fn arch_plans(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<Vec<arch_graph::Plan>>) + Send + 'static) {
        self.call("GET", "/api/arch/plans", None, ctx, on_done);
    }

    pub fn arch_validate(&self, plan: &arch_graph::PlanId, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::Report>) + Send + 'static) {
        self.call("POST", &format!("/api/arch/plans/{}/validate", urlencoding::encode(&plan.0)), Some(serde_json::json!({})), ctx, on_done);
    }

    /// The work queue over a plan.
    pub fn arch_queue(&self, plan: &arch_graph::PlanId, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::QueueSnapshot>) + Send + 'static) {
        self.call("GET", &format!("/api/arch/plans/{}/queue", urlencoding::encode(&plan.0)), None, ctx, on_done);
    }

    /// Starts the queue; `instance` runs its sessions.
    pub fn arch_queue_start(&self, plan: &arch_graph::PlanId, instance: &harness_contracts::InstanceId, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::QueueSnapshot>) + Send + 'static) {
        self.call("POST", &format!("/api/arch/plans/{}/queue/start", urlencoding::encode(&plan.0)), Some(serde_json::json!({ "instanceId": instance })), ctx, on_done);
    }

    pub fn arch_queue_stop(&self, plan: &arch_graph::PlanId, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::QueueSnapshot>) + Send + 'static) {
        self.call("POST", &format!("/api/arch/plans/{}/queue/stop", urlencoding::encode(&plan.0)), Some(serde_json::json!({})), ctx, on_done);
    }

    pub fn arch_queue_retry(&self, plan: &arch_graph::PlanId, component: &arch_graph::ComponentId, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::QueueSnapshot>) + Send + 'static) {
        self.call("POST", &format!("/api/arch/plans/{}/queue/retry", urlencoding::encode(&plan.0)), Some(serde_json::json!({ "component": component })), ctx, on_done);
    }

    /// The user approves a component's design (ArchApproved or PlanApproved).
    pub fn arch_approve(
        &self,
        plan: &arch_graph::PlanId,
        component: &arch_graph::ComponentId,
        to: arch_graph::Stage,
        by: &str,
        ctx: egui::Context,
        on_done: impl FnOnce(NetResult<arch_graph::Approval>) + Send + 'static,
    ) {
        let path = format!("/api/arch/plans/{}/components/{}/approve", urlencoding::encode(&plan.0), urlencoding::encode(component.as_str()));
        self.call("POST", &path, Some(serde_json::json!({ "to": to, "by": by })), ctx, on_done);
    }

    /// A plan's review comments (unaddressed only, unless `all`).
    pub fn arch_comments(&self, plan: &arch_graph::PlanId, all: bool, ctx: egui::Context, on_done: impl FnOnce(NetResult<Vec<arch_graph::Comment>>) + Send + 'static) {
        self.call("GET", &format!("/api/arch/plans/{}/comments?all={all}", urlencoding::encode(&plan.0)), None, ctx, on_done);
    }

    /// Raises a comment or change request: `body` is `{anchor, kind, text, by}`.
    pub fn arch_add_comment(&self, plan: &arch_graph::PlanId, body: serde_json::Value, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::Comment>) + Send + 'static) {
        self.call("POST", &format!("/api/arch/plans/{}/comments", urlencoding::encode(&plan.0)), Some(body), ctx, on_done);
    }

    /// `action` is `replies`, `accept` or `reopen`; `body` is `{text?, by}`.
    pub fn arch_comment_action(
        &self,
        plan: &arch_graph::PlanId,
        comment: &arch_graph::CommentId,
        action: &str,
        body: serde_json::Value,
        ctx: egui::Context,
        on_done: impl FnOnce(NetResult<arch_graph::Comment>) + Send + 'static,
    ) {
        let path = format!("/api/arch/plans/{}/comments/{}/{action}", urlencoding::encode(&plan.0), urlencoding::encode(&comment.0));
        self.call("POST", &path, Some(body), ctx, on_done);
    }

    pub fn arch_mark_reviewed(&self, plan: &arch_graph::PlanId, by: &str, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::ReviewSnapshot>) + Send + 'static) {
        self.call("POST", &format!("/api/arch/plans/{}/reviewed", urlencoding::encode(&plan.0)), Some(serde_json::json!({ "by": by })), ctx, on_done);
    }

    /// What changed in the plan since the user last reviewed it.
    pub fn arch_review_diff(&self, plan: &arch_graph::PlanId, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::PlanDiff>) + Send + 'static) {
        self.call("GET", &format!("/api/arch/plans/{}/diff", urlencoding::encode(&plan.0)), None, ctx, on_done);
    }

    pub fn submit_feedback<F>(&self, feedback_json: serde_json::Value, ctx: egui::Context, on_done: F)
    where
        F: FnOnce(Result<String, String>) + Send + 'static,
    {
        let url = format!("{}/api/dev/feedback", self.base_url);
        let body = serde_json::to_vec(&feedback_json).unwrap_or_default();
        let mut request = ehttp::Request::post(&url, body);
        request.headers.insert("Content-Type", "application/json");

        self.fetch(request, move |result| {
            match result {
                Ok(resp) => {
                    if resp.ok {
                        on_done(Ok("Feedback saved successfully".into()));
                    } else {
                        on_done(Err(format!("HTTP Error {}", resp.status)));
                    }
                }
                Err(e) => on_done(Err(format!("Network error: {}", e))),
            }
            ctx.request_repaint();
        });
    }

    pub fn fetch_feedback<F>(&self, ctx: egui::Context, on_done: F)
    where
        F: FnOnce(Result<Vec<serde_json::Value>, String>) + Send + 'static,
    {
        let url = format!("{}/api/dev/feedback", self.base_url);
        let request = ehttp::Request::get(&url);

        self.fetch(request, move |result| {
            match result {
                Ok(resp) => {
                    if resp.ok {
                        match serde_json::from_slice::<Vec<serde_json::Value>>(&resp.bytes) {
                            Ok(items) => on_done(Ok(items)),
                            Err(e) => on_done(Err(format!("JSON Parse error: {}", e))),
                        }
                    } else {
                        on_done(Err(format!("HTTP Error {}", resp.status)));
                    }
                }
                Err(e) => on_done(Err(format!("Network error: {}", e))),
            }
            ctx.request_repaint();
        });
    }

    pub fn update_feedback<F>(&self, feedback_json: serde_json::Value, ctx: egui::Context, on_done: F)
    where
        F: FnOnce(Result<String, String>) + Send + 'static,
    {
        let url = format!("{}/api/dev/feedback", self.base_url);
        let body = serde_json::to_vec(&feedback_json).unwrap_or_default();
        let mut request = ehttp::Request::post(&url, body);
        request.method = "PUT".to_string();
        request.headers.insert("Content-Type", "application/json");

        self.fetch(request, move |result| {
            match result {
                Ok(resp) => {
                    if resp.ok {
                        on_done(Ok("Feedback updated successfully".into()));
                    } else {
                        on_done(Err(format!("HTTP Error {}", resp.status)));
                    }
                }
                Err(e) => on_done(Err(format!("Network error: {}", e))),
            }
            ctx.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_event_socket_comes_back_with_backoff_and_asks_for_a_refresh() {
        let mut net = NetClient::new("http://127.0.0.1:9");
        net.set_backoff(500, 4000);
        net.on_ws_event(WsEvent::Opened, 0.0);
        assert!(!net.reconnected, "the first open isn't a reconnect");
        net.on_ws_event(WsEvent::Error("gone".into()), 10.0);
        assert_eq!(*net.ws_state.lock().unwrap(), WsState::Closed);
        assert_eq!(net.retry.map(|(at, _)| at), Some(10.5), "first retry after the minimum delay");
        // Each failed attempt waits twice as long, up to the cap.
        for (now, expected) in [(10.5, 11.5), (11.5, 13.5), (13.5, 17.5), (17.5, 21.5)] {
            net.retry = net.retry.map(|(_, d)| (f64::INFINITY, d));
            net.on_ws_event(WsEvent::Closed, now);
            assert_eq!(net.retry.map(|(at, _)| at), Some(expected), "at {now}");
        }
        net.on_ws_event(WsEvent::Opened, 30.0);
        assert_eq!(net.retry.map(|(at, _)| at), None);
        assert!(net.reconnected, "views refetch what they missed");
        assert_eq!(*net.ws_state.lock().unwrap(), WsState::Open);
    }

    #[test]
    fn home_says_whose_events_and_another_servers_calls_go_through_it() {
        let mut net = NetClient::new("http://127.0.0.1:9");
        net.on_ws_event(WsEvent::Opened, 0.0);
        net.on_ws_event(WsEvent::Message(WsMessage::Text(r#"{"type":"event","server":"cloud","event":{"type":"resync"}}"#.into())), 1.0);
        let events = std::mem::take(&mut *net.from_home.lock().unwrap());
        assert_eq!(events, [HomeEvent::Event { server: "cloud".into(), event: Box::new(ClientEvent::Resync) }]);
        let cloud = NetClient::for_server("http://127.0.0.1:9", "cloud");
        cloud.notifications_seen(&SlotId::new("lead"), egui::Context::default(), |_| {});
        assert_eq!(*cloud.sent.lock().unwrap(), ["POST /s/cloud/api/notifications/seen"]);
        let mut agents = crate::agents::Agents::default();
        crate::chat::tests::conversation_events().into_iter().for_each(|e| agents.apply(e));
        assert!(!agents.store().threads.is_empty());
        agents.apply(ClientEvent::Resync);
        assert!(agents.store().instances.is_empty() && agents.store().threads.is_empty(), "start from nothing; the whole state follows");
    }
}
