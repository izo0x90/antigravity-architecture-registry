use std::path::PathBuf;
use std::sync::Arc;

use harness_contracts::{McpServer, McpTransport};
use harness_core::{EventHub, InstanceRegistry, ProviderDriver, SessionManager};
use arch_service::{ArchitectureService, FsPlans, ProcessRunner, ReviewerCatalog, ToolCatalog};
use meta_harness::mcp::{McpServerState, McpService};
use meta_harness::server::{AppState, create_router};
use tracing::info;

/// The drivers harness packs can name. Every ACP agent runs on the one
/// ACP driver, configured by its pack.
fn built_in_drivers(project: &std::path::Path) -> Vec<Arc<dyn ProviderDriver>> {
    vec![Arc::new(harness_acp::AcpDriver::new(project.to_path_buf()))]
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
    /// The server's pipeline over a project (read, link); writes the
    /// result as JSON.
    Analyze {
        /// The project (default: NAH_REPO_ROOT, else the working directory).
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Packs: harnesses, languages and their servers.
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
    if let Some(Cmd::Analyze { root, out }) = &cli.command {
        let root = match root {
            Some(r) => r.canonicalize()?,
            None => repo_root()?,
        };
        match meta_harness::project::analyze_to(&root, out) {
            Ok(summary) => println!("{summary}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
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
    // The cockpit is built, not kept in git: without a build there's
    // nothing to serve.
    let build = meta_harness::server::static_dir().join(meta_harness::server::COCKPIT_BUILD);
    if !build.is_file() {
        return Err(format!("the cockpit isn't built ({} is missing): run `make ui` in the repository first", build.display()).into());
    }

    // Bind first so the base URL handed to harnesses is the real one.
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    let bound = listener.local_addr()?;
    let base_url = format!("http://127.0.0.1:{}", bound.port());
    info!("Meta-harness listening on {bound} ({base_url})");

    let hub = Arc::new(EventHub::default());
    let arch = Arc::new(open_architecture()?);
    let ui_state = Arc::new(arch_service::UiStateFile::open(arch.root())?);
    meta_harness::server::bridge_architecture_events(&arch, &hub);
    let remote = Arc::new(meta_harness::remote::Remote::default());
    let mcp_state = McpServerState::new(Arc::new(McpService::new(arch.clone(), remote.clone())));
    // First read of the code in the background; later reads are incremental.
    {
        let arch = arch.clone();
        tokio::task::spawn_blocking(move || {
            match arch.ingest(arch.root()) {
                Ok(r) => info!("Ingested {} files ({} parsed) in {} ms: {} components", r.files, r.parsed, r.millis, r.components),
                Err(e) => tracing::warn!("Initial ingest failed: {e}"),
            }
            // Then follow the code (limits.toml's watch.debounce_ms; 0: off).
            if let Err(e) = meta_harness::watch::watch(arch.clone()) {
                tracing::warn!("Not watching the repo for changes: {e}");
            }
        });
    }
    let instances = Arc::new(InstanceRegistry::new(built_in_drivers(arch.root()), hub.clone()));
    // Each harness gets its launch (its own, else its pack's once the pack
    // passes its check, which can take a while emulated): set up in the
    // background, so the server answers meanwhile.
    let catalog = meta_harness::packs::catalog(arch.root())?;
    let configuring = instances.clone();
    tokio::spawn(async move {
        let launches = match tokio::task::spawn_blocking(move || meta_harness::packs::harness_launches(&catalog)).await {
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

    // Every session gets the architecture tools.
    let mcp_servers = vec![McpServer {
        name: "nah-architecture".into(),
        transport: McpTransport::Sse { url: format!("{base_url}/api/mcp/sse"), headers: Vec::new() },
    }];
    let tool_servers = Arc::new(meta_harness::tool_servers::ToolServerState { ours: mcp_servers.clone(), file: meta_harness::tool_servers::ProjectToolServers::of(arch.root()) });
    if let Err(e) = tool_servers.project() {
        tracing::warn!("The project's tool servers can't be read: {e}");
    }
    let sessions = Arc::new(SessionManager::new(instances.clone(), hub.clone(), mcp_servers));

    let queue = meta_harness::server::queue_runner(&arch, queue_config(&arch)?, &hub);
    let cockpit = meta_harness::cockpit::CockpitAuth::for_user()?;
    let cockpit_path = cockpit.launch_path();
    let streams = mcp_state.sessions.clone();
    let app = create_router(AppState { registry: instances, sessions, hub, mcp_state, arch, cockpit, queue, ui_state, tool_servers, remote });
    info!("MCP endpoint: {base_url}/api/mcp/sse; events: ws://{bound}/ws/events");
    info!("Open the cockpit: {base_url}{cockpit_path}");
    // Stopping ends the agents' event streams, which would otherwise keep
    // the graceful shutdown waiting.
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            streams.close_all().await;
        })
        .await?;
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
    let meta_harness::project::AnalysisSetup { catalog, packs, types, rules, limits } = meta_harness::project::analysis_setup(&root).map_err(|e| e.to_string())?;
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
