//! The project's changes, from git. Reads the user's repository (its
//! commits, its index, its files) and never changes them; the only things
//! it writes are its own snapshots (`refs/nah/snapshots/…`).
//!
//! - [`Changes::list`]: the files a [`Comparison`] changes, with their
//!   counts (no content), a page at a time. The uncommitted changes come
//!   kept apart as git status does: staged, unstaged, untracked.
//! - [`Changes::file`]: one file's changes, asked for when it's opened;
//!   kept until the file or the comparison's sides change.
//! - [`Changes::snapshot`]: the project's files as they are, written as a
//!   commit of their own (`timeline`).

mod compare;
mod timeline;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use harness_contracts::changes::{ChangeList, Comparison, FileChanges, Group};

pub use timeline::{SNAPSHOT_REFS, Snapshot};

#[derive(Debug, thiserror::Error)]
pub enum ChangesError {
    #[error("{0} isn't in a git repository")]
    NotARepo(String),
    #[error("{0}")]
    BadPoint(String),
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

/// One file's changes, by what was asked and what its sides were then.
type Key = (String, String, Group, u32, String);

/// The project's changes: a repository, and the files' changes it's been
/// asked for.
pub struct Changes {
    root: PathBuf,
    kept: Mutex<HashMap<Key, FileChanges>>,
}

impl Changes {
    /// The repository `root` is in (or under).
    pub fn open(root: &Path) -> Result<Self> {
        let repo = git2::Repository::discover(root).map_err(|_| ChangesError::NotARepo(root.display().to_string()))?;
        let root = repo.workdir().map(Path::to_path_buf).ok_or_else(|| ChangesError::NotARepo(format!("{} (a bare repository)", root.display())))?;
        Ok(Self { root, kept: Mutex::default() })
    }

    /// Its working folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A fresh handle: git2's isn't shared across threads, and opening one
    /// is cheap.
    fn repo(&self) -> Result<git2::Repository> {
        Ok(git2::Repository::open(&self.root)?)
    }

    /// The files `comparison` changes, in group then path order, `limit`
    /// of them from `offset`.
    pub fn list(&self, comparison: &Comparison, offset: usize, limit: usize) -> Result<ChangeList> {
        let repo = self.repo()?;
        let mut files = compare::files(&repo, comparison)?;
        files.sort_by(|a, b| (a.group, &a.path).cmp(&(b.group, &b.path)));
        let total = files.len();
        let files = files.into_iter().skip(offset).take(limit).collect();
        Ok(ChangeList { comparison: comparison.clone(), files, offset, total })
    }

    /// Every path `comparison` changes (renames' old paths too), for
    /// filters ("only changed files").
    pub fn changed_paths(&self, comparison: &Comparison) -> Result<Vec<String>> {
        let repo = self.repo()?;
        let mut paths: Vec<String> = compare::files(&repo, comparison)?.into_iter().flat_map(|f| std::iter::once(f.path).chain(f.old_path)).collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// `path`'s changes in `comparison` (its `group` of them, for the
    /// uncommitted ones), with `context` unchanged lines around each.
    pub fn file(&self, comparison: &Comparison, path: &str, group: Group, context: u32) -> Result<FileChanges> {
        let repo = self.repo()?;
        let key = (comparison.to_string(), path.to_owned(), group, context, compare::fingerprint(&repo, comparison, path)?);
        if let Some(kept) = self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
            return Ok(kept.clone());
        }
        let changes = compare::file(&repo, comparison, path, group, context)?;
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
