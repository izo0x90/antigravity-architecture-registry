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
    /// The cockpit's part (left out by `--no-cockpit`).
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

gated!(CockpitState, HarnessState, NotifyState, ProjectState);

/// The cockpit: its page and launch link, its saved state, feedback, and
/// agents driving it.
#[derive(Clone)]
pub struct CockpitState {
    pub gate: Gate,
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

/// The harnesses: instances, threads, slots, the event stream, and the
/// tool servers every session gets.
#[derive(Clone)]
pub struct HarnessState {
    pub gate: Gate,
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
}

impl Module for ProjectState {
    fn routes(&self) -> Router {
        routes::files::routes().merge(routes::mcp::routes()).merge(crate::arch::routes()).merge(crate::graph::routes()).merge(crate::queue::routes()).with_state(self.clone())
    }
}

/// A project's server: every part.
#[derive(Clone)]
pub struct AppState {
    pub cockpit: CockpitState,
    pub harness: HarnessState,
    pub notify: NotifyState,
    pub project: ProjectState,
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
}

impl AppState {
    pub fn new(p: Parts) -> Self {
        let gate = Gate { cockpit: p.cockpit, access: p.access, tickets: Arc::default() };
        Self {
            cockpit: CockpitState { gate: gate.clone(), state: p.state, remote: p.remote },
            harness: HarnessState { gate: gate.clone(), registry: p.registry, sessions: p.sessions.clone(), slots: p.slots.clone(), hub: p.hub, tool_servers: p.tool_servers, root: p.arch.root().to_path_buf() },
            notify: NotifyState { gate: gate.clone(), notify: p.notify },
            project: ProjectState { gate, arch: p.arch, queue: p.queue, sessions: p.sessions, slots: p.slots, mcp_state: p.mcp_state },
        }
    }

    pub fn modules(&self) -> [&dyn Module; 4] {
        [&self.cockpit, &self.harness, &self.notify, &self.project]
    }
}
