//! The tools, in sets by concern: each set owns its tools' schemas and
//! handlers. The order here is the order agents see them in.

use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_service::{ArchitectureService, ServiceError};
use serde_json::Value;

use nah_access::Capability;

use crate::mcp::protocol::McpTool;

mod build;
mod changes;
mod cockpit;
mod explore;
mod plans;
mod review;
mod schema;
mod slots;
mod tags;
mod terminals;

/// A tool as agents see it, and the capability a caller needs to use it.
pub(crate) struct ToolDef {
    pub(crate) tool: McpTool,
    pub(crate) needs: Capability,
}

/// A set of tools: what they are, and running one of them.
#[async_trait::async_trait]
pub(crate) trait ToolSet: Send + Sync {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef>;
    async fn call(&self, name: &str, args: Value, caller: &crate::mcp::McpCaller) -> Result<String, ServiceError>;
}

/// Every tool, in the order agents see them.
pub fn list_tools(trees: &TreeConfig) -> Vec<McpTool> {
    tool_defs(trees).into_iter().map(|d| d.tool).collect()
}

/// Every tool with the capability it needs, in the same order.
pub(crate) fn tool_defs(trees: &TreeConfig) -> Vec<ToolDef> {
    [
        explore::Explore::schemas(trees),
        changes::Changes::schemas(trees),
        plans::Plans::schemas(trees),
        build::Build::schemas(trees),
        review::Review::schemas(trees),
        cockpit::Cockpit::schemas(trees),
        slots::Slots::schemas(trees),
        terminals::Terminals::schemas(trees),
        tags::Tags::schemas(trees),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// What the tools work with: the server's services (git's changes and the
/// timeline only in a git repository; the terminals unless the server
/// runs no shells).
pub struct Services {
    pub arch: Arc<ArchitectureService>,
    pub changes: Option<Arc<nah_changes::Changes>>,
    pub timeline: Option<Arc<nah_timeline::Timeline>>,
    pub remote: Arc<crate::remote::Remote>,
    pub slots: Arc<harness_core::SlotManager>,
    pub notify: Arc<nah_notify::NotifyHub>,
    pub terminals: Option<Arc<crate::terminals::Terminals>>,
    pub tags: Arc<nah_tags::Tags>,
}

/// The sets, in the same order as `list_tools`.
pub(crate) fn all(s: &Services) -> Vec<Box<dyn ToolSet>> {
    vec![
        Box::new(explore::Explore { arch: s.arch.clone(), changes: s.changes.clone() }),
        Box::new(changes::Changes { arch: s.arch.clone(), changes: s.changes.clone(), timeline: s.timeline.clone() }),
        Box::new(plans::Plans { arch: s.arch.clone() }),
        Box::new(build::Build { arch: s.arch.clone() }),
        Box::new(review::Review { arch: s.arch.clone() }),
        Box::new(cockpit::Cockpit { remote: s.remote.clone() }),
        Box::new(slots::Slots { slots: s.slots.clone(), notify: s.notify.clone() }),
        Box::new(terminals::Terminals { terminals: s.terminals.clone() }),
        Box::new(tags::Tags { tags: s.tags.clone(), terminals: s.terminals.clone() }),
    ]
}
