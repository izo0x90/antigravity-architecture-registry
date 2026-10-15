//! Starting the server: where it listens, the project, the harnesses, the
//! tool servers, the queue, and serving until asked to stop. `main` runs
//! these in order.

use std::path::PathBuf;
use std::sync::Arc;

use arch_service::{ArchitectureService, FsPlans, ProcessRunner, ReviewerCatalog, ToolCatalog};
use harness_contracts::{McpServer, McpTransport};
use harness_core::{EventHub, InstanceRegistry, ProviderDriver};
use tracing::info;

pub type StartError = Box<dyn std::error::Error>;

/// The drivers harness packs can name. Every ACP agent runs on the one
/// ACP driver, configured by its pack.
fn built_in_drivers(project: &std::path::Path) -> Vec<Arc<dyn ProviderDriver>> {
    vec![Arc::new(harness_acp::AcpDriver::new(project.to_path_buf()))]
}

/// The repo worked on: `NAH_REPO_ROOT`, else the working directory.
pub fn repo_root() -> Result<PathBuf, StartError> {
    let root = std::env::var("NAH_REPO_ROOT").map(PathBuf::from).or_else(|_| std::env::current_dir())?;
    Ok(root.canonicalize()?)
}

/// The cockpit is built, not kept in git: without a build there's nothing to serve.
pub fn check_cockpit_build() -> Result<(), StartError> {
    let build = crate::server::static_dir().join(crate::server::COCKPIT_BUILD);
    if !build.is_file() {
        return Err(format!("the cockpit isn't built ({} is missing): run `make ui` in the repository first", build.display()).into());
    }
    Ok(())
}

/// Binds where `HOST` and `PORT` (or `METAHARNESS_PORT`) say, first, so the
/// base URL handed to harnesses is the real one.
pub async fn listen() -> Result<(tokio::net::TcpListener, String), StartError> {
    let port: u16 = std::env::var("PORT")
        .or_else(|_| std::env::var("METAHARNESS_PORT"))
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    let bound = listener.local_addr()?;
    let base_url = format!("http://127.0.0.1:{}", bound.port());
    info!("Meta-harness listening on {bound} ({base_url})");
    Ok((listener, base_url))
}

/// The architecture service over the repo at `NAH_REPO_ROOT` (default: the
/// working directory). Languages come from the packs (shipped, then the
/// project's `.nah/packs/` and the user's); projects can add
/// types (`.nah/types.toml`), the views (`.nah/trees.toml`), limits
/// (`.nah/limits.toml`), validation tools
/// (`.nah/tools.toml`), reviewers (`.nah/reviewers.toml`) and schema
/// shorthand aliases (`.nah/schema_dsl.toml`).
pub fn open_architecture() -> Result<ArchitectureService, StartError> {
    let root = repo_root()?;
    let nah = root.join(".nah");
    let crate::project::AnalysisSetup { catalog, packs, types, rules, limits } = crate::project::analysis_setup(&root).map_err(|e| e.to_string())?;
    let mut tools = ToolCatalog::default();
    if let Ok(text) = std::fs::read_to_string(nah.join("tools.toml")) {
        tools.extend_with(ToolCatalog::from_toml(&text)?);
    }
    let mut reviewers = ReviewerCatalog::defaults()?;
    if let Ok(text) = std::fs::read_to_string(nah.join("reviewers.toml")) {
        reviewers.extend_with(ReviewerCatalog::from_toml(&text)?);
    }
    info!(
        "Architecture: {} ({} language packs, {} code rules, {} tools, {} reviewers)",
        root.display(),
        packs.packs.len(),
        rules.rules.len(),
        tools.tools.len(),
        reviewers.reviewers.len()
    );
    // No review session provider here: session reviewers (none by default)
    // report that they couldn't run.
    let mut dsl = arch_graph::Dsl::defaults()?;
    if let Ok(text) = std::fs::read_to_string(nah.join("schema_dsl.toml")) {
        dsl.extend_with(arch_graph::Dsl::from_toml(&text)?)?;
    }
    let trees = match std::fs::read_to_string(nah.join("trees.toml")) {
        Ok(text) => arch_graph::tree::TreeConfig::with_overrides(&text)?,
        Err(_) => arch_graph::tree::TreeConfig::defaults()?,
    };
    Ok(ArchitectureService::open(&root, packs, types, Box::new(FsPlans::new(&root)?), arch_service::open_facts(&root, &limits)?)?
        .with_trees(trees)
        .with_limits(limits)
        .with_dsl(dsl)
        .with_rules(rules)
        .with_tools(tools, Arc::new(ProcessRunner))
        .with_servers(&catalog)
        .with_reviewers(reviewers, None))
}

/// The first read of the code, in the background; then follow it
/// (limits.toml's watch.debounce_ms; 0: off). Later reads are incremental.
pub fn follow_code(arch: &Arc<ArchitectureService>) {
    let arch = arch.clone();
    tokio::task::spawn_blocking(move || {
        match arch.ingest(arch.root()) {
            Ok(r) => info!("Ingested {} files ({} parsed) in {} ms: {} components", r.files, r.parsed, r.millis, r.components),
            Err(e) => tracing::warn!("Initial ingest failed: {e}"),
        }
        if let Err(e) = crate::watch::watch(arch.clone()) {
            tracing::warn!("Not watching the repo for changes: {e}");
        }
    });
}

/// The harness instances. Each harness gets its launch (its own, else its
/// pack's once the pack passes its check, which can take a while emulated):
/// set up in the background, so the server answers meanwhile.
pub fn harnesses(project: &std::path::Path, hub: &Arc<EventHub>) -> Result<Arc<InstanceRegistry>, StartError> {
    let instances = Arc::new(InstanceRegistry::new(built_in_drivers(project), hub.clone()));
    let catalog = crate::packs::catalog(project)?;
    let configuring = instances.clone();
    tokio::spawn(async move {
        let launches = match tokio::task::spawn_blocking(move || crate::packs::harness_launches(&catalog)).await {
            Ok(launches) => launches,
            Err(e) => {
                tracing::warn!("Checking the harness packs failed: {e}");
                Default::default()
            }
        };
        if let Err(e) = configuring.configure(configuring.default_configs(&launches)).await {
            tracing::warn!("Couldn't set up the harnesses: {e}");
        }
    });
    Ok(instances)
}

/// The project's state database (`.nah/state/`), locked for this server.
pub fn state(arch: &ArchitectureService) -> Result<nah_state::StateDb, StartError> {
    let dir = arch_service::state_dir(arch.root())?;
    Ok(nah_state::StateDb::open(&dir, &arch_service::store_settings(arch.limits()))?)
}

/// What each agent role may do: the shipped roles, with the project's
/// `.nah/access.toml` over them.
pub fn access(project: &std::path::Path) -> Result<nah_access::Access, StartError> {
    let access = nah_access::Access::defaults();
    Ok(match std::fs::read_to_string(project.join(".nah/access.toml")) {
        Ok(text) => access.with_overrides(&text)?,
        Err(_) => access,
    })
}

/// The server's own tool server, as every session gets it: at its own
/// address, with its key (which says which slot is calling).
pub fn our_tools(base_url: &str) -> harness_core::slots::OurServer {
    let url = format!("{base_url}/api/mcp/sse");
    Arc::new(move |key: &str| McpServer { name: OUR_TOOLS.into(), transport: McpTransport::Sse { url: format!("{url}?key={key}"), headers: Vec::new() } })
}

/// The name agents see our tools under.
pub const OUR_TOOLS: &str = "nah-architecture";

/// The tool servers listed for the cockpit: ours (no key) and the project's.
pub fn tool_servers(base_url: &str, project: &std::path::Path) -> Arc<crate::tool_servers::ToolServerState> {
    let ours = vec![McpServer { name: OUR_TOOLS.into(), transport: McpTransport::Sse { url: format!("{base_url}/api/mcp/sse"), headers: Vec::new() } }];
    let tool_servers = Arc::new(crate::tool_servers::ToolServerState { ours, file: crate::tool_servers::ProjectToolServers::of(project) });
    if let Err(e) = tool_servers.project() {
        tracing::warn!("The project's tool servers can't be read: {e}");
    }
    tool_servers
}

/// Who holds an MCP key: the slot whose running session it was made for.
pub fn mcp_callers(slots: &Arc<harness_core::SlotManager>) -> crate::mcp::sse::Callers {
    let slots = slots.clone();
    Arc::new(move |key: &str| {
        slots.holder(key).map(|h| crate::mcp::McpCaller { principal: nah_access::Principal::agent(&h.role), slot: Some(h.slot), label: h.name })
    })
}

/// How long a harness may take to open a session.
pub const OPEN_WITHIN: std::time::Duration = std::time::Duration::from_secs(120);

/// The slots, as kept in the state database.
pub fn slots(
    sessions: &Arc<harness_core::SessionManager>,
    registry: &Arc<InstanceRegistry>,
    hub: &Arc<EventHub>,
    state: &Arc<nah_state::StateDb>,
    base_url: &str,
    tool_servers: &Arc<crate::tool_servers::ToolServerState>,
) -> Result<Arc<harness_core::SlotManager>, StartError> {
    let project = tool_servers.clone();
    Ok(harness_core::SlotManager::load(sessions.clone(), registry.clone(), hub.clone(), state.clone(), our_tools(base_url), Arc::new(move || project.project()), OPEN_WITHIN)?)
}

pub fn queue_config(arch: &ArchitectureService) -> Result<arch_service::QueueConfig, StartError> {
    let mut config = arch_service::QueueConfig::defaults()?;
    if let Ok(text) = std::fs::read_to_string(arch.root().join(".nah/queue.toml")) {
        config.extend_from_toml(&text)?;
    }
    Ok(config)
}

/// Serves until asked to stop. Stopping ends the agents' event streams,
/// which would otherwise keep the graceful shutdown waiting.
pub async fn serve(listener: tokio::net::TcpListener, app: axum::Router, streams: Arc<crate::mcp::SseSessionManager>) -> Result<(), StartError> {
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            streams.close_all().await;
        })
        .await?;
    Ok(())
}

/// Ctrl-C, or (on unix) a request to stop: the server stops cleanly, and
/// the agents it started with it.
async fn shutdown_signal() {
    let interrupted = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminated = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stop) => {
                stop.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminated = std::future::pending::<()>();
    tokio::select! {
        _ = interrupted => {}
        _ = terminated => {}
    }
    info!("Shutdown signal received");
}
