//! Generic ACP adapter for any Agent Client Protocol agent, built on the official
//! `agent-client-protocol` crate. Harness specifics plug in through [`AcpFlavor`];
//! how the agent is reached plugs in through [`AcpLauncher`].

mod adapter;
mod auth;
mod catalog;
mod control;
mod convert;
mod flavor;
mod launch;
mod session;
mod state;

pub use adapter::{AcpAdapter, acp_base_capabilities};
pub use auth::{AcpAuthController, AuthChanged};
pub use catalog::AcpSessionCatalog;
pub use control::{AcpDeclaredCapabilities, AcpProbe, probe};
pub use flavor::{AcpFlavor, ClientFs, default_approval_options, default_auto_approve};
pub use launch::{AcpLauncher, AcpTransport, ProcessLauncher};
pub use session::AcpSession;

/// Re-exported so flavors can name ACP schema types without their own dependency.
pub use agent_client_protocol::schema::v1 as acp;
