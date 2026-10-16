//! The tools, in sets by concern: each set owns its tools' schemas and
//! handlers. The order here is the order agents see them in.

use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_service::{ArchitectureService, ServiceError};
use serde_json::Value;

use nah_access::Capability;

use crate::mcp::protocol::McpTool;

mod build;
mod cockpit;
mod explore;
mod plans;
mod review;
mod schema;
mod slots;

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
    [explore::Explore::schemas(trees), plans::Plans::schemas(trees), build::Build::schemas(trees), review::Review::schemas(trees), cockpit::Cockpit::schemas(trees), slots::Slots::schemas(trees)].into_iter().flatten().collect()
}

/// The sets, in the same order as `list_tools`.
pub(crate) fn all(arch: &Arc<ArchitectureService>, changes: &Option<Arc<nah_changes::Changes>>, remote: &Arc<crate::remote::Remote>, slots: &Arc<harness_core::SlotManager>, notify: &Arc<nah_notify::NotifyHub>) -> Vec<Box<dyn ToolSet>> {
    vec![
        Box::new(explore::Explore { arch: arch.clone(), changes: changes.clone() }),
        Box::new(plans::Plans { arch: arch.clone() }),
        Box::new(build::Build { arch: arch.clone() }),
        Box::new(review::Review { arch: arch.clone() }),
        Box::new(cockpit::Cockpit { remote: remote.clone() }),
        Box::new(slots::Slots { slots: slots.clone(), notify: notify.clone() }),
    ]
}
