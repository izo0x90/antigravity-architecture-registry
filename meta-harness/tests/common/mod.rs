//! Shared setup for server tests: a fake harness provider and an app on a random port.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use harness_contracts::*;
use harness_core::*;
use arch_service::{ArchitectureService, FsIndex, FsPlans};
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

    fn events(&self) -> BoxStream<'static, AdapterEvent> {
        let replay = self.log.lock().unwrap().clone();
        let live = tokio_stream::wrappers::BroadcastStream::new(self.events.subscribe()).filter_map(|e| async move { e.ok() });
        futures::stream::iter(replay).chain(live).boxed()
    }

    async fn start_turn(&self, input: TurnInput) -> Result<TurnId> {
        let turn = Turn {
            id: TurnId::new("turn-1"),
            thread_id: self.thread.id.clone(),
            ordinal: 1,
            status: TurnStatus::Completed,
            failure: None,
            usage: None,
        };
        self.emit(AdapterEvent::ItemUpdated {
            item: TurnItem {
                id: ItemId::new("item-1"),
                thread_id: self.thread.id.clone(),
                turn_id: turn.id.clone(),
                native_id: None,
                parent_id: None,
                ordinal: 0,
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

#[async_trait]
impl ProviderAdapter for FakeAdapter {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    async fn open_session(&self, config: SessionConfig) -> Result<Box<dyn SessionRuntime>> {
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
        let session = FakeSession { thread: thread.clone(), events, log: Arc::default() };
        session.emit(AdapterEvent::ThreadUpdated { thread });
        session.emit(AdapterEvent::CatalogUpdated {
            update: CatalogUpdate {
                commands: Some(vec![SlashCommand { name: "plan".into(), description: "Plan first".into(), input_hint: None }]),
                models: None,
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

#[async_trait]
impl ProviderDriver for FakeDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata { kind: DriverKind::new(FAKE), display_name: "Fake".into(), supports_multiple_instances: false }
    }

    fn default_config(&self) -> serde_json::Value {
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
    let root = tempfile::tempdir().unwrap().keep().join("sample");
    copy_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/arch-index/tests/fixtures/rust/sample"), &root);
    let arch = ArchitectureService::open(
        &root,
        arch_index::PackSet::built_in().unwrap(),
        arch_graph::TypeSystem::defaults().unwrap(),
        arch_graph::ViewCatalog::defaults().unwrap(),
        Box::new(FsPlans::new(&root).unwrap()),
        Box::new(FsIndex::new(&root).unwrap()),
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
    let hub = Arc::new(EventHub::default());
    bridge_architecture_events(&arch, &hub);
    let mcp = McpServerState::new(Arc::new(McpService::new(arch.clone())));
    let registry = Arc::new(InstanceRegistry::new(vec![Arc::new(FakeDriver)], hub.clone()));
    registry.configure(registry.default_configs()).await.unwrap();
    let sessions = Arc::new(SessionManager::new(registry.clone(), hub.clone(), Vec::new()));
    let queue = meta_harness::server::queue_runner(&arch, arch_service::QueueConfig::defaults().unwrap(), &hub);
    let state = AppState { registry, sessions, hub, mcp_state: mcp, arch, cockpit: meta_harness::cockpit::CockpitAuth::random(), queue };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = create_router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, state)
}

/// One HTTP/1.1 request; returns (status code, body).
pub async fn http(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
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

/// The cockpit's session cookie, as the cockpit gets it when it loads.
pub async fn cockpit_cookie(addr: SocketAddr) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(b"GET /api/ui/config HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let line = response.lines().find(|l| l.to_ascii_lowercase().starts_with("set-cookie:")).expect("the cockpit gets a cookie");
    line.split_once(':').map(|(_, v)| v.trim()).unwrap_or_default().split(';').next().unwrap_or_default().to_owned()
}

/// Like `http`, from the cockpit (with its session cookie): what only the user may do.
pub async fn http_cockpit(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let cookie = cockpit_cookie(addr).await;
    request(addr, method, path, body, &format!("Cookie: {cookie}\r\n")).await
}

/// An MCP client over the real endpoint (SSE stream + message posts).
pub struct Mcp {
    addr: SocketAddr,
    endpoint: String,
    events: BufReader<TcpStream>,
    next_id: u64,
}

impl Mcp {
    pub async fn connect(addr: SocketAddr) -> Self {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(b"GET /api/mcp/sse HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n").await.unwrap();
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

    /// Calls a tool; returns (is_error, text).
    pub async fn tool(&mut self, name: &str, args: Value) -> (bool, String) {
        let v = self.rpc("tools/call", json!({ "name": name, "arguments": args })).await;
        let result = &v["result"];
        (result["isError"].as_bool().unwrap_or(false), result["content"][0]["text"].as_str().unwrap_or_default().to_owned())
    }
}
