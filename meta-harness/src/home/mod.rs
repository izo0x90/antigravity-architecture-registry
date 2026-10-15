//! Home: the server that serves the cockpit, linked to every other server
//! (one per project). It keeps the list (in its state database, secrets
//! included), follows each server's events, and passes the cockpit's
//! requests on to the server they're for.

pub mod links;
pub mod pass;
pub mod socket;

use std::sync::Arc;

use harness_contracts::home::HERE;
use harness_contracts::{LinkState, ServerView};
use harness_core::EventHub;

pub struct Home {
    pub db: Arc<nah_state::StateDb>,
    pub links: Arc<links::Links>,
    /// Passes the cockpit's requests on to the servers.
    pub client: pass::HttpClient,
    /// Home's own events: its project's, else only its notifications.
    pub here: Arc<EventHub>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Home {
    /// Home over its state database, following every server listed there.
    pub fn start(db: Arc<nah_state::StateDb>, here: Arc<EventHub>, notify: Arc<nah_notify::NotifyHub>) -> Result<Arc<Self>, nah_state::StateError> {
        let links = links::Links::start(db.servers()?, notify);
        Ok(Arc::new(Self { db, links, here, client: pass::client() }))
    }

    /// Every server: home's own first, then the others as added.
    pub fn servers(&self) -> Vec<ServerView> {
        std::iter::once(ServerView { name: HERE.into(), url: String::new(), link: LinkState::Connected }).chain(self.links.list()).collect()
    }

    /// Each server's events, home's own first.
    pub fn hubs(&self) -> Vec<(String, Arc<EventHub>)> {
        std::iter::once((HERE.to_owned(), self.here.clone())).chain(self.links.hubs()).collect()
    }
}
