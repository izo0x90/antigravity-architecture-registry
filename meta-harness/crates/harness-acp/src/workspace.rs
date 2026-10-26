//! The agent's file access through us (`fs/read_text_file`,
//! `fs/write_text_file`), confined to the session's folder.

use std::path::PathBuf;

use harness_core::{HarnessError, Result};
use workspace_fs::{ReadTextFileRequest, WriteTextFileRequest, read_client_text_file, write_client_text_file};

#[derive(Debug, Clone)]
pub struct WorkspaceFs {
    roots: Vec<PathBuf>,
    /// Inside the roots, never read or written (a harness's kept login).
    private: Vec<PathBuf>,
}

fn rejected(err: workspace_fs::WorkspaceFsError) -> HarnessError {
    HarnessError::InvalidConfig { reason: err.to_string() }
}

impl WorkspaceFs {
    pub fn new(roots: Vec<PathBuf>, private: Vec<PathBuf>) -> Self {
        Self { roots, private }
    }

    /// Refuses a path in a private folder (however it's named).
    fn allowed(&self, path: &str) -> Result<()> {
        let resolved = workspace_fs::resolve_client_file_path(&self.roots, path).map_err(rejected)?;
        let private = |dir: &PathBuf| dir.canonicalize().is_ok_and(|dir| resolved.starts_with(dir)) || resolved.starts_with(dir);
        if self.private.iter().any(private) {
            return Err(HarnessError::InvalidConfig {
                reason: format!("{path} is the server's own (its state, the harnesses' logins): it isn't shared"),
            });
        }
        Ok(())
    }

    pub fn read_text_file(&self, path: &str, line: Option<u32>, limit: Option<u32>) -> Result<String> {
        self.allowed(path)?;
        let request = ReadTextFileRequest { path: path.to_owned(), line: line.map(|l| l as usize), limit: limit.map(|l| l as usize) };
        read_client_text_file(&self.roots, &request).map(|r| r.content).map_err(rejected)
    }

    pub fn write_text_file(&self, path: &str, content: &str) -> Result<()> {
        self.allowed(path)?;
        let request = WriteTextFileRequest { path: path.to_owned(), content: content.to_owned() };
        write_client_text_file(&self.roots, &request).map(|_| ()).map_err(rejected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_server_keeps_is_never_read_or_written_however_its_named() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let private = root.join(".nah/harness");
        std::fs::create_dir_all(private.join("claude-code")).unwrap();
        std::fs::write(private.join("claude-code/.credentials.json"), "secret").unwrap();
        let fs = WorkspaceFs::new(vec![root.clone()], vec![private.clone()]);
        for path in [".nah/harness/claude-code/.credentials.json", "src/../.nah/harness/claude-code/.credentials.json"] {
            assert!(fs.read_text_file(path, None, None).is_err(), "{path}");
            assert!(fs.write_text_file(path, "planted").is_err(), "{path}");
        }
        // Through a link: there or not yet there.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(private.join("claude-code/.credentials.json"), root.join("notes.json")).unwrap();
            assert!(fs.read_text_file("notes.json", None, None).is_err());
            std::os::unix::fs::symlink(private.join("antigravity/token.json"), root.join("cache.json")).unwrap();
            assert!(fs.write_text_file("cache.json", "planted").is_err());
            assert!(!private.join("antigravity/token.json").exists());
        }
        assert_eq!(std::fs::read_to_string(private.join("claude-code/.credentials.json")).unwrap(), "secret");
    }
}
