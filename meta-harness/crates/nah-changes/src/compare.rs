//! A comparison as git diffs: each side resolved (a commit's tree, the
//! index, the files), the pair diffed the way git does, and read into the
//! wire types.

use git2::{Delta, Diff, DiffFindOptions, DiffOptions, Patch, Repository, Tree};
use harness_contracts::changes::{ChangedFile, Comparison, DiffLine, FileChanges, FileStatus, Group, Hunk, LineKind, Point};

use crate::{ChangesError, FileRuns, Result, Run};

/// A side of a comparison, resolved.
enum Side<'r> {
    /// A commit's files (none: before the first commit).
    Tree(Option<Tree<'r>>),
    Index,
    Files,
}

impl Side<'_> {
    /// Older to newer: commits, then the index, then the files.
    fn rank(&self) -> u8 {
        match self {
            Self::Tree(_) => 0,
            Self::Index => 1,
            Self::Files => 2,
        }
    }
}

fn head_tree(repo: &Repository) -> Result<Option<Tree<'_>>> {
    match repo.head() {
        Ok(head) => Ok(Some(head.peel_to_tree()?)),
        Err(e) if matches!(e.code(), git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn side<'r>(repo: &'r Repository, point: &Point) -> Result<Side<'r>> {
    Ok(match point {
        Point::Head => Side::Tree(head_tree(repo)?),
        Point::Index => Side::Index,
        Point::Files => Side::Files,
        Point::Rev(rev) => Side::Tree(Some(
            repo.revparse_single(rev).map_err(|e| ChangesError::BadPoint(format!("rev:{rev}: {}", e.message())))?.peel_to_tree()?,
        )),
        Point::Snapshot(id) => Side::Tree(Some(crate::timeline::tree_of(repo, id)?)),
    })
}

/// Options for every diff: `context` lines around changes, only `paths`
/// (all when empty), untracked files' content when the files are a side.
fn options(context: u32, paths: &[&str], files: bool) -> DiffOptions {
    let mut o = DiffOptions::new();
    o.context_lines(context).ignore_submodules(true);
    if files {
        o.include_untracked(true).recurse_untracked_dirs(true).show_untracked_content(true);
    }
    for p in paths {
        o.pathspec(*p).disable_pathspec_match(true);
    }
    o
}

/// The diffs `comparison` is made of, each with its group: the uncommitted
/// changes as staged and unstaged (untracked among them); anything else as
/// one, the older side first whichever way it was asked.
fn diffs<'r>(repo: &'r Repository, comparison: &Comparison, context: u32, paths: &[&str]) -> Result<Vec<(Group, Diff<'r>)>> {
    let found = |mut diff: Diff<'r>| -> Result<Diff<'r>> {
        diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;
        Ok(diff)
    };
    if comparison.is_uncommitted() {
        let head = head_tree(repo)?;
        let index = repo.index()?;
        let staged = repo.diff_tree_to_index(head.as_ref(), Some(&index), Some(&mut options(context, paths, false)))?;
        let unstaged = repo.diff_index_to_workdir(Some(&index), Some(&mut options(context, paths, true)))?;
        return Ok(vec![(Group::Staged, found(staged)?), (Group::Unstaged, found(unstaged)?)]);
    }
    let (from, to) = (side(repo, &comparison.from)?, side(repo, &comparison.to)?);
    let reverse = from.rank() > to.rank();
    let (older, newer) = if reverse { (to, from) } else { (from, to) };
    let files = matches!(newer, Side::Files);
    let mut o = options(context, paths, files);
    o.reverse(reverse);
    let diff = match (&older, &newer) {
        (Side::Tree(a), Side::Tree(b)) => repo.diff_tree_to_tree(a.as_ref(), b.as_ref(), Some(&mut o))?,
        (Side::Tree(a), Side::Index) => repo.diff_tree_to_index(a.as_ref(), None, Some(&mut o))?,
        // The files as they are, whatever the index says is tracked (a
        // snapshot holds untracked files too).
        (Side::Tree(a), Side::Files) => repo.diff_tree_to_workdir(a.as_ref(), Some(&mut o))?,
        (Side::Index, Side::Files) => repo.diff_index_to_workdir(None, Some(&mut o))?,
        // The same side twice: nothing changed.
        _ => repo.diff_tree_to_tree(None, None, Some(&mut o))?,
    };
    Ok(vec![(Group::Changes, found(diff)?)])
}

/// A delta's group (untracked files are their own among the unstaged), its
/// status, its path and its path before (a rename's), from the project's
/// folder (`prefix`: where it is in the repository, `dir/` or empty).
fn described(delta: &git2::DiffDelta<'_>, group: Group, prefix: &str) -> Option<(Group, FileStatus, String, Option<String>)> {
    let path = |f: git2::DiffFile<'_>| {
        f.path().map(|p| p.to_string_lossy().replace('\\', "/")).and_then(|p| p.strip_prefix(prefix).map(str::to_owned))
    };
    let (status, group) = match delta.status() {
        Delta::Untracked => (FileStatus::Added, if group == Group::Unstaged { Group::Untracked } else { group }),
        Delta::Added | Delta::Copied => (FileStatus::Added, group),
        Delta::Deleted => (FileStatus::Deleted, group),
        Delta::Modified | Delta::Conflicted => (FileStatus::Modified, group),
        Delta::Renamed => (FileStatus::Renamed, group),
        Delta::Typechange => (FileStatus::TypeChanged, group),
        Delta::Unmodified | Delta::Ignored | Delta::Unreadable => return None,
    };
    let new = path(delta.new_file());
    let old = path(delta.old_file());
    let shown = if status == FileStatus::Deleted { old.clone() } else { new.or_else(|| old.clone()) }?;
    let before = (status == FileStatus::Renamed).then_some(old).flatten();
    Some((group, status, shown, before))
}

/// The project's folder as a pathspec (none: the whole repository).
fn within(prefix: &str) -> Vec<&str> {
    prefix.strip_suffix('/').into_iter().collect()
}

/// The files `comparison` changes in the project's folder, with their counts.
pub(crate) fn files(repo: &Repository, comparison: &Comparison, prefix: &str) -> Result<Vec<ChangedFile>> {
    let mut out = Vec::new();
    for (group, diff) in diffs(repo, comparison, 0, &within(prefix))? {
        for (i, delta) in diff.deltas().enumerate() {
            let Some((group, status, path, old_path)) = described(&delta, group, prefix) else { continue };
            let patch = Patch::from_diff(&diff, i)?;
            let binary = delta.flags().is_binary() || patch.is_none();
            let (added, removed) = match &patch {
                Some(p) => p.line_stats().map(|(_, a, r)| (a as u32, r as u32))?,
                None => (0, 0),
            };
            out.push(ChangedFile { path, old_path, status, group, added, removed, binary });
        }
    }
    Ok(out)
}

/// Each changed file's runs on its newer side, with no context around
/// them (deleted files have none).
pub(crate) fn runs(repo: &Repository, comparison: &Comparison, prefix: &str) -> Result<Vec<FileRuns>> {
    let mut out = Vec::new();
    for (group, diff) in diffs(repo, comparison, 0, &within(prefix))? {
        for (i, delta) in diff.deltas().enumerate() {
            let Some((_, status, path, _)) = described(&delta, group, prefix) else { continue };
            if status == FileStatus::Deleted {
                continue;
            }
            let Some(patch) = Patch::from_diff(&diff, i)? else { continue };
            let runs = (0..patch.num_hunks())
                .map(|h| patch.hunk(h).map(|(hunk, _)| Run { start: hunk.new_start(), lines: hunk.new_lines(), removed: hunk.old_lines() }))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if !runs.is_empty() {
                out.push(FileRuns { path, runs });
            }
        }
    }
    Ok(out)
}

/// `path`'s changes (from the project's folder) in `group` of `comparison`.
pub(crate) fn file(
    repo: &Repository,
    comparison: &Comparison,
    path: &str,
    group: Group,
    context: u32,
    prefix: &str,
) -> Result<FileChanges> {
    let mut out = FileChanges { comparison: comparison.clone(), path: path.to_owned(), group, context, binary: false, hunks: Vec::new() };
    let full = format!("{prefix}{path}");
    for (diff_group, diff) in diffs(repo, comparison, context, &[full.as_str()])? {
        for (i, delta) in diff.deltas().enumerate() {
            let Some((delta_group, _, shown, old)) = described(&delta, diff_group, prefix) else { continue };
            if delta_group != group || (shown != path && old.as_deref() != Some(path)) {
                continue;
            }
            let Some(patch) = Patch::from_diff(&diff, i)? else {
                out.binary = true;
                continue;
            };
            out.binary |= delta.flags().is_binary();
            out.hunks.extend(hunks(&patch)?);
        }
    }
    Ok(out)
}

/// The changes from `old` to `new` (two texts, not files: a call's change
/// as its harness gave it), as a file's changes are made, with `context`
/// unchanged lines around each.
pub(crate) fn texts(old: &str, new: &str, context: u32) -> Result<Vec<Hunk>> {
    let mut options = DiffOptions::new();
    options.context_lines(context);
    let patch = Patch::from_buffers(old.as_bytes(), None, new.as_bytes(), None, Some(&mut options))?;
    hunks(&patch)
}

/// A patch's hunks, each line with its numbers on each side.
fn hunks(patch: &Patch) -> Result<Vec<Hunk>> {
    let mut out = Vec::with_capacity(patch.num_hunks());
    for h in 0..patch.num_hunks() {
        let (hunk, count) = patch.hunk(h)?;
        let mut lines = Vec::with_capacity(count);
        for l in 0..count {
            let line = patch.line_in_hunk(h, l)?;
            let kind = match line.origin() {
                '+' => LineKind::Added,
                '-' => LineKind::Removed,
                ' ' => LineKind::Context,
                // "No newline at end of file" and the like.
                _ => continue,
            };
            let text = String::from_utf8_lossy(line.content()).trim_end_matches(['\n', '\r']).to_owned();
            lines.push(DiffLine { kind, old: line.old_lineno(), new: line.new_lineno(), text });
        }
        out.push(Hunk {
            old_start: hunk.old_start(),
            old_lines: hunk.old_lines(),
            new_start: hunk.new_start(),
            new_lines: hunk.new_lines(),
            lines,
        });
    }
    Ok(out)
}

/// What `path`'s changes in `comparison` depend on, cheaply: the commits'
/// ids, the index's and the file's size and time. The same fingerprint,
/// the same changes.
pub(crate) fn fingerprint(repo: &Repository, comparison: &Comparison, path: &str, prefix: &str) -> Result<String> {
    let stat =
        |p: &std::path::Path| std::fs::metadata(p).map_or_else(|_| "-".to_owned(), |m| format!("{}:{:?}", m.len(), m.modified().ok()));
    let mut parts = Vec::new();
    for point in [&comparison.from, &comparison.to] {
        parts.push(match point {
            Point::Head => repo.head().ok().and_then(|h| h.target()).map_or_else(|| "unborn".to_owned(), |o| o.to_string()),
            Point::Rev(rev) => match repo.revparse_single(rev) {
                Ok(o) => o.id().to_string(),
                // One that isn't there: the diff says so; this only keys the cache.
                Err(_missing) => format!("missing {rev}"),
            },
            Point::Snapshot(id) => id.clone(),
            Point::Index => stat(&repo.path().join("index")),
            Point::Files => stat(&repo.workdir().unwrap_or(repo.path()).join(prefix).join(path)),
        });
    }
    if comparison.is_uncommitted() {
        parts.push(stat(&repo.path().join("index")));
    }
    Ok(parts.join("|"))
}
