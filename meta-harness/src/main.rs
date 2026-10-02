use std::path::PathBuf;
use std::sync::Arc;

use harness_contracts::{McpServer, McpTransport};
use harness_core::{EventHub, InstanceRegistry, ProviderDriver, SessionManager};
use meta_harness::mcp::{McpServerState, McpService};
use meta_harness::registry::{FsJsonRepository, RegistryService};
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

    let repo_path = std::env::var("ARCHITECTURE_REGISTRY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("system_architecture.json"));
    let registry_service = Arc::new(RegistryService::new(Arc::new(FsJsonRepository::new_or_init(repo_path).await?)));
    let mcp_state = McpServerState::new(Arc::new(McpService::new(registry_service)));

    let hub = Arc::new(EventHub::default());
    let instances = Arc::new(InstanceRegistry::new(built_in_drivers(), hub.clone()));
    instances.configure(instances.default_configs()).await?;

    // Every session gets our architecture-registry MCP tools.
    let mcp_servers = vec![McpServer {
        name: "architecture-registry".into(),
        transport: McpTransport::Http { url: format!("{base_url}/api/mcp/sse"), headers: Vec::new() },
    }];
    let sessions = Arc::new(SessionManager::new(instances.clone(), hub.clone(), mcp_servers));

    let app = create_router(AppState { registry: instances, sessions, hub, mcp_state });
    info!("MCP endpoint: {base_url}/api/mcp/sse; events: ws://{bound}/ws/events");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    info!("Meta-harness stopped.");
    Ok(())
}

async fn shutdown_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
    info!("Shutdown signal received");
}
