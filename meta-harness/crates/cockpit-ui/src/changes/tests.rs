//! The changes view on its own: the list and a file's changes as they come
//! in, what opens by itself, the cursor over files and changes, folds,
//! search, side-by-side rows.

use harness_contracts::changes::{ChangedFile, DiffLine};

use super::*;

fn file(group: Group, path: &str, status: FileStatus, added: u32, removed: u32) -> ChangedFile {
    ChangedFile { path: path.into(), old_path: None, status, group, added, removed, binary: false }
}

fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
    DiffLine { kind, old, new, text: text.into() }
}

/// a.rs: one change (line 3 replaced); b.rs deleted.
fn view_with_changes() -> (ChangesView, NetClient, egui::Context) {
    let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
    let mut view = ChangesView::default();
    let list = ChangeList { comparison: Comparison::default(), files: vec![file(Group::Unstaged, "a.rs", FileStatus::Modified, 1, 1), file(Group::Unstaged, "b.rs", FileStatus::Deleted, 0, 2)], offset: 0, total: 2 };
    push(&view.inbox, Reply::List(Comparison::default(), Ok(list)));
    view.stale = false;
    view.loading = true;
    view.tick(&net, &ctx, false);
    let hunk = Hunk { old_start: 2, old_lines: 3, new_start: 2, new_lines: 3, lines: vec![line(LineKind::Context, Some(2), Some(2), "two"), line(LineKind::Removed, Some(3), None, "old three"), line(LineKind::Added, None, Some(3), "new three"), line(LineKind::Context, Some(4), Some(4), "four")] };
    let a = FileChanges { comparison: Comparison::default(), path: "a.rs".into(), group: Group::Unstaged, context: 3, binary: false, hunks: vec![hunk] };
    push(&view.inbox, Reply::File(Comparison::default(), (Group::Unstaged, "a.rs".into()), Ok(a)));
    let b = FileChanges { comparison: Comparison::default(), path: "b.rs".into(), group: Group::Unstaged, context: 3, binary: false, hunks: vec![Hunk { old_start: 1, old_lines: 2, new_start: 0, new_lines: 0, lines: vec![line(LineKind::Removed, Some(1), None, "gone"), line(LineKind::Removed, Some(2), None, "too")] }] };
    push(&view.inbox, Reply::File(Comparison::default(), (Group::Unstaged, "b.rs".into()), Ok(b)));
    view.tick(&net, &ctx, false);
    (view, net, ctx)
}

#[test]
fn a_small_set_of_changes_opens_by_itself_and_a_big_one_waits_to_be_opened() {
    let (view, net, _) = view_with_changes();
    let asked: Vec<String> = net.sent.lock().unwrap().iter().filter(|s| s.contains("/api/changes/file")).cloned().collect();
    assert_eq!(asked.len(), 2, "both files asked for: {asked:?}");
    assert_eq!(view.rows(), [Row::File(0), Row::Hunk(0, 0), Row::File(1), Row::Hunk(1, 0)], "the diff run together");
    // A big one: only its list.
    let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
    let mut big = ChangesView::default();
    let files: Vec<ChangedFile> = (0..40).map(|i| file(Group::Unstaged, &format!("f{i}.rs"), FileStatus::Modified, 1, 0)).collect();
    push(&big.inbox, Reply::List(Comparison::default(), Ok(ChangeList { comparison: Comparison::default(), files, offset: 0, total: 40 })));
    (big.stale, big.loading) = (false, true);
    big.tick(&net, &ctx, false);
    assert!(!net.sent.lock().unwrap().iter().any(|s| s.contains("/api/changes/file")), "40 files: none opened by itself");
    assert_eq!(big.rows().len(), 40);
}

#[test]
fn the_cursor_walks_files_and_changes_and_enter_opens_the_spot() {
    let (mut view, net, ctx) = view_with_changes();
    assert_eq!(view.target(), Some(Target::Change { file: "a.rs".into(), line: 3 }), "a file: its first changed line");
    view.motion(Motion::Down, 1);
    assert_eq!((view.cursor_key().as_deref(), view.enter()), (Some("h:Unstaged:a.rs:0"), Step::GoTo(GoTo::Code)));
    assert!(view.can_leave());
    view.leave();
    assert_eq!(view.cursor_key().as_deref(), Some("f:Unstaged:a.rs"), "out: up to its file");
    view.motion(Motion::NextSibling, 1);
    assert_eq!(view.cursor_key().as_deref(), Some("f:Unstaged:b.rs"), "]]: the next file");
    assert!(matches!(view.enter(), Step::Nothing(why) if why.contains("deleted")), "a deleted file has nothing to open");
    // Folds: its changes fold away, the cursor on the file.
    view.motion(Motion::Down, 1);
    view.fold(Fold::Close);
    view.tick(&net, &ctx, false);
    assert_eq!((view.rows().len(), view.cursor_key().as_deref()), (3, Some("f:Unstaged:b.rs")));
    view.fold(Fold::CloseAll);
    view.tick(&net, &ctx, false);
    assert_eq!(view.rows(), [Row::File(0), Row::File(1)]);
    // Places come back by key.
    let place = view.place();
    view.put_cursor(0);
    view.restore(&place);
    assert_eq!(view.cursor_key().as_deref(), Some("f:Unstaged:b.rs"));
}

#[test]
fn search_finds_file_names_and_changed_lines_removed_ones_too() {
    let (view, _, _) = view_with_changes();
    let items = view.items();
    assert!(items.iter().any(|i| i.key == "f:Unstaged:a.rs" && i.name == "a.rs"));
    assert!(items.iter().any(|i| i.key == "h:Unstaged:a.rs:0" && i.text.contains("old three") && i.text.contains("new three")), "{items:?}");
    assert!(items.iter().any(|i| i.text.contains("gone")), "a deleted file's lines");
}

#[test]
fn side_by_side_puts_a_run_of_removed_lines_beside_the_added_ones() {
    let lines = [line(LineKind::Context, Some(1), Some(1), "a"), line(LineKind::Removed, Some(2), None, "b"), line(LineKind::Removed, Some(3), None, "c"), line(LineKind::Added, None, Some(2), "B"), line(LineKind::Context, Some(4), Some(3), "d")];
    let rows: Vec<(Option<&str>, Option<&str>)> = paired(&lines).into_iter().map(|(l, r)| (l.map(|l| l.text.as_str()), r.map(|r| r.text.as_str()))).collect();
    assert_eq!(rows, [(Some("a"), Some("a")), (Some("b"), Some("B")), (Some("c"), None), (Some("d"), Some("d"))]);
    let mut view = ChangesView::default();
    assert!(view.side_by_side(1200.0) && !view.side_by_side(500.0), "auto: by the column's width");
    view.layout = DiffLayout::Unified;
    assert!(!view.side_by_side(1200.0));
}

#[test]
fn a_comparison_is_picked_by_its_two_points() {
    assert_eq!(compared("HEAD..files"), Some(Comparison::default()));
    assert_eq!(compared("rev:main..snapshot:1-ab"), Some(Comparison { from: Point::Rev("main".into()), to: Point::Snapshot("1-ab".into()) }));
    let (mut view, _, _) = view_with_changes();
    view.compare(Comparison { from: Point::Head, to: Point::Index });
    assert!(view.list().is_none() && view.rows().is_empty() && view.stale, "another comparison: asked afresh");
}
