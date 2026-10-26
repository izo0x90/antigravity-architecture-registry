//! A pack's manifest (`pack.toml`): what it is, the external tool it manages
//! and the versions of that tool it supports, how to install it on each
//! platform and how to check it, and, by kind, how to start it (a language
//! server), what it provides (a harness), the files describing a language,
//! or a font's faces (the cockpit's text, drawn in the browser).
//! Everything is checked when the pack is loaded, so a bad pack never gets
//! as far as an install.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::PackError;
use crate::platform::Platforms;
use crate::template;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub pack: PackInfo,
    #[serde(default)]
    pub tool: Option<Tool>,
    #[serde(default)]
    pub install: Vec<Install>,
    /// Language servers: how to start one and what to send it.
    #[serde(default)]
    pub server: Option<ServerContract>,
    /// Harnesses: how the harness driver starts it.
    #[serde(default)]
    pub provides: Option<Provides>,
    /// Languages: the files describing the language.
    #[serde(default)]
    pub language: Option<LanguageFiles>,
    /// Fonts: the face's file.
    #[serde(default)]
    pub font: Option<FontFiles>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackInfo {
    pub name: String,
    pub kind: Kind,
    /// This pack's own version.
    pub version: String,
    pub description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// An agent harness the server drives.
    Harness,
    /// A language: its syntax rules, highlight queries and language server.
    Language,
    /// A language server (LSP): exact links for a language.
    Server,
    /// A font the cockpit can draw its text in.
    Font,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Harness => "harness",
            Kind::Language => "language",
            Kind::Server => "server",
            Kind::Font => "font",
        }
    }
}

/// The external program a pack manages.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    /// The version this pack installs, when it pins one.
    #[serde(default)]
    pub version: Option<String>,
    /// The versions this pack was made for (a semver range).
    pub supports: String,
    /// How to ask the installed tool its version.
    #[serde(default)]
    pub check: Option<Check>,
}

/// How the installed tool is checked and its version found: by running a
/// command, or (a harness) by starting it and asking over ACP.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Check {
    Acp { acp: AcpCheck },
    Command(CommandCheck),
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandCheck {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// A pattern over its output; the first group is the version.
    pub version: String,
}

/// The pack's launch is started and sent ACP `initialize`; the version is
/// the one its answer's `agentInfo` gives.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcpCheck {
    /// How long it may take to answer (emulated, it's slow).
    pub timeout_s: u64,
    /// A pattern over the version it answers with; the first group is the
    /// version (when it says more than the version).
    #[serde(default)]
    pub version: Option<String>,
}

/// How to install on some platforms: values per platform, then the steps.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Install {
    pub platforms: Vec<String>,
    /// Per platform: values the steps can use (`{name}`), such as a file
    /// name or a checksum. Every platform listed defines the same names.
    #[serde(default)]
    pub vars: BTreeMap<String, BTreeMap<String, String>>,
    pub steps: Vec<Step>,
    /// How a harness installed this way is started, instead of
    /// `[provides].launch` (an emulator in front of it, say).
    #[serde(default)]
    pub launch: Option<Launch>,
    /// Files the install leaves in its folder that its check doesn't reach
    /// (a helper only a session starts, say): a missing one makes the pack
    /// broken.
    #[serde(default)]
    pub holds: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// Fetch a file, check its checksum, and unpack or place it.
    Download(Download),
    /// Run a program (a package manager, say), without a shell.
    Run(Command),
    /// A relative symbolic link inside the install folder.
    Link(Link),
    /// A system group, created if it's missing (only as root; never
    /// through sudo).
    EnsureGroup(EnsureGroup),
    /// A certificate from the system's store, by its subject, saved in the
    /// install folder.
    SystemCa(SystemCa),
    /// A file appended to the members of an archive in the install folder
    /// that match a pattern.
    PatchArchive(PatchArchive),
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Download {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub unpack: Unpack,
    /// Where under the install folder it goes ("" for the folder itself).
    #[serde(default)]
    pub into: String,
    /// The file's name when it isn't unpacked.
    #[serde(default)]
    pub save_as: Option<String>,
    /// Only these members of an archive (a path takes everything under it);
    /// none: all of it.
    #[serde(default)]
    pub extract: Vec<String>,
    /// How long it may take (else the index's default).
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unpack {
    #[default]
    None,
    Zip,
    TarGz,
    /// A Debian package's data archive.
    Deb,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// How long it may take (else the index's default).
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    /// Where the link is, under the install folder.
    pub path: String,
    /// What it points to, relative to the link's folder.
    pub target: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnsureGroup {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemCa {
    /// The system's certificate store (a PEM bundle).
    pub store: String,
    /// Text the certificate's subject contains.
    pub subject: String,
    /// Where it's saved, under the install folder.
    pub save_as: String,
    /// Not finding it is fine (the step is skipped).
    #[serde(default)]
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchArchive {
    /// The archive, under the install folder (a zip, possibly behind a
    /// launcher).
    pub file: String,
    /// Patterns for the members to append to.
    pub members: Vec<String>,
    /// The file appended, under the install folder.
    pub append: String,
    /// Nothing to append (an optional file that wasn't found) is fine.
    #[serde(default)]
    pub optional: bool,
    /// The archive can't change size (it sits in a fixed-size section of a
    /// program): each member keeps its size, the file goes first and
    /// trailing blocks are dropped to make room.
    #[serde(default)]
    pub keep_size: Option<KeepSize>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeepSize {
    /// What starts each block of a member (only whole blocks are dropped).
    pub block: String,
    /// The character a padding line is made of (a comment in the member's format).
    pub pad: String,
}

/// How a language server is started (over stdio) and what it's sent:
/// `{root}` is the project, `{cache}` a folder for its caches under the
/// user's data folder, one per project (caches never go into the project).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerContract {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment for the server, with the same placeholders.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// The LSP requests it must answer (textDocument/definition at least).
    pub requires: Vec<String>,
    /// How long it may take to be ready, and to answer a request.
    pub ready_timeout_s: u64,
    pub request_timeout_s: u64,
    /// How long it must look ready before it counts as ready (it announces
    /// its loading only after `initialized`).
    #[serde(default)]
    pub settle_ms: u64,
    /// Sent with `initialize`.
    #[serde(default)]
    pub initialization_options: toml::Table,
    /// Its workspace settings (`workspace/didChangeConfiguration`).
    #[serde(default)]
    pub settings: toml::Table,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provides {
    /// How it's started: `{dest}` is the install folder, `{user}` the user.
    pub launch: Launch,
    /// The harness driver that runs it (its kind).
    pub driver: String,
    /// The instance it is (default: the pack's name).
    #[serde(default)]
    pub instance: Option<String>,
    /// Its name in the cockpit (default: the pack's name).
    #[serde(default)]
    pub display_name: Option<String>,
    /// The driver's settings for it: their shape is the driver's, which
    /// checks them.
    #[serde(default)]
    pub settings: toml::Table,
}

/// A program, its arguments and environment, with placeholders.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Variables of the server's environment it doesn't get.
    #[serde(default)]
    pub env_remove: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageFiles {
    /// The syntax rules (a file of the pack).
    pub syntax: String,
    /// tree-sitter highlight queries (a file of the pack).
    #[serde(default)]
    pub highlights: Option<String>,
    /// The language server pack that knows this language exactly.
    #[serde(default)]
    pub server: Option<String>,
}

/// A font: the face the cockpit draws in, a file its install keeps (in
/// its `holds`). One face: egui draws no bold or italic of its own.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontFiles {
    /// The face (TTF or OTF), a path in the install folder.
    pub face: String,
}

/// Placeholders a server contract may use.
pub const SERVER_PLACEHOLDERS: [&str; 4] = ["root", "cache", "dest", "tool_version"];
/// Placeholders a check may use.
pub const CHECK_PLACEHOLDERS: [&str; 2] = ["dest", "tool_version"];
/// Placeholders a harness's launch may use.
pub const LAUNCH_PLACEHOLDERS: [&str; 3] = ["dest", "user", "tool_version"];

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Checks everything that can be checked before installing: the kind's
    /// sections, versions and ranges, patterns, platforms, placeholders, and
    /// that every download has a checksum. `files` are the pack's other files.
    pub fn validate(&self, platforms: &Platforms, files: &BTreeSet<String>) -> Result<(), PackError> {
        let fail = |what: String| PackError::Manifest { pack: self.pack.name.clone(), reason: what };
        let p = &self.pack;
        if p.name.is_empty() || !p.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(fail("name: lowercase letters, digits and '-' only".into()));
        }
        semver::Version::parse(&p.version).map_err(|e| fail(format!("pack.version: {e}")))?;
        if p.description.trim().is_empty() {
            return Err(fail("pack.description is empty".into()));
        }
        // What each kind has, and doesn't.
        let has = |present: bool, section: &str, wanted: bool| -> Result<(), PackError> {
            match (present, wanted) {
                (false, true) => Err(fail(format!("{} packs need [{section}]", p.kind.name()))),
                (true, false) => Err(fail(format!("{} packs have no [{section}]", p.kind.name()))),
                _ => Ok(()),
            }
        };
        let (tool, install, server, provides, language, font) = match p.kind {
            Kind::Harness => (true, true, false, true, false, false),
            Kind::Server => (true, true, true, false, false, false),
            Kind::Language => (false, false, false, false, true, false),
            Kind::Font => (true, true, false, false, false, true),
        };
        has(self.tool.is_some(), "tool", tool)?;
        has(!self.install.is_empty(), "install", install)?;
        has(self.server.is_some(), "server", server)?;
        has(self.provides.is_some(), "provides", provides)?;
        has(self.language.is_some(), "language", language)?;
        has(self.font.is_some(), "font", font)?;
        // A font's face is what its install keeps, on every platform.
        if let Some(font) = &self.font {
            for (i, install) in self.install.iter().enumerate() {
                if !install.holds.contains(&font.face) {
                    return Err(fail(format!("install[{i}]: holds doesn't keep the face {}", font.face)));
                }
            }
        }

        let mut known: BTreeSet<&str> = ["dest", "platform"].into_iter().collect();
        if let Some(tool) = &self.tool {
            let range = semver::VersionReq::parse(&tool.supports).map_err(|e| fail(format!("tool.supports: {e}")))?;
            if let Some(v) = &tool.version {
                let v = semver::Version::parse(v).map_err(|e| fail(format!("tool.version: {e}")))?;
                if !range.matches(&v) {
                    return Err(fail(format!("tool.version {v} isn't in tool.supports {range}")));
                }
                known.insert("tool_version");
            }
            match &tool.check {
                Some(Check::Command(check)) => {
                    let re = regex::Regex::new(&check.version).map_err(|e| fail(format!("tool.check.version: {e}")))?;
                    if re.captures_len() < 2 {
                        return Err(fail("tool.check.version needs a group for the version".into()));
                    }
                    let allowed: BTreeSet<&str> =
                        CHECK_PLACEHOLDERS.into_iter().filter(|n| *n != "tool_version" || tool.version.is_some()).collect();
                    for text in std::iter::once(&check.command).chain(&check.args) {
                        template::check(text, &allowed).map_err(|e| fail(format!("tool.check: {e}")))?;
                    }
                }
                Some(Check::Acp { acp }) => {
                    if p.kind != Kind::Harness {
                        return Err(fail("tool.check.acp is for harness packs".into()));
                    }
                    if acp.timeout_s == 0 {
                        return Err(fail("tool.check.acp.timeout_s must be more than 0".into()));
                    }
                    if let Some(pattern) = &acp.version {
                        let re = regex::Regex::new(pattern).map_err(|e| fail(format!("tool.check.acp.version: {e}")))?;
                        if re.captures_len() < 2 {
                            return Err(fail("tool.check.acp.version needs a group for the version".into()));
                        }
                    }
                }
                None => {}
            }
            // A server is checked before it's started: it needs a check.
            if p.kind == Kind::Server && tool.check.is_none() {
                return Err(fail("a server pack needs [tool.check]".into()));
            }
        }

        let mut covered: BTreeSet<&str> = BTreeSet::new();
        for (i, install) in self.install.iter().enumerate() {
            let at = format!("install[{i}]");
            if install.platforms.is_empty() || install.steps.is_empty() {
                return Err(fail(format!("{at}: needs platforms and steps")));
            }
            for held in &install.holds {
                inside(held).map_err(|e| fail(format!("{at}: holds: {e}")))?;
            }
            // Values per platform: every platform listed has them, all the same names.
            if !install.vars.is_empty() {
                let names: BTreeSet<&String> = install.vars.values().next().map(|v| v.keys().collect()).unwrap_or_default();
                for platform in install.vars.keys() {
                    if !install.platforms.contains(platform) {
                        return Err(fail(format!("{at}: vars for '{platform}', which isn't in its platforms")));
                    }
                }
                for platform in &install.platforms {
                    match install.vars.get(platform) {
                        None => return Err(fail(format!("{at}: no vars for '{platform}'"))),
                        Some(v) if v.keys().collect::<BTreeSet<_>>() != names => {
                            return Err(fail(format!("{at}: '{platform}' doesn't define the same vars as the others")));
                        }
                        Some(_) => {}
                    }
                }
            }
            for platform in &install.platforms {
                let def = platforms.get(platform).ok_or_else(|| fail(format!("{at}: no platform '{platform}' in platforms.toml")))?;
                if !covered.insert(platform) {
                    return Err(fail(format!("{at}: platform '{platform}' is installed by two entries")));
                }
                let vars = install.vars.get(platform);
                // Every step must expand on every platform it's for.
                let mut values: BTreeMap<&str, &str> = def.vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                if let Some(vars) = vars {
                    values.extend(vars.iter().map(|(k, v)| (k.as_str(), v.as_str())));
                }
                for name in &known {
                    values.entry(name).or_insert("");
                }
                for (s, step) in install.steps.iter().enumerate() {
                    let at = format!("{at}.steps[{s}] ({platform})");
                    match step {
                        Step::Download(d) => {
                            template::expand(&d.url, &values).map_err(|e| fail(format!("{at}: {e}")))?;
                            for text in std::iter::once(&d.into).chain(d.save_as.as_ref()) {
                                inside(&template::expand(text, &values).map_err(|e| fail(format!("{at}: {e}")))?)
                                    .map_err(|e| fail(format!("{at}: {e}")))?;
                            }
                            let sum = template::expand(&d.sha256, &values).map_err(|e| fail(format!("{at}: {e}")))?;
                            if sum.len() != 64 || !sum.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
                                return Err(fail(format!("{at}: sha256 must be 64 lowercase hex digits (got '{sum}')")));
                            }
                            match (d.unpack, &d.save_as) {
                                (Unpack::None, None) => return Err(fail(format!("{at}: a download that isn't unpacked needs save_as"))),
                                (Unpack::Zip | Unpack::TarGz | Unpack::Deb, Some(_)) => {
                                    return Err(fail(format!("{at}: save_as is for downloads that aren't unpacked")));
                                }
                                _ => {}
                            }
                            if d.unpack == Unpack::None && !d.extract.is_empty() {
                                return Err(fail(format!("{at}: extract is for downloads that are unpacked")));
                            }
                            for path in &d.extract {
                                inside(&template::expand(path, &values).map_err(|e| fail(format!("{at}: {e}")))?)
                                    .map_err(|e| fail(format!("{at}: extract: {e}")))?;
                            }
                        }
                        Step::Run(c) => {
                            for text in std::iter::once(&c.command).chain(&c.args) {
                                template::expand(text, &values).map_err(|e| fail(format!("{at}: {e}")))?;
                            }
                        }
                        Step::Link(l) => {
                            let path = template::expand(&l.path, &values).map_err(|e| fail(format!("{at}: {e}")))?;
                            inside(&path).map_err(|e| fail(format!("{at}: link.path: {e}")))?;
                            let target = template::expand(&l.target, &values).map_err(|e| fail(format!("{at}: {e}")))?;
                            if target.starts_with('/') {
                                return Err(fail(format!("{at}: link.target must be relative (got '{target}')")));
                            }
                            // Where it points, from the link's folder, stays inside too.
                            let parent = std::path::Path::new(&path).parent().map(std::path::Path::to_path_buf).unwrap_or_default();
                            resolved_inside(&parent.join(&target)).map_err(|e| fail(format!("{at}: link.target: {e}")))?;
                        }
                        Step::EnsureGroup(g) => {
                            let name = template::expand(&g.name, &values).map_err(|e| fail(format!("{at}: {e}")))?;
                            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                                return Err(fail(format!("{at}: ensure_group.name '{name}' isn't a group name")));
                            }
                        }
                        Step::SystemCa(c) => {
                            for text in [&c.store, &c.subject] {
                                template::expand(text, &values).map_err(|e| fail(format!("{at}: {e}")))?;
                            }
                            inside(&template::expand(&c.save_as, &values).map_err(|e| fail(format!("{at}: {e}")))?)
                                .map_err(|e| fail(format!("{at}: system_ca.save_as: {e}")))?;
                        }
                        Step::PatchArchive(p) => {
                            for text in [&p.file, &p.append] {
                                inside(&template::expand(text, &values).map_err(|e| fail(format!("{at}: {e}")))?)
                                    .map_err(|e| fail(format!("{at}: patch_archive: {e}")))?;
                            }
                            if p.members.is_empty() {
                                return Err(fail(format!("{at}: patch_archive.members is empty")));
                            }
                            for m in &p.members {
                                glob::Pattern::new(m).map_err(|e| fail(format!("{at}: patch_archive.members '{m}': {e}")))?;
                            }
                            if let Some(k) = &p.keep_size
                                && (k.block.is_empty() || k.pad.chars().count() != 1)
                            {
                                return Err(fail(format!("{at}: patch_archive.keep_size needs a block marker and a one-character pad")));
                            }
                        }
                    }
                }
            }
        }

        if let Some(server) = &self.server {
            if server.ready_timeout_s == 0 {
                return Err(fail("server.ready_timeout_s must be more than 0".into()));
            }
            if server.request_timeout_s == 0 {
                return Err(fail("server.request_timeout_s must be more than 0".into()));
            }
            if !server.requires.iter().any(|r| r == "textDocument/definition") {
                return Err(fail("server.requires must include textDocument/definition".into()));
            }
            let allowed: BTreeSet<&str> =
                SERVER_PLACEHOLDERS.into_iter().filter(|n| *n != "tool_version" || known.contains("tool_version")).collect();
            for text in std::iter::once(&server.command).chain(&server.args).chain(server.env.values()) {
                template::check(text, &allowed).map_err(|e| fail(format!("server: {e}")))?;
            }
            // Where it writes (its environment) is never the project.
            if let Some((k, _)) = server.env.iter().find(|(_, v)| template::names(v).contains(&"root")) {
                return Err(fail(format!("server.env.{k}: caches go outside the project (use {{cache}}, not {{root}})")));
            }
            // Nor a folder under the project handed in an argument (the
            // project itself may be named: `--root={root}`).
            if let Some(arg) = server.args.iter().find(|a| a.contains("{root}/") || a.contains("{root}\\")) {
                return Err(fail(format!("server.args: '{arg}' names a folder in the project: caches go outside it (use {{cache}})")));
            }
        }
        let allowed: BTreeSet<&str> =
            LAUNCH_PLACEHOLDERS.into_iter().filter(|n| *n != "tool_version" || known.contains("tool_version")).collect();
        let launches = self
            .provides
            .as_ref()
            .map(|p| ("provides.launch".to_owned(), &p.launch))
            .into_iter()
            .chain(self.install.iter().enumerate().filter_map(|(i, e)| e.launch.as_ref().map(|l| (format!("install[{i}].launch"), l))));
        for (at, launch) in launches {
            if self.provides.is_none() {
                return Err(fail(format!("{at}: only harness packs are launched")));
            }
            for text in std::iter::once(&launch.program).chain(&launch.args).chain(launch.env.values()) {
                template::check(text, &allowed).map_err(|e| fail(format!("{at}: {e}")))?;
            }
        }
        if let Some(language) = &self.language {
            for file in std::iter::once(&language.syntax).chain(&language.highlights) {
                if !files.contains(file) {
                    return Err(fail(format!("language: no file '{file}' in the pack")));
                }
            }
        }
        Ok(())
    }
}

/// A relative path that, `..` taken back, stays inside the folder it's
/// relative to.
fn resolved_inside(path: &std::path::Path) -> Result<(), String> {
    let mut depth: i32 = 0;
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => depth -= 1,
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir => {}
            _ => return Err(format!("'{}' must stay inside the install folder", path.display())),
        }
        if depth < 0 {
            return Err(format!("'{}' leads out of the install folder", path.display()));
        }
    }
    Ok(())
}

/// A path inside the install folder: relative, never going up.
fn inside(path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(format!("'{path}' must stay inside the install folder"));
    }
    Ok(())
}
