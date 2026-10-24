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
            return Err(HarnessError::InvalidConfig { reason: format!("{path} is in the harness's own home, which isn't shared") });
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
