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
    assert!(view.actions().iter().any(|a| a.id == "nav.leave"), "on a change: up to its file");
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

#[test]
fn going_to_a_components_changes_opens_its_file_at_its_change() {
    let (mut view, net, ctx) = view_with_changes();
    let id = arch_graph::ComponentId::new("rust:a::three");
    assert!(!view.reveal(&id), "nothing known about it changed");
    let list = ComponentChangeList { comparison: Comparison::default(), components: vec![harness_contracts::changes::ComponentChanges { component: id.as_str().into(), file: "a.rs".into(), added: 1, removed: 1, runs: vec![(3, 1)], direct: true }], out_of_date: vec![] };
    push(&view.inbox, Reply::Components(Comparison::default(), Ok(list)));
    view.tick(&net, &ctx, false);
    assert!(view.touches(&id) && view.components().0.is_some());
    assert_eq!(view.by_component().get(&id).map(|c| (c.added, c.removed)), Some((1, 1)));
    view.close(&(Group::Unstaged, "a.rs".into()));
    assert!(view.reveal(&id));
    view.tick(&net, &ctx, false);
    let a = FileChanges { comparison: Comparison::default(), path: "a.rs".into(), group: Group::Unstaged, context: 3, binary: false, hunks: vec![Hunk { old_start: 2, old_lines: 1, new_start: 2, new_lines: 2, lines: vec![line(LineKind::Added, None, Some(3), "new three")] }] };
    push(&view.inbox, Reply::File(Comparison::default(), (Group::Unstaged, "a.rs".into()), Ok(a)));
    view.tick(&net, &ctx, false);
    assert_eq!(view.cursor_key().as_deref(), Some("h:Unstaged:a.rs:0"), "its file opened again, the cursor on the change");
}

fn window(from: &str, to: Option<&str>, started_ms: u64, slots: &[&str], files: u32) -> Window {
    let working = slots.iter().map(|s| harness_contracts::changes::Working { slot: (*s).into(), thread: "t".into(), turn: "u".into() }).collect();
    Window { from: from.into(), to: to.map(Into::into), started_ms, ended_ms: to.map(|_| started_ms + 1), working, files }
}

#[test]
fn the_timeline_lists_windows_newest_first_and_picking_one_compares_it() {
    let (mut view, net, ctx) = view_with_changes();
    let timeline = Timeline { windows: vec![window("1-a", Some("2-b"), 1, &["lead"], 2), window("2-b", None, 2, &[], 1)], turns: vec![] };
    push(&view.inbox, Reply::Timeline(Ok(timeline)));
    view.tick(&net, &ctx, false);
    assert_eq!(view.timeline().1, 1);
    let mut cx_net = (NetClient::new("http://127.0.0.1:9"), None, None, crate::keymap::InputMode::Normal);
    let mut cx = RunCx { net: &cx_net.0, ctx: &ctx, input_mode: &mut cx_net.3, slot: 0, note: &mut cx_net.1, asked: &mut cx_net.2 };
    let timeline = view.actions().into_iter().find(|a| a.id == "changes.timeline").unwrap();
    view.run(&timeline, None, &mut cx);
    assert_eq!(view.rows(), [Row::Window(0), Row::Window(1)]);
    assert_eq!(view.cursor_key().as_deref(), Some("w:2-b"), "newest first: the open one");
    assert!(view.items().iter().any(|i| i.text.contains("lead")), "who was working is searchable");
    view.motion(Motion::Down, 1);
    assert_eq!(view.enter(), Step::Done);
    assert_eq!((view.comparison.clone(), view.on_timeline), (Comparison { from: Point::Snapshot("1-a".into()), to: Point::Snapshot("2-b".into()) }, false), "that window, compared");
    // "Since here": from a window's start to the files now.
    view.show_timeline(true);
    let since = view.actions().into_iter().find(|a| a.id == "changes.since").expect("on a window");
    view.run(&since, None, &mut cx);
    assert_eq!(view.comparison, Comparison { from: Point::Snapshot("2-b".into()), to: Point::Files });
}

#[test]
fn the_user_stages_a_file_or_a_change_and_the_cursor_follows_the_file() {
    let (mut view, net, ctx) = view_with_changes();
    let mut parts = (None, None, crate::keymap::InputMode::Normal);
    let mut cx = RunCx { net: &net, ctx: &ctx, input_mode: &mut parts.2, slot: 0, note: &mut parts.0, asked: &mut parts.1 };
    let ids = |view: &ChangesView| view.actions().into_iter().map(|a| a.id).collect::<Vec<_>>();
    assert!(ids(&view).contains(&"changes.stage".to_owned()) && !ids(&view).contains(&"changes.unstage".to_owned()), "unstaged: stage it");
    let stage = view.actions().into_iter().find(|a| a.id == "changes.stage").unwrap();
    view.run(&stage, None, &mut cx);
    assert!(net.sent.lock().unwrap().iter().any(|s| s.ends_with("POST /api/changes/stage")));
    assert_eq!(view.restore.as_deref(), Some("f:Staged:a.rs"), "the cursor follows it");
    push(&view.inbox, Reply::Staged(Ok(serde_json::Value::Null)));
    view.tick(&net, &ctx, false);
    assert!(view.loading, "read again");
    // Another comparison's changes can't be staged.
    view.compare(Comparison { from: Point::Head, to: Point::Rev("main".into()) });
    assert!(!ids(&view).contains(&"changes.stage".to_owned()));
}

#[test]
fn switching_the_comparison_while_a_list_is_on_its_way_asks_for_the_new_one() {
    let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
    let mut view = ChangesView::default();
    view.tick(&net, &ctx, false);
    assert!(view.loading, "the first list asked for");
    let staged = Comparison { from: Point::Head, to: Point::Index };
    view.compare(staged.clone());
    view.tick(&net, &ctx, false);
    let asked = |net: &NetClient| net.sent.lock().unwrap().iter().filter(|s| s.contains("/api/changes?")).count();
    assert_eq!(asked(&net), 2, "the new comparison asked for at once, not after the old answer");
    // The old answer comes late: dropped; the new one shows.
    push(&view.inbox, Reply::List(Comparison::default(), Ok(ChangeList { comparison: Comparison::default(), files: vec![], offset: 0, total: 0 })));
    view.tick(&net, &ctx, false);
    assert!(view.loading && view.list().is_none(), "still waiting for the new one");
    push(&view.inbox, Reply::List(staged.clone(), Ok(ChangeList { comparison: staged, files: vec![file(Group::Staged, "a.rs", FileStatus::Modified, 1, 0)], offset: 0, total: 1 })));
    view.tick(&net, &ctx, false);
    assert_eq!((view.loading, view.list().map(|l| l.total)), (false, Some(1)));
}

#[test]
fn as_a_destination_it_shows_touched_components_and_turns_it_saw() {
    let (mut view, net, ctx) = view_with_changes();
    let id = arch_graph::ComponentId::new("rust:a::three");
    let component = Target::Component(id.clone());
    assert!(!view.can_show(&component), "nothing known to touch it");
    let list = ComponentChangeList { comparison: Comparison::default(), components: vec![harness_contracts::changes::ComponentChanges { component: id.as_str().into(), file: "a.rs".into(), added: 1, removed: 1, runs: vec![(3, 1)], direct: true }], out_of_date: vec![] };
    push(&view.inbox, Reply::Components(Comparison::default(), Ok(list)));
    view.tick(&net, &ctx, false);
    assert!(view.can_show(&component));
    assert!(!view.can_show(&Target::Change { file: "a.rs".into(), line: 3 }), "a change is what it shows");
    let message = |turn: &str| Target::Message { thread: None, turn: Some(turn.into()), index: 0, link: None };
    let turn = Comparison { from: Point::Snapshot("1-a".into()), to: Point::Snapshot("2-b".into()) };
    view.take_timeline_for_test(Timeline { windows: vec![], turns: vec![harness_contracts::changes::TurnChanges { thread: "t".into(), turn: "u1".into(), comparison: turn.clone(), files: 1, alongside: vec![] }] });
    assert!(view.can_show(&message("u1")) && !view.can_show(&message("u2")));
    assert_eq!(view.show_target(&message("u1")), Ok(()));
    assert_eq!(view.comparison, turn, "its turn, compared");
    assert!(view.show_target(&message("u2")).is_err());
}
