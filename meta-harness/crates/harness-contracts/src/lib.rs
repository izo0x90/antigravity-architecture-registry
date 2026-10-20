//! Harness-neutral contracts: everything the server, adapters and clients exchange.
//!
//! Plain serde types only (no async, no I/O) so the WASM cockpit can use them as-is.
//! Shapes follow t3code's orchestration-v2 contracts so its adapters translate 1:1.
//! Provider-specific behavior lives in adapters; nothing here names a harness.

pub mod capabilities;
pub mod changes;
pub mod discussions;
pub mod events;
pub mod home;
pub mod ids;
pub mod items;
pub mod launch;
pub mod notifications;
pub mod plan;
pub mod policy;
pub mod requests;
pub mod session;
pub mod slots;
pub mod snapshot;
pub mod tags;
pub mod terminals;

pub use capabilities::*;
pub use discussions::*;
pub use events::*;
pub use home::{HomeEvent, LinkState, ServerView};
pub use ids::*;
pub use items::*;
pub use launch::*;
pub use notifications::*;
pub use plan::*;
pub use policy::*;
pub use requests::*;
pub use session::*;
pub use slots::*;
pub use snapshot::*;
pub use tags::*;
pub use terminals::*;
