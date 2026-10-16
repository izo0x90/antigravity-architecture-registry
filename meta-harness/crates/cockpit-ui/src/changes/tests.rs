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
pub(crate) fn view_with_changes() -> (ChangesView, NetClient, egui::Context) {
    let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
    let mut view = ChangesView::default();
    let list = ChangeList { comparison: Comparison::default(), files: vec![file(Group::Unstaged, "a.rs", FileStatus::Modified, 1, 1), file(Group::Unstaged, "b.rs", FileStatus::Deleted, 0, 2)], offset: 0, total: 2 };
    push(&view.inbox, Reply::List(Comparison::default(), Ok(list)));
    view.stale = false;
    view.loading = true;
    view.tick(&net, &ctx, false);
    arrive(&mut view, &net, &ctx);
    (view, net, ctx)
}

/// a.rs's and b.rs's changes come in.
fn arrive(view: &mut ChangesView, net: &NetClient, ctx: &egui::Context) {
    let hunk = Hunk { old_start: 2, old_lines: 3, new_start: 2, new_lines: 3, lines: vec![line(LineKind::Context, Some(2), Some(2), "two"), line(LineKind::Removed, Some(3), None, "old three"), line(LineKind::Added, None, Some(3), "new three"), line(LineKind::Context, Some(4), Some(4), "four")] };
    let a = FileChanges { comparison: Comparison::default(), path: "a.rs".into(), group: Group::Unstaged, context: 3, binary: false, hunks: vec![hunk] };
    push(&view.inbox, Reply::File(Comparison::default(), (Group::Unstaged, "a.rs".into()), Ok(a)));
    let b = FileChanges { comparison: Comparison::default(), path: "b.rs".into(), group: Group::Unstaged, context: 3, binary: false, hunks: vec![Hunk { old_start: 1, old_lines: 2, new_start: 0, new_lines: 0, lines: vec![line(LineKind::Removed, Some(1), None, "gone"), line(LineKind::Removed, Some(2), None, "too")] }] };
    push(&view.inbox, Reply::File(Comparison::default(), (Group::Unstaged, "b.rs".into()), Ok(b)));
    view.tick(net, ctx, false);
}

#[test]
fn a_small_set_of_changes_opens_by_itself_and_a_big_one_waits_to_be_opened() {
    let (view, net, _) = view_with_changes();
    let asked: Vec<String> = net.sent.lock().unwrap().iter().filter(|s| s.contains("/api/changes/file")).cloned().collect();
    assert_eq!(asked.len(), 2, "both files asked for: {asked:?}");
    let (l, h) = (Row::Line, Row::Hunk);
    assert_eq!(view.rows(), [Row::Group(Group::Unstaged), Row::File(0), h(0, 0), l(0, 0, 0), l(0, 0, 1), l(0, 0, 2), l(0, 0, 3), Row::File(1), h(1, 0), l(1, 0, 0), l(1, 0, 1)], "the diff run together, a row per line");
    // A big one: only its list.
    let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
    let mut big = ChangesView::default();
    let files: Vec<ChangedFile> = (0..40).map(|i| file(Group::Unstaged, &format!("f{i}.rs"), FileStatus::Modified, 1, 0)).collect();
    push(&big.inbox, Reply::List(Comparison::default(), Ok(ChangeList { comparison: Comparison::default(), files, offset: 0, total: 40 })));
    (big.stale, big.loading) = (false, true);
    big.tick(&net, &ctx, false);
    assert!(!net.sent.lock().unwrap().iter().any(|s| s.contains("/api/changes/file")), "40 files: none opened by itself");
    assert_eq!(big.rows().len(), 41, "its heading and its files");
}

#[test]
fn the_cursor_walks_files_and_changes_and_enter_opens_the_spot() {
    let (mut view, net, ctx) = view_with_changes();
    assert_eq!(view.target(), Some(Target::Change { file: "a.rs".into(), line: 3 }), "a file: its first changed line");
    view.motion(Motion::Down, 1);
    assert_eq!((view.cursor_key().as_deref(), view.enter()), (Some("h:Unstaged:a.rs:0"), Step::GoTo(GoTo::Code)));
    // A line at a time: each line is where it is in the file (a removed
    // one, where it was).
    view.motion(Motion::Down, 2);
    assert_eq!((view.cursor_key().as_deref(), view.target()), (Some("l:Unstaged:a.rs:0:1"), Some(Target::Change { file: "a.rs".into(), line: 3 })));
    assert!(view.actions().iter().any(|a| a.id == "nav.leave" && a.label == "Up to its change"));
    view.leave();
    assert_eq!(view.cursor_key().as_deref(), Some("h:Unstaged:a.rs:0"), "out: up to its change");
    view.leave();
    assert_eq!(view.cursor_key().as_deref(), Some("f:Unstaged:a.rs"), "then its file");
    // ]] and [[: from change to change, across files.
    view.motion(Motion::NextSibling, 1);
    assert_eq!(view.cursor_key().as_deref(), Some("h:Unstaged:a.rs:0"));
    view.motion(Motion::NextSibling, 1);
    assert_eq!(view.cursor_key().as_deref(), Some("h:Unstaged:b.rs:0"), "]]: the next change");
    view.motion(Motion::PrevSibling, 1);
    assert_eq!(view.cursor_key().as_deref(), Some("h:Unstaged:a.rs:0"), "[[: the one before");
    view.move_to("f:Unstaged:b.rs");
    assert!(matches!(view.enter(), Step::Nothing(why) if why.contains("deleted")), "a deleted file has nothing to open");
    // Folds: on a line, its change folds to one line, the cursor on it;
    // on the file, the file folds.
    view.motion(Motion::Down, 2);
    view.fold(Fold::Close);
    view.tick(&net, &ctx, false);
    assert_eq!((view.rows().len(), view.cursor_key().as_deref()), (9, Some("h:Unstaged:b.rs:0")));
    view.leave();
    view.fold(Fold::Close);
    view.tick(&net, &ctx, false);
    assert_eq!((view.rows().len(), view.cursor_key().as_deref()), (8, Some("f:Unstaged:b.rs")));
    view.fold(Fold::CloseAll);
    view.tick(&net, &ctx, false);
    assert_eq!(view.rows(), [Row::Group(Group::Unstaged), Row::File(0), Row::File(1)]);
    // Places come back by key.
    let place = view.place();
    view.put_cursor(0, Show::InView);
    view.restore(&place);
    assert_eq!(view.cursor_key().as_deref(), Some("f:Unstaged:b.rs"));
}

#[test]
fn search_finds_file_names_and_changed_lines_removed_ones_too() {
    let (view, _, _) = view_with_changes();
    let items = view.items();
    assert!(items.iter().any(|i| i.key == "f:Unstaged:a.rs" && i.name == "a.rs"));
    assert!(items.iter().any(|i| i.key == "l:Unstaged:a.rs:0:1" && i.text == "old three"), "each line where it is, removed ones too: {items:?}");
    assert!(items.iter().any(|i| i.key == "l:Unstaged:a.rs:0:2" && i.text == "new three"));
    assert!(items.iter().any(|i| i.text.contains("gone")), "a deleted file's lines");
}

#[test]
fn side_by_side_puts_a_run_of_removed_lines_beside_the_added_ones() {
    let lines = [line(LineKind::Context, Some(1), Some(1), "a"), line(LineKind::Removed, Some(2), None, "b"), line(LineKind::Removed, Some(3), None, "c"), line(LineKind::Added, None, Some(2), "B"), line(LineKind::Context, Some(4), Some(3), "d")];
    let rows: Vec<(Option<&str>, Option<&str>)> = paired(&lines).into_iter().map(|(l, r)| (l.map(|l| l.text.as_str()), r.map(|r| r.text.as_str()))).collect();
    assert_eq!(rows, [(Some("a"), Some("a")), (Some("b"), Some("B")), (Some("c"), None), (Some("d"), Some("d"))]);
    let mut view = ChangesView::default();
    assert!(view.side_by_side(1200.0) && !view.side_by_side(500.0), "auto: by the panel's width");
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
    let mut cx = RunCx { net: &cx_net.0, ctx: &ctx, input_mode: &mut cx_net.3, panel: 0, note: &mut cx_net.1, asked: &mut cx_net.2 };
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
    let mut cx = RunCx { net: &net, ctx: &ctx, input_mode: &mut parts.2, panel: 0, note: &mut parts.0, asked: &mut parts.1 };
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

/// Draws the view in a window `h` high; the text drawn.
fn drawn(view: &mut ChangesView, ctx: &egui::Context, h: f32) -> Vec<String> {
    let net = NetClient::new("http://127.0.0.1:9");
    let output = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, h))), ..Default::default() }, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let (mut mode, mut note, mut asked) = (crate::keymap::InputMode::Normal, None, None);
            view.render(ui, &mut RunCx { net: &net, ctx, input_mode: &mut mode, panel: 0, note: &mut note, asked: &mut asked });
        });
    });
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
            egui::Shape::Vec(list) => list.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

#[test]
fn a_big_change_is_drawn_only_where_it_shows_and_moved_through_a_line_at_a_time() {
    let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
    let mut view = ChangesView::default();
    let list = ChangeList { comparison: Comparison::default(), files: vec![file(Group::Untracked, "big.rs", FileStatus::Added, 5000, 0)], offset: 0, total: 1 };
    push(&view.inbox, Reply::List(Comparison::default(), Ok(list)));
    (view.stale, view.loading) = (false, true);
    view.tick(&net, &ctx, false);
    view.ask_file((Group::Untracked, "big.rs".into()), 3, &net, &ctx);
    let lines = (1..=5000).map(|n| line(LineKind::Added, None, Some(n), &format!("let line_{n} = {n};"))).collect();
    let big = FileChanges { comparison: Comparison::default(), path: "big.rs".into(), group: Group::Untracked, context: 3, binary: false, hunks: vec![Hunk { old_start: 0, old_lines: 0, new_start: 1, new_lines: 5000, lines }] };
    push(&view.inbox, Reply::File(Comparison::default(), (Group::Untracked, "big.rs".into()), Ok(big)));
    view.tick(&net, &ctx, false);
    let mut texts = Vec::new();
    for _ in 0..3 {
        texts = drawn(&mut view, &ctx, 600.0);
    }
    let shown = texts.iter().filter(|t| t.starts_with("let line_")).count();
    assert!(shown > 10 && shown < 80, "only the lines on screen are drawn: {shown}");
    // Half a window down: the view and the cursor move together, a line
    // being a stop.
    view.move_to("l:Untracked:big.rs:0:0");
    view.motion(Motion::HalfDown, 1);
    for _ in 0..12 {
        texts = drawn(&mut view, &ctx, 600.0);
    }
    let at: usize = view.cursor_key().unwrap().rsplit(':').next().unwrap().parse().unwrap();
    assert!(at > 5 && at < 40, "half a window of lines: {at}");
    assert!(texts.iter().any(|t| *t == format!("let line_{} = {};", at + 1, at + 1)), "the cursor's line shows");
    view.motion(Motion::Last, 1);
    for _ in 0..12 {
        texts = drawn(&mut view, &ctx, 600.0);
    }
    assert!(texts.iter().any(|t| t == "let line_5000 = 5000;"), "the last line shows");
    assert!(texts.iter().filter(|t| t.starts_with("let line_")).count() < 80, "still only what shows");
}

#[test]
fn a_change_folds_to_one_line_and_opens_again_and_everything_folds_and_unfolds() {
    let (mut view, net, ctx) = view_with_changes();
    let fold = |view: &mut ChangesView, fold: Fold| {
        view.fold(fold);
        view.tick(&net, &ctx, false);
    };
    let offered = |view: &ChangesView, id: &str| view.actions().into_iter().find(|a| a.id == id && a.when.holds(&crate::actions::Situation::default())).map(|a| a.label);
    view.move_to("h:Unstaged:a.rs:0");
    assert_eq!(offered(&view, "nav.fold_toggle").as_deref(), Some("Fold this change"), "said on the change, and on it as a control");
    assert!(view.actions().iter().any(|a| a.id == "nav.fold_toggle" && a.at() == Some(crate::actions::At::Item)));
    fold(&mut view, Fold::Toggle);
    let (l, h) = (Row::Line, Row::Hunk);
    assert_eq!(view.rows(), [Row::Group(Group::Unstaged), Row::File(0), h(0, 0), Row::File(1), h(1, 0), l(1, 0, 0), l(1, 0, 1)], "one line for it");
    assert_eq!(offered(&view, "nav.enter").as_deref(), Some("Unfold this change"), "Enter (a double click) opens it");
    assert_eq!(view.enter(), Step::Done);
    view.tick(&net, &ctx, false);
    assert_eq!(view.rows().len(), 11, "open again");
    assert_eq!(offered(&view, "nav.enter").as_deref(), Some("Open it in the code"), "open: Enter goes to the code");
    // z M folds every file; z R unfolds every file and change.
    fold(&mut view, Fold::Close);
    fold(&mut view, Fold::CloseAll);
    assert_eq!(view.rows(), [Row::Group(Group::Unstaged), Row::File(0), Row::File(1)]);
    assert_eq!(offered(&view, "nav.enter").as_deref(), Some("Unfold a.rs"), "a folded file opens with Enter too");
    assert_eq!(offered(&view, "nav.fold_open_all").as_deref(), Some("Unfold everything"));
    fold(&mut view, Fold::OpenAll);
    assert!(view.folded.is_empty(), "its changes unfolded too");
    arrive(&mut view, &net, &ctx);
    assert_eq!(view.rows().len(), 11, "every file and change open");
}

#[test]
fn more_context_is_offered_where_theres_more_to_show_and_adds_ten_lines_each_side() {
    let (mut view, net, ctx) = view_with_changes();
    let offered = |view: &ChangesView| view.actions().iter().any(|a| a.id == "changes.more_context");
    view.move_to("h:Unstaged:a.rs:0");
    assert!(offered(&view), "a change in an edited file");
    view.move_to("h:Unstaged:b.rs:0");
    assert!(!offered(&view), "a deleted file's change is the whole of it");
    // Asked for: ten more lines on each side, and each change says so.
    view.move_to("h:Unstaged:a.rs:0");
    let more = view.actions().into_iter().find(|a| a.id == "changes.more_context").unwrap();
    assert_eq!(more.label, "Show 10 more lines around its changes");
    let mut parts = (None, None, crate::keymap::InputMode::Normal);
    let mut cx = RunCx { net: &net, ctx: &ctx, input_mode: &mut parts.2, panel: 0, note: &mut parts.0, asked: &mut parts.1 };
    view.run(&more, None, &mut cx);
    assert!(net.sent.lock().unwrap().last().is_some_and(|s| s.contains("a.rs") && s.contains("context=13")), "{:?}", net.sent.lock().unwrap().last());
    let ctx_line = |n: u32| line(LineKind::Context, Some(n), Some(n), "same");
    let mut lines: Vec<DiffLine> = (20..33).map(ctx_line).collect();
    lines.extend([line(LineKind::Removed, Some(33), None, "old"), line(LineKind::Added, None, Some(33), "new")]);
    lines.extend((34..47).map(ctx_line));
    let hunk = Hunk { old_start: 20, old_lines: 27, new_start: 20, new_lines: 27, lines };
    assert_eq!(around_words(&hunk), "13 unchanged lines around");
    push(&view.inbox, Reply::File(Comparison::default(), (Group::Unstaged, "a.rs".into()), Ok(FileChanges { comparison: Comparison::default(), path: "a.rs".into(), group: Group::Unstaged, context: 13, binary: false, hunks: vec![hunk] })));
    view.tick(&net, &ctx, false);
    assert!(offered(&view), "still more of the file to show");
    // The whole file shown: from its first line, stopping short of the lines asked for after it.
    let whole = Hunk { old_start: 1, old_lines: 4, new_start: 1, new_lines: 4, lines: vec![ctx_line(1), line(LineKind::Removed, Some(2), None, "old"), line(LineKind::Added, None, Some(2), "new"), ctx_line(3), ctx_line(4)] };
    assert_eq!(around_words(&whole), "1 unchanged line before, 2 after");
    push(&view.inbox, Reply::File(Comparison::default(), (Group::Unstaged, "a.rs".into()), Ok(FileChanges { comparison: Comparison::default(), path: "a.rs".into(), group: Group::Unstaged, context: 13, binary: false, hunks: vec![whole] })));
    view.tick(&net, &ctx, false);
    view.move_to("h:Unstaged:a.rs:0");
    assert!(!offered(&view), "nothing more to show");
}
