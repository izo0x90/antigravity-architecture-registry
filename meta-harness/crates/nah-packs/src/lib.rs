//! Packs: everything the tool installs or reads per language, as versioned
//! data. A pack is a folder with a `pack.toml` (see `manifest`) and its
//! files. Kinds: harnesses, languages and language servers. Where packs come from,
//! the platform names, and where installs go are config (`index.toml`,
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
pub use engine::{HarnessLaunch, ServerLaunch, State, harness_launch, install, server_launch, state, tool_version};
pub use lock::{Installed, Lock};
pub use manifest::{Kind, Manifest};
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
    #[error("installing {pack}: {reason}")]
    Install { pack: String, reason: String },
    #[error("pack lock: {0}")]
    Lock(String),
}
