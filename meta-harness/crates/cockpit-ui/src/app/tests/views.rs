//! What each view draws and how views link.

use super::*;

#[test]
fn places_a_server_gave_that_couldnt_be_read_are_shown_in_every_state() {
    for state in [arch_graph::ServerState::Ready, arch_graph::ServerState::Starting, arch_graph::ServerState::Failed] {
        let s = arch_graph::ServerStatus { language: "rust".into(), server: "rust-analyzer".into(), state, reason: Some("why".into()), dropped_positions: 3, ..Default::default() };
        assert!(server_line(&s).contains("3 places it gave couldn't be read"), "{state:?}: {}", server_line(&s));
    }
}

#[test]
fn the_size_note_offers_its_fixes_as_buttons() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Graph);
    let cut_tree = || {
        let mut tree = crate::graph::tests::sample_tree();
        tree.cut = Some(arch_graph::tree::Cut { reason: arch_graph::tree::CutReason::Budget, starts_left_out: 0, shown: 2, asked: 3, hidden: 7 });
        tree
    };
    app.desk.graph.set_tree(cut_tree());
    click_on(&ctx, &mut app, "Show 2 levels");
    assert_eq!(app.desk.graph.nav.depth, Some(2), "only the levels that fit");
    app.desk.graph.set_tree(cut_tree());
    app.desk.graph.select(Some("rust:a::step".into()));
    click_on(&ctx, &mut app, "Make step the root");
    assert_eq!(app.desk.graph.nav.root.as_deref(), Some("rust:a::step"), "started from the card");
}

#[test]
fn graph_draws_server_views_with_loading_and_empty_states() {
    assert!(!std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/samples.rs").exists(), "sample graphs are gone");
    let (ctx, mut app) = app_with_graph();
    let out = frame(&ctx, &mut app, vec![]);
    let text = texts(&out);
    for name in ["run", "step", "save", "log"] {
        assert!(text.contains(name), "node {name} drawn: {text}");
    }
    assert!(text.contains("· 4 cards"), "{text}");
    app.desk.graph = loading();
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Loading the architecture"), "{text}");
    let empty = crate::graph::tests::tree_of(&arch_graph::Graph::default(), None, arch_graph::tree::TreeRequest { view: arch_graph::tree::ViewKind::Calls, ..Default::default() });
    app.desk.graph = showing(empty);
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Nothing to show in this view yet"), "{text}");
}

#[test]
fn graph_and_code_link_both_ways() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph = showing(sample_tree());
    crate::graph::tests::with_detail(&mut app.desk.graph, "rust:a::step", "src/a.rs", 10, 20);
    crate::graph::tests::with_detail(&mut app.desk.graph, "rust:a::save", "src/b.rs", 3, 5);
    app.desk.graph.select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    // Graph -> code: lands on the symbol's span, the code view appears and has focus.
    keys(&ctx, &mut app, "gd");
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.desk.code.current_file.as_str(), app.desk.code.highlight), ("src/a.rs", Some((10, 20))));
    assert_eq!(app.focused_kind(), BufferKind::Code);
    // Code -> graph: a name clicked is what the user is on; the graph follows.
    app.desk.code.name_clicked(ComponentId::new("rust:a::save"), false);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(selected(&app), "rust:a::save");
    // No follow mode: selecting in the graph leaves the code where it is.
    app.desk.graph.select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.code.current_file, "src/a.rs");
    assert!(!app.bound_actions().iter().any(|b| b.action.id == "link.follow"), "no follow action");
    // Gutter marks come from the file's components.
    let mut file_comp = crate::graph::tests::sample_graph().component(&ComponentId::new("rust:a::log")).unwrap().clone();
    file_comp.anchor = Some(CodeAnchor { language: "rust".into(), file: RepoPath::new("src/b.rs").unwrap(), symbol: "a::log".into(), syntax_kind: "function_item".into(), span: Span { start_line: 7, end_line: 9, start_byte: 70, end_byte: 90 }, content_hash: String::new() });
    file_comp.placeholders.push(Span { start_line: 8, end_line: 8, start_byte: 80, end_byte: 87 });
    let mut missing = Relation::new(RelationKind::Calls, ComponentId::new("rust:a::log"), ComponentId::new("rust:a::save"), Provenance::PLANNED);
    missing.order = Some(0);
    let view = arch_graph::FileView { file: "src/b.rs".into(), components: vec![(file_comp, arch_graph::Alignment::Present)], relations: vec![(missing, arch_graph::Alignment::Missing)], stale: false };
    let marks = crate::code::gutter_marks(&view);
    assert!(matches!(marks[&7][..], [crate::code::Mark::Starts(_, _), crate::code::Mark::MissingCall(_, _)]));
    assert!(matches!(marks[&8][..], [crate::code::Mark::Placeholder(_)]));
}

#[test]
fn the_graph_says_when_an_answer_was_slow_or_cut() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Graph);
    let mut tree = crate::graph::tests::sample_tree();
    tree.slow_ms = Some(340);
    tree.cut = Some(arch_graph::tree::Cut { reason: arch_graph::tree::CutReason::Time, starts_left_out: 0, shown: 2, asked: 3, hidden: 7 });
    app.desk.graph.set_tree(tree);
    let shown = texts(&frame(&ctx, &mut app, vec![]));
    assert!(shown.contains("slow: 340 ms"), "{shown}");
    assert!(shown.contains("showing 2 of 3 levels: 7 more (out of time)"), "{shown}");
    // The status line says it too, with the graph out of sight.
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.panels = panels_of(&[BufferKind::Code], 0);
    let shown = texts(&frame(&ctx, &mut app, vec![]));
    assert!(shown.contains("graph cut (out of time), slow: 340 ms"), "{shown}");
}

#[test]
fn find_in_the_code_asks_the_graph_as_its_typed() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Graph);
    let find = app.palette_actions().into_iter().find(|b| b.action.id == "graph.find").expect("the graph offers find");
    app.open_palette(Some(&find));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![egui::Event::Text("run".into())]);
    assert_eq!(app.desk.graph.find_choices(), (Vec::new(), true), "the query went to the graph, which waits for answers");
    // The palette's own list is matched the same way as everywhere.
    let ranked = arch_search::rank("grph fnd", &["Graph · Find in the code", "Code · Reveal"], arch_search::Matching::shared());
    assert_eq!(ranked.first().map(|r| r.index), Some(0));
}

#[test]
fn the_code_view_scrolls_with_a_chat_beside_it() {
    let (ctx, mut app) = full_app();
    app.desk.panels = panels_of(&[BufferKind::Chat, BufferKind::Code, BufferKind::Tree], 1);
    app.desk.code.set_content((1..=500).map(|i| format!("fn line_{i}() {{}}\n")).collect());
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    app.run_move(app.focused_kind(), Move::Motion(Motion::Line(300)), None, &ctx);
    let mut text = String::new();
    for _ in 0..3 {
        text = texts(&frame(&ctx, &mut app, vec![]));
    }
    assert!(text.contains("fn line_300() {}"), "the code scrolled to its cursor");
    frame(&ctx, &mut app, key("G"));
    // A key's move glides a sixth of a second (a frame is a sixtieth here).
    for _ in 0..12 {
        text = texts(&frame(&ctx, &mut app, vec![]));
    }
    assert!(text.contains("fn line_500() {}"));
}

#[test]
fn the_selection_survives_a_view_without_it() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Graph);
    app.desk.graph.select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.spec.target.as_ref().map(|t| t.as_str()), Some("rust:a::step"));
    // A view without it: nothing selected there, but it's remembered (the spec keeps it).
    app.desk.spec.pinned = false;
    // Flipped: what calls run (nothing does), so step isn't there.
    app.desk.graph.navigate(crate::graph::NavChange::Flip);
    let without = crate::graph::tests::tree_of(&crate::graph::tests::sample_graph(), None, app.desk.graph.nav.request());
    app.desk.graph.set_tree(without);
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.desk.graph.selection.clone(), app.desk.graph.current().map(|c| c.0)), (None, Some("rust:a::step".to_owned())));
    assert_eq!(app.desk.spec.target.as_ref().map(|t| t.as_str()), Some("rust:a::step"), "the spec still shows it");
    // Back in a view that has it: selected again.
    app.desk.graph.navigate(crate::graph::NavChange::Flip);
    app.desk.graph.set_tree(sample_tree());
    assert_eq!(selected(&app), "rust:a::step");
    // Enter and ]c with nothing to act on say so.
    app.desk.graph.select(None);
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert!(app.note.as_deref().unwrap_or_default().contains("Select a card first"), "{:?}", app.note);
    focus(&ctx, &mut app, BufferKind::Spec);
    app.desk.spec.comments.clear();
    keys(&ctx, &mut app, "]c");
    assert!(app.note.as_deref().unwrap_or_default().contains("No open review points"), "{:?}", app.note);
}

#[test]
fn indexing_shows_its_state_everywhere_and_can_be_run_from_the_cockpit() {
    let (ctx, mut app) = full_app();
    crate::status::set(app.config.status.clone());
    // Re-index actions: changed files, from scratch, the open file.
    let bound = app.bound_actions();
    let leader = |id: &str| bound.iter().find(|b| b.action.id == id).and_then(|b| b.leader.clone()).map(|l| l.into_iter().collect::<String>());
    assert_eq!((leader("index.changed"), leader("index.rebuild"), leader("index.file")), (Some("Ir".into()), Some("IR".into()), Some("If".into())));
    assert!(bound.iter().any(|b| b.action.id == "index.file" && b.action.label.contains("src/a.rs")), "names the open file");
    // The light: indexing with progress.
    app.desk.net.incoming.lock().unwrap().push(index_event(arch_graph::IndexStatus { state: arch_graph::IndexState::Indexing, done: 3, total: 10, ..Default::default() }));
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("INDEX") && text.contains("indexing 3 of 10"), "{text}");
    // Done, but two files changed since: one word for it everywhere.
    let stale = arch_graph::IndexStatus { stale: vec!["src/a.rs".into(), "src/b.rs".into()], last: Some(arch_graph::IndexSummary { millis: 40, files: 10, parsed: 2, components: 30, relations: 50, unresolved: 1, at: "now".into() }), ..Default::default() };
    app.desk.net.incoming.lock().unwrap().push(index_event(stale));
    focus(&ctx, &mut app, BufferKind::Code);
    let text = texts(&frame(&ctx, &mut app, vec![]));
    let (_, word, _) = crate::status::look("stale");
    assert_eq!(word, "out of date");
    assert!(text.contains("2 out of date"), "the light: {text}");
    assert!(app.desk.code.out_of_date && text.contains("This file changed since it was indexed"), "the code view says so");
    assert!(text.contains("fn run() {"), "and still shows the code (nothing waits)");
    // The spec's code and the graph's cards say it the same way.
    let mut anchored = crate::spec::tests::detail(&ComponentId::new("rust:a::step"), &ComponentId::new("rust:a::save"));
    anchored.component.anchor = Some(CodeAnchor { language: "rust".into(), file: RepoPath::new("src/b.rs").unwrap(), symbol: "a::step".into(), syntax_kind: "function_item".into(), span: Span { start_line: 1, end_line: 2, start_byte: 0, end_byte: 1 }, content_hash: String::new() });
    anchored.code = Some("fn step() {}".into());
    app.desk.spec.detail = Some(anchored);
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.spec.out_of_date);
    focus(&ctx, &mut app, BufferKind::Spec);
    app.desk.spec.move_to("code#Header");
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("out of date") && text.contains("This code changed since it was indexed"), "{text}");
    focus(&ctx, &mut app, BufferKind::Graph);
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("2 out of date") && text.contains("Re-read"), "the graph offers to re-read: {text}");
    // A failed run says so.
    app.desk.net.incoming.lock().unwrap().push(index_event(arch_graph::IndexStatus { state: arch_graph::IndexState::Failed, error: Some("disk".into()), ..Default::default() }));
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("indexing failed"));
    // Every state names a real icon and colour (config is checked).
    assert!(UiConfig::with_overrides("[status.stale]\nword = \"old\"\nicon = \"nope\"\ncolor = \"warn\"\n").unwrap_err().contains("unknown icon"));
}



/// The changes view: `Space b f` shows it; Enter on a change opens its
/// file at the changed line in the code view.
#[test]
fn enter_on_a_change_opens_its_file_at_the_line() {
    use harness_contracts::changes::{ChangeList, ChangedFile, Comparison, FileStatus, Group};
    let (ctx, mut app) = full_app();
    from_outside(&ctx, &mut app, r#"{"op":"keys","keys":"space b f"}"#).unwrap();
    assert_eq!(app.desk.panels.focused_kind(), BufferKind::Changes, "Space b f shows the changes");
    let list = ChangeList { comparison: Comparison::default(), files: vec![ChangedFile { path: "src/a.rs".into(), old_path: None, status: FileStatus::Modified, group: Group::Unstaged, added: 1, removed: 0, binary: false }], offset: 0, total: 1 };
    app.desk.shared.changes.take_list_for_test(list);
    app.desk.share();
    app.desk.changes.move_to("f:Unstaged:src/a.rs");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!((app.desk.panels.focused_kind(), app.desk.code.current_file.as_str()), (BufferKind::Code, "src/a.rs"));
}

#[test]
fn z_d_flips_the_deleted_lines_in_the_code_by_the_key_alone() {
    let (ctx, mut app) = full_app();
    from_outside(&ctx, &mut app, r#"{"op":"keys","keys":"space b v"}"#).unwrap();
    app.desk.code.set_content("let a = 1;\n".into());
    let shown = app.desk.code.show_deleted;
    from_outside(&ctx, &mut app, r#"{"op":"keys","keys":"z d"}"#).unwrap();
    assert_eq!((app.desk.code.show_deleted, app.input_mode), (!shown, InputMode::Normal), "a switch's key flips it: no palette");
    from_outside(&ctx, &mut app, r#"{"op":"keys","keys":"z d"}"#).unwrap();
    assert_eq!(app.desk.code.show_deleted, shown);
}

#[test]
fn g_x_on_a_message_compares_its_turns_changes() {
    use harness_contracts::changes::{Comparison, Point, Timeline, TurnChanges};
    let (ctx, mut app) = full_app();
    crate::chat::tests::conversation_events().into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    from_outside(&ctx, &mut app, r#"{"op":"keys","keys":"space b c"}"#).unwrap();
    from_outside(&ctx, &mut app, r#"{"op":"keys","keys":"g g"}"#).unwrap();
    from_outside(&ctx, &mut app, r#"{"op":"keys","keys":"g x"}"#).unwrap();
    assert_eq!(app.desk.panels.focused_kind(), BufferKind::Chat, "the timeline hasn't seen it: nothing to go to");
    let turn = Comparison { from: Point::Snapshot("1-a".into()), to: Point::Snapshot("2-b".into()) };
    app.desk.shared.changes.take_timeline_for_test(Timeline { windows: vec![], turns: vec![TurnChanges { thread: "t".into(), turn: "turn".into(), comparison: turn.clone(), files: 3, alongside: vec![] }] });
    frame(&ctx, &mut app, Vec::new());
    assert_eq!(app.desk.chat.turn_files.get("turn"), Some(&3), "the chat knows how much each turn changed");
    from_outside(&ctx, &mut app, r#"{"op":"keys","keys":"g x"}"#).unwrap();
    frame(&ctx, &mut app, Vec::new());
    assert_eq!((app.desk.panels.focused_kind(), app.desk.shared.changes.comparison().clone()), (BufferKind::Changes, turn));
}
