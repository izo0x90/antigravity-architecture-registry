//! Shared setup for server tests: a fake harness provider and an app on a random port.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use harness_contracts::*;
use harness_core::*;
use arch_service::{ArchitectureService, FsPlans};
use meta_harness::mcp::{McpServerState, McpService};
use meta_harness::server::{AppState, bridge_architecture_events, create_router};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, watch};

pub const FAKE: &str = "fake";

/// A session that answers every prompt with one assistant message.
pub struct FakeSession {
    thread: Thread,
    events: broadcast::Sender<AdapterEvent>,
    log: Arc<Mutex<Vec<AdapterEvent>>>,
    /// Turns so far: each turn and its item get ids of their own.
    turns: std::sync::atomic::AtomicU32,
}

impl FakeSession {
    fn emit(&self, event: AdapterEvent) {
        self.log.lock().unwrap().push(event.clone());
        let _ = self.events.send(event);
    }
}

#[async_trait]
impl SessionRuntime for FakeSession {
    fn thread_id(&self) -> &ThreadId {
        &self.thread.id
    }

    fn native_id(&self) -> Option<NativeId> {
        Some(NativeId::new(format!("native-{}", self.thread_id())))
    }

    fn events(&self) -> BoxStream<'static, AdapterEvent> {
        let replay = self.log.lock().unwrap().clone();
        let live = tokio_stream::wrappers::BroadcastStream::new(self.events.subscribe()).filter_map(|e| async move { e.ok() });
        futures::stream::iter(replay).chain(live).boxed()
    }

    async fn start_turn(&self, input: TurnInput) -> Result<TurnId> {
        // "[hang]" in the input: a turn that never finishes (for timeouts).
        let status = if input.text.contains("[hang]") { TurnStatus::Running } else { TurnStatus::Completed };
        let n = self.turns.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let turn = Turn {
            id: TurnId::new(format!("turn-{n}")),
            thread_id: self.thread.id.clone(),
            ordinal: n,
            status,
            failure: None,
            usage: None,
        };
        self.emit(AdapterEvent::ItemUpdated {
            item: TurnItem {
                id: ItemId::new(format!("item-{n}")),
                thread_id: self.thread.id.clone(),
                turn_id: turn.id.clone(),
                native_id: None,
                parent_id: None,
                ordinal: n,
                status: ItemStatus::Completed,
                title: None,
                body: ItemBody::AssistantMessage {
                    content: vec![ContentBlock::Text { text: format!("echo: {}", input.text) }],
                    streaming: false,
                },
            },
        });
        self.emit(AdapterEvent::TurnUpdated { turn: turn.clone() });
        Ok(turn.id)
    }

    async fn steer(&self, _input: SteerInput) -> Result<()> {
        Err(HarnessError::Unsupported { operation: "steering" })
    }

    async fn interrupt(&self) -> Result<()> {
        Ok(())
    }

    async fn respond(&self, request: &RequestId, _response: RequestResponse) -> Result<()> {
        Err(HarnessError::NotFound { what: format!("request {request}") })
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

pub struct FakeAdapter;

/// The tool servers each fake session was given, by thread: what an agent
/// would connect to (its key included).
static GIVEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<ThreadId, Vec<McpServer>>>> = std::sync::LazyLock::new(Default::default);

/// The tool servers a fake session was given.
pub fn given_tools(thread: &ThreadId) -> Vec<McpServer> {
    GIVEN.lock().unwrap().get(thread).cloned().unwrap_or_default()
}

#[async_trait]
impl ProviderAdapter for FakeAdapter {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    async fn open_session(&self, config: SessionConfig) -> Result<Box<dyn SessionRuntime>> {
        // A working directory of "[hang-open]": a harness that never finishes opening.
        if config.policy.cwd.as_deref() == Some("[hang-open]") {
            futures::future::pending::<()>().await;
        }
        // A session saved as "gone" can't be resumed.
        if config.resume.as_ref().is_some_and(|n| n.as_str() == "gone") {
            return Err(HarnessError::Provider { reason: "no such session".into() });
        }
        GIVEN.lock().unwrap().insert(config.thread_id.clone(), config.mcp_servers.clone());
        let thread = Thread {
            id: config.thread_id,
            instance_id: InstanceId::new(FAKE),
            native_id: Some(NativeId::new("native-1")),
            title: None,
            status: ThreadStatus::Idle,
            policy: config.policy,
            model: config.model,
            controls: Default::default(),
            mcp_servers: Vec::new(),
        };
        let (events, _) = broadcast::channel(64);
        let session = FakeSession { thread: thread.clone(), events, log: Arc::default(), turns: Default::default() };
        session.emit(AdapterEvent::ThreadUpdated { thread });
        session.emit(AdapterEvent::CatalogUpdated {
            update: CatalogUpdate {
                commands: Some(vec![SlashCommand { name: "plan".into(), description: "Plan first".into(), input_hint: None }]),
                ..CatalogUpdate::default()
            },
        });
        Ok(Box::new(session))
    }
}

pub struct FakeInstance {
    id: InstanceId,
    snapshot: watch::Sender<ProviderSnapshot>,
}

#[async_trait]
impl ProviderInstance for FakeInstance {
    fn id(&self) -> &InstanceId {
        &self.id
    }

    fn snapshot(&self) -> watch::Receiver<ProviderSnapshot> {
        self.snapshot.subscribe()
    }

    async fn refresh(&self) -> Result<()> {
        Ok(())
    }

    fn auth(&self) -> Option<Arc<dyn AuthController>> {
        None
    }

    fn adapter(&self) -> Arc<dyn ProviderAdapter> {
        Arc::new(FakeAdapter)
    }

    fn sessions(&self) -> Option<Arc<dyn SessionCatalog>> {
        None
    }

    fn apply_catalog(&self, update: &CatalogUpdate) {
        self.snapshot.send_modify(|s| {
            if let Some(commands) = &update.commands {
                s.commands = commands.clone();
            }
        });
    }
}

pub struct FakeDriver;

/// The harness the fake driver runs, as instance "fake".
pub fn fake_harness() -> harness_contracts::HarnessEntry {
    harness_contracts::HarnessEntry {
        profile: harness_contracts::HarnessProfile { instance: InstanceId::new(FAKE), driver: DriverKind::new(FAKE), display_name: "Fake".into(), settings: serde_json::Value::Null },
        launch: harness_contracts::HarnessLaunch::Unavailable { reason: "fake".into(), fix: None },
    }
}

#[async_trait]
impl ProviderDriver for FakeDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata { kind: DriverKind::new(FAKE), display_name: "Fake".into(), supports_multiple_instances: false }
    }

    fn default_config(&self, _harness: &harness_contracts::HarnessEntry) -> serde_json::Value {
        serde_json::Value::Null
    }

    async fn create(&self, id: InstanceId, _config: serde_json::Value) -> Result<Arc<dyn ProviderInstance>> {
        let (snapshot, _) = watch::channel(ProviderSnapshot {
            instance_id: id.clone(),
            driver: DriverKind::new(FAKE),
            display_name: "Fake".into(),
            status: ProviderStatus::Ready,
            auth: AuthState { status: AuthStatus::SignedIn, account: None, methods: Vec::new() },
            capabilities: Capabilities::default(),
            models: Vec::new(),
            commands: Vec::new(),
            skills: Vec::new(),
            runtime_modes: vec![RuntimeMode::ApprovalRequired],
            supports_plan_mode: false,
            tool_servers: Vec::new(),
            problems: Vec::new(),
        });
        Ok(Arc::new(FakeInstance { id, snapshot }))
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The architecture service over a fresh copy of the Rust fixture repo
/// (`sample`: lib.rs, store.rs, util.rs), already ingested. The copy lives
/// for the rest of the test process.
pub fn fixture_architecture() -> Arc<ArchitectureService> {
    fixture_architecture_with(|a| a)
}

/// The fixture repo's service, set up further (tools, reviewers) before the
/// first ingest.
pub fn fixture_architecture_with(setup: impl FnOnce(ArchitectureService) -> ArchitectureService) -> Arc<ArchitectureService> {
    let root = lasting_dir().join("sample");
    copy_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/code-analyzer/tests/fixtures/rust/sample"), &root);
    let arch = ArchitectureService::open(
        &root,
        shipped_packs(),
        arch_graph::TypeSystem::defaults().unwrap(),
        Box::new(FsPlans::new(&root).unwrap()),
        arch_service::open_facts(&root, &arch_service::Limits::defaults().unwrap()).unwrap(),
    )
    .unwrap();
    let arch = setup(arch);
    arch.ingest(&root).unwrap();
    Arc::new(arch)
}

/// Serves the app with the fake provider configured as instance "fake".
pub async fn spawn_app() -> (SocketAddr, AppState) {
    spawn_app_with(fixture_architecture()).await
}

/// Serves the app over a given architecture service.
pub async fn spawn_app_with(arch: Arc<ArchitectureService>) -> (SocketAddr, AppState) {
    spawn_app_with_hub(arch, Arc::new(EventHub::default())).await
}

/// Serves the app over a given architecture service and event hub.
pub async fn spawn_app_with_hub(arch: Arc<ArchitectureService>, hub: Arc<EventHub>) -> (SocketAddr, AppState) {
    // Bound first, as the server does: sessions are given its real address.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    bridge_architecture_events(&arch, &hub);
    let remote = Arc::new(meta_harness::remote::Remote::default());
    let access = Arc::new(nah_access::Access::defaults());
    let registry = Arc::new(InstanceRegistry::new(vec![Arc::new(FakeDriver)], hub.clone()));
    registry.configure(registry.default_configs(&[fake_harness()])).await.unwrap();
    let sessions = Arc::new(SessionManager::new(registry.clone(), hub.clone()));
    let queue = meta_harness::server::queue_runner(&arch, arch_service::QueueConfig::defaults().unwrap(), &hub);
    let db = Arc::new(nah_state::StateDb::open(&arch_service::state_dir(arch.root()).unwrap(), &arch_service::store_settings(arch.limits())).unwrap());
    let tool_servers = meta_harness::startup::tool_servers(&base_url, arch.root());
    let slots = meta_harness::startup::slots(&sessions, &registry, &hub, &db, &base_url, &tool_servers).unwrap();
    let mcp = McpServerState::new(Arc::new(McpService::new(arch.clone(), remote.clone(), access.clone())), meta_harness::startup::mcp_callers(&slots));
    let state = AppState { registry, sessions, slots, hub, mcp_state: mcp, arch, cockpit: meta_harness::cockpit::CockpitAuth::new(COCKPIT_TOKEN), queue, state: db, tool_servers, remote, access, tickets: Arc::default() };
    let app = create_router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, state)
}

/// One HTTP/1.1 request from the cockpit (with its session cookie); returns
/// (status code, body).
pub async fn http(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    request(addr, method, path, body, &format!("Cookie: nah_cockpit={COCKPIT_TOKEN}\r\n")).await
}

/// The same request with no credentials: what anyone else on the machine gets.
pub async fn http_anonymous(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    request(addr, method, path, body, "").await
}

async fn request(addr: SocketAddr, method: &str, path: &str, body: Option<&str>, extra_headers: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let status = response.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = response.split_once("\r\n\r\n").map(|(_, b)| b.to_owned()).unwrap_or_default();
    (status, body)
}

/// A one-time ticket for a socket, as the cockpit gets one.
pub async fn ticket(addr: SocketAddr) -> String {
    let (status, body) = http(addr, "POST", "/api/cockpit/ticket", Some("{}")).await;
    assert_eq!(status, 200, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()["ticket"].as_str().unwrap().to_owned()
}

/// The event socket, opened as the cockpit opens it (with a ticket).
pub async fn events_socket(addr: SocketAddr) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let ticket = ticket(addr).await;
    tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events?ticket={ticket}")).await.unwrap().0
}

/// The test server's cockpit token (the user's, in a real launch).
pub const COCKPIT_TOKEN: &str = "test-cockpit-token";

/// The cockpit's session cookie, as the user's launch link sets it.
pub async fn cockpit_cookie(addr: SocketAddr) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("GET /cockpit?nah={COCKPIT_TOKEN} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let line = response.lines().find(|l| l.to_ascii_lowercase().starts_with("set-cookie:")).expect("the launch link sets the cookie");
    line.split_once(':').map(|(_, v)| v.trim()).unwrap_or_default().split(';').next().unwrap_or_default().to_owned()
}

/// An MCP client over the real endpoint (SSE stream + message posts).
pub struct Mcp {
    addr: SocketAddr,
    endpoint: String,
    events: BufReader<TcpStream>,
    next_id: u64,
}

impl Mcp {
    /// As an agent NAH didn't start (no key): read-only.
    pub async fn connect(addr: SocketAddr) -> Self {
        Self::connect_to(addr, "/api/mcp/sse").await
    }

    /// As the lead: opens a lead session on the fake harness and connects
    /// the way its agent would, with the address (and key) it was given.
    pub async fn lead(addr: SocketAddr) -> Self {
        let (status, body) = http(addr, "POST", "/api/threads", Some(&format!(r#"{{"instanceId":"{FAKE}"}}"#))).await;
        assert_eq!(status, 200, "{body}");
        let thread = ThreadId::new(serde_json::from_str::<Value>(&body).unwrap()["threadId"].as_str().unwrap());
        Self::as_session(addr, &thread).await
    }

    /// As a fake session's agent, with the address it was given.
    pub async fn as_session(addr: SocketAddr, thread: &ThreadId) -> Self {
        let ours = given_tools(thread).into_iter().find(|s| s.name == "nah-architecture").expect("the session was given our tools");
        let McpTransport::Sse { url, .. } = ours.transport else { panic!("ours is SSE: {ours:?}") };
        let path = url.split_once("//").and_then(|(_, rest)| rest.split_once('/')).map(|(_, p)| format!("/{p}")).unwrap();
        Self::connect_to(addr, &path).await
    }

    async fn connect_to(addr: SocketAddr, path: &str) -> Self {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n").as_bytes()).await.unwrap();
        let mut events = BufReader::new(stream);
        let endpoint = loop {
            let mut line = String::new();
            events.read_line(&mut line).await.unwrap();
            if let Some(url) = line.strip_prefix("data: /api/mcp/messages") {
                break format!("/api/mcp/messages{}", url.trim());
            }
        };
        Self { addr, endpoint, events, next_id: 1 }
    }

    pub async fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();
        let (status, _) = http(self.addr, "POST", &self.endpoint, Some(&body)).await;
        assert_eq!(status, 200);
        // The response also arrives on the SSE stream.
        loop {
            let mut line = String::new();
            self.events.read_line(&mut line).await.unwrap();
            if let Some(data) = line.strip_prefix("data: ")
                && let Ok(v) = serde_json::from_str::<Value>(data.trim())
                && v["id"] == id
            {
                return v;
            }
        }
    }

    /// Sends a notification (no id); returns the HTTP status.
    pub async fn notify(&mut self, method: &str) -> u16 {
        let body = json!({ "jsonrpc": "2.0", "method": method }).to_string();
        http(self.addr, "POST", &self.endpoint, Some(&body)).await.0
    }

    /// Sends a request and returns the very next message on the stream.
    pub async fn rpc_next(&mut self, method: &str) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method }).to_string();
        let (status, _) = http(self.addr, "POST", &self.endpoint, Some(&body)).await;
        assert_eq!(status, 200);
        loop {
            let mut line = String::new();
            self.events.read_line(&mut line).await.unwrap();
            if let Some(data) = line.strip_prefix("data: ")
                && let Ok(v) = serde_json::from_str::<Value>(data.trim())
            {
                return v;
            }
        }
    }

    /// Calls a tool; returns (is_error, text).
    pub async fn tool(&mut self, name: &str, args: Value) -> (bool, String) {
        let v = self.rpc("tools/call", json!({ "name": name, "arguments": args })).await;
        let result = &v["result"];
        (result["isError"].as_bool().unwrap_or(false), result["content"][0]["text"].as_str().unwrap_or_default().to_owned())
    }
}

/// The language packs shipped in this repo, without any project's or
/// user's own (tests must not depend on the machine).
pub fn shipped_packs() -> code_analyzer::PackSet {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packs");
    let places = nah_packs::Places::isolated(&std::env::temp_dir().join(format!("nah-test-packs-{}", std::process::id())));
    let catalog = nah_packs::Catalog::load(&nah_packs::Distribution::from_dir(&dir).unwrap(), &places).unwrap();
    code_analyzer::PackSet::from_catalog(&catalog).unwrap()
}

/// A folder for a fixture copy that lives as long as the test process, in
/// the build's own scratch folder: one per process, and copies left by runs
/// that ended over an hour ago are removed.
#[allow(dead_code)]
pub fn lasting_dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let all = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(env!("CARGO_CRATE_NAME"));
    if n == 0
        && let Ok(entries) = std::fs::read_dir(&all)
    {
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for entry in entries.flatten() {
            if entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < hour_ago) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let dir = all.join(format!("{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}
