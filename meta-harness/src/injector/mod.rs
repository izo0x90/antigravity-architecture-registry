use serde_json::{json, Value};
use crate::driver::ProcessLaunchRecord;
use crate::error::HarnessError;

/// Scaffolding trait for injecting in-process or remote tooling into agent harnesses.
///
/// Decouples how different harnesses receive MCP server configurations:
/// - ACP-based harnesses (e.g. Antigravity) receive MCP servers dynamically over the protocol wire in `session/new`.
/// - CLI-based harnesses (e.g. future Claude Code, Codex adapters) may configure CLI flags or environment variables.
pub trait HarnessToolInjector: Send + Sync {
    /// Injects configuration, arguments, or environment variables into the process launch record before spawning.
    /// Default implementation is a no-op, meaning no process-level modifications are needed.
    fn prepare_launch(
        &self,
        _base_url: &str,
        _launch: &mut ProcessLaunchRecord,
    ) -> Result<(), HarnessError> {
        Ok(())
    }

    /// Provides MCP server entries to transmit over the ACP wire protocol during `session/new`.
    /// For ACP harnesses like Antigravity, this allows pure in-memory zero-config tool injection
    /// without touching the filesystem or leaving stale files on disk.
    fn session_mcp_servers(&self, _base_url: &str) -> Vec<Value> {
        vec![]
    }
}

/// Tool injector specialized for Antigravity (AGY) sessions.
///
/// Antigravity implements the ACP (Agent Client Protocol), which natively supports
/// injecting HTTP/SSE MCP servers directly in the `session/new` RPC request.
/// This avoids touching the filesystem, modifying user profiles, or leaving stale configs.
pub struct AgyToolInjector {
    server_name: String,
}

impl Default for AgyToolInjector {
    fn default() -> Self {
        Self {
            server_name: "architecture-registry".to_string(),
        }
    }
}

impl AgyToolInjector {
    pub fn new(server_name: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
        }
    }
}

impl HarnessToolInjector for AgyToolInjector {
    fn session_mcp_servers(&self, base_url: &str) -> Vec<Value> {
        let sse_url = format!("{}/api/mcp/sse", base_url.trim_end_matches('/'));
        vec![json!({
            "type": "http",
            "name": &self.server_name,
            "url": sse_url,
            "headers": []
        })]
    }
}

