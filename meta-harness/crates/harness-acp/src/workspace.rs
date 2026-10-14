//! The agent's file access through us (`fs/read_text_file`,
//! `fs/write_text_file`), confined to the session's folder.

use std::path::PathBuf;

use harness_core::{HarnessError, Result};
use workspace_fs::{ReadTextFileRequest, WriteTextFileRequest, read_client_text_file, write_client_text_file};

#[derive(Debug, Clone)]
pub struct WorkspaceFs {
    roots: Vec<PathBuf>,
}

fn rejected(err: workspace_fs::WorkspaceFsError) -> HarnessError {
    HarnessError::InvalidConfig { reason: err.to_string() }
}

impl WorkspaceFs {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots }
    }

    pub fn read_text_file(&self, path: &str, line: Option<u32>, limit: Option<u32>) -> Result<String> {
        let request = ReadTextFileRequest { path: path.to_owned(), line: line.map(|l| l as usize), limit: limit.map(|l| l as usize) };
        read_client_text_file(&self.roots, &request).map(|r| r.content).map_err(rejected)
    }

    pub fn write_text_file(&self, path: &str, content: &str) -> Result<()> {
        let request = WriteTextFileRequest { path: path.to_owned(), content: content.to_owned() };
        write_client_text_file(&self.roots, &request).map(|_| ()).map_err(rejected)
    }
}
