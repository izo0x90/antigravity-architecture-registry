//! The tool (MCP) servers a harness's own configuration adds: each file
//! is JSON, its servers an object (name → server) at a JSON pointer. A file
//! that isn't there adds none; one that can't be read says so.

use std::path::PathBuf;

use harness_contracts::{Scope, ToolServerInfo};

/// Where a harness keeps a list of tool servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolConfigFile {
    pub path: PathBuf,
    /// Where in the file the servers are (`/mcpServers`).
    pub pointer: String,
    pub scope: Scope,
}

/// The servers in `files`, in order, and what couldn't be read.
pub fn list_tool_servers(files: &[ToolConfigFile]) -> (Vec<ToolServerInfo>, Vec<String>) {
    let (mut servers, mut problems) = (Vec::new(), Vec::new());
    for file in files {
        let text = match std::fs::read_to_string(&file.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                problems.push(format!("couldn't read {}: {e}", file.path.display()));
                continue;
            }
        };
        let json: serde_json::Value = match serde_json::from_str(&text) {
            Ok(json) => json,
            Err(e) => {
                problems.push(format!("{} isn't JSON: {e}", file.path.display()));
                continue;
            }
        };
        let Some(listed) = json.pointer(&file.pointer).and_then(serde_json::Value::as_object) else { continue };
        servers.extend(listed.iter().map(|(name, server)| ToolServerInfo {
            name: name.clone(),
            transport: transport(server).to_owned(),
            file: file.path.display().to_string(),
            scope: file.scope,
        }));
    }
    (servers, problems)
}

/// Its `type`, else `stdio` for a command and `http` for a URL.
fn transport(server: &serde_json::Value) -> &str {
    match server.get("type").and_then(serde_json::Value::as_str) {
        Some(kind) => kind,
        None if server.get("command").is_some() => "stdio",
        None => "http",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_come_from_each_file_at_its_pointer_and_bad_files_say_so() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.json");
        std::fs::write(
            &user,
            r#"{"projects":{"/p/x":{"mcpServers":{"docs":{"type":"sse","url":"u"}}}},"mcpServers":{"git":{"command":"git-mcp"}}}"#,
        )
        .unwrap();
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{").unwrap();
        let file = |path: &std::path::Path, pointer: &str, scope| ToolConfigFile { path: path.into(), pointer: pointer.into(), scope };
        let (servers, problems) = list_tool_servers(&[
            file(&user, "/mcpServers", Scope::User),
            file(&user, "/projects/~1p~1x/mcpServers", Scope::Project),
            file(&broken, "/mcpServers", Scope::Project),
            file(&dir.path().join("missing.json"), "/mcpServers", Scope::Project),
        ]);
        let listed: Vec<_> = servers.iter().map(|s| (s.name.as_str(), s.transport.as_str(), s.scope)).collect();
        assert_eq!(listed, [("git", "stdio", Scope::User), ("docs", "sse", Scope::Project)]);
        assert!(matches!(problems.as_slice(), [p] if p.contains("broken.json")), "{problems:?}");
    }
}
