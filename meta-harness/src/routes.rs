//! The HTTP and WebSocket routes, by concern. The architecture, graph and
//! queue routes live with their modules (`arch`, `graph`, `queue`).

pub(crate) mod changes;
pub(crate) mod cockpit;
pub(crate) mod discussions;
pub(crate) mod events;
pub(crate) mod feedback;
pub(crate) mod files;
pub(crate) mod harness;
pub(crate) mod home;
pub(crate) mod mcp;
pub(crate) mod notifications;
pub(crate) mod server;
pub(crate) mod slots;
pub(crate) mod tags;
pub(crate) mod terminals;
pub(crate) mod tool_servers;
