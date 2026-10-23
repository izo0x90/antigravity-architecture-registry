//! Panels, views per panel, saved layout, the agent moving views.

use super::*;

#[test]
fn the_layout_comes_back_as_it_was_saved() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code_mut().move_to("6");
    keys(&ctx, &mut app, "mc");
    app.desk.panels = panels_of(&[BufferKind::Tree, BufferKind::Graph, BufferKind::Chat, BufferKind::Code], 2);
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    let saved = app.saved_ui();
    // Through the server (as JSON) into a cockpit that just opened.
    let json = serde_json::to_value(&saved).unwrap();
    let (_, mut fresh) = full_app();
    fresh.restore_saved(serde_json::from_value(json).unwrap());
    assert_eq!((fresh.desk.panels.kinds(), fresh.desk.panels.focused()), (saved.panels.kinds(), 2));
    assert_eq!((fresh.desk.marks.clone(), fresh.desk.jumps.clone()), (saved.marks.clone(), saved.jumps.clone()));
    let graph_place = |s: &SavedUi| s.views.iter().find(|(p, _)| p.kind == BufferKind::Graph).map(|(_, p)| p.clone());
    assert_eq!(fresh.desk.graph().place(), graph_place(&saved).unwrap(), "the graph where it was, the same card selected");
    let code = fresh.saved_ui().views.into_iter().find(|(p, _)| p.kind == BufferKind::Code).and_then(|(_, place)| place.read::<crate::code::CodePlace>());
    assert_eq!(code.map(|c| c.line), Some(6));
    // A cockpit that has loaded nothing loads the saved file, even one
    // named like the file it starts on.
    let mut opened = new_app();
    let mut only_code = saved.clone();
    only_code.views = vec![(BufferId::new(BufferKind::Code, 0), Place::of(&crate::code::CodePlace { file: opened.desk.code().current_file.clone(), line: 3 }))];
    opened.restore_saved(only_code);
    assert!(opened.desk.code().is_loading, "the saved file is fetched");
}

#[test]
fn each_column_keeps_its_own_graph_and_the_agent_moves_the_one_named_or_used_last() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph_mut().select(Some("rust:a::run".into()));
    // A second graph, in a third panel, showing its own selection.
    app.desk.panels.set_count(3, 12);
    let second = app.desk.panels.show(2, BufferKind::Graph);
    assert_ne!(second, app.desk.panels.buffer(1), "a new graph, not the first one moved");
    app.desk.panels.focus(2);
    frame(&ctx, &mut app, vec![]);
    // It starts where the first one was (its view; its selection once
    // its tree is in).
    let nav = |place: Option<Place>| place.and_then(|p| p.read::<crate::graph::GraphPlace>()).map(|p| p.nav);
    let first_nav = nav(app.desk.buffer(BufferId::new(BufferKind::Graph, 0)).map(|g| g.place()));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(nav(Some(app.desk.view(BufferKind::Graph).place())), first_nav);
    *app.desk.graph_mut() = showing(sample_tree());
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(selected(&app), "rust:a::step");
    // Focus back on the first: its own selection, untouched.
    app.desk.panels.focus(1);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(selected(&app), "rust:a::run");
    // The agent names the third panel: that graph moves, this one doesn't.
    agent_moves(&mut app, "rust:a::save", Some(3));
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.graph().agent.is_none(), "the focused graph wasn't moved");
    app.desk.panels.focus(2);
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.graph().agent.is_some(), "the third panel's was");
    // Naming none: the graph used last (this one, focused last).
    app.desk.panels.focus(0);
    frame(&ctx, &mut app, vec![]);
    agent_moves(&mut app, "rust:a::run", None);
    frame(&ctx, &mut app, vec![]);
    app.desk.panels.focus(1);
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.graph().agent.is_none(), "the first graph, not used last, stayed");
    // Saved and put back: both graphs, each where it was.
    let saved = app.saved_ui();
    assert_eq!(saved.views.iter().filter(|(p, _)| p.kind == BufferKind::Graph).count(), 2);
    let (_, mut fresh) = full_app();
    fresh.restore_saved(saved.clone());
    // (Each one's selection comes back when its tree is in.)
    let nav = |place: Option<Place>| place.and_then(|p| p.read::<crate::graph::GraphPlace>()).map(|p| p.nav);
    for (buffer, place) in saved.views.iter().filter(|(p, _)| p.kind == BufferKind::Graph) {
        assert_eq!(nav(fresh.place_of(*buffer)), nav(Some(place.clone())), "{buffer:?} where it was");
    }
}

#[test]
fn views_are_hidden_by_closing_and_gone_only_when_deleted() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph_mut().select(Some("rust:a::run".into()));
    app.desk.panels.set_count(3, 12);
    let second = app.desk.panels.show(2, BufferKind::Graph);
    app.desk.panels.focus(2);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    app.desk.graph_mut().select(Some("rust:a::step".into()));
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
    assert_eq!(app.desk.graph().nav, crate::graph::Nav::default(), "new, not a copy");
    // Deleting: gone for good; the view hidden last shows instead.
    app.run_shell(ShellCommand::DeleteView, None, None, &ctx);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.panels.focused_buffer(), second, "the graph hidden last came back");
    app.run_shell(ShellCommand::DeleteView, None, None, &ctx);
    frame(&ctx, &mut app, vec![]);
    assert!(!app.desk.panels.has(second) && app.desk.buffer(second).is_none(), "deleted: not anywhere");
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
    crate::graph::tests::with_detail(app.desk.graph_mut(), "rust:a::step", "src/a.rs", 10, 20);
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    // Not this panel: it says so, and nothing moves.
    keys(&ctx, &mut app, "2gd");
    assert!(app.note.as_deref().unwrap_or_default().contains("Panel 2 is this one"), "{:?}", app.note);
    assert_eq!(app.desk.panels.focused(), 1);
    keys(&ctx, &mut app, "9gd");
    assert!(app.note.as_deref().unwrap_or_default().contains("There are 3 panels"), "{:?}", app.note);
    // The first panel: the code opens there (the chat put away).
    keys(&ctx, &mut app, "1gd");
    assert_eq!((app.desk.panels.focused(), app.desk.panels.kind(0), app.desk.code().current_file.as_str()), (0, BufferKind::Code, "src/a.rs"));
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
    // The server has no drafts yet (read: the views report theirs from here on).
    app.desk.shared.drafts.get_mut().arrived(Default::default());
    crate::chat::tests::conversation_events().into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    let mut sub = crate::chat::tests::lead(None);
    (sub.id, sub.role, sub.name) = (harness_contracts::SlotId::new("sub-1"), "sub".into(), "reviewer".into());
    app.desk.shared.agents.get_mut().apply(harness_contracts::ClientEvent::SlotUpdated { slot: sub });
    let (first, second) = (BufferId::new(BufferKind::Chat, 0), BufferId::new(BufferKind::Chat, 1));
    app.desk.panels = crate::panels::Panels::new(vec![first, second], 0);
    app.use_buffer(first);
    app.desk.chat_mut().ui.input.text = "to the lead".into();
    app.desk.panels.focus(1);
    app.use_buffer(second);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.chat().ui.slot.as_str(), "lead", "a new chat view starts where the one before it was");
    assert!(app.desk.chat().ui.input.text.is_empty(), "with its own prompt");
    app.desk.chat_mut().ui.slot = harness_contracts::SlotId::new("sub-1");
    app.desk.chat_mut().ui.input.text = "to the reviewer".into();
    assert_eq!(app.desk.chat().active_thread(), None, "the sub-slot runs no session");
    let ids: Vec<String> = app.desk.chat_mut().actions().into_iter().map(|a| a.id).collect();
    assert!(ids.contains(&"agent.close".to_owned()) && ids.contains(&"agent.pick".to_owned()), "{ids:?}");
    // (A frame while it's the chat in use: what it asks is taken then.)
    frame(&ctx, &mut app, vec![]);
    app.desk.panels.focus(0);
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.desk.chat().ui.slot.as_str(), app.desk.chat().ui.input.text.as_str(), app.desk.chat().active_thread().map(|t| t.to_string())), ("lead", "to the lead", Some("t".into())));
    assert!(!app.desk.chat().actions().iter().any(|a| a.id == "agent.close"), "the lead isn't closed");
    // Each view's draft is to be saved for its slot (the server keeps them
    // as their own record, not in the layout).
    frame(&ctx, &mut app, vec![]);
    let drafts = app.desk.shared.drafts.get();
    assert_eq!((drafts.to_save(&harness_contracts::SlotId::new("lead")), drafts.to_save(&harness_contracts::SlotId::new("sub-1"))), (Some("to the lead"), Some("to the reviewer")));
    drop(drafts);
    let saved = serde_json::to_value(app.saved_ui()).unwrap();
    assert!(!saved.to_string().contains("to the lead"), "the draft isn't in the layout: {saved}");
    // Saved and opened again: each view on its slot, and its slot's draft
    // once the drafts are read.
    let (ctx, mut fresh) = full_app();
    fresh.restore_saved(serde_json::from_value(saved).unwrap());
    // The lead's draft links a file: it comes back linked.
    let link = harness_contracts::Link { start: 0, end: 5, to: harness_contracts::Linked::Thing { subject: harness_contracts::Subject::CodeFile { path: "a.rs".into() } }, include: harness_contracts::Include::Whole };
    let linked = harness_contracts::Message { text: "@a.rs to the lead".into(), links: vec![link.clone()] };
    fresh.desk.shared.drafts.get_mut().arrived([(harness_contracts::SlotId::new("lead"), linked), (harness_contracts::SlotId::new("sub-1"), "to the reviewer".into())].into());
    let view = |app: &mut CockpitApp, buffer| {
        app.use_buffer(buffer);
        frame(&ctx, app, vec![]);
        let chat: &crate::chat::ChatStreamState = app.desk.typed_at(buffer);
        (chat.ui.slot.to_string(), chat.ui.input.text.clone())
    };
    assert_eq!(view(&mut fresh, second), ("sub-1".into(), "to the reviewer".into()));
    assert_eq!(view(&mut fresh, first), ("lead".into(), "@a.rs to the lead".into()));
    let chat: &crate::chat::ChatStreamState = fresh.desk.typed_at(first);
    assert_eq!(chat.ui.input.links().iter().map(|c| c.link.clone()).collect::<Vec<_>>(), std::slice::from_ref(&link), "its link too");
    // Its links are checked before it's sent (once still a moment), and what
    // they come to is said under the box.
    let still = fresh.config.net.save_after_ms as f64 / 1000.0;
    frame_at(&ctx, &mut fresh, vec![], 10.0);
    let asked = bodies(&fresh, "POST /api/links/check");
    assert_eq!(asked, [serde_json::json!({ "text": "@a.rs to the lead", "links": [{ "start": 0, "end": 5, "to": { "kind": "thing", "subject": { "kind": "code_file", "path": "a.rs" } }, "include": "whole" }] })]);
    // Its link changed (a pointer now): not asked again at once; the answer
    // about the link as it was is dropped; asked once it's still a moment.
    let pointer = harness_contracts::Message { text: "@a.rs to the lead".into(), links: vec![harness_contracts::Link { include: harness_contracts::Include::Pointer, ..link.clone() }] };
    fresh.desk.buffer_mut(first).and_then(|b| b.typing_box()).unwrap().restore(&pointer);
    frame_at(&ctx, &mut fresh, vec![], 20.0);
    assert!(bodies(&fresh, "POST /api/links/check").is_empty(), "not at once");
    assert!(fresh.desk.net.answer("POST /api/links/check", 200, &serde_json::json!({ "notes": ["about the link as it was"] })));
    frame_at(&ctx, &mut fresh, vec![], 20.0 + still);
    assert!(!texts(&frame_at(&ctx, &mut fresh, vec![], 20.0 + still)).contains("as it was"), "an old answer: dropped");
    assert_eq!(bodies(&fresh, "POST /api/links/check").len(), 1);
    assert!(fresh.desk.net.answer("POST /api/links/check", 200, &serde_json::json!({ "notes": ["file a.rs: too big to go in whole with the rest"] })));
    frame_at(&ctx, &mut fresh, vec![], 21.0 + still);
    assert!(texts(&frame_at(&ctx, &mut fresh, vec![], 21.1 + still)).contains("file a.rs: too big to go in whole"), "said before it's sent");
}

/// A buffer kind saved in another version of itself starts new; the rest
/// of the layout (the other kinds, the panels, the marks) comes back.
#[test]
fn a_kind_saved_in_another_version_starts_new_and_the_rest_of_the_layout_holds() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![super::terminal::with_commands(1000, vec![super::terminal::command(1, 1003, Some(1008), "make")])]);
    let _ = app.desk.view_mut(BufferKind::Code).show_target(&Target::Span { file: "src/a.rs".into(), start: 3, end: 3 });
    app.desk.marks.insert('a', crate::nav::Jump { kind: BufferKind::Code, panel: 0, place: app.desk.view(BufferKind::Code).place() });
    frame(&ctx, &mut app, vec![]);
    let mut saved = app.saved_ui();
    assert_eq!(saved.versions.get(&BufferKind::Code), Some(&BufferKind::Code.saved_version()));
    saved.versions.insert(BufferKind::Code, BufferKind::Code.saved_version() + 1);
    let (ctx, mut fresh) = full_app();
    fresh.restore_saved(saved);
    frame(&ctx, &mut fresh, vec![]);
    assert_eq!(fresh.desk.view(BufferKind::Terminal).place(), app.desk.view(BufferKind::Terminal).place(), "the terminal came back");
    assert!(fresh.desk.marks.contains_key(&'a'), "the marks came back");
    assert_ne!(fresh.desk.view(BufferKind::Code).place(), app.desk.view(BufferKind::Code).place(), "the code view started new");
    assert!(fresh.note.as_deref().is_some_and(|n| n.contains("code") && n.contains("another version")), "{:?}", fresh.note);
}

#[test]
fn the_agent_is_talked_to_from_anywhere_without_moving_anything() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    let before = (app.desk.panels.clone(), app.desk.graph_mut().place());
    // As a browser sends it: the key down with its text, then up.
    let t = |pressed| egui::Event::Key { key: egui::Key::T, physical_key: Some(egui::Key::T), pressed, repeat: false, modifiers: Default::default() };
    frame(&ctx, &mut app, vec![t(true), egui::Event::Text("t".into())]);
    frame(&ctx, &mut app, vec![t(false)]);
    frame(&ctx, &mut app, vec![]);
    let chat = BufferId::new(BufferKind::Chat, crate::quick_look::PANE);
    assert_eq!((app.quick_look.as_ref().map(|q| q.buffer), app.input_mode), (Some(chat), InputMode::Insert { at: Host::Look }), "t: the lead's chat in the quick look, typing");
    assert_eq!(app.desk.chat().ui.slot.as_str(), "lead");
    frame(&ctx, &mut app, key("what calls step?"));
    assert_eq!(app.desk.chat().ui.input.text, "what calls step?", "typing goes into it, not to the keys");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert!(app.desk.chat().ui.input.text.is_empty(), "sent");
    assert!(matches!(app.desk.shared.agents.get().lead_status(), SessionStatus::Failed(e) if e.contains("No harness instance")), "it went to the lead (no harness here)");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!((app.quick_look.is_some(), app.input_mode), (true, InputMode::Normal), "Escape stops typing");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert!(app.quick_look.is_none(), "and again closes it");
    assert_eq!((app.desk.panels.clone(), app.desk.graph().place()), before, "nothing on screen moved");
}

#[test]
fn typing_in_the_quick_look_stays_there_over_a_chat_or_a_discussion() {
    for kind in [BufferKind::Chat, BufferKind::Discussion] {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, kind);
        if kind == BufferKind::Discussion {
            let on = harness_contracts::Subject::Plan { plan: PlanId::new("p").unwrap() };
            told(&app, harness_contracts::ClientEvent::DiscussionUpdated { discussion: crate::discussions::tests::discussion(1, on, harness_contracts::DiscussionStatus::Open) });
            frame(&ctx, &mut app, vec![]);
            app.desk.view_mut(BufferKind::Discussion).show_target(&crate::nav::Target::Discussion(harness_contracts::DiscussionId::new("d1"))).unwrap();
        }
        // The panel's own box has the keyboard first: what's typed goes in it.
        app.input_mode = InputMode::Insert { at: Host::Panel(app.desk.panels.focused()) };
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, key("ok"));
        let in_box = |app: &CockpitApp| match kind {
            BufferKind::Chat => app.desk.chat().ui.input.text.clone(),
            _ => app.desk.typed::<crate::discussion::DiscussionView>(BufferKind::Discussion).writing.text.clone(),
        };
        assert_eq!(in_box(&app), "ok", "{kind:?}: its box has the keyboard");
        frame(&ctx, &mut app, press(egui::Key::Escape, false));
        match kind {
            BufferKind::Chat => app.desk.chat_mut().ui.input.text.clear(),
            _ => app.desk.typed_mut::<crate::discussion::DiscussionView>(BufferKind::Discussion).writing.text.clear(),
        }
        frame(&ctx, &mut app, key("t"));
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, key("hello"));
        frame(&ctx, &mut app, vec![]);
        let in_look = app.desk.chat().ui.input.text.clone();
        app.close_quick_look();
        let in_panel = match kind {
            BufferKind::Chat => app.desk.chat().ui.input.text.clone(),
            _ => app.desk.typed::<crate::discussion::DiscussionView>(BufferKind::Discussion).writing.text.clone(),
        };
        assert_eq!((in_look.as_str(), in_panel.as_str()), ("hello", ""), "over a {kind:?}: typing goes into the quick look, not the panel under it");
    }
}
