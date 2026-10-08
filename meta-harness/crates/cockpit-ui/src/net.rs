use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ewebsock::{WsEvent, WsMessage, WsReceiver, WsSender};
use harness_contracts::{
    ClientEvent, InstanceId, ModelSelection, NativeId, RequestId, RequestResponse, RuntimePolicy, SavedSessionPage,
    SteerInput, ThreadId, TurnInput,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Event WebSocket to the meta-harness server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsState {
    Connecting,
    Open,
    Closed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenThreadBody {
    pub instance_id: InstanceId,
    pub policy: RuntimePolicy,
    pub model: Option<ModelSelection>,
    pub resume: Option<NativeId>,
    pub mcp_servers: Vec<harness_contracts::McpServer>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadOpened {
    thread_id: ThreadId,
}

#[derive(serde::Deserialize)]
struct UiConfigFile {
    text: Option<String>,
}

#[derive(serde::Deserialize)]
struct AuthChallenge {
    instructions: String,
}

pub type NetResult<T> = Result<T, String>;

pub struct NetClient {
    base_url: String,
    ws_url: String,
    ws_sender: Option<WsSender>,
    ws_receiver: Option<WsReceiver>,
    /// Server events received since the last drain.
    pub incoming: Arc<Mutex<Vec<ClientEvent>>>,
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
}

/// Error text from a non-2xx response: the server's `{"error": ...}` or the status.
fn http_error(response: &ehttp::Response) -> String {
    let server_message = serde_json::from_slice::<serde_json::Value>(&response.bytes)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned));
    match server_message {
        Some(message) => message,
        None => format!("HTTP {}", response.status),
    }
}

impl NetClient {
    pub fn new(host_origin: &str) -> Self {
        let base_url = host_origin.trim_end_matches('/').to_string();
        let ws_url = if base_url.starts_with("https://") {
            base_url.replacen("https://", "wss://", 1) + "/ws/events"
        } else {
            base_url.replacen("http://", "ws://", 1) + "/ws/events"
        };

        Self {
            base_url,
            ws_url,
            ws_sender: None,
            ws_receiver: None,
            incoming: Arc::new(Mutex::new(Vec::new())),
            ws_state: Arc::new(Mutex::new(WsState::Closed)),
            activity: Arc::new(AtomicU64::new(0)),
            last_error: Arc::new(Mutex::new(None)),
            backoff: (0.5, 10.0),
            retry: None,
            dropped: false,
            reconnected: false,
        }
    }

    /// Reconnect delays from config (milliseconds).
    pub fn set_backoff(&mut self, min_ms: u64, max_ms: u64) {
        self.backoff = (min_ms as f64 / 1000.0, (max_ms.max(min_ms)) as f64 / 1000.0);
    }

    /// When the next reconnect attempt is due, if one is scheduled.
    pub fn retry_at(&self) -> Option<f64> {
        self.retry.map(|(at, _)| at)
    }

    fn set_ws_state(&self, state: WsState) {
        if let Ok(mut s) = self.ws_state.lock() {
            *s = state;
        }
    }

    /// All HTTP goes through here so the activity LED sees requests and responses.
    fn fetch(&self, request: ehttp::Request, on_done: impl 'static + Send + FnOnce(ehttp::Result<ehttp::Response>)) {
        self.activity.fetch_add(1, Ordering::Relaxed);
        let activity = self.activity.clone();
        ehttp::fetch(request, move |result| {
            activity.fetch_add(1, Ordering::Relaxed);
            on_done(result);
        });
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
        let url = format!("{}{}", self.base_url, path);
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
                    serde_json::from_slice::<T>(bytes).map_err(|e| format!("Bad response: {e}"))
                }
                Ok(resp) => Err(http_error(&resp)),
                Err(e) => Err(format!("Network error: {e}")),
            };
            on_done(outcome);
            ctx.request_repaint();
        });
    }

    pub fn connect_ws(&mut self, ctx: egui::Context) {
        let options = ewebsock::Options::default();
        let wakeup = move || {
            ctx.request_repaint();
        };

        match ewebsock::connect_with_wakeup(&self.ws_url, options, wakeup) {
            Ok((sender, receiver)) => {
                self.ws_sender = Some(sender);
                self.ws_receiver = Some(receiver);
                self.set_ws_state(WsState::Connecting);
                // Keep the delay: if this attempt fails too, the next waits longer.
                self.retry = self.retry.map(|(_, delay)| (f64::INFINITY, delay));
            }
            Err(e) => {
                let now = self.retry.map_or(0.0, |(at, _)| if at.is_finite() { at } else { 0.0 });
                self.dropped_at(now, Some(format!("WebSocket connect error: {e}")));
            }
        }
    }

    /// Drains socket events; reconnects (with backoff) after the socket drops.
    pub fn poll_ws(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let mut events = Vec::new();
        if let Some(receiver) = &self.ws_receiver {
            while let Some(event) = receiver.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            self.on_ws_event(event, now);
        }
        if let Some((at, _)) = self.retry.filter(|(at, _)| at.is_finite()) {
            if now >= at {
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
                match serde_json::from_str::<ClientEvent>(&text) {
                    Ok(parsed) => {
                        if let Ok(mut events) = self.incoming.lock() {
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
        self.ws_receiver = None;
        self.dropped = true;
        let delay = match self.retry {
            Some((_, last)) => (last * 2.0).min(self.backoff.1),
            None => self.backoff.0,
        };
        self.retry = Some((now + delay, delay));
    }

    // ---- threads ----

    pub fn open_thread(&self, body: &OpenThreadBody, ctx: egui::Context, on_done: impl FnOnce(NetResult<ThreadId>) + Send + 'static) {
        let json = serde_json::to_value(body).unwrap_or_default();
        self.call::<ThreadOpened>("POST", "/api/threads", Some(json), ctx, move |r| on_done(r.map(|o| o.thread_id)));
    }

    pub fn start_turn(&self, thread: &ThreadId, input: &TurnInput, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        let json = serde_json::to_value(input).unwrap_or_default();
        self.call::<serde_json::Value>("POST", &format!("/api/threads/{thread}/turns"), Some(json), ctx, move |r| on_done(r.map(|_| ())));
    }

    pub fn steer(&self, thread: &ThreadId, input: &SteerInput, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        let json = serde_json::to_value(input).unwrap_or_default();
        self.call::<()>("POST", &format!("/api/threads/{thread}/steer"), Some(json), ctx, on_done);
    }

    pub fn interrupt(&self, thread: &ThreadId, ctx: egui::Context) {
        self.call::<()>("POST", &format!("/api/threads/{thread}/interrupt"), Some(serde_json::json!({})), ctx, |_| {});
    }

    pub fn close_thread(&self, thread: &ThreadId, ctx: egui::Context) {
        self.call::<()>("DELETE", &format!("/api/threads/{thread}"), None, ctx, |_| {});
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

    // ---- instances ----

    pub fn list_saved_sessions(
        &self,
        instance: &InstanceId,
        cwd: &str,
        ctx: egui::Context,
        on_done: impl FnOnce(NetResult<SavedSessionPage>) + Send + 'static,
    ) {
        let path = format!("/api/instances/{instance}/sessions?cwd={}", urlencoding::encode(cwd));
        self.call("GET", &path, None, ctx, on_done);
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

    pub fn begin_auth(&self, instance: &InstanceId, method: &str, ctx: egui::Context, on_done: impl FnOnce(NetResult<String>) + Send + 'static) {
        let path = format!("/api/instances/{instance}/auth/{}", urlencoding::encode(method));
        self.call::<AuthChallenge>("POST", &path, Some(serde_json::json!({})), ctx, move |r| on_done(r.map(|c| c.instructions)));
    }

    pub fn complete_auth(&self, instance: &InstanceId, response: &str, ctx: egui::Context, on_done: impl FnOnce(NetResult<()>) + Send + 'static) {
        let path = format!("/api/instances/{instance}/auth/complete");
        self.call::<()>("POST", &path, Some(serde_json::json!({ "response": response })), ctx, on_done);
    }

    // ---- app-level ----

    pub fn fetch_file<F>(&self, path: &str, ctx: egui::Context, on_done: F)
    where
        F: FnOnce(Result<String, String>) + Send + 'static,
    {
        let encoded_path = urlencoding::encode(path);
        let url = format!("{}/api/fs/read?path={}", self.base_url, encoded_path);
        let request = ehttp::Request::get(&url);

        self.fetch(request, move |result| {
            match result {
                Ok(resp) => {
                    if resp.ok {
                        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&resp.bytes) {
                            if let Some(content) = json.get("content").and_then(|c| c.as_str()) {
                                on_done(Ok(content.to_string()));
                            } else {
                                on_done(Err("Invalid response format".into()));
                            }
                        } else {
                            on_done(Err("Failed to parse JSON".into()));
                        }
                    } else {
                        on_done(Err(format!("File read HTTP error {}", resp.status)));
                    }
                }
                Err(e) => on_done(Err(format!("Network error: {}", e))),
            }
            ctx.request_repaint();
        });
    }

    // ---- architecture ----

    /// The component types and what each requires.
    pub fn arch_types(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::TypeSystem>) + Send + 'static) {
        self.call("GET", "/api/arch/types", None, ctx, on_done);
    }

    /// One of the three views as a laid-out tree.
    pub fn graph_tree(&self, request: &arch_graph::tree::TreeRequest, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::tree::Tree>) + Send + 'static) {
        self.call("POST", "/api/graph/tree", serde_json::to_value(request).ok(), ctx, on_done);
    }

    /// Everything about one card (its component's detail).
    pub fn graph_card(&self, key: &str, plan: Option<&arch_graph::PlanId>, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::ComponentDetail>) + Send + 'static) {
        let mut path = format!("/api/graph/card?key={}", urlencoding::encode(key));
        if let Some(plan) = plan {
            path.push_str(&format!("&plan={}", urlencoding::encode(&plan.0)));
        }
        self.call("GET", &path, None, ctx, on_done);
    }

    /// The views' settings: descriptions, depths and the sizes trees are laid out with.
    pub fn graph_config(&self, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::tree::TreeConfig>) + Send + 'static) {
        self.call("GET", "/api/graph/config", None, ctx, on_done);
    }

    pub fn arch_component(
        &self,
        id: &arch_graph::ComponentId,
        plan: Option<&arch_graph::PlanId>,
        ctx: egui::Context,
        on_done: impl FnOnce(NetResult<arch_graph::ComponentDetail>) + Send + 'static,
    ) {
        let mut path = format!("/api/arch/components/{}", urlencoding::encode(id.as_str()));
        if let Some(plan) = plan {
            path.push_str(&format!("?plan={}", urlencoding::encode(&plan.0)));
        }
        self.call("GET", &path, None, ctx, on_done);
    }

    /// The component enclosing `file:line`.
    pub fn arch_at(&self, file: &str, line: usize, plan: Option<&arch_graph::PlanId>, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::Component>) + Send + 'static) {
        let mut path = format!("/api/arch/at?file={}&line={line}", urlencoding::encode(file));
        if let Some(plan) = plan {
            path.push_str(&format!("&plan={}", urlencoding::encode(&plan.0)));
        }
        self.call("GET", &path, None, ctx, on_done);
    }

    /// Components in one file (for the code view's gutter).
    pub fn arch_file(&self, file: &str, plan: Option<&arch_graph::PlanId>, ctx: egui::Context, on_done: impl FnOnce(NetResult<arch_graph::FileView>) + Send + 'static) {
        let mut path = format!("/api/arch/file?path={}", urlencoding::encode(file));
        if let Some(plan) = plan {
            path.push_str(&format!("&plan={}", urlencoding::encode(&plan.0)));
        }
        self.call("GET", &path, None, ctx, on_done);
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

    // req: P9.2
    #[test]
    fn a_dropped_event_socket_comes_back_with_backoff_and_asks_for_a_refresh() {
        let mut net = NetClient::new("http://127.0.0.1:9");
        net.set_backoff(500, 4000);
        net.on_ws_event(WsEvent::Opened, 0.0);
        assert!(!net.reconnected, "the first open isn't a reconnect");
        net.on_ws_event(WsEvent::Error("gone".into()), 10.0);
        assert_eq!(*net.ws_state.lock().unwrap(), WsState::Closed);
        assert_eq!(net.retry_at(), Some(10.5), "first retry after the minimum delay");
        // Each failed attempt waits twice as long, up to the cap.
        for (now, expected) in [(10.5, 11.5), (11.5, 13.5), (13.5, 17.5), (17.5, 21.5)] {
            net.retry = net.retry.map(|(_, d)| (f64::INFINITY, d));
            net.on_ws_event(WsEvent::Closed, now);
            assert_eq!(net.retry_at(), Some(expected), "at {now}");
        }
        net.on_ws_event(WsEvent::Opened, 30.0);
        assert_eq!(net.retry_at(), None);
        assert!(net.reconnected, "views refetch what they missed");
        assert_eq!(*net.ws_state.lock().unwrap(), WsState::Open);
    }
}
