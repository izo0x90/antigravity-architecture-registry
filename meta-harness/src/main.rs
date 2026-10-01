use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;
use tracing::info;

use meta_harness::adapter::{find_antigravity_binary, AntigravityAdapter};
use meta_harness::driver::{
    AntigravityDriver, AntigravityDriverConfig, AuthMethod, DriverCreateOptions,
};
use meta_harness::injector::AgyToolInjector;
use meta_harness::installation::{AntigravityInstallation, AntigravityInstallationOptions};
use meta_harness::mcp::{McpServerState, McpService};
use meta_harness::process::{make_native_runtime_factory_with_injector, TokioProcessSpawner};
use meta_harness::registry::{FsJsonRepository, RegistryService};
use meta_harness::server::{create_router, AppState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let port: u16 = std::env::var("PORT")
        .or_else(|_| std::env::var("METAHARNESS_PORT"))
        .unwrap_or_else(|_| "8080".to_string())
        .parse()
        .unwrap_or(8080);

    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let addr = format!("{}:{}", host, port);

    // 1. Bind TCP listener FIRST to guarantee the exact runtime port
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound_addr = listener.local_addr()?;
    let live_port = bound_addr.port();
    let base_url_str = format!("http://127.0.0.1:{}", live_port);

    info!("=== Meta-Harness Control Plane Starting ===");
    info!("Bound to socket: {} (Base URL: {})", bound_addr, base_url_str);

    let base_url = Arc::new(RwLock::new(Some(base_url_str.clone())));
    let base_url_clone = base_url.clone();
    let base_url_provider = Arc::new(move || {
        base_url_clone.read().ok().and_then(|g| g.clone())
    });

    // 2. Discover the real Antigravity binary
    let agy_bin = find_antigravity_binary()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/Users/izo/.local/bin/agy".to_string());

    info!("Target Antigravity Binary: {}", agy_bin);

    // 3. Initialize Architecture Registry and In-Process MCP Service
    let repo_path = std::env::var("ARCHITECTURE_REGISTRY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("system_architecture.json"));
    let repo = Arc::new(FsJsonRepository::new_or_init(repo_path).await?);
    let registry = Arc::new(RegistryService::new(repo));

    let injector = Arc::new(AgyToolInjector::default());

    // 4. Build Process Spawner and Native Runtime Factory with Tool Injector
    let spawner = Arc::new(TokioProcessSpawner::new());
    let runtime_factory = make_native_runtime_factory_with_injector(
        spawner.clone(),
        agy_bin.clone(),
        vec![],
        AuthMethod::OAuthPersonal,
        None,
        Some(injector),
        Some(base_url_provider),
    );

    // 5. Initialize Antigravity Adapter
    let adapter = Arc::new(AntigravityAdapter::with_runtime_factory(runtime_factory));

    // 6. Initialize McpService and ServerState
    let mcp_service = Arc::new(McpService::new(
        registry.clone(),
        Some(adapter.event_sender().clone()),
    ));
    let mcp_state = McpServerState::new(mcp_service);

    // 7. Initialize Antigravity Driver
    let profile_dir = std::env::temp_dir().join("meta-harness-driver-profile");
    let installation = Arc::new(AntigravityInstallation::new(
        AntigravityInstallationOptions {
            base_dir: profile_dir.clone(),
            ..Default::default()
        },
    )?);
    let driver_config = AntigravityDriverConfig {
        auth_method: AuthMethod::OAuthPersonal,
        api_key: None,
    };

    let mut driver = AntigravityDriver::create(DriverCreateOptions {
        instance_id: "antigravity-local".to_string(),
        display_name: "Google Antigravity Local".to_string(),
        enabled: true,
        config: driver_config,
        environment: std::env::vars().collect(),
        profile_directory: profile_dir,
        installation,
        spawner: spawner.clone(),
    })?;

    // Refresh models dynamically from the real CLI if available
    match driver.refresh_models() {
        Ok(()) => {
            let snap = driver.get_snapshot();
            info!(
                "Discovered {} models from Antigravity CLI",
                snap.models.len()
            );
            for m in &snap.models {
                info!("  - Model: {} ({})", m.name, m.slug);
            }
        }
        Err(e) => {
            info!("Driver model discovery notice: {}", e);
        }
    }

    let driver = Arc::new(Mutex::new(driver));

    // 8. Create Axum Router & Start Server
    let state = AppState {
        adapter,
        driver,
        mcp_state,
    };
    let app = create_router(state);

    info!("Meta-Harness HTTP & WebSocket server running at {}", base_url_str);
    info!("MCP SSE Endpoint: {}/api/mcp/sse", base_url_str);
    info!("MCP Messages Endpoint: {}/api/mcp/messages", base_url_str);
    info!("WebSocket Events Endpoint: ws://{}/ws/events", bound_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Meta-Harness server stopped cleanly.");
    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");
    info!("Shutdown signal received, shutting down gracefully...");
}
