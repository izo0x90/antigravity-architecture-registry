//! Extension points for a specific ACP agent. Every hook has a protocol-generic
//! default, so a flavor only overrides what its agent does differently.

use std::sync::Arc;

use agent_client_protocol::schema::v1 as acp;
use async_trait::async_trait;
use harness_contracts::{ApprovalOption, Capabilities, RuntimeMode, RuntimePolicy, ToolKind};
use harness_core::Result;

use crate::convert;

pub trait AcpFlavor: Send + Sync + 'static {
    /// Adjusts capabilities derived from the agent's `initialize` response.
    fn capabilities(&self, base: Capabilities) -> Capabilities {
        base
    }

    /// Native mode id for an app runtime policy, chosen from the session's modes.
    fn native_mode(&self, _policy: &RuntimePolicy, _available: &[String]) -> Option<String> {
        None
    }

    /// App runtime mode for a native mode id the agent switched to.
    fn app_mode(&self, _native: &str) -> Option<RuntimeMode> {
        None
    }

    /// Options shown to the user for a permission request.
    fn approval_options(&self, native: &[acp::PermissionOption]) -> Vec<ApprovalOption> {
        native.iter().map(convert::approval_option).collect()
    }

    /// Option to select without asking, per the runtime policy.
    fn auto_approve(&self, policy: &RuntimePolicy, kind: &ToolKind, native: &[acp::PermissionOption]) -> Option<String> {
        default_auto_approve(policy, kind, native)
    }

    /// File system access offered to the agent; `None` means not advertised.
    fn client_fs(&self) -> Option<Arc<dyn ClientFs>> {
        None
    }
}

/// Selects the agent's "allow once" option when the runtime mode permits the tool.
pub fn default_auto_approve(policy: &RuntimePolicy, kind: &ToolKind, native: &[acp::PermissionOption]) -> Option<String> {
    let allowed = match policy.runtime_mode {
        RuntimeMode::FullAccess | RuntimeMode::Auto => true,
        RuntimeMode::AutoAcceptEdits => matches!(kind, ToolKind::Edit | ToolKind::Delete | ToolKind::Move | ToolKind::Read),
        RuntimeMode::ApprovalRequired => false,
    };
    if !allowed {
        return None;
    }
    native
        .iter()
        .find(|o| matches!(o.kind, acp::PermissionOptionKind::AllowOnce))
        .map(|o| o.option_id.to_string())
}

/// Client-side file system the agent may use (`fs/read_text_file`, `fs/write_text_file`).
#[async_trait]
pub trait ClientFs: Send + Sync {
    async fn read_text_file(&self, path: &str, line: Option<u32>, limit: Option<u32>) -> Result<String>;

    async fn write_text_file(&self, path: &str, content: &str) -> Result<()>;
}
