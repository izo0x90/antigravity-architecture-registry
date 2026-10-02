//! Shared setup for server tests: a fake harness provider and an app on a random port.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use harness_contracts::*;
use harness_core::*;
use meta_harness::mcp::{McpServerState, McpService};
use meta_harness::registry::{FsJsonRepository, RegistryService};
use meta_harness::server::{AppState, create_router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

pub fn mcp_state(registry: Arc<RegistryService>) -> McpServerState {
    McpServerState::new(Arc::new(McpService::new(registry)))
}

pub async fn empty_registry_service() -> Arc<RegistryService> {
    let file = tempfile::NamedTempFile::new().unwrap();
    let repo = FsJsonRepository::from_architecture(architecture_model::SystemArchitecture::default(), file.path());
    Arc::new(RegistryService::new(Arc::new(repo)))
}

/// Serves the app with the fake provider configured as instance "fake".
pub async fn spawn_app(mcp: McpServerState) -> (SocketAddr, AppState) {
    let hub = Arc::new(EventHub::default());
    let registry = Arc::new(InstanceRegistry::new(vec![Arc::new(FakeDriver)], hub.clone()));
    registry.configure(registry.default_configs()).await.unwrap();
    let sessions = Arc::new(SessionManager::new(registry.clone(), hub.clone(), Vec::new()));
    let state = AppState { registry, sessions, hub, mcp_state: mcp };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = create_router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, state)
}

/// One HTTP/1.1 request; returns (status code, body).
pub async fn http(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let status = response.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = response.split_once("\r\n\r\n").map(|(_, b)| b.to_owned()).unwrap_or_default();
    (status, body)
}
