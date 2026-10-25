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
    let (start, new) = changes.snapshot("turn started", None).unwrap();
    assert!(new);
    assert_eq!((repo.head().unwrap().target(), listed(&changes, &Comparison::default())), before, "the user's index, branch and files are untouched");
    // What changed since: only what happened after it.
    write(dir.path(), "c.txt", "untracked\nedited after\n");
    let since = Comparison { from: Point::Snapshot(start.id.clone()), to: Point::Files };
    assert_eq!(listed(&changes, &since), [(Group::Changes, "c.txt".into(), FileStatus::Modified, 1, 0)]);
    let (end, _) = changes.snapshot("turn ended", Some(&start)).unwrap();
    assert_eq!(changes.snapshot("again", Some(&end)).unwrap(), (end.clone(), false), "nothing changed since: the same one");
    let between = Comparison { from: Point::Snapshot(start.id.clone()), to: Point::Snapshot(end.id.clone()) };
    assert_eq!(listed(&changes, &between), [(Group::Changes, "c.txt".into(), FileStatus::Modified, 1, 0)]);
    assert_eq!(changes.snapshots().unwrap().iter().map(|s| s.id.clone()).collect::<Vec<_>>(), [start.id.clone(), end.id.clone()]);
    assert_eq!(changes.snapshots_for("turn ended").unwrap(), std::slice::from_ref(&end), "each told apart by what it was taken for");
    assert!(changes.snapshots_for("some other reason").unwrap().is_empty());
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

#[test]
fn runs_are_where_the_files_now_changed_and_files_changed_since_the_newer_side_are_apart() {
    let (dir, repo) = repo();
    write(dir.path(), "a.txt", "1\n2\n3\n4\n5\n6\n");
    write(dir.path(), "b.txt", "x\n");
    stage(&repo, &["a.txt", "b.txt"]);
    commit(&repo, "first");
    write(dir.path(), "a.txt", "1\ntwo\n3\n4\n5\n6\n");
    stage(&repo, &["a.txt"]);
    write(dir.path(), "a.txt", "1\ntwo\n3\n4\n6\nseven\n");
    let changes = Changes::open(dir.path()).unwrap();
    let now = changes.on_files(&Comparison::default()).unwrap();
    assert!(now.moved.is_empty());
    assert_eq!(now.files.len(), 1);
    let runs: Vec<(u32, u32, u32)> = now.files[0].runs.iter().map(|r| (r.start, r.lines, r.removed)).collect();
    assert_eq!(runs, [(2, 1, 1), (4, 0, 1), (6, 1, 0)], "staged and unstaged together: changed, removed after line 4, added");
    let removed = now.files[0].runs[1];
    assert!(removed.within(4, 4) && !removed.within(5, 9) && removed.lines_in(1, 9) == 0, "a removal sits on the line before it");
    assert_eq!(now.files[0].runs[2].lines_in(1, 6), 1);
    // Up to the staged files: a.txt changed since, so it's apart.
    let staged = changes.on_files(&Comparison { from: Point::Head, to: Point::Index }).unwrap();
    assert_eq!((staged.files.len(), staged.moved), (0, vec!["a.txt".to_owned()]));
}


#[test]
fn the_user_stages_and_unstages_a_file_or_one_change_in_it() {
    let (dir, _repo) = repo();
    write(dir.path(), "a.txt", "ONE\ntwo\nthree\nfour\nFIVE\n");
    write(dir.path(), "c.txt", "new\n");
    std::fs::remove_file(dir.path().join("b.txt")).unwrap();
    let changes = Changes::open(dir.path()).unwrap();
    let groups = |changes: &Changes| listed(changes, &Comparison::default()).into_iter().map(|(g, p, ..)| (g, p)).collect::<Vec<_>>();
    // One of a.txt's two changes (shown with no context: two hunks).
    let unstaged = changes.file(&Comparison::default(), "a.txt", Group::Unstaged, 0).unwrap();
    assert_eq!(unstaged.hunks.len(), 2);
    let first = HunkAt { old_start: unstaged.hunks[0].old_start, new_start: unstaged.hunks[0].new_start, context: 0 };
    changes.stage("a.txt", Some(first)).unwrap();
    let staged = changes.file(&Comparison::default(), "a.txt", Group::Staged, 0).unwrap();
    assert_eq!(staged.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind == LineKind::Added).map(|l| l.text.as_str()).collect::<Vec<_>>(), ["ONE"], "only that change");
    assert_eq!(changes.file(&Comparison::default(), "a.txt", Group::Unstaged, 0).unwrap().hunks.len(), 1, "the other still unstaged");
    assert!(changes.stage("a.txt", Some(first)).is_err(), "not there any more: says so");
    // And back.
    let at = HunkAt { old_start: staged.hunks[0].old_start, new_start: staged.hunks[0].new_start, context: 0 };
    changes.unstage("a.txt", Some(at)).unwrap();
    assert!(changes.file(&Comparison::default(), "a.txt", Group::Staged, 0).unwrap().hunks.is_empty());
    // Whole files: changed, new, deleted.
    for path in ["a.txt", "c.txt", "b.txt"] {
        changes.stage(path, None).unwrap();
    }
    assert_eq!(groups(&changes), [(Group::Staged, "a.txt".into()), (Group::Staged, "b.txt".into()), (Group::Staged, "c.txt".into())]);
    for path in ["a.txt", "c.txt", "b.txt"] {
        changes.unstage(path, None).unwrap();
    }
    assert_eq!(groups(&changes), [(Group::Unstaged, "a.txt".into()), (Group::Unstaged, "b.txt".into()), (Group::Untracked, "c.txt".into())]);
    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "ONE\ntwo\nthree\nfour\nFIVE\n", "the files as they were");
}

#[test]
fn a_big_repository_and_a_30000_line_file_stay_fast() {
    let (dir, repo) = repo();
    let big: String = (0..30_000).map(|i| format!("line {i}\n")).collect();
    write(dir.path(), "big.txt", &big);
    let small: Vec<String> = (0..2000).map(|i| format!("src/m{}/f{i}.rs", i % 40)).collect();
    for p in &small {
        write(dir.path(), p, "fn a() {}\n");
    }
    let mut index = repo.index().unwrap();
    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None).unwrap();
    index.write().unwrap();
    commit(&repo, "big");
    let edited: String = (0..30_000).map(|i| if i % 100 == 0 { format!("changed {i}\n") } else { format!("line {i}\n") }).collect();
    write(dir.path(), "big.txt", &edited);
    for p in small.iter().step_by(40) {
        write(dir.path(), p, "fn a() {}\nfn b() {}\n");
    }
    let changes = Changes::open(dir.path()).unwrap();
    let started = std::time::Instant::now();
    let list = changes.list(&Comparison::default(), 0, PAGE).unwrap();
    let listed_in = started.elapsed();
    assert_eq!(list.total, 51);
    let started = std::time::Instant::now();
    let file = changes.file(&Comparison::default(), "big.txt", Group::Unstaged, CONTEXT).unwrap();
    let read_in = started.elapsed();
    assert_eq!(file.hunks.len(), 300);
    let started = std::time::Instant::now();
    changes.file(&Comparison::default(), "big.txt", Group::Unstaged, CONTEXT).unwrap();
    let kept_in = started.elapsed();
    assert!(listed_in.as_millis() < 1500 && read_in.as_millis() < 1000 && kept_in < read_in, "list {listed_in:?}, the big file {read_in:?}, again {kept_in:?}");
}

#[test]
fn more_context_shows_that_many_more_unchanged_lines_on_each_side() {
    let (dir, repo) = repo();
    let text: String = (1..=60).map(|n| format!("line {n}\n")).collect();
    write(dir.path(), "long.txt", &text);
    stage(&repo, &["long.txt"]);
    commit(&repo, "long");
    write(dir.path(), "long.txt", &text.replace("line 30\n", "LINE 30\n"));
    let changes = Changes::open(dir.path()).unwrap();
    let around = |context: u32| {
        let file = changes.file(&Comparison::default(), "long.txt", Group::Unstaged, context).unwrap();
        let lines = &file.hunks[0].lines;
        let before = lines.iter().take_while(|l| l.kind == LineKind::Context).count();
        let after = lines.iter().rev().take_while(|l| l.kind == LineKind::Context).count();
        (before, after)
    };
    assert_eq!(around(3), (3, 3));
    assert_eq!(around(13), (13, 13), "ten more on each side");
}

#[test]
fn snapshots_taken_in_the_same_millisecond_keep_the_order_they_were_taken_in() {
    let (dir, _repo) = repo();
    let changes = Changes::open(dir.path()).unwrap();
    let mut taken = Vec::new();
    for n in 0..20 {
        write(dir.path(), "a.txt", &format!("edit {n}\n"));
        taken.push(changes.snapshot("quick", taken.last()).unwrap().0);
    }
    let listed: Vec<String> = changes.snapshots().unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(listed, taken.iter().map(|s| s.id.clone()).collect::<Vec<_>>());
}

/// Two texts compare as a file's sides do: each change with its context
/// around it, numbered on each side.
#[test]
fn two_texts_compare_as_a_files_sides_do() {
    let hunks = texts("a\nb\nc\nd\ne\nf\ng\n", "a\nB\nc\nd\ne\nf\ng\nh\n", 1).unwrap();
    type Said<'a> = (LineKind, Option<u32>, Option<u32>, &'a str);
    let said: Vec<Vec<Said>> = hunks.iter().map(|h| h.lines.iter().map(|l| (l.kind, l.old, l.new, l.text.as_str())).collect()).collect();
    assert_eq!(said, [
        vec![(LineKind::Context, Some(1), Some(1), "a"), (LineKind::Removed, Some(2), None, "b"), (LineKind::Added, None, Some(2), "B"), (LineKind::Context, Some(3), Some(3), "c")],
        vec![(LineKind::Context, Some(7), Some(7), "g"), (LineKind::Added, None, Some(8), "h")],
    ]);
}
