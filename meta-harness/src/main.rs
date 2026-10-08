use std::path::PathBuf;
use std::sync::Arc;

use harness_contracts::{McpServer, McpTransport};
use harness_core::{EventHub, InstanceRegistry, ProviderDriver, SessionManager};
use arch_graph::TypeSystem;
use arch_index::{CodeRules, PackSet};
use arch_service::{ArchitectureService, FsPlans, ProcessRunner, ReviewerCatalog, ToolCatalog};
use meta_harness::mcp::{McpServerState, McpService};
use meta_harness::server::{AppState, create_router};
use provider_antigravity::AntigravityDriver;
use tracing::info;

/// Every harness the server can drive. Adding one means adding its driver here.
fn built_in_drivers() -> Vec<Arc<dyn ProviderDriver>> {
    vec![Arc::new(AntigravityDriver)]
}

/// The meta-harness: with no command, the server.
#[derive(clap::Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Packs: harnesses, languages and indexers.
    Pack {
        #[command(subcommand)]
        action: PackCmd,
    },
}

#[derive(clap::Subcommand)]
enum PackCmd {
    /// Every pack: kind, version, supported tool versions, state.
    List,
    /// One pack's state (or every pack's).
    Check { name: Option<String> },
    /// Install a pack.
    Install { name: String },
    /// Reinstall what's outdated or out of range.
    Update { name: Option<String> },
    /// Where an installed pack's program is.
    Which { name: String },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let cli = <Cli as clap::Parser>::parse();
    if let Some(Cmd::Pack { action }) = cli.command {
        use meta_harness::packs::Command as C;
        let command = match action {
            PackCmd::List => C::List,
            PackCmd::Check { name } => C::Check(name),
            PackCmd::Install { name } => C::Install(name),
            PackCmd::Update { name } => C::Update(name),
            PackCmd::Which { name } => C::Which(name),
        };
        let catalog = meta_harness::packs::catalog(&repo_root()?)?;
        match meta_harness::packs::run(&command, &catalog) {
            Ok(out) => print!("{out}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

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
        tokio::task::spawn_blocking(move || {
            match arch.ingest(arch.root()) {
                Ok(r) => info!("Ingested {} files ({} parsed) in {} ms: {} components", r.files, r.parsed, r.millis, r.components),
                Err(e) => tracing::warn!("Initial ingest failed: {e}"),
            }
            // Then follow the code: NAH_WATCH_MS is the quiet time before a
            // burst of changes is read (0 turns watching off).
            let quiet = std::env::var("NAH_WATCH_MS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(300);
            if quiet > 0
                && let Err(e) = meta_harness::watch::watch(arch.clone(), std::time::Duration::from_millis(quiet))
            {
                tracing::warn!("Not watching the repo for changes: {e}");
            }
        });
    }
    let instances = Arc::new(InstanceRegistry::new(built_in_drivers(), hub.clone()));
    // Each harness gets the program its pack installed.
    let programs = meta_harness::packs::harness_programs(&meta_harness::packs::catalog(arch.root())?)?;
    instances.configure(instances.default_configs(&programs)).await?;

    // Every session gets the architecture tools.
    let mcp_servers = vec![McpServer {
        name: "nah-architecture".into(),
        transport: McpTransport::Http { url: format!("{base_url}/api/mcp/sse"), headers: Vec::new() },
    }];
    let sessions = Arc::new(SessionManager::new(instances.clone(), hub.clone(), mcp_servers));

    let queue = meta_harness::server::queue_runner(&arch, queue_config(&arch)?, &hub);
    let cockpit = meta_harness::cockpit::CockpitAuth::for_user()?;
    let cockpit_path = cockpit.launch_path();
    let app = create_router(AppState { registry: instances, sessions, hub, mcp_state, arch, cockpit, queue });
    info!("MCP endpoint: {base_url}/api/mcp/sse; events: ws://{bound}/ws/events");
    info!("Open the cockpit: {base_url}{cockpit_path}");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    info!("Meta-harness stopped.");
    Ok(())
}

/// The repo worked on: `NAH_REPO_ROOT`, else the working directory.
fn repo_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let root = std::env::var("NAH_REPO_ROOT").map(PathBuf::from).or_else(|_| std::env::current_dir())?;
    Ok(root.canonicalize()?)
}

/// How the work queue runs: defaults plus the project's `.nah/queue.toml`.
fn queue_config(arch: &ArchitectureService) -> Result<arch_service::QueueConfig, Box<dyn std::error::Error>> {
    let mut config = arch_service::QueueConfig::defaults()?;
    if let Ok(text) = std::fs::read_to_string(arch.root().join(".nah/queue.toml")) {
        config.extend_from_toml(&text)?;
    }
    Ok(config)
}

/// The architecture service over the repo at `NAH_REPO_ROOT` (default: the
/// working directory). Languages come from the packs (shipped, then the
/// project's `.nah/packs/` and the user's); projects can add
/// types (`.nah/types.toml`), the views (`.nah/trees.toml`), limits
/// (`.nah/limits.toml`), validation tools
/// (`.nah/tools.toml`), reviewers (`.nah/reviewers.toml`) and schema
/// shorthand aliases (`.nah/schema_dsl.toml`).
fn open_architecture() -> Result<ArchitectureService, Box<dyn std::error::Error>> {
    let root = repo_root()?;
    let nah = root.join(".nah");
    let packs = PackSet::from_catalog(&meta_harness::packs::catalog(&root)?)?;
    let mut types = TypeSystem::defaults()?;
    if let Ok(text) = std::fs::read_to_string(nah.join("types.toml")) {
        types.extend_from_toml(&text)?;
    }
    let mut rules = CodeRules::defaults(&packs, &types)?;
    if nah.join("rules").is_dir() {
        rules.load_dir(&nah.join("rules"), &packs, &types)?;
    }
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
    let limits = match std::fs::read_to_string(nah.join("limits.toml")) {
        Ok(text) => arch_service::Limits::with_overrides(&text)?,
        Err(_) => arch_service::Limits::defaults()?,
    };
    Ok(ArchitectureService::open(&root, packs, types, Box::new(FsPlans::new(&root)?), Box::new(arch_service::open_facts(&root, &limits)?))?
        .with_trees(trees)
        .with_limits(limits)
        .with_dsl(dsl)
        .with_rules(rules)
        .with_tools(tools, Arc::new(ProcessRunner))
        .with_reviewers(reviewers, None))
}

async fn shutdown_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
    info!("Shutdown signal received");
}
