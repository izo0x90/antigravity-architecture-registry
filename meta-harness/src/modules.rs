//! The server's parts, each with the state it uses and its routes: the
//! cockpit (its page, saved state, feedback, agents driving it), the
//! harnesses (instances, threads, slots, the event stream, tool servers),
//! notifications, and the project (its code, plans, work queue and MCP
//! tools). Every part checks who's calling the same way (`Gate`). A server
//! is the parts it runs.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use harness_core::{EventHub, InstanceRegistry, SessionManager, SlotManager};
use nah_access::Access;

use crate::cockpit::{CockpitAuth, Tickets};
use crate::routes;

/// A part of the server: its routes, given its state.
pub trait Module {
    fn routes(&self) -> Router;
    /// The cockpit's, or home's (left out by `--no-cockpit`).
    fn is_cockpit(&self) -> bool {
        false
    }
}

/// Who's calling: the cockpit's cookie, the roles, the sockets' tickets.
#[derive(Clone)]
pub struct Gate {
    pub cockpit: CockpitAuth,
    pub access: Arc<Access>,
    pub tickets: Arc<Tickets>,
}

macro_rules! gated {
    ($($state:ty),*) => {$(
        impl FromRef<$state> for CockpitAuth {
            fn from_ref(state: &$state) -> Self {
                state.gate.cockpit.clone()
            }
        }

        impl FromRef<$state> for Arc<Access> {
            fn from_ref(state: &$state) -> Self {
                state.gate.access.clone()
            }
        }

        impl FromRef<$state> for Arc<Tickets> {
            fn from_ref(state: &$state) -> Self {
                state.gate.tickets.clone()
            }
        }
    )*};
}

gated!(CockpitState, HarnessState, NotifyState, ProjectState, HomeState, ServerState, TerminalsState, TagsState, DiscussionsState);

/// Asks the server to stop (as Ctrl-C does); its serving waits on it.
#[derive(Clone, Default)]
pub struct Shutdown(Arc<tokio::sync::watch::Sender<bool>>);

impl Shutdown {
    pub fn ask(&self) {
        self.0.send_replace(true);
    }

    /// Returns once it's asked.
    pub async fn asked(&self) {
        let mut asked = self.0.subscribe();
        let _ = asked.wait_for(|a| *a).await;
    }

    /// Runs `socket` until it ends, or until the server stops (the socket
    /// is closed then, and its client sees it go).
    pub async fn or_stop(&self, socket: impl std::future::Future<Output = ()>) {
        tokio::select! {
            _ = socket => {}
            _ = self.asked() => {}
        }
    }
}

macro_rules! stoppable {
    ($($state:ty),*) => {$(
        impl FromRef<$state> for Shutdown {
            fn from_ref(state: &$state) -> Self {
                state.stopping.clone()
            }
        }
    )*};
}

stoppable!(CockpitState, HarnessState, HomeState, TerminalsState);

/// Every server's own part: being asked to stop.
#[derive(Clone)]
pub struct ServerState {
    pub gate: Gate,
    pub shutdown: Shutdown,
}

impl Module for ServerState {
    fn routes(&self) -> Router {
        routes::server::routes().with_state(self.clone())
    }
}

/// The cockpit: its page and launch link, its saved state, feedback, and
/// agents driving it.
#[derive(Clone)]
pub struct CockpitState {
    pub gate: Gate,
    /// Its socket ends when the server stops.
    pub stopping: Shutdown,
    /// The state database (the cockpit's saved views, feedback).
    pub state: Arc<nah_state::StateDb>,
    /// The cockpit, for agents driving it.
    pub remote: Arc<crate::remote::Remote>,
}

impl FromRef<CockpitState> for Arc<crate::remote::Remote> {
    fn from_ref(state: &CockpitState) -> Self {
        state.remote.clone()
    }
}

impl Module for CockpitState {
    /// Its routes, and its page and build (from `STATIC_DIR`) for anything else.
    fn routes(&self) -> Router {
        let router = routes::cockpit::routes().merge(routes::feedback::routes()).with_state(self.clone());
        let page = crate::server::static_dir();
        if page.exists() { router.fallback_service(tower_http::services::ServeDir::new(page)) } else { router }
    }

    fn is_cockpit(&self) -> bool {
        true
    }
}

/// Home: the servers it links to, their events, and (G5) the cockpit's
/// requests passed on to them. It runs with the cockpit.
#[derive(Clone)]
pub struct HomeState {
    pub gate: Gate,
    pub stopping: Shutdown,
    pub home: Arc<crate::home::Home>,
}

impl Module for HomeState {
    fn routes(&self) -> Router {
        routes::home::routes().with_state(self.clone())
    }

    fn is_cockpit(&self) -> bool {
        true
    }
}

/// The harnesses: instances, threads, slots, the event stream, and the
/// tool servers every session gets.
#[derive(Clone)]
pub struct HarnessState {
    pub gate: Gate,
    /// The event sockets end when the server stops.
    pub stopping: Shutdown,
    pub registry: Arc<InstanceRegistry>,
    pub sessions: Arc<SessionManager>,
    /// The slots: the lead, sub-slots, the queue's workers.
    pub slots: Arc<SlotManager>,
    pub hub: Arc<EventHub>,
    pub tool_servers: Arc<crate::tool_servers::ToolServerState>,
    /// The project's folder (where harness packs are looked up).
    pub root: PathBuf,
}

impl Module for HarnessState {
    fn routes(&self) -> Router {
        routes::harness::routes().merge(routes::slots::routes()).merge(routes::events::routes()).merge(routes::tool_servers::routes()).with_state(self.clone())
    }
}

/// The terminals: shells the server runs for the user (left out by
/// `--no-shells`).
#[derive(Clone)]
pub struct TerminalsState {
    pub gate: Gate,
    /// Their sockets end when the server stops.
    pub stopping: Shutdown,
    pub terminals: Arc<crate::terminals::Terminals>,
}

impl Module for TerminalsState {
    fn routes(&self) -> Router {
        routes::terminals::routes().with_state(self.clone())
    }
}

/// Tags: names given to things in the project, to find them again and to
/// point agents at them.
#[derive(Clone)]
pub struct TagsState {
    pub gate: Gate,
    pub tags: Arc<nah_tags::Tags>,
}

impl Module for TagsState {
    fn routes(&self) -> Router {
        routes::tags::routes().with_state(self.clone())
    }
}

/// Discussions: a question or a change asked for on any thing, and the
/// back-and-forth on it.
#[derive(Clone)]
pub struct DiscussionsState {
    pub gate: Gate,
    pub discussions: Arc<nah_discussions::Discussions>,
    /// Who can be asked.
    pub slots: Arc<SlotManager>,
    /// What things say (what a discussion is on).
    pub things: Arc<nah_things::Things>,
}

impl Module for DiscussionsState {
    fn routes(&self) -> Router {
        routes::discussions::routes().with_state(self.clone())
    }
}

/// What needs the user, and what happened.
#[derive(Clone)]
pub struct NotifyState {
    pub gate: Gate,
    pub notify: Arc<nah_notify::NotifyHub>,
}

impl Module for NotifyState {
    fn routes(&self) -> Router {
        routes::notifications::routes().with_state(self.clone())
    }
}

/// The project: its code and graph, plans and reviews, the work queue,
/// its files, and the agents' tools over it.
#[derive(Clone)]
pub struct ProjectState {
    pub gate: Gate,
    pub arch: Arc<arch_service::ArchitectureService>,
    pub queue: Arc<crate::queue::QueueRunner>,
    /// The queue runs its jobs in worker slots.
    pub sessions: Arc<SessionManager>,
    pub slots: Arc<SlotManager>,
    pub mcp_state: crate::mcp::McpServerState,
    /// Its changes, from git (none when it isn't a git repository), and
    /// when they changed (snapshots at every turn's start and end).
    pub changes: Option<Arc<nah_changes::Changes>>,
    pub timeline: Option<Arc<nah_timeline::Timeline>>,
}

impl Module for ProjectState {
    fn routes(&self) -> Router {
        routes::files::routes().merge(routes::changes::routes()).merge(routes::mcp::routes()).merge(crate::arch::routes()).merge(crate::graph::routes()).merge(crate::queue::routes()).with_state(self.clone())
    }
}

/// A project's server: every part (the terminals unless left out).
#[derive(Clone)]
pub struct AppState {
    pub server: ServerState,
    pub cockpit: CockpitState,
    pub home: HomeState,
    pub harness: HarnessState,
    pub notify: NotifyState,
    pub project: ProjectState,
    pub terminals: Option<TerminalsState>,
    pub tags: TagsState,
    pub discussions: DiscussionsState,
}

/// What a project's server is made of, before it's split into its parts.
pub struct Parts {
    pub registry: Arc<InstanceRegistry>,
    pub sessions: Arc<SessionManager>,
    pub slots: Arc<SlotManager>,
    pub hub: Arc<EventHub>,
    pub mcp_state: crate::mcp::McpServerState,
    pub arch: Arc<arch_service::ArchitectureService>,
    pub cockpit: CockpitAuth,
    pub queue: Arc<crate::queue::QueueRunner>,
    pub state: Arc<nah_state::StateDb>,
    pub tool_servers: Arc<crate::tool_servers::ToolServerState>,
    pub remote: Arc<crate::remote::Remote>,
    pub access: Arc<Access>,
    pub notify: Arc<nah_notify::NotifyHub>,
    pub changes: Option<Arc<nah_changes::Changes>>,
    pub timeline: Option<Arc<nah_timeline::Timeline>>,
    /// None: the server runs no shells (`--no-shells`).
    pub terminals: Option<Arc<crate::terminals::Terminals>>,
    pub tags: Arc<nah_tags::Tags>,
    pub discussions: Arc<nah_discussions::Discussions>,
    pub things: Arc<nah_things::Things>,
}

impl AppState {
    /// A project's server; home follows the servers its state database lists.
    pub fn new(p: Parts) -> Result<Self, nah_state::StateError> {
        let gate = Gate { cockpit: p.cockpit, access: p.access, tickets: Arc::default() };
        let home = crate::home::Home::start(p.state.clone(), p.hub.clone(), p.notify.clone(), true)?;
        let stopping = Shutdown::default();
        // Every prompt a slot is sent hands the agent what the tags it
        // names (`#name`) are on: what an agent passes on, only what the
        // slot told may read.
        let (tags, slots, access, discussions) = (p.tags.clone(), Arc::downgrade(&p.slots), gate.access.clone(), p.discussions.clone());
        p.slots.expand_prompts_with(Arc::new(move |text: &str, readable_by: Option<&harness_contracts::SlotId>| {
            let slot = readable_by.and_then(|id| slots.upgrade().and_then(|s| s.get(id).map(|slot| (s, slot))));
            match (readable_by, slot) {
                (None, _) => tags.expand(text, &|_| true),
                (Some(_), Some((slots, slot))) => {
                    let reach = crate::mcp::sets::reach::Reach::of_slot(&slots, &access, &discussions, &slot);
                    tags.expand(text, &|s| reach.reaches(s))
                }
                // A slot gone meanwhile: nothing a chat says.
                (Some(_), None) => tags.expand(text, &|s| !matches!(s, harness_contracts::Subject::ChatSession { .. } | harness_contracts::Subject::ChatTurn { .. } | harness_contracts::Subject::ChatMessage { .. })),
            }
        }));
        Ok(Self {
            server: ServerState { gate: gate.clone(), shutdown: stopping.clone() },
            home: HomeState { gate: gate.clone(), stopping: stopping.clone(), home },
            cockpit: CockpitState { gate: gate.clone(), stopping: stopping.clone(), state: p.state, remote: p.remote },
            harness: HarnessState { gate: gate.clone(), stopping: stopping.clone(), registry: p.registry, sessions: p.sessions.clone(), slots: p.slots.clone(), hub: p.hub, tool_servers: p.tool_servers, root: p.arch.root().to_path_buf() },
            notify: NotifyState { gate: gate.clone(), notify: p.notify },
            terminals: p.terminals.map(|terminals| TerminalsState { gate: gate.clone(), stopping: stopping.clone(), terminals }),
            tags: TagsState { gate: gate.clone(), tags: p.tags },
            discussions: DiscussionsState { gate: gate.clone(), discussions: p.discussions, slots: p.slots.clone(), things: p.things },
            project: ProjectState { gate, arch: p.arch, queue: p.queue, sessions: p.sessions, slots: p.slots, mcp_state: p.mcp_state, changes: p.changes, timeline: p.timeline },
        })
    }

    pub fn modules(&self) -> Vec<&dyn Module> {
        let terminals = self.terminals.as_ref().map(|t| t as &dyn Module);
        [&self.server as &dyn Module, &self.cockpit, &self.home, &self.harness, &self.notify, &self.project, &self.tags, &self.discussions].into_iter().chain(terminals).collect()
    }
}
