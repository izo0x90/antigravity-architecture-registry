//! Where packs come from and what's available. `index.toml` (shipped with
//! the distribution) lists the sources in order, the distribution's own
//! packs first, then folders; a later source replaces a pack of the same
//! name. It also says where packs are installed and where the lock is. A
//! pack is a folder holding a `pack.toml` and its other files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::manifest::{Kind, Manifest};
use crate::platform::Platforms;
use crate::{PackError, template};

/// The files shipped with this build (paths relative to its packs folder).
#[derive(Clone, Debug, Default)]
pub struct Distribution {
    files: BTreeMap<String, Vec<u8>>,
}

impl Distribution {
    pub fn new(files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self {
        Self { files: files.into_iter().collect() }
    }

    /// A packs folder on disk, as a distribution (for tests).
    #[cfg(any(test, feature = "testing"))]
    pub fn from_dir(dir: &Path) -> Result<Self, PackError> {
        Ok(Self { files: read_tree(dir)? })
    }

    fn text(&self, name: &str) -> Result<&str, PackError> {
        let bytes = self.files.get(name).ok_or_else(|| PackError::Config(format!("the distribution has no {name}")))?;
        std::str::from_utf8(bytes).map_err(|e| PackError::Config(format!("{name}: {e}")))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    source: Vec<SourceSpec>,
    install: InstallSpec,
    harnesses: HarnessesSpec,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HarnessesSpec {
    overrides: Vec<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SourceSpec {
    /// The packs shipped with this build.
    Distribution,
    /// A folder of packs (missing is fine).
    Folder { path: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallSpec {
    root: String,
    lock: String,
    download_timeout_s: u64,
    run_timeout_s: u64,
    check_timeout_s: u64,
}

/// How long install steps may take, unless a step says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeouts {
    pub download: std::time::Duration,
    pub run: std::time::Duration,
    /// A tool's version check (its command), stopped past this.
    pub check: std::time::Duration,
}

/// The folders source and install paths may name: `{project}`, `{home}`,
/// `{config_home}`, `{data_home}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Places {
    pub project: PathBuf,
    pub home: PathBuf,
    pub config_home: PathBuf,
    pub data_home: PathBuf,
}

impl Places {
    /// The user's own folders (as the platform names them), for `project`.
    pub fn of_user(project: &Path) -> Result<Self, PackError> {
        let need = |dir: Option<PathBuf>, what: &str| dir.ok_or_else(|| PackError::Config(format!("can't tell the user's {what} folder")));
        Ok(Self {
            project: project.to_path_buf(),
            home: need(dirs::home_dir(), "home")?,
            config_home: need(dirs::config_dir(), "config")?,
            data_home: need(dirs::data_dir(), "data")?,
        })
    }

    /// Every folder under `root` (which needn't exist): sees no project's
    /// or user's own packs, for runs that mustn't depend on the machine.
    #[cfg(any(test, feature = "testing"))]
    pub fn isolated(root: &Path) -> Self {
        Self { project: root.join("project"), home: root.join("home"), config_home: root.join("config"), data_home: root.join("data") }
    }

    fn path(&self, template: &str) -> Result<PathBuf, PackError> {
        let values: BTreeMap<&str, &str> = [
            ("project", self.project.to_str()),
            ("home", self.home.to_str()),
            ("config_home", self.config_home.to_str()),
            ("data_home", self.data_home.to_str()),
        ]
        .into_iter()
        .map(|(k, v)| (k, v.unwrap_or_default()))
        .collect();
        template::expand(template, &values).map(PathBuf::from).map_err(|e| PackError::Config(format!("index.toml: {e}")))
    }
}

/// One loaded, checked pack.
#[derive(Clone, Debug)]
pub struct Pack {
    pub manifest: Manifest,
    /// Its other files, by path within the pack.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Where it came from (for messages).
    pub origin: String,
}

impl Pack {
    pub fn name(&self) -> &str {
        &self.manifest.pack.name
    }

    pub fn kind(&self) -> Kind {
        self.manifest.pack.kind
    }

    /// One of its files as text.
    pub fn text(&self, file: &str) -> Result<&str, PackError> {
        let bytes = self
            .files
            .get(file)
            .ok_or_else(|| PackError::Manifest { pack: self.name().to_owned(), reason: format!("no file '{file}'") })?;
        std::str::from_utf8(bytes).map_err(|e| PackError::Manifest { pack: self.name().to_owned(), reason: format!("{file}: {e}") })
    }
}

/// Every available pack, the platforms, and where installs go.
#[derive(Clone, Debug)]
pub struct Catalog {
    pub platforms: Platforms,
    packs: BTreeMap<String, Pack>,
    /// Packs are installed under `<root>/<name>/<version>`.
    pub root: PathBuf,
    pub lock_path: PathBuf,
    pub timeouts: Timeouts,
    /// Files with instances' own launches, in order (a later one wins).
    pub harness_overrides: Vec<PathBuf>,
    /// The project it was loaded for.
    pub project: PathBuf,
}

impl Catalog {
    /// Reads the index and platforms from the distribution, then every
    /// source in order.
    pub fn load(distribution: &Distribution, places: &Places) -> Result<Self, PackError> {
        let index: Index = toml::from_str(distribution.text("index.toml")?).map_err(|e| PackError::Config(format!("index.toml: {e}")))?;
        let platforms = Platforms::parse(distribution.text("platforms.toml")?)?;
        let mut packs: BTreeMap<String, Pack> = BTreeMap::new();
        for source in &index.source {
            let (files, origin) = match source {
                SourceSpec::Distribution => (distribution.files.clone(), "the distribution".to_owned()),
                SourceSpec::Folder { path } => {
                    let dir = places.path(path)?;
                    if !dir.is_dir() {
                        continue;
                    }
                    let files = read_tree(&dir)?;
                    let origin = dir.display().to_string();
                    // Everything in a folder of packs belongs to one: a
                    // file beside them is a mistake, not something to skip.
                    if let Some(stray) = files.keys().find(|f| !in_a_pack(&files, f)) {
                        return Err(PackError::Config(format!("{origin}/{stray} isn't in a pack: a pack is a folder holding {MANIFEST}")));
                    }
                    (files, origin)
                }
            };
            let mut here = BTreeSet::new();
            for pack in packs_in(&files, &origin, &platforms)? {
                if !here.insert(pack.name().to_owned()) {
                    return Err(PackError::Config(format!("{origin} has two packs named '{}'", pack.name())));
                }
                packs.insert(pack.name().to_owned(), pack);
            }
        }
        // A language names its server: that pack must exist and be one.
        for pack in packs.values() {
            if let Some(server) = pack.manifest.language.as_ref().and_then(|l| l.server.as_ref())
                && packs.get(server).map(Pack::kind) != Some(Kind::Server)
            {
                return Err(PackError::Manifest {
                    pack: pack.name().to_owned(),
                    reason: format!("its server '{server}' isn't a server pack"),
                });
            }
        }
        let secs = std::time::Duration::from_secs;
        let timeouts = Timeouts {
            download: secs(index.install.download_timeout_s),
            run: secs(index.install.run_timeout_s),
            check: secs(index.install.check_timeout_s),
        };
        let harness_overrides = index.harnesses.overrides.iter().map(|p| places.path(p)).collect::<Result<_, _>>()?;
        Ok(Self {
            platforms,
            packs,
            root: places.path(&index.install.root)?,
            lock_path: places.path(&index.install.lock)?,
            timeouts,
            harness_overrides,
            project: places.project.clone(),
        })
    }

    pub fn get(&self, name: &str) -> Result<&Pack, PackError> {
        self.packs.get(name).ok_or_else(|| PackError::NotFound(name.to_owned()))
    }

    pub fn packs(&self) -> impl Iterator<Item = &Pack> {
        self.packs.values()
    }

    pub fn of_kind(&self, kind: Kind) -> impl Iterator<Item = &Pack> {
        self.packs.values().filter(move |p| p.kind() == kind)
    }
}

/// Every file under `dir`, by path relative to it (with `/`).
fn read_tree(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>, PackError> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        let entries = std::fs::read_dir(&at).map_err(|e| PackError::Config(format!("{}: {e}", at.display())))?;
        for entry in entries {
            let path = entry.map_err(|e| PackError::Config(format!("{}: {e}", at.display())))?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(dir)
                    .unwrap_or(&path)
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                let bytes = std::fs::read(&path).map_err(|e| PackError::Config(format!("{}: {e}", path.display())))?;
                out.insert(relative, bytes);
            }
        }
    }
    Ok(out)
}

/// The file that makes a folder a pack.
const MANIFEST: &str = "pack.toml";

/// Whether `file` is inside a folder (of `files`) holding a pack manifest.
fn in_a_pack(files: &BTreeMap<String, Vec<u8>>, file: &str) -> bool {
    let mut folder = file;
    while let Some((up, _)) = folder.rsplit_once('/') {
        if files.contains_key(&format!("{up}/{MANIFEST}")) {
            return true;
        }
        folder = up;
    }
    files.contains_key(MANIFEST)
}

/// The packs among `files`: each folder holding a `pack.toml`, with the
/// files under it. Packs don't nest.
fn packs_in(files: &BTreeMap<String, Vec<u8>>, origin: &str, platforms: &Platforms) -> Result<Vec<Pack>, PackError> {
    let folders: Vec<String> =
        files.keys().filter_map(|f| f.strip_suffix(MANIFEST)).filter(|d| d.is_empty() || d.ends_with('/')).map(str::to_owned).collect();
    for a in &folders {
        if let Some(b) = folders.iter().find(|b| *b != a && b.starts_with(a.as_str())) {
            return Err(PackError::Config(format!("{origin}: pack '{b}' is inside pack '{a}'")));
        }
    }
    let mut out = Vec::new();
    for folder in folders {
        let manifest_path = format!("{folder}{MANIFEST}");
        let text = std::str::from_utf8(&files[&manifest_path]).map_err(|e| PackError::Config(format!("{origin}/{manifest_path}: {e}")))?;
        let manifest = Manifest::parse(text).map_err(|reason| PackError::Manifest { pack: format!("{origin}/{manifest_path}"), reason })?;
        let own: BTreeMap<String, Vec<u8>> = files
            .iter()
            .filter_map(|(path, bytes)| {
                path.strip_prefix(folder.as_str()).filter(|p| *p != MANIFEST).map(|p| (p.to_owned(), bytes.clone()))
            })
            .collect();
        manifest.validate(platforms, &own.keys().cloned().collect())?;
        out.push(Pack { manifest, files: own, origin: format!("{origin}/{folder}") });
    }
    Ok(out)
}
