//! Staging and unstaging: the user's own act, from the cockpit (never an
//! agent's): a whole file, or one change in it. Only the index is written;
//! the files and history are as they were.

use git2::{ApplyLocation, ApplyOptions, Repository};

use crate::{Changes, ChangesError, Result};

/// One change in a file, as the Changes view showed it: where it starts on
/// each side, and how many unchanged lines it was shown with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HunkAt {
    pub old_start: u32,
    pub new_start: u32,
    pub context: u32,
}

impl Changes {
    /// Stages `path` (project-relative), or one of its unstaged changes.
    pub fn stage(&self, path: &str, hunk: Option<HunkAt>) -> Result<()> {
        let repo = self.repo()?;
        let full = format!("{}{path}", self.prefix);
        match hunk {
            None => {
                let mut index = repo.index()?;
                index.update_all([full.as_str()], None)?;
                if self.root.join(path).exists() {
                    index.add_all([full.as_str()], git2::IndexAddOption::DEFAULT, None)?;
                }
                index.write()?;
                Ok(())
            }
            Some(at) => {
                let mut o = git2::DiffOptions::new();
                o.context_lines(at.context)
                    .pathspec(&full)
                    .disable_pathspec_match(true)
                    .include_untracked(true)
                    .recurse_untracked_dirs(true)
                    .show_untracked_content(true);
                let diff = repo.diff_index_to_workdir(None, Some(&mut o))?;
                apply_one(&repo, &diff, (at.old_start, at.new_start), path)
            }
        }
    }

    /// Unstages `path` (project-relative), or one of its staged changes:
    /// the index gets back what the last commit has there.
    pub fn unstage(&self, path: &str, hunk: Option<HunkAt>) -> Result<()> {
        let repo = self.repo()?;
        let full = format!("{}{path}", self.prefix);
        let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        match hunk {
            None => {
                match &head {
                    Some(commit) => repo.reset_default(Some(commit.as_object()), [full.as_str()])?,
                    None => {
                        let mut index = repo.index()?;
                        index.remove_path(std::path::Path::new(&full))?;
                        index.write()?;
                    }
                }
                Ok(())
            }
            Some(at) => {
                let tree = head.as_ref().map(|c| c.tree()).transpose()?;
                let mut o = git2::DiffOptions::new();
                o.context_lines(at.context).pathspec(&full).disable_pathspec_match(true).reverse(true);
                let diff = repo.diff_tree_to_index(tree.as_ref(), None, Some(&mut o))?;
                // Reversed: the index's side is the old one now.
                apply_one(&repo, &diff, (at.new_start, at.old_start), path)
            }
        }
    }
}

/// Applies to the index only the hunk of `diff` that starts at `starts`
/// (old, new); none there means the file changed since it was shown.
fn apply_one(repo: &Repository, diff: &git2::Diff<'_>, starts: (u32, u32), path: &str) -> Result<()> {
    let found = std::cell::Cell::new(false);
    {
        let mut options = ApplyOptions::new();
        options.hunk_callback(|h| {
            let this = h.is_some_and(|h| (h.old_start(), h.new_start()) == starts);
            found.set(found.get() | this);
            this
        });
        repo.apply(diff, ApplyLocation::Index, Some(&mut options))?;
    }
    if found.get() { Ok(()) } else { Err(ChangesError::BadPoint(format!("{path} changed since it was shown: read it again"))) }
}
