//! Platform names come from config (`platforms.toml`): which operating
//! system, architecture and (where it matters) memory page size the running
//! binary reports map to which name, and the values packs can use for each
//! (`{os}`, `{arch}`). The most specific match wins.

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
    /// Supported for developing this tool, not for its users (a sandbox).
    #[serde(default)]
    pub developer: bool,
    /// How this platform's system groups are kept, when packs may need one.
    #[serde(default)]
    pub groups: Option<Groups>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Groups {
    /// The file listing them (`name:...` per line).
    pub file: String,
    /// The program and arguments creating one (its name goes last).
    pub create: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Matches {
    pub os: String,
    pub arch: String,
    /// The memory page size in bytes, when only some machines of the os
    /// and arch can run a build.
    #[serde(default)]
    pub page_size: Option<u64>,
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

    /// The platform for an operating system, architecture and page size:
    /// of those that match, the one that says the most.
    pub fn for_target(&self, os: &str, arch: &str, page_size: u64) -> Result<&PlatformDef, PackError> {
        let matching: Vec<&PlatformDef> = self.list.iter().filter(|p| p.matches.os == os && p.matches.arch == arch && p.matches.page_size.is_none_or(|s| s == page_size)).collect();
        let most = matching.iter().map(|p| p.matches.page_size.is_some()).max().ok_or_else(|| PackError::Config(format!("platforms.toml has no platform for {os} {arch} ({page_size}-byte pages)")))?;
        match matching.into_iter().filter(|p| p.matches.page_size.is_some() == most).collect::<Vec<_>>().as_slice() {
            [one] => Ok(one),
            many => Err(PackError::Config(format!("platforms.toml: {} all match {os} {arch} ({page_size}-byte pages) as closely", many.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")))),
        }
    }

    /// `platform`, then the platforms it narrows (same os and arch, no page
    /// size): a pack without an entry for the narrow one uses the general.
    pub fn and_general<'a>(&'a self, platform: &'a PlatformDef) -> impl Iterator<Item = &'a PlatformDef> {
        let general = self.list.iter().filter(move |p| platform.matches.page_size.is_some() && p.matches.page_size.is_none() && p.matches.os == platform.matches.os && p.matches.arch == platform.matches.arch);
        std::iter::once(platform).chain(general)
    }

    /// The platform this binary runs on.
    pub fn current(&self) -> Result<&PlatformDef, PackError> {
        self.for_target(std::env::consts::OS, std::env::consts::ARCH, page_size())
    }
}

/// This machine's memory page size in bytes.
pub fn page_size() -> u64 {
    #[cfg(unix)]
    return rustix::param::page_size() as u64;
    // Windows: platforms there don't match on it.
    #[cfg(not(unix))]
    return 4096;
}
