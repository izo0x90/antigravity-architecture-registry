use std::path::PathBuf;
use std::sync::Arc;

use harness_contracts::{McpServer, McpTransport};
use harness_core::{EventHub, InstanceRegistry, ProviderDriver, SessionManager};
use arch_graph::{TypeSystem, ViewCatalog};
use arch_index::PackSet;
use arch_service::{ArchitectureService, FsIndex, FsPlans};
use meta_harness::mcp::{McpServerState, McpService};
use meta_harness::server::{AppState, create_router};
use provider_antigravity::AntigravityDriver;
use tracing::info;

/// Every harness the server can drive. Adding one means adding its driver here.
fn built_in_drivers() -> Vec<Arc<dyn ProviderDriver>> {
    vec![Arc::new(AntigravityDriver)]
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let port: u16 = std::env::var("PORT")
        .or_else(|_| std::env::var("METAHARNESS_PORT"))
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());

    // Bind first so the base URL handed to harnesses is the real one.
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    let bound = listener.local_addr()?;
    let base_url = format!("http://127.0.0.1:{}", bound.port());
    info!("Meta-harness listening on {bound} ({base_url})");

    let hub = Arc::new(EventHub::default());
    let arch = Arc::new(open_architecture()?);
    meta_harness::server::bridge_architecture_events(&arch, &hub);
    let mcp_state = McpServerState::new(Arc::new(McpService::new(arch.clone())));
    // First read of the code in the background; later reads are incremental.
    {
        let arch = arch.clone();
        tokio::task::spawn_blocking(move || match arch.ingest(arch.root()) {
            Ok(r) => info!("Ingested {} files ({} parsed) in {} ms: {} components", r.files, r.parsed, r.millis, r.components),
            Err(e) => tracing::warn!("Initial ingest failed: {e}"),
        });
    }
    let instances = Arc::new(InstanceRegistry::new(built_in_drivers(), hub.clone()));
    instances.configure(instances.default_configs()).await?;

    // Every session gets the architecture tools.
    let mcp_servers = vec![McpServer {
        name: "nah-architecture".into(),
        transport: McpTransport::Http { url: format!("{base_url}/api/mcp/sse"), headers: Vec::new() },
    }];
    let sessions = Arc::new(SessionManager::new(instances.clone(), hub.clone(), mcp_servers));

    let app = create_router(AppState { registry: instances, sessions, hub, mcp_state, arch });
    info!("MCP endpoint: {base_url}/api/mcp/sse; events: ws://{bound}/ws/events");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    info!("Meta-harness stopped.");
    Ok(())
}

/// The architecture service over the repo at `NAH_REPO_ROOT` (default: the
/// working directory). Projects can add language packs (`.nah/packs/*.toml`),
/// types (`.nah/types.toml`) and views (`.nah/views.toml`).
fn open_architecture() -> Result<ArchitectureService, Box<dyn std::error::Error>> {
    let root = std::env::var("NAH_REPO_ROOT").map(PathBuf::from).or_else(|_| std::env::current_dir())?;
    let root = root.canonicalize()?;
    let nah = root.join(".nah");
    let mut packs = PackSet::built_in()?;
    if nah.join("packs").is_dir() {
        packs.load_dir(&nah.join("packs"))?;
    }
    let mut types = TypeSystem::defaults()?;
    if let Ok(text) = std::fs::read_to_string(nah.join("types.toml")) {
        types.extend_with(TypeSystem::from_toml(&text)?)?;
    }
    let mut views = ViewCatalog::defaults()?;
    if let Ok(text) = std::fs::read_to_string(nah.join("views.toml")) {
        views.extend_with(ViewCatalog::from_toml(&text)?);
    }
    info!("Architecture: {} ({} language packs)", root.display(), packs.packs.len());
    Ok(ArchitectureService::open(&root, packs, types, views, Box::new(FsPlans::new(&root)?), Box::new(FsIndex::new(&root)?))?)
}

async fn shutdown_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
    info!("Shutdown signal received");
}
