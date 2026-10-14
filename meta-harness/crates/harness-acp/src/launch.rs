//! How an ACP connection is obtained: a child process in production, an in-memory
//! pipe in tests. The adapter only sees byte streams.

use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use futures::{AsyncRead, AsyncWrite};
use harness_contracts::SessionConfig;
use harness_core::{HarnessError, Result};
use tokio::io::AsyncBufReadExt;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

pub struct AcpTransport {
    pub reader: Box<dyn AsyncRead + Send + Unpin>,
    pub writer: Box<dyn AsyncWrite + Send + Unpin>,
    /// Dropped when the session closes (e.g. kills the child process).
    pub guard: Box<dyn Send + Sync>,
}

#[async_trait]
pub trait AcpLauncher: Send + Sync {
    async fn launch(&self, config: &SessionConfig) -> Result<AcpTransport>;
}

/// Spawns the agent as a child process speaking ACP on stdio; stderr goes to tracing.
pub struct ProcessLauncher {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The server's variables it doesn't get.
    pub env_remove: Vec<String>,
}

impl From<&harness_contracts::Launch> for ProcessLauncher {
    fn from(launch: &harness_contracts::Launch) -> Self {
        Self { program: PathBuf::from(&launch.program), args: launch.args.clone(), env: launch.env.clone(), env_remove: launch.env_remove.clone() }
    }
}

#[async_trait]
impl AcpLauncher for ProcessLauncher {
    async fn launch(&self, config: &SessionConfig) -> Result<AcpTransport> {
        // A temp folder of its own, removed with it: what it leaves there
        // (logs, unpacked files) doesn't pile up.
        let temp = tempfile::Builder::new().prefix("nah-agent-").tempdir()?;
        let mut command = tokio::process::Command::new(&self.program);
        for name in &self.env_remove {
            command.env_remove(name);
        }
        command
            .args(&self.args)
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .envs(["TMPDIR", "TMP", "TEMP"].map(|k| (k, temp.path())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Its own process group: what it starts (a harness, an emulator's
        // guest) goes with it.
        #[cfg(unix)]
        command.process_group(0);
        if let Some(cwd) = &config.policy.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            return Err(HarnessError::Provider { reason: "agent process has no stdio pipes".into() });
        };
        let program = self.program.display().to_string();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "acp_agent_stderr", agent = %program, "{line}");
            }
        });
        Ok(AcpTransport {
            reader: Box::new(stdout.compat()),
            writer: Box::new(stdin.compat_write()),
            guard: Box::new(ProcessTree { child, _temp: temp }),
        })
    }
}

/// The agent's process and everything it started: all stopped when the
/// session lets go of it, then its temp folder removed.
struct ProcessTree {
    child: tokio::process::Child,
    _temp: tempfile::TempDir,
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.id().and_then(|id| rustix::process::Pid::from_raw(id as i32)) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
}
