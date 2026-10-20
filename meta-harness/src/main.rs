use std::path::PathBuf;
use std::sync::Arc;

use harness_core::{EventHub, SessionManager};
use meta_harness::mcp::{McpServerState, McpService};
use meta_harness::modules::{AppState, Parts};
use meta_harness::server::create_router;
use tracing::info;

/// The meta-harness: with no command, the server.
#[derive(clap::Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
    #[command(flatten)]
    serve: ServeArgs,
}

/// How the server runs (with no command, or `serve`).
#[derive(clap::Args, Clone, Default)]
struct ServeArgs {
    /// Don't serve the cockpit (its page, its saved state): agents and other
    /// servers only.
    #[arg(long)]
    no_cockpit: bool,
    /// Serve no project: the cockpit only (a lighter process).
    #[arg(long)]
    no_project: bool,
    /// Run no shells: the cockpit's terminals and the agents' terminal
    /// tools are left out.
    #[arg(long)]
    no_shells: bool,
    /// Where to listen, as address:port (default: HOST and PORT, else
    /// 127.0.0.1:8080).
    #[arg(long)]
    listen: Option<String>,
    /// Allow an address that isn't private (by default only loopback, a LAN
    /// and Tailscale's 100.64/10).
    #[arg(long)]
    listen_public: bool,
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
    /// The server (as with no command).
    Serve(ServeArgs),
    /// Prints this server's secret: what another server (a home) needs to
    /// reach it.
    Token,
    /// The servers a home links to (in its state database: the user's
    /// home, or a project's with --project). The server holding it must be
    /// stopped; while it runs, use its cockpit.
    Servers {
        #[command(subcommand)]
        action: ServersCmd,
        /// The project whose server is the home (default: the user's home).
        #[arg(long, global = true)]
        project: Option<PathBuf>,
    },
}

#[derive(clap::Subcommand)]
enum ServersCmd {
    List,
    /// Adds one: its name, where it is (http://host:port) and its secret
    /// (`meta-harness token` there).
    Add { name: String, url: String, secret: String },
    Remove { name: String },
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

    if let Some(Cmd::Servers { action, project }) = &cli.command {
        let db = match project {
            Some(project) => {
                let arch = arch_service::Limits::defaults()?;
                nah_state::StateDb::open(&arch_service::state_dir(&project.canonicalize()?)?, &arch_service::store_settings(&arch))?
            }
            None => meta_harness::startup::home_state()?,
        };
        match action {
            ServersCmd::List => {
                for s in db.servers()? {
                    println!("{}\t{}", s.name, s.url);
                }
            }
            ServersCmd::Add { name, url, secret } => {
                let added_at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
                db.add_server(&nah_state::ServerEntry { name: name.clone(), url: url.clone(), secret: secret.clone(), added_at_ms })?;
                println!("Added {name}: home links to it from its next start.");
            }
            ServersCmd::Remove { name } => {
                if !db.remove_server(name)? {
                    return Err(format!("there's no server named {name:?}").into());
                }
                println!("Removed {name}.");
            }
        }
        return Ok(());
    }
    if let Some(Cmd::Token) = cli.command {
        println!("{}", meta_harness::cockpit::CockpitAuth::for_user()?.secret());
        return Ok(());
    }
    let serve = match cli.command {
        Some(Cmd::Serve(args)) => args,
        _ => cli.serve,
    };
    if serve.no_project && serve.no_cockpit {
        return Err("--no-project and --no-cockpit leave nothing to serve".into());
    }
    if !serve.no_cockpit {
        meta_harness::startup::check_cockpit_build()?;
    }
    let (listener, base_url) = meta_harness::startup::listen(serve.listen.as_deref(), serve.listen_public).await?;
    let bound = listener.local_addr()?;
    let cockpit = meta_harness::cockpit::CockpitAuth::for_user()?;
    if serve.no_project {
        let cockpit_path = cockpit.launch_path();
        let gate = meta_harness::modules::Gate { cockpit, access: Arc::new(nah_access::Access::defaults()), tickets: Arc::default() };
        let state = Arc::new(meta_harness::startup::home_state()?);
        meta_harness::startup::reconcile_on_start(&state)?;
        let here = Arc::new(EventHub::default());
        let notify = nah_notify::NotifyHub::start(here.clone(), state.clone(), Arc::new(nah_notify::NoSessions), Vec::new())?;
        let stopping = meta_harness::modules::Shutdown::default();
        let home = meta_harness::modules::HomeState { gate: gate.clone(), stopping: stopping.clone(), home: meta_harness::home::Home::start(state.clone(), here, notify, false)? };
        let server = meta_harness::modules::ServerState { gate: gate.clone(), shutdown: stopping.clone() };
        let page = meta_harness::modules::CockpitState { gate, stopping, state, remote: Arc::default() };
        info!("No project here: the cockpit and home. Open it: {base_url}{cockpit_path}");
        meta_harness::startup::serve(listener, create_router(&[&server, &page, &home]), None, server.shutdown.clone()).await?;
        info!("Meta-harness stopped.");
        return Ok(());
    }

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
    meta_harness::startup::reconcile_on_start(&state)?;
    let notify = meta_harness::startup::notifications(&hub, &state, &slots)?;
    let changes = meta_harness::startup::changes(&arch);
    let timeline = meta_harness::startup::timeline(&arch, &changes, &state, &hub)?;
    let tags = meta_harness::startup::tags(&arch, &state, &hub)?;
    meta_harness::startup::chats(&slots, &hub, &tags);
    meta_harness::startup::code_tags(&arch, &changes, &tags);
    let terminals = if serve.no_shells { None } else { Some(meta_harness::startup::terminals(&arch, &state, &hub, &tags)?) };
    let services = meta_harness::mcp::sets::Services { arch: arch.clone(), changes: changes.clone(), timeline: timeline.clone(), remote: remote.clone(), slots: slots.clone(), notify: notify.clone(), terminals: terminals.clone(), tags: tags.clone() };
    let mcp_state = McpServerState::new(Arc::new(McpService::new(services, access.clone())), meta_harness::startup::mcp_callers(&slots));

    let queue = meta_harness::server::queue_runner(&arch, meta_harness::startup::queue_config(&arch)?, &hub);
    let cockpit_path = cockpit.launch_path();
    let streams = mcp_state.sessions.clone();
    let app = AppState::new(Parts { registry: instances, sessions, slots, hub, mcp_state, arch, cockpit, queue, state, tool_servers, remote, access, notify, changes, timeline, terminals, tags })?;
    let parts: Vec<&dyn meta_harness::modules::Module> = app.modules().into_iter().filter(|m| !(serve.no_cockpit && m.is_cockpit())).collect();
    let router = create_router(&parts);
    info!("MCP endpoint: {base_url}/api/mcp/sse; events: ws://{bound}/ws/events");
    if !serve.no_cockpit {
        info!("Open the cockpit: {base_url}{cockpit_path}");
    }
    meta_harness::startup::serve(listener, router, Some(streams), app.server.shutdown.clone()).await?;
    info!("Meta-harness stopped.");
    Ok(())
}
