//! Packs: everything the tool installs or reads per language, as versioned
//! data. A pack is a folder with a `pack.toml` (see `manifest`) and its
//! files. Kinds: harnesses, languages, language servers and fonts. Where
//! packs come from, the platform names, and where installs go are config (`index.toml`,
//! `platforms.toml`); the installer (`engine`) only knows kinds of steps.

mod acp;
pub mod catalog;
pub mod engine;
pub mod lock;
pub mod manifest;
pub mod platform;
mod steps;
pub mod template;

pub use catalog::{Catalog, Distribution, Pack, Places, Timeouts};

/// `bytes`' SHA-256, in lowercase hex (as a download's checksum is
/// written).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(bytes))
}

/// `bytes` in lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub use engine::{HarnessLaunch, ServerLaunch, State, font_face, harness_launch, install, server_launch, state, tool_version};
pub use lock::{Installed, Lock};
pub use manifest::{Kind, Manifest, Provides};
pub use platform::{PlatformDef, Platforms};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PackError {
    #[error("pack config: {0}")]
    Config(String),
    #[error("pack {pack}: {reason}")]
    Manifest { pack: String, reason: String },
    #[error("no pack named '{0}'")]
    NotFound(String),
    #[error("pack '{0}' isn't installed (meta-harness pack install {0})")]
    NotInstalled(String),
    #[error("pack '{pack}' isn't a {} pack", kind.name())]
    NotA { pack: String, kind: Kind },
    /// Installed at another version, or its tool out of its range: its
    /// state (`State`'s words) and the command that fixes it.
    #[error("pack '{pack}' needs updating: {state} (meta-harness pack update {pack})")]
    Outdated { pack: String, state: String },
    #[error("pack '{pack}' is {state} (meta-harness pack install {pack})")]
    Broken { pack: String, state: String },
    #[error("installing {pack}: {reason}")]
    Install { pack: String, reason: String },
    #[error("pack lock: {0}")]
    Lock(String),
}
