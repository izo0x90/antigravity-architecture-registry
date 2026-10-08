//! Platform names come from config (`platforms.toml`): which operating
//! system and architecture the running binary reports map to which name,
//! and the values packs can use for each (`{os}`, `{arch}`).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::PackError;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Platforms {
    #[serde(rename = "platform")]
    pub list: Vec<PlatformDef>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformDef {
    pub name: String,
    /// What the binary reports (Rust's `std::env::consts::OS` and `ARCH`).
    pub matches: Matches,
    /// Values packs can use as placeholders on this platform.
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Matches {
    pub os: String,
    pub arch: String,
}

impl Platforms {
    pub fn parse(text: &str) -> Result<Self, PackError> {
        let platforms: Self = toml::from_str(text).map_err(|e| PackError::Config(format!("platforms.toml: {e}")))?;
        let mut seen = std::collections::BTreeSet::new();
        for p in &platforms.list {
            if !seen.insert(&p.name) {
                return Err(PackError::Config(format!("platforms.toml: '{}' is defined twice", p.name)));
            }
        }
        Ok(platforms)
    }

    pub fn get(&self, name: &str) -> Option<&PlatformDef> {
        self.list.iter().find(|p| p.name == name)
    }

    /// The platform for an operating system and architecture.
    pub fn for_target(&self, os: &str, arch: &str) -> Result<&PlatformDef, PackError> {
        self.list
            .iter()
            .find(|p| p.matches.os == os && p.matches.arch == arch)
            .ok_or_else(|| PackError::Config(format!("platforms.toml has no platform for {os} {arch}")))
    }

    /// The platform this binary runs on.
    pub fn current(&self) -> Result<&PlatformDef, PackError> {
        self.for_target(std::env::consts::OS, std::env::consts::ARCH)
    }
}
