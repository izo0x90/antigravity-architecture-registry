//! A home server: the one serving the cockpit, linked to every other
//! server (one per project). Its socket (`/ws/home`) carries each server's
//! events, said by name, and the list of servers whenever it changes.

use serde::{Deserialize, Serialize};

use crate::events::Stamped;

/// Home's own project (when it has one).
pub const HERE: &str = "here";

/// How home's link to a server is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LinkState {
    Connecting,
    Connected,
    /// It stopped answering (home tries again, waiting longer each time).
    Lost { reason: String },
    /// The user disconnected it (the server runs on; home doesn't follow it).
    Disconnected,
}

/// A server as home lists it (never with its secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    pub name: String,
    /// Where it is (empty for `here`).
    pub url: String,
    pub link: LinkState,
    /// It serves a project (a home with none serves only the cockpit).
    #[serde(default = "yes")]
    pub project: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum HomeEvent {
    /// The servers and their links, whenever that changes.
    Servers { servers: Vec<ServerView> },
    /// One of a server's events, and when it happened there (`Resync`:
    /// drop what's held for it; its whole state follows).
    Event { server: String, event: Box<Stamped> },
}
