//! On scratch repositories: what each comparison lists, one file's
//! changes, snapshots leaving the user's repository as it was.

use std::path::Path;

use harness_contracts::changes::{Comparison, FileStatus, Group, LineKind, Point};

use super::*;

fn write(root: &Path, path: &str, text: &str) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, text).unwrap();
}

/// Stages `paths` (removing those that are gone).
fn stage(repo: &git2::Repository, paths: &[&str]) {
    let mut index = repo.index().unwrap();
    for p in paths {
        if repo.workdir().unwrap().join(p).exists() {
            index.add_path(Path::new(p)).unwrap();
        } else {
            index.remove_path(Path::new(p)).unwrap();
        }
    }
    index.write().unwrap();
}

fn commit(repo: &git2::Repository, message: &str) -> git2::Oid {
    let mut index = repo.index().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let who = git2::Signature::now("t", "t@t").unwrap();
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    repo.commit(Some("HEAD"), &who, &who, message, &tree, &parent.iter().collect::<Vec<_>>()).unwrap()
}

/// A repository with `a.txt` (five lines) and `b.txt` committed, and an
/// ignore rule for `build/`.
fn repo() -> (tempfile::TempDir, git2::Repository) {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    write(dir.path(), "a.txt", "one\ntwo\nthree\nfour\nfive\n");
    write(dir.path(), "b.txt", "bee\n");
    write(dir.path(), ".gitignore", "build/\n");
    stage(&repo, &["a.txt", "b.txt", ".gitignore"]);
    commit(&repo, "first");
    (dir, repo)
}

fn listed(changes: &Changes, comparison: &Comparison) -> Vec<(Group, String, FileStatus, u32, u32)> {
    changes.list(comparison, 0, PAGE).unwrap().files.into_iter().map(|f| (f.group, f.path, f.status, f.added, f.removed)).collect()
}

#[test]
fn the_uncommitted_changes_are_staged_unstaged_and_untracked_as_git_says() {
    let (dir, repo) = repo();
    let root = dir.path();
    write(root, "a.txt", "one\nTWO\nthree\nfour\nfive\n");
    stage(&repo, &["a.txt"]);
    write(root, "a.txt", "one\nTWO\nthree\nfour\nfive\nsix\n");
    std::fs::remove_file(root.join("b.txt")).unwrap();
    write(root, "c.txt", "new\nfile\n");
    write(root, "build/out.bin", "ignored\n");
    let changes = Changes::open(root).unwrap();
    assert_eq!(
        listed(&changes, &Comparison::default()),
        [
            (Group::Staged, "a.txt".into(), FileStatus::Modified, 1, 1),
            (Group::Unstaged, "a.txt".into(), FileStatus::Modified, 1, 0),
            (Group::Unstaged, "b.txt".into(), FileStatus::Deleted, 0, 1),
            (Group::Untracked, "c.txt".into(), FileStatus::Added, 2, 0),
        ],
        "ignored files aren't changes"
    );
    // A page of them.
    let page = changes.list(&Comparison::default(), 1, 2).unwrap();
    assert_eq!((page.total, page.files.len(), page.files[0].path.as_str()), (4, 2, "a.txt"));
    assert_eq!(changes.changed_paths(&Comparison::default()).unwrap(), ["a.txt", "b.txt", "c.txt"]);
}

#[test]
fn a_files_changes_have_their_lines_on_each_side_and_context_around() {
    let (dir, _repo) = repo();
    write(dir.path(), "a.txt", "one\ntwo\nTHREE\nfour\nfive\n");
    let changes = Changes::open(dir.path()).unwrap();
    let file = changes.file(&Comparison::default(), "a.txt", Group::Unstaged, 1).unwrap();
    assert_eq!(file.hunks.len(), 1);
    let lines: Vec<(LineKind, Option<u32>, Option<u32>, &str)> = file.hunks[0].lines.iter().map(|l| (l.kind, l.old, l.new, l.text.as_str())).collect();
    assert_eq!(lines, [(LineKind::Context, Some(2), Some(2), "two"), (LineKind::Removed, Some(3), None, "three"), (LineKind::Added, None, Some(3), "THREE"), (LineKind::Context, Some(4), Some(4), "four")]);
    assert!(changes.file(&Comparison::default(), "a.txt", Group::Staged, 1).unwrap().hunks.is_empty(), "nothing of it is staged");
    // Kept, until the file changes.
    write(dir.path(), "a.txt", "one\ntwo\nthree\nFOUR!\nfive\n");
    let again = changes.file(&Comparison::default(), "a.txt", Group::Unstaged, 1).unwrap();
    assert!(again.hunks[0].lines.iter().any(|l| l.text == "FOUR!"), "the file changed: read again");
}

#[test]
fn any_two_points_compare_either_way() {
    let (dir, repo) = repo();
    let first = repo.head().unwrap().target().unwrap().to_string();
    write(dir.path(), "a.txt", "one\n");
    write(dir.path(), "d.txt", "dee\n");
    stage(&repo, &["a.txt", "d.txt"]);
    commit(&repo, "second");
    let changes = Changes::open(dir.path()).unwrap();
    let forward = Comparison { from: Point::Rev(first.clone()), to: Point::Head };
    assert_eq!(listed(&changes, &forward), [(Group::Changes, "a.txt".into(), FileStatus::Modified, 0, 4), (Group::Changes, "d.txt".into(), FileStatus::Added, 1, 0)]);
    let back = Comparison { from: Point::Head, to: Point::Rev(first) };
    assert_eq!(listed(&changes, &back), [(Group::Changes, "a.txt".into(), FileStatus::Modified, 4, 0), (Group::Changes, "d.txt".into(), FileStatus::Deleted, 0, 1)], "the other way: what was added is removed");
    // The staged files against the last commit, and the files against what's staged.
    write(dir.path(), "e.txt", "e\n");
    stage(&repo, &["e.txt"]);
    write(dir.path(), "e.txt", "e\nmore\n");
    assert_eq!(listed(&changes, &Comparison { from: Point::Head, to: Point::Index }), [(Group::Changes, "e.txt".into(), FileStatus::Added, 1, 0)]);
    assert_eq!(listed(&changes, &Comparison { from: Point::Index, to: Point::Files }), [(Group::Changes, "e.txt".into(), FileStatus::Modified, 1, 0)]);
    assert!(matches!(changes.list(&Comparison { from: Point::Rev("nope".into()), to: Point::Head }, 0, PAGE), Err(ChangesError::BadPoint(_))));
}

#[test]
fn a_staged_rename_says_where_it_came_from() {
    let (dir, repo) = repo();
    std::fs::rename(dir.path().join("a.txt"), dir.path().join("renamed.txt")).unwrap();
    stage(&repo, &["a.txt", "renamed.txt"]);
    let changes = Changes::open(dir.path()).unwrap();
    let files = changes.list(&Comparison::default(), 0, PAGE).unwrap().files;
    assert_eq!((files[0].status, files[0].path.as_str(), files[0].old_path.as_deref()), (FileStatus::Renamed, "renamed.txt", Some("a.txt")));
}

#[test]
fn a_snapshot_keeps_the_files_as_they_are_and_leaves_the_repository_as_it_was() {
    let (dir, repo) = repo();
    write(dir.path(), "a.txt", "staged\n");
    stage(&repo, &["a.txt"]);
    write(dir.path(), "c.txt", "untracked\n");
    let changes = Changes::open(dir.path()).unwrap();
    let before = (repo.head().unwrap().target(), listed(&changes, &Comparison::default()));
    let start = changes.snapshot("turn started").unwrap();
    assert_eq!((repo.head().unwrap().target(), listed(&changes, &Comparison::default())), before, "the user's index, branch and files are untouched");
    // What changed since: only what happened after it.
    write(dir.path(), "c.txt", "untracked\nedited after\n");
    let since = Comparison { from: Point::Snapshot(start.id.clone()), to: Point::Files };
    assert_eq!(listed(&changes, &since), [(Group::Changes, "c.txt".into(), FileStatus::Modified, 1, 0)]);
    let end = changes.snapshot("turn ended").unwrap();
    let between = Comparison { from: Point::Snapshot(start.id.clone()), to: Point::Snapshot(end.id.clone()) };
    assert_eq!(listed(&changes, &between), [(Group::Changes, "c.txt".into(), FileStatus::Modified, 1, 0)]);
    assert_eq!(changes.snapshots().unwrap().iter().map(|s| s.id.clone()).collect::<Vec<_>>(), [start.id.clone(), end.id.clone()]);
    changes.drop_snapshot(&start.id).unwrap();
    assert!(matches!(changes.list(&between, 0, PAGE), Err(ChangesError::BadPoint(_))), "pruned: gone");
}

#[test]
fn a_folder_outside_git_says_so() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(Changes::open(dir.path()), Err(ChangesError::NotARepo(_))));
}

#[test]
fn a_project_in_a_folder_of_the_repository_sees_its_own_files_by_its_own_paths() {
    let (dir, repo) = repo();
    write(dir.path(), "project/src/lib.rs", "pub fn a() {}\n");
    stage(&repo, &["project/src/lib.rs"]);
    commit(&repo, "project");
    write(dir.path(), "project/src/lib.rs", "pub fn a() {}\npub fn b() {}\n");
    write(dir.path(), "a.txt", "outside the project\n");
    let changes = Changes::open(&dir.path().join("project")).unwrap();
    assert_eq!(listed(&changes, &Comparison::default()), [(Group::Unstaged, "src/lib.rs".into(), FileStatus::Modified, 1, 0)], "only its folder, by its paths");
    let file = changes.file(&Comparison::default(), "src/lib.rs", Group::Unstaged, 0).unwrap();
    assert_eq!(file.hunks[0].lines[0].text, "pub fn b() {}");
}
