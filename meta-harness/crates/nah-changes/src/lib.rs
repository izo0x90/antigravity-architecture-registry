//! The project's changes, from git. Reads the user's repository (its
//! commits, its index, its files) and never changes them on its own: it
//! writes its own snapshots (`refs/nah/snapshots/…`), and the index only
//! when the user stages or unstages (`staging`).
//!
//! - [`Changes::list`]: the files a [`Comparison`] changes, with their
//!   counts (no content), a page at a time. The uncommitted changes come
//!   kept apart as git status does: staged, unstaged, untracked.
//! - [`Changes::file`]: one file's changes, asked for when it's opened;
//!   kept until the file or the comparison's sides change.
//! - [`Changes::snapshot`]: the project's files as they are, written as a
//!   commit of their own (`timeline`).

mod compare;
mod staging;
mod timeline;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use harness_contracts::changes::{ChangeList, Comparison, FileChanges, Group, Point};

pub use staging::HunkAt;
pub use timeline::{SNAPSHOT_REFS, Snapshot};

#[derive(Debug, thiserror::Error)]
pub enum ChangesError {
    #[error("{0} isn't in a git repository")]
    NotARepo(String),
    #[error("{0}")]
    BadPoint(String),
    /// More asked of at once than is taken.
    #[error("{0}")]
    TooBig(String),
    #[error("git: {0}")]
    Git(String),
}

impl From<git2::Error> for ChangesError {
    fn from(e: git2::Error) -> Self {
        Self::Git(e.message().to_owned())
    }
}

pub type Result<T> = std::result::Result<T, ChangesError>;

/// How many files a page lists, unless asked.
pub const PAGE: usize = 500;

/// How many unchanged lines show around a change, unless asked.
pub const CONTEXT: u32 = 3;

/// How many files' changes are kept.
const KEPT: usize = 256;

/// A run of changed lines on a file's newer side: `lines` from `start`
/// added or changed, `removed` lines gone there (with none added, they were
/// after line `start`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: u32,
    pub lines: u32,
    pub removed: u32,
}

impl Run {
    /// The line it's at: its first, or (only removed lines) the one before
    /// them (the first, at the top).
    pub fn at(&self) -> u32 {
        self.start.max(1)
    }

    /// Whether it touches lines `first..=last`.
    pub fn within(&self, first: u32, last: u32) -> bool {
        let end = self.start + self.lines.max(1) - 1;
        self.at() <= last && end.max(self.at()) >= first
    }

    /// How many of its lines are in `first..=last`.
    pub fn lines_in(&self, first: u32, last: u32) -> u32 {
        let (a, b) = (self.start.max(first), (self.start + self.lines).min(last + 1));
        b.saturating_sub(a)
    }
}

/// A changed file's runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRuns {
    pub path: String,
    pub runs: Vec<Run>,
}

/// Where a comparison changed the files as they are now, for matching to
/// what's at those lines: each changed file's runs, and the files whose
/// lines moved since the comparison's newer side (left out: their runs
/// would point at the wrong lines).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OnFiles {
    pub files: Vec<FileRuns>,
    pub moved: Vec<String>,
}

/// The changes from `old` to `new`, two texts (a call's change as its
/// harness gave it), made as a file's changes are, with `context`
/// unchanged lines around each.
pub fn texts(old: &str, new: &str, context: u32) -> Result<Vec<harness_contracts::changes::Hunk>> {
    compare::texts(old, new, context)
}

/// One file's changes, by what was asked and what its sides were then.
type Key = (String, String, Group, u32, String);

/// The project's changes: a repository, and the files' changes it's been
/// asked for.
pub struct Changes {
    /// The repository's working folder, and the project's folder in it
    /// (`dir/`, or empty when they're the same): paths in and out are the
    /// project's, and only its folder's changes are listed.
    workdir: PathBuf,
    root: PathBuf,
    prefix: String,
    kept: Mutex<HashMap<Key, FileChanges>>,
}

impl Changes {
    /// The repository the project's folder `project` is in (or is).
    pub fn open(project: &Path) -> Result<Self> {
        let repo = git2::Repository::discover(project).map_err(|_| ChangesError::NotARepo(project.display().to_string()))?;
        let workdir = repo
            .workdir()
            .map(Path::to_path_buf)
            .ok_or_else(|| ChangesError::NotARepo(format!("{} (a bare repository)", project.display())))?;
        let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let inside = canonical(project).strip_prefix(canonical(&workdir)).map(Path::to_path_buf).unwrap_or_default();
        let prefix = inside.components().map(|c| format!("{}/", c.as_os_str().to_string_lossy())).collect();
        Ok(Self { workdir, root: project.to_path_buf(), prefix, kept: Mutex::default() })
    }

    /// The project's folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A fresh handle: git2's isn't shared across threads, and opening one
    /// is cheap.
    fn repo(&self) -> Result<git2::Repository> {
        Ok(git2::Repository::open(&self.workdir)?)
    }

    /// The files `comparison` changes, in group then path order, `limit`
    /// of them from `offset`.
    pub fn list(&self, comparison: &Comparison, offset: usize, limit: usize) -> Result<ChangeList> {
        let repo = self.repo()?;
        let mut files = compare::files(&repo, comparison, &self.prefix)?;
        files.sort_by(|a, b| (a.group, &a.path).cmp(&(b.group, &b.path)));
        let total = files.len();
        let files = files.into_iter().skip(offset).take(limit).collect();
        Ok(ChangeList { comparison: comparison.clone(), files, offset, total })
    }

    /// Every path `comparison` changes (renames' old paths too), for
    /// filters ("only changed files").
    pub fn changed_paths(&self, comparison: &Comparison) -> Result<Vec<String>> {
        let repo = self.repo()?;
        let mut paths: Vec<String> =
            compare::files(&repo, comparison, &self.prefix)?.into_iter().flat_map(|f| std::iter::once(f.path).chain(f.old_path)).collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// Where `comparison` changed the files as they are now. Up to the
    /// files, it's the comparison itself (staged and unstaged together);
    /// otherwise its newer side's runs, less the files changed since.
    pub fn on_files(&self, comparison: &Comparison) -> Result<OnFiles> {
        let repo = self.repo()?;
        if comparison.to == Point::Files {
            return Ok(OnFiles { files: compare::runs(&repo, &comparison.on_files(), &self.prefix)?, moved: Vec::new() });
        }
        let moved = self.changed_paths(&Comparison { from: comparison.to.clone(), to: Point::Files })?;
        let files = compare::runs(&repo, comparison, &self.prefix)?.into_iter().filter(|f| !moved.contains(&f.path)).collect();
        Ok(OnFiles { files, moved })
    }

    /// `path`'s changes in `comparison` (its `group` of them, for the
    /// uncommitted ones), with `context` unchanged lines around each.
    pub fn file(&self, comparison: &Comparison, path: &str, group: Group, context: u32) -> Result<FileChanges> {
        let repo = self.repo()?;
        let key = (comparison.to_string(), path.to_owned(), group, context, compare::fingerprint(&repo, comparison, path, &self.prefix)?);
        if let Some(kept) = self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
            return Ok(kept.clone());
        }
        let changes = compare::file(&repo, comparison, path, group, context, &self.prefix)?;
        let mut kept = self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if kept.len() >= KEPT {
            kept.clear();
        }
        kept.insert(key, changes.clone());
        Ok(changes)
    }
}

#[cfg(test)]
mod tests;
