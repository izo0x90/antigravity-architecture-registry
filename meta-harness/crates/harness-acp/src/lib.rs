//! The driver for any Agent Client Protocol agent, built on the official
//! `agent-client-protocol` crate. An agent's own ways (its modes, logins,
//! questions, where its skills and tool servers are configured) are data,
//! its harness pack's settings ([`AcpSettings`]), put to use by its
//! [`Profile`]; how it's reached plugs in through [`AcpLauncher`].

mod adapter;
mod auth;
mod catalog;
mod classify;
mod control;
mod convert;
mod driver;
mod instance;
mod launch;
mod login;
mod pieces;
mod profile;
mod questions;
mod session;
mod settings;
mod state;
mod workspace;

#[cfg(feature = "testing")]
pub mod testing;

pub use adapter::AcpAdapter;
pub use auth::{AcpAuthController, AuthChanged, AuthEvent};
pub use catalog::AcpSessionCatalog;
pub use control::{AcpDeclaredCapabilities, AcpProbe, CommandLogin, probe};
pub use driver::{AcpDriver, DRIVER_KIND};
pub use instance::{AcpInstance, InstanceParts};
pub use launch::{AcpLauncher, AcpTransport, ProcessLauncher};
pub use profile::{Declared, Profile};
pub use session::AcpSession;
pub use settings::AcpSettings;
pub use workspace::WorkspaceFs;

/// Re-exported so tests and tools can name ACP schema types without their own dependency.
pub use agent_client_protocol::schema::v1 as acp;
