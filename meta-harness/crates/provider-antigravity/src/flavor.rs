//! What Antigravity's ACP server does differently from the protocol defaults.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use harness_acp::{AcpFlavor, ClientFs, acp, default_approval_options};
use harness_contracts::{ApprovalDecision, ApprovalOption, Capabilities, RuntimeMode, RuntimePolicy, SessionConfig};
use harness_core::{HarnessError, Result};
use workspace_fs::{ReadTextFileRequest, WriteTextFileRequest, read_client_text_file, write_client_text_file};

/// Antigravity's native session modes.
const MODE_DEFAULT: &str = "default";
const MODE_AUTO_EDIT: &str = "auto_edit";
const MODE_YOLO: &str = "yolo";

pub struct AntigravityFlavor;

impl AcpFlavor for AntigravityFlavor {
    fn capabilities(&self, mut base: Capabilities) -> Capabilities {
        // Session/prompt/MCP flags come from `initialize` (see the instance probe).
        base.sessions.model_switch_in_session = true;
        base.tools.client_file_system = true;
        base
    }

    fn native_mode(&self, policy: &RuntimePolicy, available: &[String]) -> Option<String> {
        let mode = match policy.runtime_mode {
            RuntimeMode::ApprovalRequired => MODE_DEFAULT,
            RuntimeMode::AutoAcceptEdits => MODE_AUTO_EDIT,
            RuntimeMode::Auto | RuntimeMode::FullAccess => MODE_YOLO,
        };
        available.iter().find(|m| *m == mode).cloned()
    }

    fn app_mode(&self, native: &str) -> Option<RuntimeMode> {
        match native {
            MODE_DEFAULT => Some(RuntimeMode::ApprovalRequired),
            MODE_AUTO_EDIT => Some(RuntimeMode::AutoAcceptEdits),
            MODE_YOLO => Some(RuntimeMode::FullAccess),
            _ => None,
        }
    }

    fn approval_options(&self, native: &[acp::PermissionOption]) -> Vec<ApprovalOption> {
        default_approval_options(native)
            .into_iter()
            .map(|mut option| {
                if option.decision == ApprovalDecision::AcceptForSession {
                    option.warning = Some("Also allows every later call like this in the thread.".into());
                }
                option
            })
            .collect()
    }

    fn client_fs(&self, config: &SessionConfig) -> Option<Arc<dyn ClientFs>> {
        let cwd = config.policy.cwd.clone()?;
        Some(Arc::new(WorkspaceFs { roots: vec![PathBuf::from(cwd)] }))
    }
}

/// Client file access confined to the session's working directory.
struct WorkspaceFs {
    roots: Vec<PathBuf>,
}

fn rejected(err: workspace_fs::WorkspaceFsError) -> HarnessError {
    HarnessError::InvalidConfig { reason: err.to_string() }
}

#[async_trait]
impl ClientFs for WorkspaceFs {
    async fn read_text_file(&self, path: &str, line: Option<u32>, limit: Option<u32>) -> Result<String> {
        let request = ReadTextFileRequest {
            path: path.to_owned(),
            line: line.map(|l| l as usize),
            limit: limit.map(|l| l as usize),
        };
        read_client_text_file(&self.roots, &request).map(|r| r.content).map_err(rejected)
    }

    async fn write_text_file(&self, path: &str, content: &str) -> Result<()> {
        let request = WriteTextFileRequest { path: path.to_owned(), content: content.to_owned() };
        write_client_text_file(&self.roots, &request).map(|_| ()).map_err(rejected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_modes_round_trip_through_native_modes() {
        let available: Vec<String> = [MODE_DEFAULT, MODE_AUTO_EDIT, MODE_YOLO].map(String::from).to_vec();
        for mode in [RuntimeMode::ApprovalRequired, RuntimeMode::AutoAcceptEdits, RuntimeMode::FullAccess] {
            let policy = RuntimePolicy { runtime_mode: mode, ..RuntimePolicy::default() };
            let native = AntigravityFlavor.native_mode(&policy, &available).unwrap();
            assert_eq!(AntigravityFlavor.app_mode(&native), Some(mode));
        }
    }
}
