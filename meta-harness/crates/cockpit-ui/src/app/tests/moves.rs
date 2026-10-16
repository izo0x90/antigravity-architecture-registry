//! The same moves in every view: motions, search, marks, hints, jumps, go-tos.

use super::*;

#[test]
fn search_marks_counts_and_ends_work_through_the_keys() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    // Counts: 2l moves twice (the first move lands on the root).
    frame(&ctx, &mut app, key("2"));
    assert_eq!(app.pending_count, Some(2));
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("NORMAL 2"), "the count shows");
    frame(&ctx, &mut app, key("l"));
    assert_eq!((selected(&app), app.pending_count), ("rust:a::step", None));
    // ... and repeats a view's own actions: 2+ adds two levels, as one jump.
    keys(&ctx, &mut app, "2+");
    assert_eq!(app.desk.graph.nav.depth, Some(5));
    ctrl(&ctx, &mut app, egui::Key::O);
    assert_eq!(app.desk.graph.nav.depth, Some(3), "back undoes both");
    app.desk.graph.set_tree(sample_tree());
    // G / gg: last and first card in tree order, as jumps.
    frame(&ctx, &mut app, key("G"));
    assert_eq!(selected(&app), "rust:a::save");
    keys(&ctx, &mut app, "gg");
    assert_eq!(selected(&app), "rust:a::run");
    ctrl(&ctx, &mut app, egui::Key::O);
    assert_eq!(selected(&app), "rust:a::save", "ctrl+o returns to where the jump left");
    ctrl(&ctx, &mut app, egui::Key::I);
    assert_eq!(selected(&app), "rust:a::run", "ctrl+i goes forward again");
    // / searches incrementally; Enter jumps; Escape gives up.
    frame(&ctx, &mut app, key("/"));
    assert_eq!(app.input_mode, InputMode::Capture { slot: 1 });
    frame(&ctx, &mut app, key("s"));
    assert_eq!(selected(&app), "rust:a::step", "first match shows while typing");
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("/s| 2 matches"));
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!((selected(&app), app.input_mode), ("rust:a::step", InputMode::Normal));
    frame(&ctx, &mut app, key("n"));
    assert_eq!(selected(&app), "rust:a::save");
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("/s 2/2"));
    frame(&ctx, &mut app, key("n"));
    assert_eq!(selected(&app), "rust:a::step", "wraps");
    frame(&ctx, &mut app, key("N"));
    assert_eq!(selected(&app), "rust:a::save");
    ctrl(&ctx, &mut app, egui::Key::O);
    assert_eq!(selected(&app), "rust:a::step", "search jumps are in the jump list");
    keys(&ctx, &mut app, "/lo");
    assert_eq!(selected(&app), "rust:a::log");
    frame(&ctx, &mut app, press(egui::Key::Backspace, false));
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!((selected(&app), app.input_mode), ("rust:a::step", InputMode::Normal), "Escape restores the selection");
    // Marks: m<letter> sets, '<letter> jumps back (through the jump list).
    keys(&ctx, &mut app, "ma");
    frame(&ctx, &mut app, key("G"));
    assert_eq!(selected(&app), "rust:a::save");
    keys(&ctx, &mut app, "'a");
    assert_eq!(selected(&app), "rust:a::step");
    ctrl(&ctx, &mut app, egui::Key::O);
    assert_eq!(selected(&app), "rust:a::save");
    keys(&ctx, &mut app, "'z");
    assert_eq!((selected(&app), app.input_mode), ("rust:a::save", InputMode::Normal), "an unset mark goes nowhere");
    assert!(app.note.as_deref().unwrap_or_default().contains("No mark z"), "and says so");
    // Keys come from config.
    app.config = UiConfig::with_overrides("[keymap.bind]\n\"nav.last\" = { keys = [\"E\"] }\n").unwrap();
    keys(&ctx, &mut app, "gg");
    frame(&ctx, &mut app, key("E"));
    assert_eq!(selected(&app), "rust:a::save");
}

#[test]
fn the_jump_list_crosses_views_and_slots() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph = showing(sample_tree());
    crate::graph::tests::with_detail(&mut app.desk.graph, "rust:a::step", "src/a.rs", 10, 20);
    app.desk.graph.select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    keys(&ctx, &mut app, "gd");
    assert_eq!((app.focused_kind(), app.desk.columns.focused()), (BufferKind::Code, 0), "the code opened beside the graph, with focus");
    app.desk.code.set_content((1..=40).map(|i| format!("line {i}\n")).collect());
    keys(&ctx, &mut app, "G");
    assert_eq!(app.desk.code.target_line, Some(40));
    ctrl(&ctx, &mut app, egui::Key::O);
    assert_eq!(app.desk.code.target_line, Some(10), "back within the code");
    ctrl(&ctx, &mut app, egui::Key::O);
    assert_eq!((app.desk.columns.focused(), app.focused_kind(), selected(&app)), (1, BufferKind::Graph, "rust:a::step"), "back to the graph's slot, focused, where it was");
    ctrl(&ctx, &mut app, egui::Key::I);
    assert_eq!((app.focused_kind(), app.desk.code.target_line), (BufferKind::Code, Some(10)));
    // Pointer jumps in the graph are in it too.
    app.desk.graph.select(Some("rust:a::step".into()));
    app.desk.graph.jumped = Some(app.desk.graph.place());
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.jumps.can_back());
    // Nothing left to go back to: says so.
    let mut fresh = CockpitApp::new("http://127.0.0.1:9");
    fresh.initialized = true;
    ctrl(&ctx, &mut fresh, egui::Key::O);
    assert!(fresh.note.as_deref().unwrap_or_default().contains("Nothing to go back to"));
}

#[test]
fn the_same_moves_work_in_every_view() {
    let (ctx, mut app) = full_app();
    // A longer file, so half pages mean something.
    app.desk.code.set_content((1..=300).map(|i| format!("line {i}\n")).collect());
    app.desk.code.target_line = Some(1);
    for _ in 0..3 {
        app.desk.chat.process_event(harness_contracts::ClientEvent::ThreadClosed { thread_id: harness_contracts::ThreadId::new("none") });
    }
    for kind in [BufferKind::Tree, BufferKind::Code, BufferKind::Spec, BufferKind::Queue, BufferKind::Chat] {
        focus(&ctx, &mut app, kind);
        frame(&ctx, &mut app, key("G"));
        let last = cursor(&app);
        keys(&ctx, &mut app, "gg");
        let first = cursor(&app);
        assert_ne!(first, last, "{kind:?}: gg and G go to the two ends");
        // Where the cursor is in the view's own order (a chat link counts as its message).
        let order = |app: &CockpitApp| {
            let items = app.desk.view(app.focused_kind()).items();
            let c = cursor(app)?;
            items.iter().position(|i| i.key == c).or_else(|| c.split_once(':').and_then(|(head, _)| items.iter().position(|i| i.key == head)))
        };
        let start = order(&app).unwrap_or(0);
        frame(&ctx, &mut app, key("j"));
        let one = order(&app).unwrap();
        assert!(one > start || kind == BufferKind::Chat && one == start, "{kind:?}: j moves down ({start} -> {one})");
        keys(&ctx, &mut app, "2j");
        let three = order(&app).unwrap();
        assert!(three > one, "{kind:?}: 2j moves on ({one} -> {three})");
        for _ in 0..2 {
            frame(&ctx, &mut app, vec![]);
        }
        ctrl(&ctx, &mut app, egui::Key::D);
        let half = order(&app).unwrap();
        assert!(half > three, "{kind:?}: ctrl+d moves half a page ({three} -> {half})");
        ctrl(&ctx, &mut app, egui::Key::U);
        assert!(order(&app).unwrap() < half, "{kind:?}: ctrl+u comes back up");
    }
    // The graph: hjkl, half pages pan.
    focus(&ctx, &mut app, BufferKind::Graph);
    frame(&ctx, &mut app, key("l"));
    assert_eq!(selected(&app), "rust:a::run");
    frame(&ctx, &mut app, key("l"));
    frame(&ctx, &mut app, key("j"));
    assert_eq!(selected(&app), "rust:a::log");
    frame(&ctx, &mut app, key("k"));
    frame(&ctx, &mut app, key("h"));
    assert_eq!(selected(&app), "rust:a::run");
    keys(&ctx, &mut app, "G");
    assert_eq!(selected(&app), "rust:a::save");
}

#[test]
fn the_graph_keys_flip_change_depth_switch_view_go_in_and_back() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, key("r"));
    assert_eq!(app.desk.graph.nav.way, arch_graph::tree::Way::Up, "r flips");
    frame(&ctx, &mut app, key("r"));
    frame(&ctx, &mut app, key("+"));
    assert_eq!(app.desk.graph.nav.depth, Some(4), "+ shows a level more");
    frame(&ctx, &mut app, key("-"));
    assert_eq!(app.desk.graph.nav.depth, Some(3));
    app.desk.graph.select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(app.desk.graph.nav.root.as_deref(), Some("rust:a::step"), "Enter goes in");
    frame(&ctx, &mut app, press(egui::Key::Backspace, false));
    assert_eq!(app.desk.graph.nav.root.as_deref(), Some("rust:a::run"), "Backspace comes back");
    // v: the views to pick from.
    frame(&ctx, &mut app, key("v"));
    assert_eq!(app.input_mode, InputMode::Palette, "v offers the views");
    let switch = app.bound_actions().into_iter().find(|b| b.action.id == "graph.view").unwrap();
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    app.invoke(Invocation { action: switch.action, origin: switch.origin, value: Some("data_flow".into()) }, &ctx);
    assert_eq!(app.desk.graph.nav.view, arch_graph::tree::ViewKind::DataFlow, "the view picked");
}

#[test]
fn f_labels_what_is_on_screen_in_every_view() {
    let (ctx, mut app) = full_app();
    app.desk.graph.set_tree(sample_tree());
    // Something in the inbox, to label.
    let kind = harness_contracts::NotificationKind::TurnFinished;
    let notification = harness_contracts::Notification { id: harness_contracts::NotificationId::new("turn:1"), server: None, level: kind.level(), kind, slot: None, thread: None, request: None, title: "done".into(), detail: None, at_ms: 0, state: harness_contracts::NotificationState::Open };
    app.desk.chat.process_event(harness_contracts::ClientEvent::NotificationUpdated { notification });
    // A changed file, to label.
    use harness_contracts::changes::{ChangeList, ChangedFile, Comparison, FileStatus, Group};
    let changed = ChangedFile { path: "src/a.rs".into(), old_path: None, status: FileStatus::Modified, group: Group::Unstaged, added: 1, removed: 0, binary: false };
    app.desk.changes.take_list_for_test(ChangeList { comparison: Comparison::default(), files: vec![changed], offset: 0, total: 1 });
    for kind in BufferKind::ALL {
        focus(&ctx, &mut app, kind);
        let spots = app.desk.view(kind).spots();
        assert!(!spots.is_empty(), "{kind:?}: something drawn to label");
        frame(&ctx, &mut app, key("f"));
        let Some(Capture::Hint(hint)) = &app.capture else { panic!("{kind:?}: hint mode") };
        assert_eq!(hint.labels.len(), spots.len());
        let (label, spot) = hint.labels.last().unwrap().clone();
        let text = texts(&frame(&ctx, &mut app, vec![]));
        assert!(text.contains(&label.to_uppercase()), "{kind:?}: the label is drawn");
        keys(&ctx, &mut app, &label);
        assert_eq!(app.input_mode, InputMode::Normal);
        let at = cursor(&app).unwrap_or_default();
        assert!(spot.key == at || spot.key.starts_with(&format!("{at}:")), "{kind:?}: the cursor went to {} (at {at})", spot.key);
    }
    // F opens: in the graph, it goes into the card.
    focus(&ctx, &mut app, BufferKind::Graph);
    frame(&ctx, &mut app, key("F"));
    let Some(Capture::Hint(hint)) = &app.capture else { panic!() };
    let (label, spot) = hint.labels.last().unwrap().clone();
    keys(&ctx, &mut app, &label);
    assert_eq!((selected(&app), app.desk.graph.nav.root.as_deref()), (spot.key.as_str(), Some(spot.key.as_str())));
    // A key no label starts with gives up; Escape too.
    frame(&ctx, &mut app, key("f"));
    frame(&ctx, &mut app, key("z"));
    assert!(app.capture.is_none() && app.input_mode == InputMode::Normal);
}

#[test]
fn slash_searches_every_view() {
    let (ctx, mut app) = full_app();
    for (kind, query, lands) in [
        (BufferKind::Graph, "save", "rust:a::save"),
        (BufferKind::Tree, "log", "rust:a::log"),
        (BufferKind::Code, "log(", "4"),
        (BufferKind::Spec, "write", "logic#Step(1)"),
        (BufferKind::Queue, "c", "2"),
        (BufferKind::Chat, "parser", "0"),
    ] {
        focus(&ctx, &mut app, kind);
        frame(&ctx, &mut app, key("/"));
        keys(&ctx, &mut app, query);
        frame(&ctx, &mut app, press(egui::Key::Enter, false));
        assert_eq!(cursor(&app).as_deref(), Some(lands), "{kind:?}: /{query}");
        assert!(app.desk.searches.contains_key(&kind));
    }
    // n / N step through a view's matches.
    focus(&ctx, &mut app, BufferKind::Code);
    keys(&ctx, &mut app, "/step");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(cursor(&app).as_deref(), Some("3"));
    frame(&ctx, &mut app, key("n"));
    assert_eq!(cursor(&app).as_deref(), Some("6"));
    frame(&ctx, &mut app, key("N"));
    assert_eq!(cursor(&app).as_deref(), Some("3"));
}

#[test]
fn enter_goes_in_and_backspace_comes_out_in_every_view() {
    let (ctx, mut app) = full_app();
    // A key, then a frame for what it asked of other views.
    let enter = |ctx: &egui::Context, app: &mut CockpitApp| {
        frame(ctx, app, press(egui::Key::Enter, false));
        frame(ctx, app, vec![])
    };
    let leave = |ctx: &egui::Context, app: &mut CockpitApp| {
        frame(ctx, app, press(egui::Key::Backspace, false));
        frame(ctx, app, vec![])
    };
    // Graph: go in, back.
    focus(&ctx, &mut app, BufferKind::Graph);
    app.desk.graph.select(Some("rust:a::step".into()));
    enter(&ctx, &mut app);
    assert_eq!(app.desk.graph.nav.root.as_deref(), Some("rust:a::step"));
    leave(&ctx, &mut app);
    assert_eq!(app.desk.graph.nav.root.as_deref(), Some("rust:a::run"), "back where going in started");
    app.desk.graph.set_tree(sample_tree());
    // Tree: Enter selects in the graph (focus stays).
    focus(&ctx, &mut app, BufferKind::Tree);
    app.desk.tree.move_to("rust:a::save");
    enter(&ctx, &mut app);
    assert_eq!((selected(&app), app.focused_kind()), ("rust:a::save", BufferKind::Tree));
    leave(&ctx, &mut app);
    assert_eq!(cursor(&app).as_deref(), Some("rust:a"), "Backspace: up to what holds it");
    // Code: Enter on a call goes to its definition.
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code.move_to("3");
    enter(&ctx, &mut app);
    assert_eq!(app.desk.code.target_line, Some(6), "step's definition");
    leave(&ctx, &mut app);
    // Spec: Enter on a section title folds it.
    focus(&ctx, &mut app, BufferKind::Spec);
    keys(&ctx, &mut app, "gg");
    enter(&ctx, &mut app);
    assert!(app.desk.spec.folded.contains(&crate::spec::Section::Overview));
    // Queue: Enter shows a failed job's findings.
    focus(&ctx, &mut app, BufferKind::Queue);
    app.desk.queue.move_to("2");
    enter(&ctx, &mut app);
    assert!(app.desk.queue.expanded.contains(&ComponentId::new("rust:c")));
    leave(&ctx, &mut app);
    assert!(app.desk.queue.expanded.is_empty());
    // Chat: Enter on a message with a file opens it in the code view.
    focus(&ctx, &mut app, BufferKind::Chat);
    app.desk.chat.move_to("2");
    enter(&ctx, &mut app);
    assert_eq!((app.focused_kind(), app.desk.code.current_file.as_str()), (BufferKind::Code, "src/parse.rs"));
    // Nothing to do: it says why.
    focus(&ctx, &mut app, BufferKind::Chat);
    app.desk.chat.move_to("0");
    enter(&ctx, &mut app);
    assert!(app.note.as_deref().unwrap_or_default().contains("No file"));
}

#[test]
fn z_keys_fold_in_every_view_that_folds() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code.move_to("3");
    keys(&ctx, &mut app, "zc");
    assert_eq!(app.desk.code.visible_lines().len(), 6);
    keys(&ctx, &mut app, "zo");
    assert_eq!(app.desk.code.visible_lines().len(), 9);
    keys(&ctx, &mut app, "zM");
    assert_eq!(app.desk.code.visible_lines().len(), 4);
    keys(&ctx, &mut app, "zR");
    focus(&ctx, &mut app, BufferKind::Tree);
    keys(&ctx, &mut app, "zM");
    assert_eq!(app.desk.tree.rows().len(), 2);
    keys(&ctx, &mut app, "zR");
    assert_eq!(app.desk.tree.rows().len(), 6);
    focus(&ctx, &mut app, BufferKind::Spec);
    keys(&ctx, &mut app, "zM");
    assert_eq!(app.desk.spec.folded.len(), crate::spec::Section::ALL.len());
    keys(&ctx, &mut app, "za");
    assert_eq!(app.desk.spec.folded.len(), crate::spec::Section::ALL.len() - 1);
}

#[test]
fn marks_work_across_views() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code.move_to("6");
    keys(&ctx, &mut app, "mc");
    app.desk.columns.focus(2);
    focus(&ctx, &mut app, BufferKind::Tree);
    app.desk.tree.move_to("rust:b");
    keys(&ctx, &mut app, "mt");
    app.desk.columns.focus(0);
    focus(&ctx, &mut app, BufferKind::Graph);
    keys(&ctx, &mut app, "'c");
    assert_eq!((app.desk.columns.focused(), app.focused_kind(), app.desk.code.target_line), (1, BufferKind::Code, Some(6)), "back to the code, in its slot");
    keys(&ctx, &mut app, "'t");
    assert_eq!((app.desk.columns.focused(), app.desk.tree.cursor.as_ref().map(|c| c.as_str())), (2, Some("rust:b")));
    ctrl(&ctx, &mut app, egui::Key::O);
    assert_eq!(app.focused_kind(), BufferKind::Code, "a mark jump is in the jump list");
    // The popup after ' lists them.
    frame_at(&ctx, &mut app, key("'"), 50.0);
    // (egui lays a new popup out unseen first, then draws it.)
    frame_at(&ctx, &mut app, vec![], 51.0);
    let text = texts(&frame_at(&ctx, &mut app, vec![], 51.05));
    assert!(text.contains("code · src/a.rs:6") && text.contains("tree · b"), "{text}");
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 51.1);
    assert_eq!(app.input_mode, InputMode::Normal);
}

#[test]
fn go_to_keys_work_from_every_view() {
    let (ctx, mut app) = full_app();
    // Outline -> graph: g m shows it and moves focus there.
    focus(&ctx, &mut app, BufferKind::Tree);
    app.desk.tree.move_to("rust:a::step");
    keys(&ctx, &mut app, "gm");
    assert_eq!((app.focused_kind(), selected(&app)), (BufferKind::Graph, "rust:a::step"));
    // Graph -> outline: g t.
    app.desk.graph.select(Some("rust:a::log".into()));
    keys(&ctx, &mut app, "gt");
    assert_eq!((app.focused_kind(), app.desk.tree.cursor.as_ref().map(|c| c.as_str())), (BufferKind::Tree, Some("rust:a::log")));
    // Queue -> spec: g s (a pinned spec shows it anyway).
    focus(&ctx, &mut app, BufferKind::Queue);
    app.desk.queue.move_to("1");
    keys(&ctx, &mut app, "gs");
    assert_eq!((app.focused_kind(), app.desk.spec.target.as_ref().map(|c| c.as_str())), (BufferKind::Spec, Some("rust:a")));
    // The queue's own keys: o shows the slot running the job's session.
    let mut worker = crate::chat::tests::lead(Some("t1"));
    (worker.id, worker.role) = (harness_contracts::SlotId::new("worker-1"), "worker".into());
    app.desk.chat.process_event(harness_contracts::ClientEvent::SlotUpdated { slot: worker });
    focus(&ctx, &mut app, BufferKind::Queue);
    app.desk.queue.on_event(&arch_graph::ArchEvent::QueueUpdated { snapshot: Box::new(crate::queue::tests::snapshot(arch_graph::QueueStatus::Running)) });
    keys(&ctx, &mut app, "gg");
    frame(&ctx, &mut app, key("o"));
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.focused_kind(), app.desk.chat.active_thread().as_ref().map(|t| t.as_str())), (BufferKind::Chat, Some("t1")));
    assert_eq!(app.desk.chat.ui.slot.as_str(), "worker-1");
    // Graph -> queue: g q finds the component's job, or says there isn't one.
    focus(&ctx, &mut app, BufferKind::Graph);
    app.desk.graph.select(Some("rust:a::run".into()));
    keys(&ctx, &mut app, "gq");
    assert!(app.note.as_deref().unwrap_or_default().contains("no job"), "{:?}", app.note);
    // Code: g r on a definition lists every use of it; one: straight to it.
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code.move_to("6");
    let mut step = crate::goto::tests::detail("rust:a::step", vec![], vec![call("rust:a::run", "rust:a::step", 0, 3)]);
    step.references = vec![arch_graph::code::Reference { file: "src/a.rs".into(), line: 3, start: 0, end: 4, kind: "call".into(), within: Some(arch_graph::ComponentId::new("rust:a::run")), evidence: "name".into(), stale: false }];
    app.desk.details.insert(step, &app.desk.graph.nav.source);
    keys(&ctx, &mut app, "gr");
    assert_eq!(app.desk.code.target_line, Some(3), "at the use in run");
    // A detail not here yet: the go-to waits for it.
    app.desk.code.move_to("2");
    keys(&ctx, &mut app, "gc");
    assert!(app.desk.pending_goto.is_some(), "waiting for run's relations");
    app.desk.details.insert(crate::goto::tests::detail("rust:a::run", vec![call("rust:a::run", "rust:a::step", 0, 3), call("rust:a::run", "rust:x::log", 1, 4)], vec![]), &app.desk.graph.nav.source);
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.pending_goto.is_none() && app.input_mode == InputMode::Palette, "two callees: pick one");
    let pick = app.bound_actions().into_iter().find(|b| b.action.id == "goto.pick").unwrap();
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    app.invoke(Invocation { action: pick.action, origin: pick.origin, value: Some("rust:a::step".into()) }, &ctx);
    assert_eq!(app.desk.code.target_line, Some(6), "picked: step's definition");
    // Spec: a relation's g s (or Enter) follows it.
    let step = ComponentId::new("rust:a::step");
    app.desk.spec.show(step.clone());
    app.desk.spec.detail = Some(crate::spec::tests::detail(&step, &ComponentId::new("rust:a::save")));
    focus(&ctx, &mut app, BufferKind::Spec);
    app.desk.spec.move_to(&crate::spec::Entry { section: crate::spec::Section::Relations, item: crate::nav::SpecItem::Relation { kind: RelationKind::Calls, other: ComponentId::new("rust:a::save"), outgoing: true } }.key());
    keys(&ctx, &mut app, "gs");
    assert_eq!(app.desk.spec.target.as_ref().map(|c| c.as_str()), Some("rust:a::save"));
    // Nothing under the cursor: says so.
    let (ctx, mut empty) = app_with_graph();
    frame(&ctx, &mut empty, vec![]);
    keys(&ctx, &mut empty, "gd");
    assert!(empty.note.as_deref().unwrap_or_default().contains("Nothing under the cursor"));
}

#[test]
fn spec_keys_walk_items_and_review_them() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Spec);
    keys(&ctx, &mut app, "J");
    assert_eq!(app.desk.spec.current, crate::spec::Section::Signature);
    keys(&ctx, &mut app, "]]");
    assert_eq!(app.desk.spec.current, crate::spec::Section::Logic);
    keys(&ctx, &mut app, "j");
    assert_eq!(app.desk.spec.entry().unwrap().item, crate::nav::SpecItem::Step(0));
    frame(&ctx, &mut app, key("c"));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.input_mode, InputMode::Insert { slot: 1 }, "c: the composer has the keyboard");
    assert!(app.desk.spec.composer.is_some());
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!(app.input_mode, InputMode::Normal);
}

#[test]
fn hint_labels_sit_beside_their_items() {
    let slot = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 400.0));
    let size = egui::vec2(24.0, 18.0);
    let room_left = hint_chip(egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(80.0, 20.0)), size, slot, &[]);
    assert!(room_left.right() <= 100.0, "left of it: {room_left:?}");
    let at_edge = hint_chip(egui::Rect::from_min_size(egui::pos2(4.0, 50.0), egui::vec2(80.0, 20.0)), size, slot, &[]);
    assert!(at_edge.left() >= 84.0, "right of it: {at_edge:?}");
    let full = hint_chip(egui::Rect::from_min_size(egui::pos2(4.0, 50.0), egui::vec2(394.0, 20.0)), size, slot, &[]);
    assert!(full.right() <= 398.0 && full.left() > 300.0, "inside its right end: {full:?}");
    // Never on a label already placed: the next spot over.
    let crowded = hint_chip(egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(80.0, 20.0)), size, slot, &[room_left]);
    assert!(!crowded.intersects(room_left) && crowded.left() >= 180.0, "{crowded:?}");
}

#[test]
fn searches_and_restores_put_things_back_exactly() {
    let (ctx, mut app) = full_app();
    // A graph search that matches nothing, from no selection, ends with no selection.
    focus(&ctx, &mut app, BufferKind::Graph);
    app.desk.graph.select(None);
    keys(&ctx, &mut app, "/sa");
    assert_eq!(selected(&app), "rust:a::save", "a preview");
    keys(&ctx, &mut app, "zz");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(selected(&app), "", "nothing selected");
    // An empty search forgets the last one (no stale position in the bar).
    focus(&ctx, &mut app, BufferKind::Tree);
    keys(&ctx, &mut app, "/step");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert!(app.search_status().is_some());
    frame(&ctx, &mut app, key("/"));
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert!(app.search_status().is_none() && !app.desk.searches.contains_key(&BufferKind::Tree));
    // The spec comes back to the item, not just its old position.
    focus(&ctx, &mut app, BufferKind::Spec);
    app.desk.spec.move_to("logic#Step(1)");
    let place = app.desk.spec.place();
    app.desk.spec.folded.insert(crate::spec::Section::Overview);
    app.desk.spec.motion(Motion::First, 1);
    app.desk.spec.restore(&place);
    assert_eq!(app.desk.spec.entry().unwrap().item, crate::nav::SpecItem::Step(1));
    assert_eq!(app.desk.spec.current, crate::spec::Section::Logic);
    // One column and back: the slots as they were.
    app.desk.columns = columns_of(&[BufferKind::Code, BufferKind::Graph, BufferKind::Queue], 2);
    app.run_shell(ShellCommand::SetLayout(1), None, None, &ctx);
    assert_eq!(app.desk.columns.kind(0), BufferKind::Queue);
    app.run_shell(ShellCommand::SetLayout(3), None, None, &ctx);
    assert_eq!((app.desk.columns.kinds(), app.desk.columns.focused()), (vec![BufferKind::Code, BufferKind::Graph, BufferKind::Queue], 2));
}
