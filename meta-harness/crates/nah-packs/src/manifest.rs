//! A pack's manifest (`pack.toml`): what it is, the external tool it manages
//! and the versions of that tool it supports, how to install it on each
//! platform and how to check it, and, by kind, how to run it (an indexer),
//! what it provides (a harness) or the files describing a language.
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
    /// Indexers: how to make their output.
    #[serde(default)]
    pub run: Option<RunContract>,
    /// Harnesses: the program the harness driver starts.
    #[serde(default)]
    pub provides: Option<Provides>,
    /// Languages: the files describing the language.
    #[serde(default)]
    pub language: Option<LanguageFiles>,
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
    /// A language: its syntax rules, highlight queries and indexer.
    Language,
    /// A program that writes a language's SCIP index.
    Indexer,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Harness => "harness",
            Kind::Language => "language",
            Kind::Indexer => "indexer",
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

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// A pattern over its output; the first group is the version.
    pub version: String,
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
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// Fetch a file, check its checksum, and unpack or place it.
    Download(Download),
    /// Run a program (a package manager, say), without a shell.
    Run(Command),
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
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unpack {
    #[default]
    None,
    Zip,
    TarGz,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// How an indexer is run: `{root}` is the project, `{out}` where it writes.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunContract {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub output: Output,
    pub timeout_s: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Output {
    Scip,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provides {
    /// The program, relative to the install folder.
    pub executable: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageFiles {
    /// The syntax rules (a file of the pack).
    pub syntax: String,
    /// tree-sitter highlight queries (a file of the pack).
    #[serde(default)]
    pub highlights: Option<String>,
    /// The indexer pack that knows this language exactly.
    #[serde(default)]
    pub indexer: Option<String>,
}

/// Placeholders a run contract may use.
pub const RUN_PLACEHOLDERS: [&str; 4] = ["root", "out", "dest", "tool_version"];
/// Placeholders a check may use.
pub const CHECK_PLACEHOLDERS: [&str; 2] = ["dest", "tool_version"];

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
        let (tool, install, run, provides, language) = match p.kind {
            Kind::Harness => (true, true, false, true, false),
            Kind::Indexer => (true, true, true, false, false),
            Kind::Language => (false, false, false, false, true),
        };
        has(self.tool.is_some(), "tool", tool)?;
        has(!self.install.is_empty(), "install", install)?;
        has(self.run.is_some(), "run", run)?;
        has(self.provides.is_some(), "provides", provides)?;
        has(self.language.is_some(), "language", language)?;

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
            if let Some(check) = &tool.check {
                let re = regex::Regex::new(&check.version).map_err(|e| fail(format!("tool.check.version: {e}")))?;
                if re.captures_len() < 2 {
                    return Err(fail("tool.check.version needs a group for the version".into()));
                }
                let allowed: BTreeSet<&str> = CHECK_PLACEHOLDERS.into_iter().filter(|n| *n != "tool_version" || tool.version.is_some()).collect();
                for text in std::iter::once(&check.command).chain(&check.args) {
                    template::check(text, &allowed).map_err(|e| fail(format!("tool.check: {e}")))?;
                }
            }
            // An indexer is checked before it's run: it needs a check.
            if p.kind == Kind::Indexer && tool.check.is_none() {
                return Err(fail("an indexer pack needs [tool.check]".into()));
            }
        }

        let mut covered: BTreeSet<&str> = BTreeSet::new();
        for (i, install) in self.install.iter().enumerate() {
            let at = format!("install[{i}]");
            if install.platforms.is_empty() || install.steps.is_empty() {
                return Err(fail(format!("{at}: needs platforms and steps")));
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
                            for text in [&d.url, &d.into].into_iter().chain(d.save_as.as_ref()) {
                                template::expand(text, &values).map_err(|e| fail(format!("{at}: {e}")))?;
                            }
                            let sum = template::expand(&d.sha256, &values).map_err(|e| fail(format!("{at}: {e}")))?;
                            if sum.len() != 64 || !sum.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
                                return Err(fail(format!("{at}: sha256 must be 64 lowercase hex digits (got '{sum}')")));
                            }
                            match (d.unpack, &d.save_as) {
                                (Unpack::None, None) => return Err(fail(format!("{at}: a download that isn't unpacked needs save_as"))),
                                (Unpack::Zip | Unpack::TarGz, Some(_)) => return Err(fail(format!("{at}: save_as is for downloads that aren't unpacked"))),
                                _ => {}
                            }
                        }
                        Step::Run(c) => {
                            for text in std::iter::once(&c.command).chain(&c.args) {
                                template::expand(text, &values).map_err(|e| fail(format!("{at}: {e}")))?;
                            }
                        }
                    }
                }
            }
        }

        if let Some(run) = &self.run {
            if run.timeout_s == 0 {
                return Err(fail("run.timeout_s must be more than 0".into()));
            }
            let allowed: BTreeSet<&str> = RUN_PLACEHOLDERS.into_iter().filter(|n| *n != "tool_version" || known.contains("tool_version")).collect();
            for text in std::iter::once(&run.command).chain(&run.args) {
                template::check(text, &allowed).map_err(|e| fail(format!("run: {e}")))?;
            }
            for needed in ["root", "out"] {
                if !run.args.iter().chain(std::iter::once(&run.command)).any(|a| template::names(a).contains(&needed)) {
                    return Err(fail(format!("run: the arguments must say where {{{needed}}} goes")));
                }
            }
        }
        if let Some(provides) = &self.provides {
            template::check(&provides.executable, &BTreeSet::new()).map_err(|e| fail(format!("provides.executable: {e}")))?;
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
