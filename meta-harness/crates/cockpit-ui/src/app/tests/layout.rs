//! Panels, views per panel, saved layout, the agent moving views.

use super::*;

#[test]
fn the_layout_comes_back_as_it_was_saved() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code.move_to("6");
    keys(&ctx, &mut app, "mc");
    app.desk.panels = panels_of(&[BufferKind::Tree, BufferKind::Graph, BufferKind::Chat, BufferKind::Code], 2);
    app.desk.graph.select(Some("rust:a::step".into()));
    app.desk.chat.ui.input_buffer = "half a thought".into();
    let saved = app.saved_ui();
    // Through the server (as JSON) into a cockpit that just opened.
    let json = serde_json::to_value(&saved).unwrap();
    let (_, mut fresh) = full_app();
    fresh.restore_saved(serde_json::from_value(json).unwrap());
    assert_eq!((fresh.desk.panels.kinds(), fresh.desk.panels.focused()), (saved.panels.kinds(), 2));
    assert_eq!((fresh.desk.marks.clone(), fresh.desk.jumps.clone(), fresh.desk.chat.ui.input_buffer.as_str()), (saved.marks.clone(), saved.jumps.clone(), "half a thought"));
    let graph_place = |s: &SavedUi| s.views.iter().find(|(p, _)| p.kind == BufferKind::Graph).map(|(_, p)| p.clone());
    assert_eq!(fresh.desk.graph.place(), graph_place(&saved).unwrap(), "the graph where it was, the same card selected");
    assert!(matches!(fresh.saved_ui().views.iter().find(|(p, _)| p.kind == BufferKind::Code), Some((_, Place::Code { line: 6, .. }))));
    // A cockpit that has loaded nothing loads the saved file, even one
    // named like the file it starts on.
    let mut opened = CockpitApp::new("http://127.0.0.1:9");
    let mut only_code = saved.clone();
    only_code.views = vec![(Buffer::new(BufferKind::Code, 0), Place::Code { file: opened.desk.code.current_file.clone(), line: 3 })];
    opened.restore_saved(only_code);
    assert!(opened.desk.code.is_loading, "the saved file is fetched");
}

#[test]
fn each_column_keeps_its_own_graph_and_the_agent_moves_the_one_named_or_used_last() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph.select(Some("rust:a::run".into()));
    // A second graph, in a third panel, showing its own selection.
    app.desk.panels.set_count(3, 12);
    let second = app.desk.panels.show(2, BufferKind::Graph);
    assert_ne!(second, app.desk.panels.buffer(1), "a new graph, not the first one moved");
    app.desk.panels.focus(2);
    frame(&ctx, &mut app, vec![]);
    // It starts where the first one was (its view; its selection once
    // its tree is in).
    let first_nav = app.desk.parked.graphs.get(&0).map(|g| g.nav.clone());
    frame(&ctx, &mut app, vec![]);
    assert_eq!(Some(app.desk.graph.nav.clone()), first_nav);
    app.desk.graph = showing(sample_tree());
    app.desk.graph.select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(selected(&app), "rust:a::step");
    // Focus back on the first: its own selection, untouched.
    app.desk.panels.focus(1);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(selected(&app), "rust:a::run");
    // The agent names the third panel: that graph moves, this one doesn't.
    agent_moves(&mut app, "rust:a::save", Some(3));
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.graph.agent.is_none(), "the focused graph wasn't moved");
    app.desk.panels.focus(2);
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.graph.agent.is_some(), "the third panel's was");
    // Naming none: the graph used last (this one, focused last).
    app.desk.panels.focus(0);
    frame(&ctx, &mut app, vec![]);
    agent_moves(&mut app, "rust:a::run", None);
    frame(&ctx, &mut app, vec![]);
    app.desk.panels.focus(1);
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.graph.agent.is_none(), "the first graph, not used last, stayed");
    // Saved and put back: both graphs, each where it was.
    let saved = app.saved_ui();
    assert_eq!(saved.views.iter().filter(|(p, _)| p.kind == BufferKind::Graph).count(), 2);
    let (_, mut fresh) = full_app();
    fresh.restore_saved(saved.clone());
    // (Each one's selection comes back when its tree is in.)
    let nav = |place: Option<Place>| match place {
        Some(Place::Graph { nav, .. }) => Some(nav),
        _ => None,
    };
    for (buffer, place) in saved.views.iter().filter(|(p, _)| p.kind == BufferKind::Graph) {
        assert_eq!(nav(fresh.place_of(*buffer)), nav(Some(place.clone())), "{buffer:?} where it was");
    }
}

#[test]
fn views_are_hidden_by_closing_and_gone_only_when_deleted() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph.select(Some("rust:a::run".into()));
    app.desk.panels.set_count(3, 12);
    let second = app.desk.panels.show(2, BufferKind::Graph);
    app.desk.panels.focus(2);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    app.desk.graph.select(Some("rust:a::step".into()));
    // Closing its panel hides it; the list shows it, and picking it
    // brings it back, as it was, in the focused panel.
    app.run_shell(ShellCommand::ClosePanel, None, None, &ctx);
    assert!(app.desk.panels.panel_of(second).is_none() && app.desk.panels.has(second) && app.desk.panels.hidden() > 0);
    let list = app.bound_actions().into_iter().find(|b| b.action.id == "buffer.list").expect("the list");
    assert_eq!(list.key_hint(), "Space b b");
    let Param::Choice(views) = &list.action.param else { panic!("a choice") };
    let line = views.iter().find(|c| c.value == format!("graph:{}", second.id)).expect("the hidden graph is listed");
    assert_eq!(line.detail.as_deref(), Some("hidden"));
    app.run_shell(ShellCommand::PickView, Some(line.value.clone()), None, &ctx);
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.desk.panels.focused_buffer(), selected(&app)), (second, "rust:a::step"), "the same graph, where it was");
    // A new view here: the one there is hidden, the new one starts afresh.
    app.run_shell(ShellCommand::NewView(BufferKind::Graph), None, None, &ctx);
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.panels.focused_buffer() != second && app.desk.panels.has(second));
    assert_eq!(app.desk.graph.nav, crate::graph::Nav::default(), "new, not a copy");
    // Deleting: gone for good; the view hidden last shows instead.
    app.run_shell(ShellCommand::DeleteView, None, None, &ctx);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.panels.focused_buffer(), second, "the graph hidden last came back");
    app.run_shell(ShellCommand::DeleteView, None, None, &ctx);
    frame(&ctx, &mut app, vec![]);
    assert!(!app.desk.panels.has(second) && !app.desk.parked.graphs.contains_key(&second.id), "deleted: not anywhere");
    // The queue isn't deleted (there's one of it).
    app.desk.panels.show(0, BufferKind::Queue);
    app.desk.panels.focus(0);
    frame(&ctx, &mut app, vec![]);
    app.run_shell(ShellCommand::DeleteView, None, None, &ctx);
    assert!(app.note.as_deref().unwrap_or_default().contains("There's one queue"), "{:?}", app.note);
    // The first graph is untouched throughout.
    let first = app.desk.panels.buffers().iter().copied().find(|p| p.kind == BufferKind::Graph && p.id == 0).expect("the first graph");
    app.show_view(first);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(selected(&app), "rust:a::run");
}

#[test]
fn a_count_before_a_go_to_names_the_panel_it_opens_in() {
    let (ctx, mut app) = app_with_graph();
    app.desk.panels.set_count(3, 12);
    crate::graph::tests::with_detail(&mut app.desk.graph, "rust:a::step", "src/a.rs", 10, 20);
    app.desk.graph.select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    // Not this panel: it says so, and nothing moves.
    keys(&ctx, &mut app, "2gd");
    assert!(app.note.as_deref().unwrap_or_default().contains("Panel 2 is this one"), "{:?}", app.note);
    assert_eq!(app.desk.panels.focused(), 1);
    keys(&ctx, &mut app, "9gd");
    assert!(app.note.as_deref().unwrap_or_default().contains("There are 3 panels"), "{:?}", app.note);
    // The first panel: the code opens there (the chat put away).
    keys(&ctx, &mut app, "1gd");
    assert_eq!((app.desk.panels.focused(), app.desk.panels.kind(0), app.desk.code.current_file.as_str()), (0, BufferKind::Code, "src/a.rs"));
    // One past the last: a new panel, with a code view of its own.
    app.desk.panels.focus(1);
    frame(&ctx, &mut app, vec![]);
    keys(&ctx, &mut app, "4gd");
    assert_eq!((app.desk.panels.len(), app.desk.panels.focused(), app.desk.panels.kind(3)), (4, 3, BufferKind::Code));
    assert_ne!(app.desk.panels.buffer(3), app.desk.panels.buffer(0), "two code views");
}

#[test]
fn each_chat_view_shows_its_own_slot_with_its_own_draft_and_both_come_back() {
    let (ctx, mut app) = full_app();
    app.desk.chat = crate::chat::tests::conversation();
    let mut sub = crate::chat::tests::lead(None);
    (sub.id, sub.role, sub.name) = (harness_contracts::SlotId::new("sub-1"), "sub".into(), "reviewer".into());
    app.desk.chat.process_event(harness_contracts::ClientEvent::SlotUpdated { slot: sub });
    let (first, second) = (Buffer::new(BufferKind::Chat, 0), Buffer::new(BufferKind::Chat, 1));
    app.desk.panels = crate::panels::Panels::new(vec![first, second], 0);
    app.use_buffer(first);
    app.desk.chat.ui.input_buffer = "to the lead".into();
    app.desk.panels.focus(1);
    app.use_buffer(second);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.chat.ui.slot.as_str(), "lead", "a new chat view starts where the one before it was");
    assert!(app.desk.chat.ui.input_buffer.is_empty(), "with its own prompt");
    app.desk.chat.ui.slot = harness_contracts::SlotId::new("sub-1");
    app.desk.chat.ui.input_buffer = "to the reviewer".into();
    assert_eq!(app.desk.chat.active_thread(), None, "the sub-slot runs no session");
    let ids: Vec<String> = app.desk.chat.actions().into_iter().map(|a| a.id).collect();
    assert!(ids.contains(&"agent.close".to_owned()) && ids.contains(&"agent.pick".to_owned()), "{ids:?}");
    app.desk.panels.focus(0);
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.desk.chat.ui.slot.as_str(), app.desk.chat.ui.input_buffer.as_str(), app.desk.chat.active_thread().map(|t| t.to_string())), ("lead", "to the lead", Some("t".into())));
    assert!(!app.desk.chat.actions().iter().any(|a| a.id == "agent.close"), "the lead isn't closed");
    // Saved and opened again: each view on its slot, with its draft.
    let saved = serde_json::to_value(app.saved_ui()).unwrap();
    let (_, mut fresh) = full_app();
    fresh.restore_saved(serde_json::from_value(saved).unwrap());
    let view = |app: &mut CockpitApp, buffer| {
        app.use_buffer(buffer);
        (app.desk.chat.ui.slot.to_string(), app.desk.chat.ui.input_buffer.clone())
    };
    assert_eq!(view(&mut fresh, second), ("sub-1".into(), "to the reviewer".into()));
    assert_eq!(view(&mut fresh, first), ("lead".into(), "to the lead".into()));
}

#[test]
fn the_agent_is_talked_to_from_anywhere_without_moving_anything() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph.select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    let before = (app.desk.panels.clone(), app.desk.graph.place());
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
    assert!(matches!(app.desk.chat.session_status(), SessionStatus::Failed(e) if e.contains("No harness instance")), "it went to the chat (no harness here)");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert!(app.talk.is_none() && app.input_mode == InputMode::Normal);
    assert_eq!((app.desk.panels.clone(), app.desk.graph.place()), before, "nothing on screen moved");
}
