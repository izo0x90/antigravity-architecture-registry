//! Opaque identifiers. App-owned ids are minted by the server; native ids (the
//! harness's own session/turn/item ids) are carried alongside, never mixed in.

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    )*};
}

id_type!(
    /// Kind of harness driver, e.g. "antigravity" or "claude-code".
    DriverKind,
    /// A configured instance of a driver (one driver can have several instances).
    InstanceId,
    /// App-owned conversation thread.
    ThreadId,
    /// App-owned turn within a thread.
    TurnId,
    /// App-owned item within a turn (message, tool call, file change, ...).
    ItemId,
    /// A pending question/approval the harness is waiting on.
    RequestId,
    /// A plan or to-do list artifact.
    PlanId,
    /// A subagent spawned by the harness.
    SubagentId,
    /// The harness's own identifier for something (session id, tool call id, ...).
    NativeId,
    /// A seat for an agent: the lead, a sub-slot, a queue job's worker.
    SlotId,
    /// One of a slot's sessions, the same across restarts (its thread isn't).
    SessionKey,
    /// A notification, by its subject (`req:<request>`, `turn:<turn>`, ...).
    NotificationId,
    /// A shell the project server runs for the user.
    TerminalId,
    /// A tag (stays when it's renamed).
    TagId,
    /// A discussion on a thing (`d1`, `d2`...).
    DiscussionId,
);
