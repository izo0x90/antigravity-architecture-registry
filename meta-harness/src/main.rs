use std::path::PathBuf;
use std::sync::Arc;

use harness_core::{EventHub, SessionManager};
use meta_harness::mcp::{McpServerState, McpService};
use meta_harness::server::{AppState, create_router};
use tracing::info;

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
            None => meta_harness::startup::repo_root()?,
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
        let catalog = meta_harness::packs::catalog(&meta_harness::startup::repo_root()?)?;
        match meta_harness::packs::run(&command, &catalog) {
            Ok(out) => print!("{out}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    meta_harness::startup::check_cockpit_build()?;
    let (listener, base_url) = meta_harness::startup::listen().await?;
    let bound = listener.local_addr()?;

    let hub = Arc::new(EventHub::default());
    let arch = Arc::new(meta_harness::startup::open_architecture()?);
    let state = Arc::new(meta_harness::startup::state(&arch)?);
    meta_harness::server::bridge_architecture_events(&arch, &hub);
    let remote = Arc::new(meta_harness::remote::Remote::default());
    let access = Arc::new(meta_harness::startup::access(arch.root())?);
    meta_harness::startup::follow_code(&arch);
    let instances = meta_harness::startup::harnesses(arch.root(), &hub)?;
    let tool_servers = meta_harness::startup::tool_servers(&base_url, arch.root());
    let sessions = Arc::new(SessionManager::new(instances.clone(), hub.clone()));
    let slots = meta_harness::startup::slots(&sessions, &instances, &hub, &state, &base_url, &tool_servers)?;
    let mcp_state = McpServerState::new(Arc::new(McpService::new(arch.clone(), remote.clone(), access.clone(), slots.clone())), meta_harness::startup::mcp_callers(&slots));

    let queue = meta_harness::server::queue_runner(&arch, meta_harness::startup::queue_config(&arch)?, &hub);
    let cockpit = meta_harness::cockpit::CockpitAuth::for_user()?;
    let cockpit_path = cockpit.launch_path();
    let streams = mcp_state.sessions.clone();
    let app = create_router(AppState { registry: instances, sessions, slots, hub, mcp_state, arch, cockpit, queue, state, tool_servers, remote, access, tickets: Arc::default() });
    info!("MCP endpoint: {base_url}/api/mcp/sse; events: ws://{bound}/ws/events");
    info!("Open the cockpit: {base_url}{cockpit_path}");
    meta_harness::startup::serve(listener, app, streams).await?;
    info!("Meta-harness stopped.");
    Ok(())
}
