//! Snapshots: the project's files as they are at a moment, kept in git as
//! a commit of their own under `refs/nah/snapshots/<id>` (so git's cleanup
//! keeps them until they're pruned).
//!
//! Taken the way `git stash create` does: the repository's index is read
//! into this handle's memory, every file is added to that copy (ignored
//! ones left out; what didn't change since it was staged isn't read again)
//! and its tree written. The copy is never saved: the user's index,
//! branch, history and files are as they were. Content git already has
//! isn't stored again.

use git2::{IndexAddOption, Repository, Tree};

use crate::{Changes, ChangesError, Result};

/// Where snapshots are kept.
pub const SNAPSHOT_REFS: &str = "refs/nah/snapshots/";

/// A snapshot: its id (`snapshot:<id>` as a point) and when it was taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub id: String,
    pub at_ms: u64,
}

pub(crate) fn tree_of<'r>(repo: &'r Repository, id: &str) -> Result<Tree<'r>> {
    let reference = repo.find_reference(&format!("{SNAPSHOT_REFS}{id}")).map_err(|_| ChangesError::BadPoint(format!("snapshot:{id} is gone (pruned)")))?;
    Ok(reference.peel_to_tree()?)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Changes {
    /// The project's files as they are now, kept as a snapshot (`why` says
    /// what it was taken for); when they're as snapshot `last` has them,
    /// that one (and `false`: nothing new was kept).
    pub fn snapshot(&self, why: &str, last: Option<&Snapshot>) -> Result<(Snapshot, bool)> {
        let repo = self.repo()?;
        let mut index = repo.index()?;
        // The project's folder (the rest as the index has it).
        let within = if self.prefix.is_empty() { "*".to_owned() } else { format!("{}*", self.prefix) };
        index.add_all([within.as_str()], IndexAddOption::DEFAULT, None)?;
        index.update_all([within.as_str()], None)?;
        // Its tree only; the copy itself is never written back.
        let tree = repo.find_tree(index.write_tree()?)?;
        if let Some(last) = last
            && tree_of(&repo, &last.id).is_ok_and(|t| t.id() == tree.id())
        {
            return Ok((last.clone(), false));
        }
        let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let who = git2::Signature::now("NAH", "nah@localhost")?;
        let commit = repo.commit(None, &who, &who, &format!("NAH snapshot: {why}"), &tree, &head.iter().collect::<Vec<_>>())?;
        // Later than every one before it, even in the same millisecond:
        // they're told apart, and listed, in the order they were taken.
        let at_ms = now_ms().max(self.snapshots()?.last().map_or(0, |s| s.at_ms + 1));
        let id = format!("{at_ms}-{}", &commit.to_string()[..10]);
        repo.reference(&format!("{SNAPSHOT_REFS}{id}"), commit, true, "NAH snapshot")?;
        Ok((Snapshot { id, at_ms }, true))
    }

    /// File `path` (from the project's folder) as snapshot `id` has it
    /// (none: it had no such file).
    pub fn text_at(&self, id: &str, path: &str) -> Result<Option<String>> {
        let repo = self.repo()?;
        let tree = tree_of(&repo, id)?;
        let Ok(entry) = tree.get_path(std::path::Path::new(&format!("{}{path}", self.prefix))) else { return Ok(None) };
        let blob = repo.find_blob(entry.id())?;
        Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
    }

    /// Every snapshot kept, oldest first.
    pub fn snapshots(&self) -> Result<Vec<Snapshot>> {
        let repo = self.repo()?;
        let mut out: Vec<Snapshot> = repo
            .references_glob(&format!("{SNAPSHOT_REFS}*"))?
            .filter_map(|r| r.ok())
            .filter_map(|r| {
                let id = r.name().ok()?.strip_prefix(SNAPSHOT_REFS)?.to_owned();
                let at_ms = id.split_once('-').and_then(|(ms, _)| ms.parse().ok())?;
                Some(Snapshot { id, at_ms })
            })
            .collect();
        out.sort_by_key(|s| s.at_ms);
        Ok(out)
    }

    /// The snapshots taken for `why` (as `snapshot` was told), oldest first.
    pub fn snapshots_for(&self, why: &str) -> Result<Vec<Snapshot>> {
        let repo = self.repo()?;
        let said = format!("NAH snapshot: {why}");
        Ok(self.snapshots()?.into_iter().filter(|s| repo.find_reference(&format!("{SNAPSHOT_REFS}{}", s.id)).ok().and_then(|r| r.peel_to_commit().ok()).is_some_and(|c| c.message().ok() == Some(said.as_str()))).collect())
    }

    /// Lets snapshot `id` go (git's cleanup takes what nothing else keeps).
    pub fn drop_snapshot(&self, id: &str) -> Result<()> {
        let repo = self.repo()?;
        match repo.find_reference(&format!("{SNAPSHOT_REFS}{id}")) {
            Ok(mut r) => Ok(r.delete()?),
            Err(_) => Ok(()),
        }
    }
}
