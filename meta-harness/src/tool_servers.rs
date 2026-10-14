//! The project's tool (MCP) servers, kept in `<project>/.nah/mcp.toml`
//! (committed with the project) and given to every harness's sessions,
//! after the meta-harness's own:
//!
//! ```toml
//! [[server]]
//! name = "docs"
//! url = "https://host/mcp"      # transport = "sse" for the older kind
//!
//! [[server]]
//! name = "git"
//! command = "git-mcp"
//! args = ["--repo", "."]
//! env = { TOKEN = "..." }
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use harness_contracts::{McpServer, McpTransport, ToolServers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    server: Vec<Entry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    /// `http` (the default) or `sse`, for a URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transport: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, String>,
}

impl Entry {
    fn server(self) -> Result<McpServer, String> {
        let pairs = |m: BTreeMap<String, String>| m.into_iter().collect::<Vec<_>>();
        let transport = match (self.url, self.command, self.transport.as_deref()) {
            (Some(url), None, None | Some("http")) => McpTransport::Http { url, headers: pairs(self.headers) },
            (Some(url), None, Some("sse")) => McpTransport::Sse { url, headers: pairs(self.headers) },
            (None, Some(command), None) => McpTransport::Stdio { command, args: self.args, env: pairs(self.env) },
            (Some(_), None, Some(other)) => return Err(format!("{}: transport is http or sse, not {other}", self.name)),
            _ => return Err(format!("{}: give a url or a command", self.name)),
        };
        Ok(McpServer { name: self.name, transport })
    }

    fn of(server: &McpServer) -> Self {
        let map = |pairs: &[(String, String)]| pairs.iter().cloned().collect::<BTreeMap<_, _>>();
        let mut entry = Self { name: server.name.clone(), url: None, transport: None, headers: BTreeMap::new(), command: None, args: Vec::new(), env: BTreeMap::new() };
        match &server.transport {
            McpTransport::Http { url, headers } => (entry.url, entry.headers) = (Some(url.clone()), map(headers)),
            McpTransport::Sse { url, headers } => (entry.url, entry.transport, entry.headers) = (Some(url.clone()), Some("sse".into()), map(headers)),
            McpTransport::Stdio { command, args, env } => (entry.command, entry.args, entry.env) = (Some(command.clone()), args.clone(), map(env)),
        }
        entry
    }
}

/// Ours, and the project's as its file says now.
pub struct ToolServerState {
    pub ours: Vec<McpServer>,
    pub file: ProjectToolServers,
}

impl ToolServerState {
    pub fn current(&self) -> Result<ToolServers, String> {
        Ok(ToolServers { ours: self.ours.clone(), project: self.file.read()? })
    }

    /// The project's, without any named like one of ours (ours are given
    /// to every session already).
    pub fn project(&self) -> Result<Vec<McpServer>, String> {
        Ok(self.file.read()?.into_iter().filter(|p| !self.ours.iter().any(|o| o.name == p.name)).collect())
    }
}

/// `.nah/mcp.toml` in a project.
pub struct ProjectToolServers {
    path: PathBuf,
}

impl ProjectToolServers {
    pub fn of(project: &Path) -> Self {
        Self { path: project.join(".nah").join("mcp.toml") }
    }

    /// The servers it lists (none when there's no file).
    pub fn read(&self) -> Result<Vec<McpServer>, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };
        let file: File = toml::from_str(&text).map_err(|e| format!("{}: {e}", self.path.display()))?;
        let mut servers: Vec<McpServer> = Vec::new();
        for entry in file.server {
            let server = entry.server().map_err(|e| format!("{}: {e}", self.path.display()))?;
            if servers.iter().any(|s| s.name == server.name) {
                return Err(format!("{}: {} is listed twice", self.path.display(), server.name));
            }
            servers.push(server);
        }
        Ok(servers)
    }

    /// Replaces the list.
    pub fn write(&self, servers: &[McpServer]) -> Result<(), String> {
        let file = File { server: servers.iter().map(Entry::of).collect() };
        let text = toml::to_string_pretty(&file).map_err(|e| e.to_string())?;
        let header = "# The project's tool (MCP) servers: every harness's sessions get them.\n\n";
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let temp = self.path.with_extension("toml.tmp");
        std::fs::write(&temp, format!("{header}{text}")).and_then(|()| std::fs::rename(&temp, &self.path)).map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_written_and_read_back_and_a_wrong_one_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let file = ProjectToolServers::of(dir.path());
        assert_eq!(file.read(), Ok(Vec::new()), "no file: none");
        let servers = vec![
            McpServer { name: "docs".into(), transport: McpTransport::Http { url: "https://h/mcp".into(), headers: vec![("A".into(), "b".into())] } },
            McpServer { name: "old".into(), transport: McpTransport::Sse { url: "https://h/sse".into(), headers: Vec::new() } },
            McpServer { name: "git".into(), transport: McpTransport::Stdio { command: "git-mcp".into(), args: vec!["-v".into()], env: Vec::new() } },
        ];
        file.write(&servers).unwrap();
        assert_eq!(file.read(), Ok(servers));
        std::fs::write(dir.path().join(".nah/mcp.toml"), "[[server]]\nname = \"x\"\n").unwrap();
        assert!(file.read().unwrap_err().contains("give a url or a command"));
    }
}
