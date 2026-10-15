//! Notifications: what needs the user (an approval, a question, a sign-in)
//! and what happened that they'd want to know (a crash, a finished or failed
//! turn, a message). The rules read the event stream; the hub keeps what
//! they say (database, then memory, then the event stream) and hands each
//! change to its adapters. The cockpit reads the event stream itself;
//! adapters are for elsewhere (Slack, email, a phone).

mod hub;
pub mod rules;
#[cfg(feature = "testing")]
pub mod testing;

pub use hub::{Live, NotifyHub};

use harness_contracts::{Notification, NotificationId, TransitionError};

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("no notification {0}")]
    NotFound(NotificationId),
    #[error(transparent)]
    Transition(#[from] TransitionError),
    #[error("notifications couldn't be kept: {0}")]
    Store(String),
}

/// Where notifications are kept (the project's state database).
pub trait NotificationStore: Send + Sync {
    /// The open ones.
    fn open(&self) -> Result<Vec<Notification>, String>;
    /// The latest `limit`, in any state, newest first.
    fn recent(&self, limit: usize) -> Result<Vec<Notification>, String>;
    fn get(&self, id: &NotificationId) -> Result<Option<Notification>, String>;
    /// Writes these as they are now, all or none.
    fn put(&self, notifications: &[Notification]) -> Result<(), String>;
    /// How many have an id starting with `prefix`.
    fn count(&self, prefix: &str) -> Result<u32, String>;
    /// At start: what waited on the run before is expired. Returns how many.
    fn expire_waiting(&self) -> Result<usize, String>;
}

/// Takes notifications elsewhere. `deliver` must not block: an adapter keeps
/// its own bounded queue and drops the oldest (saying how many) when full.
pub trait Adapter: Send + Sync {
    fn deliver(&self, notification: &Notification);
}
