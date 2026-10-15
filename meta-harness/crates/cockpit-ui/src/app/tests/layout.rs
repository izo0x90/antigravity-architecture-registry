//! Columns, views per column, saved layout, the agent moving views.

use super::*;

#[test]
fn the_layout_comes_back_as_it_was_saved() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    app.code.move_to("6");
    keys(&ctx, &mut app, "mc");
    app.columns = columns_of(&[BufferKind::Tree, BufferKind::Graph, BufferKind::Chat, BufferKind::Code], 2);
    app.graph.select(Some("rust:a::step".into()));
    app.chat.ui.input_buffer = "half a thought".into();
    let saved = app.saved_ui();
    // Through the server (as JSON) into a cockpit that just opened.
    let json = serde_json::to_value(&saved).unwrap();
    let (ctx2, mut fresh) = full_app();
    fresh.restore_saved(serde_json::from_value(json).unwrap(), &ctx2);
    assert_eq!((fresh.columns.kinds(), fresh.columns.focused()), (saved.columns.kinds(), 2));
    assert_eq!((fresh.marks.clone(), fresh.jumps.clone(), fresh.chat.ui.input_buffer.as_str()), (saved.marks.clone(), saved.jumps.clone(), "half a thought"));
    let graph_place = |s: &SavedUi| s.views.iter().find(|(p, _)| p.kind == BufferKind::Graph).map(|(_, p)| p.clone());
    assert_eq!(fresh.graph.place(), graph_place(&saved).unwrap(), "the graph where it was, the same card selected");
    assert!(matches!(fresh.saved_ui().views.iter().find(|(p, _)| p.kind == BufferKind::Code), Some((_, Place::Code { line: 6, .. }))));
    // A cockpit that has loaded nothing loads the saved file, even one
    // named like the file it starts on.
    let mut opened = CockpitApp::new("http://127.0.0.1:9");
    let mut only_code = saved.clone();
    only_code.views = vec![(Pane::new(BufferKind::Code, 0), Place::Code { file: opened.code.current_file.clone(), line: 3 })];
    opened.restore_saved(only_code, &ctx2);
    assert!(opened.code.is_loading, "the saved file is fetched");
}

#[test]
fn each_column_keeps_its_own_graph_and_the_agent_moves_the_one_named_or_used_last() {
    let (ctx, mut app) = app_with_graph();
    app.graph.select(Some("rust:a::run".into()));
    // A second graph, in a third column, showing its own selection.
    app.columns.set_count(3, 12);
    let second = app.columns.show(2, BufferKind::Graph);
    assert_ne!(second, app.columns.pane(1), "a new graph, not the first one moved");
    app.columns.focus(2);
    frame(&ctx, &mut app, vec![]);
    // It starts where the first one was (its view; its selection once
    // its tree is in).
    let first_nav = app.parked.graphs.get(&0).map(|g| g.nav.clone());
    frame(&ctx, &mut app, vec![]);
    assert_eq!(Some(app.graph.nav.clone()), first_nav);
    app.graph = showing(sample_tree());
    app.graph.select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(selected(&app), "rust:a::step");
    // Focus back on the first: its own selection, untouched.
    app.columns.focus(1);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(selected(&app), "rust:a::run");
    // The agent names the third column: that graph moves, this one doesn't.
    agent_moves(&mut app, "rust:a::save", Some(3));
    frame(&ctx, &mut app, vec![]);
    assert!(app.graph.agent.is_none(), "the focused graph wasn't moved");
    app.columns.focus(2);
    frame(&ctx, &mut app, vec![]);
    assert!(app.graph.agent.is_some(), "the third column's was");
    // Naming none: the graph used last (this one, focused last).
    app.columns.focus(0);
    frame(&ctx, &mut app, vec![]);
    agent_moves(&mut app, "rust:a::run", None);
    frame(&ctx, &mut app, vec![]);
    app.columns.focus(1);
    frame(&ctx, &mut app, vec![]);
    assert!(app.graph.agent.is_none(), "the first graph, not used last, stayed");
    // Saved and put back: both graphs, each where it was.
    let saved = app.saved_ui();
    assert_eq!(saved.views.iter().filter(|(p, _)| p.kind == BufferKind::Graph).count(), 2);
    let (ctx2, mut fresh) = full_app();
    fresh.restore_saved(saved.clone(), &ctx2);
    // (Each one's selection comes back when its tree is in.)
    let nav = |place: Option<Place>| match place {
        Some(Place::Graph { nav, .. }) => Some(nav),
        _ => None,
    };
    for (pane, place) in saved.views.iter().filter(|(p, _)| p.kind == BufferKind::Graph) {
        assert_eq!(nav(fresh.place_of(*pane)), nav(Some(place.clone())), "{pane:?} where it was");
    }
}

#[test]
fn views_are_hidden_by_closing_and_gone_only_when_deleted() {
    let (ctx, mut app) = app_with_graph();
    app.graph.select(Some("rust:a::run".into()));
    app.columns.set_count(3, 12);
    let second = app.columns.show(2, BufferKind::Graph);
    app.columns.focus(2);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    app.graph.select(Some("rust:a::step".into()));
    // Closing its column hides it; the list shows it, and picking it
    // brings it back, as it was, in the focused column.
    app.run_shell(ShellCommand::CloseColumn, None, None, &ctx);
    assert!(app.columns.slot_of(second).is_none() && app.columns.has(second) && app.columns.hidden() > 0);
    let list = app.bound_actions().into_iter().find(|b| b.action.id == "buffer.list").expect("the list");
    assert_eq!(list.key_hint(), "SPC b b");
    let Param::Choice(views) = &list.action.param else { panic!("a choice") };
    let line = views.iter().find(|c| c.value == format!("graph:{}", second.id)).expect("the hidden graph is listed");
    assert_eq!(line.detail.as_deref(), Some("hidden"));
    app.run_shell(ShellCommand::PickView, Some(line.value.clone()), None, &ctx);
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.columns.focused_pane(), selected(&app)), (second, "rust:a::step"), "the same graph, where it was");
    // A new view here: the one there is hidden, the new one starts afresh.
    app.run_shell(ShellCommand::NewView(BufferKind::Graph), None, None, &ctx);
    frame(&ctx, &mut app, vec![]);
    assert!(app.columns.focused_pane() != second && app.columns.has(second));
    assert_eq!(app.graph.nav, crate::graph::Nav::default(), "new, not a copy");
    // Deleting: gone for good; the view hidden last shows instead.
    app.run_shell(ShellCommand::DeleteView, None, None, &ctx);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.columns.focused_pane(), second, "the graph hidden last came back");
    app.run_shell(ShellCommand::DeleteView, None, None, &ctx);
    frame(&ctx, &mut app, vec![]);
    assert!(!app.columns.has(second) && !app.parked.graphs.contains_key(&second.id), "deleted: not anywhere");
    // The chat isn't deleted (there's one of it).
    app.columns.focus(0);
    frame(&ctx, &mut app, vec![]);
    app.run_shell(ShellCommand::DeleteView, None, None, &ctx);
    assert!(app.note.as_deref().unwrap_or_default().contains("There's one chat"), "{:?}", app.note);
    // The first graph is untouched throughout.
    let first = app.columns.panes().iter().copied().find(|p| p.kind == BufferKind::Graph && p.id == 0).expect("the first graph");
    app.show_view(first);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(selected(&app), "rust:a::run");
}

#[test]
fn a_count_before_a_go_to_names_the_column_it_opens_in() {
    let (ctx, mut app) = app_with_graph();
    app.columns.set_count(3, 12);
    crate::graph::tests::with_detail(&mut app.graph, "rust:a::step", "src/a.rs", 10, 20);
    app.graph.select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    // Not this column: it says so, and nothing moves.
    keys(&ctx, &mut app, "2gd");
    assert!(app.note.as_deref().unwrap_or_default().contains("Column 2 is this one"), "{:?}", app.note);
    assert_eq!(app.columns.focused(), 1);
    keys(&ctx, &mut app, "9gd");
    assert!(app.note.as_deref().unwrap_or_default().contains("There are 3 columns"), "{:?}", app.note);
    // The first column: the code opens there (the chat put away).
    keys(&ctx, &mut app, "1gd");
    assert_eq!((app.columns.focused(), app.columns.kind(0), app.code.current_file.as_str()), (0, BufferKind::Code, "src/a.rs"));
    // One past the last: a new column, with a code view of its own.
    app.columns.focus(1);
    frame(&ctx, &mut app, vec![]);
    keys(&ctx, &mut app, "4gd");
    assert_eq!((app.columns.len(), app.columns.focused(), app.columns.kind(3)), (4, 3, BufferKind::Code));
    assert_ne!(app.columns.pane(3), app.columns.pane(0), "two code views");
}

#[test]
fn the_agent_is_talked_to_from_anywhere_without_moving_anything() {
    let (ctx, mut app) = app_with_graph();
    app.graph.select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    let before = (app.columns.clone(), app.graph.place());
    // As a browser sends it: the key down with its text, then up.
    let t = |pressed| egui::Event::Key { key: egui::Key::T, physical_key: Some(egui::Key::T), pressed, repeat: false, modifiers: Default::default() };
    frame(&ctx, &mut app, vec![t(true), egui::Event::Text("t".into())]);
    frame(&ctx, &mut app, vec![t(false)]);
    frame(&ctx, &mut app, vec![]);
    assert!(app.talk.is_some() && matches!(app.input_mode, InputMode::Insert { .. }), "t opens the box, the keyboard in it");
    frame(&ctx, &mut app, key("what calls step?"));
    assert_eq!(app.talk.as_ref().unwrap().text, "what calls step?", "typing goes into it, not to the keys");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert!(app.talk.as_ref().unwrap().text.is_empty(), "sent");
    assert!(matches!(app.chat.session_status(), SessionStatus::Failed(e) if e.contains("No harness instance")), "it went to the chat (no harness here)");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert!(app.talk.is_none() && app.input_mode == InputMode::Normal);
    assert_eq!((app.columns.clone(), app.graph.place()), before, "nothing on screen moved");
}
