//! What's installed: one entry per pack, written after a successful install.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::PackError;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    #[serde(default)]
    pub packs: BTreeMap<String, Installed>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    pub pack_version: String,
    /// The tool's version: what the check reported, else what the pack pins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_version: Option<String>,
    pub platform: String,
    /// The pack's install folder.
    pub dest: PathBuf,
    /// The checksums of what was downloaded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sha256: Vec<String>,
    /// Seconds since the epoch.
    pub installed_at: u64,
}

impl Lock {
    /// The lock at `path` (empty when there's none yet).
    pub fn read(path: &Path) -> Result<Self, PackError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| PackError::Lock(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(PackError::Lock(format!("{}: {e}", path.display()))),
        }
    }

    /// Writes it whole: to a temp file, then renamed into place.
    pub fn write(&self, path: &Path) -> Result<(), PackError> {
        let io = |e: std::io::Error| PackError::Lock(format!("{}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| PackError::Lock(e.to_string()))?;
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }
}
