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

/// Binds where `--listen` says, else `HOST` and `PORT` (or
/// `METAHARNESS_PORT`), else 127.0.0.1:8080, first, so the base URL handed
/// to harnesses is the real one. Unless `public`, only addresses that
/// aren't the internet's (see `check_bind`).
pub async fn listen(asked: Option<&str>, public: bool) -> Result<(tokio::net::TcpListener, String), StartError> {
    let addr: std::net::SocketAddr = match asked {
        Some(asked) => asked
            .parse()
            .map_err(|e| format!("--listen {asked}: not an address and port ({e}); e.g. 127.0.0.1:8080 or 100.101.102.103:8080"))?,
        None => {
            let port: u16 =
                std::env::var("PORT").or_else(|_| std::env::var("METAHARNESS_PORT")).ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
            let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
            let ip: std::net::IpAddr = host.parse().map_err(|e| format!("HOST={host}: not an address ({e})"))?;
            std::net::SocketAddr::new(ip, port)
        }
    };
    if !public {
        check_bind(addr.ip())?;
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let base_url = match bound.ip() {
        ip if ip.is_unspecified() => format!("http://127.0.0.1:{}", bound.port()),
        _ => format!("http://{bound}"),
    };
    info!("Meta-harness listening on {bound} ({base_url})");
    Ok((listener, base_url))
}

/// Whether a server may listen on `ip` without `--listen-public`: loopback,
/// private networks (10/8, 172.16/12, 192.168/16), Tailscale's (100.64/10),
/// IPv6 unique-local (fc00::/7) and link-local (fe80::/10). Not the
/// unspecified address: it takes every interface, the internet's too.
pub fn check_bind(ip: std::net::IpAddr) -> Result<(), String> {
    use std::net::IpAddr;
    let private = match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback() || v4.is_private() || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80,
    };
    match () {
        _ if private => Ok(()),
        _ if ip.is_unspecified() => Err(format!(
            "listening on {ip} takes every interface, the internet's too: listen on a private address (127.0.0.1, your LAN's, Tailscale's 100.x) with --listen, or pass --listen-public to mean it"
        )),
        _ => Err(format!(
            "{ip} isn't a private address (loopback, a LAN, Tailscale's 100.64/10): pass --listen-public to listen on it anyway"
        )),
    }
}

/// The state database of a server with no project (`<data dir>/nah/home/`).
pub fn home_state() -> Result<nah_state::StateDb, StartError> {
    let data = dirs::data_dir().ok_or("no data directory for this user (set XDG_DATA_HOME)")?;
    let defaults = arch_service::Limits::defaults()?;
    Ok(nah_state::StateDb::open(&data.join("nah/home"), &arch_service::store_settings(&defaults))?)
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
    let crate::project::AnalysisSetup { catalog, packs, types, rules, limits } =
        crate::project::analysis_setup(&root).map_err(|e| e.to_string())?;
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
/// The project's changes, from git; none (and said why) when it isn't a
/// git repository.
pub fn changes(arch: &ArchitectureService) -> Option<Arc<nah_changes::Changes>> {
    match nah_changes::Changes::open(arch.root()) {
        Ok(c) => Some(Arc::new(c)),
        Err(e) => {
            tracing::warn!("No changes view: {e}");
            None
        }
    }
}

/// The timeline over the project's changes, kept in its state database and
/// following every session's turns (none without git). How long it keeps
/// things: `.nah/changes.toml`'s `[timeline]`.
pub fn timeline(
    arch: &ArchitectureService,
    changes: &Option<Arc<nah_changes::Changes>>,
    state: &Arc<nah_state::StateDb>,
    hub: &Arc<EventHub>,
) -> Result<Option<Arc<nah_timeline::Timeline>>, StartError> {
    let Some(changes) = changes else { return Ok(None) };
    let policy = match std::fs::read_to_string(arch.root().join(".nah/changes.toml")) {
        Ok(text) => nah_timeline::Policy::from_toml(&text)?,
        Err(_) => nah_timeline::Policy::default(),
    };
    Ok(Some(nah_timeline::Timeline::new(changes.clone(), state.clone(), policy)?.start(hub.clone())))
}

pub fn state(arch: &ArchitectureService) -> Result<nah_state::StateDb, StartError> {
    let dir = arch_service::state_dir(arch.root())?;
    Ok(nah_state::StateDb::open(&dir, &arch_service::store_settings(arch.limits()))?)
}

/// Before the server takes requests: what an earlier run left that can't
/// hold now is put right, and said in one line. (What waited on the user
/// then can't be answered now: those notifications expire. Worker slots
/// went when the slots loaded.)
pub fn reconcile_on_start(state: &nah_state::StateDb) -> Result<(), StartError> {
    let expired = nah_notify::NotificationStore::expire_waiting(state).map_err(nah_notify::NotifyError::Store)?;
    info!("start: {expired} notification(s) that waited on the last run expired");
    Ok(())
}

/// Notifications: from the event stream, kept in the state database.
pub fn notifications(
    hub: &Arc<EventHub>,
    state: &Arc<nah_state::StateDb>,
    slots: &Arc<harness_core::SlotManager>,
) -> Result<Arc<nah_notify::NotifyHub>, StartError> {
    Ok(nah_notify::NotifyHub::start(hub.clone(), state.clone(), slots.clone(), Vec::new())?)
}

/// `.nah/terminals.toml`: how long a command runs before its end is news
/// (`long_command_secs`, 10 by default), how many lines of scrollback each
/// terminal keeps (`scrollback_lines`, 10,000), and how much of a running
/// command a note keeps (`running_log_mb`, 64).
pub struct TerminalSettings {
    pub terminals: crate::terminals::Settings,
    pub running_log: u64,
}

pub fn terminal_settings(arch: &ArchitectureService) -> Result<TerminalSettings, StartError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Settings {
        #[serde(default = "ten")]
        long_command_secs: u64,
        #[serde(default = "ten_thousand")]
        scrollback_lines: usize,
        #[serde(default = "sixty_four")]
        running_log_mb: u64,
    }
    fn ten() -> u64 {
        10
    }
    fn ten_thousand() -> usize {
        10_000
    }
    fn sixty_four() -> u64 {
        nah_things::GROWN_MOST >> 20
    }
    let settings: Settings = match std::fs::read_to_string(arch.root().join(".nah/terminals.toml")) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!(".nah/terminals.toml: {e}"))?,
        Err(_) => Settings { long_command_secs: ten(), scrollback_lines: ten_thousand(), running_log_mb: sixty_four() },
    };
    if settings.scrollback_lines < 100 {
        return Err(format!(".nah/terminals.toml: scrollback_lines is {}: at least 100", settings.scrollback_lines).into());
    }
    if settings.running_log_mb == 0 {
        return Err(".nah/terminals.toml: running_log_mb is 0: at least 1".into());
    }
    Ok(TerminalSettings {
        terminals: crate::terminals::Settings {
            long: std::time::Duration::from_secs(settings.long_command_secs),
            scrollback: settings.scrollback_lines,
        },
        running_log: settings.running_log_mb.saturating_mul(1 << 20),
    })
}

/// The terminals: the user's shells, started in the project's folder (their
/// start-up hooks kept in the cache folder: they're written again at each
/// start); those kept from the last run come back ended. Their lines and
/// commands are things notes can be on.
pub fn terminals(
    arch: &ArchitectureService,
    state: &Arc<nah_state::StateDb>,
    hub: &Arc<EventHub>,
    things: &Arc<nah_things::Things>,
    settings: crate::terminals::Settings,
) -> Result<Arc<crate::terminals::Terminals>, StartError> {
    let hooks = arch_service::cache_dir(arch.root())?.join("terminal-hooks");
    let terminals = crate::terminals::Terminals::load(arch.root().to_path_buf(), hooks, hub.clone(), state.clone(), settings)?;
    things.register(terminals.clone());
    // Noted lines kept where they are now when they move, before they can
    // move again (a terminal tells it on the blocking thread that moved
    // them; notes are kept in the state database).
    let weak = Arc::downgrade(things);
    terminals.when_moved(move || {
        if let Some(things) = weak.upgrade() {
            things.moved();
        }
    });
    Ok(terminals)
}

/// The code owns its components and lines as things notes can be on; as
/// the files change, noted lines are kept where they are now.
pub fn code_tags(arch: &Arc<ArchitectureService>, changes: &Option<Arc<nah_changes::Changes>>, things: &Arc<nah_things::Things>) {
    things.register(crate::code_tags::Code::new(arch.clone(), changes.clone()));
    let weak = Arc::downgrade(things);
    arch.subscribe(move |event| {
        if matches!(event, arch_graph::ArchEvent::Changed { reason: arch_graph::ChangeReason::Ingested, .. })
            && let Some(things) = weak.upgrade()
        {
            things.moved();
        }
    });
}

/// The plans own their parts (a plan, a component's card and its
/// sections, a relation, an item) as things notes can be on.
pub fn plan_parts(arch: &Arc<ArchitectureService>, things: &Arc<nah_things::Things>) {
    things.register(crate::plan_things::PlanParts::new(arch.clone()));
}

/// The chats own their turns and messages as things notes can be on;
/// when a session numbers its turns anew, noted ones are kept where they
/// are now.
pub fn chats(slots: &Arc<harness_core::SlotManager>, hub: &Arc<EventHub>, things: &Arc<nah_things::Things>) {
    let chats = crate::chats::Chats::new(slots.clone(), hub.clone());
    things.register(chats.clone());
    let weak = Arc::downgrade(things);
    chats.when_moved(move || {
        if let Some(things) = weak.upgrade() {
            things.moved();
        }
    });
}

/// The things notes are on: what each says is its owner's to tell (each
/// owner registers itself as it's made), kept in the logs (in the
/// project's state folder) for one its owner doesn't keep; a running
/// command's, up to `running_log` bytes.
pub fn things(arch: &ArchitectureService, running_log: u64) -> Result<Arc<nah_things::Things>, StartError> {
    Ok(nah_things::Things::open(&arch_service::state_dir(arch.root())?.join("tagged"), running_log))
}

/// Discussions: kept in the state database (their messages' text in
/// `.nah/state/discussions/`), told on the event stream, on things; a
/// discussion is a thing too. A plan's component isn't approved while one
/// on it isn't settled. Comments plans' files held before move in. The
/// slot a discussion asks is told what's new in it.
pub fn discussions(
    arch: &Arc<ArchitectureService>,
    state: &Arc<nah_state::StateDb>,
    hub: &Arc<EventHub>,
    things: &Arc<nah_things::Things>,
    slots: &Arc<harness_core::SlotManager>,
) -> Result<Arc<nah_discussions::Discussions>, StartError> {
    let discussions = nah_discussions::Discussions::open(
        state.clone(),
        hub.clone(),
        things.clone(),
        &arch_service::state_dir(arch.root())?.join("discussions"),
    )
    .map_err(|e| e.to_string())?;
    things.register(discussions.clone());
    // The agents in a discussion: by their slots' ids, named as they are.
    let named = Arc::downgrade(slots);
    discussions.name_with(Box::new(move |id| named.upgrade()?.get(&harness_contracts::SlotId::new(id)).map(|s| s.name)));
    let weak = Arc::downgrade(&discussions);
    arch.approvals_ask(Box::new(move |plan, id| {
        let Some(discussions) = weak.upgrade() else { return Some("the discussions are closed: approvals wait for them".into()) };
        let holding: Vec<String> = discussions
            .unsettled(&harness_contracts::Subject::PlanComponent { plan: plan.clone(), id: id.clone() })
            .iter()
            .map(|d| match d.status {
                harness_contracts::DiscussionStatus::Answered => format!("{} on {} (answered: settle or reopen it)", d.id, d.on.what),
                _ => format!("{} on {} (open: \"{}\")", d.id, d.on.what, d.asking().chars().take(60).collect::<String>()),
            })
            .collect();
        (!holding.is_empty()).then(|| format!("{id} has discussions not settled: {}", holding.join("; ")))
    }));
    // Comments plans' files still hold (from before discussions) move in.
    let weak = Arc::downgrade(&discussions);
    arch.move_comments_to(Box::new(move |plan, comments| match weak.upgrade() {
        Some(discussions) => crate::plan_comments::move_into(&discussions, plan, comments),
        None => Err("the discussions are closed".into()),
    }))?;
    // From now on (what was brought in waits in `discussions`, untold).
    crate::discussing::tell_the_asked(hub, slots, &discussions);
    Ok(discussions)
}

/// Tags: kept in the state database, told on the event stream, on things.
pub fn tags(
    state: &Arc<nah_state::StateDb>,
    hub: &Arc<EventHub>,
    things: &Arc<nah_things::Things>,
) -> Result<Arc<nah_tags::Tags>, StartError> {
    nah_tags::Tags::open(state.clone(), hub.clone(), things.clone()).map_err(|e| e.to_string().into())
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
    Arc::new(move |key: &str| McpServer {
        name: OUR_TOOLS.into(),
        transport: McpTransport::Sse { url: format!("{url}?key={key}"), headers: Vec::new() },
    })
}

/// The name agents see our tools under.
pub const OUR_TOOLS: &str = "nah-architecture";

/// The tool servers listed for the cockpit: ours (no key) and the project's.
pub fn tool_servers(base_url: &str, project: &std::path::Path) -> Arc<crate::tool_servers::ToolServerState> {
    let ours = vec![McpServer {
        name: OUR_TOOLS.into(),
        transport: McpTransport::Sse { url: format!("{base_url}/api/mcp/sse"), headers: Vec::new() },
    }];
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
        slots.holder(key).map(|h| crate::mcp::McpCaller {
            principal: nah_access::Principal::agent(&h.role),
            slot: Some(h.slot),
            label: h.name,
        })
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
    Ok(harness_core::SlotManager::load(
        sessions.clone(),
        registry.clone(),
        hub.clone(),
        state.clone(),
        our_tools(base_url),
        Arc::new(move || project.project()),
        OPEN_WITHIN,
    )?)
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
pub async fn serve(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    streams: Option<Arc<crate::mcp::SseSessionManager>>,
    asked: crate::modules::Shutdown,
) -> Result<(), StartError> {
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = shutdown_signal() => asked.ask(),
                _ = asked.asked() => {}
            }
            if let Some(streams) = streams {
                streams.close_all().await;
            }
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

#[cfg(test)]
mod tests {
    #[test]
    fn a_server_listens_where_the_internet_isnt_unless_told() {
        let allowed = [
            "127.0.0.1",
            "127.8.9.10",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.20",
            "100.64.0.1",
            "100.101.102.103",
            "100.127.255.255",
            "::1",
            "fd7a:115c:a1e0::1",
            "fe80::1",
        ];
        for ip in allowed {
            assert_eq!(super::check_bind(ip.parse().unwrap()), Ok(()), "{ip}");
        }
        let refused = ["0.0.0.0", "::", "8.8.8.8", "100.63.255.255", "100.128.0.1", "172.32.0.1", "192.169.0.1", "2001:db8::1"];
        for ip in refused {
            let err = super::check_bind(ip.parse().unwrap()).unwrap_err();
            assert!(err.contains(ip) && err.contains("--listen-public"), "{ip}: {err}");
        }
    }
}
