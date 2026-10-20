//! What buffers ask: each ask is done the one way, whichever buffer asked.

use super::*;

#[test]
fn a_name_clicked_in_the_code_is_followed_and_isnt_a_jump() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.spec_mut().pinned = false;
    let jumps = app.desk.jumps.clone();
    app.desk.code_mut().name_clicked(ComponentId::new("rust:a::save"), false);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(selected(&app), "rust:a::save", "the graph follows");
    assert_eq!(app.desk.spec().target.as_ref().map(ComponentId::as_str), Some("rust:a::save"), "a card not pinned follows");
    assert_eq!((app.focused_kind(), &app.desk.jumps), (BufferKind::Code, &jumps), "the cursor stays; following isn't a jump");
    // Pinned, a card keeps its component.
    app.desk.spec_mut().pinned = true;
    app.desk.code_mut().name_clicked(ComponentId::new("rust:a::run"), false);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!((selected(&app), app.desk.spec().target.as_ref().map(ComponentId::as_str)), ("rust:a::run", Some("rust:a::save")));
}

#[test]
fn a_name_ctrl_clicked_goes_to_its_code_as_a_go_to_does() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code_mut().move_to("3");
    app.desk.code_mut().name_clicked(ComponentId::new("rust:a::step"), true);
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.focused_kind(), app.desk.code().target_line), (BufferKind::Code, Some(6)), "step's definition");
    ctrl(&ctx, &mut app, egui::Key::O);
    assert_eq!(app.desk.code().target_line, Some(3), "a jump: ctrl+o comes back");
}

#[test]
fn a_session_no_agent_slot_runs_says_so_and_nothing_moves() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Queue);
    app.desk.queue_mut().on_event(&arch_graph::ArchEvent::QueueUpdated { snapshot: Box::new(crate::queue::tests::snapshot(arch_graph::QueueStatus::Running)) });
    keys(&ctx, &mut app, "gg");
    let jumps = app.desk.jumps.clone();
    frame(&ctx, &mut app, key("o"));
    assert_eq!(app.note.as_deref(), Some("No agent slot runs session t1 now."));
    assert_eq!((app.focused_kind(), &app.desk.jumps), (BufferKind::Queue, &jumps));
}

#[test]
fn a_typing_box_opened_takes_the_keyboard_and_a_reread_reads_the_file_again() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Spec);
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"spec.comment"}"#).unwrap();
    assert_eq!(app.input_mode, InputMode::Insert { at: Host::Panel(app.desk.panels.focused()) }, "the comment box has the keyboard");
    app.input_mode = InputMode::Normal;
    focus(&ctx, &mut app, BufferKind::Code);
    sent(&app);
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"index.file"}"#).unwrap();
    assert!(sent(&app).contains(&"POST /api/arch/refresh".to_owned()));
}

#[test]
fn going_back_to_a_card_moves_the_graph_to_it() {
    let (ctx, mut app) = full_app();
    app.desk.spec_mut().pinned = false;
    focus(&ctx, &mut app, BufferKind::Graph);
    app.desk.graph_mut().select(Some("rust:a::save".into()));
    keys(&ctx, &mut app, "gs");
    assert_eq!((app.focused_kind(), app.desk.spec().target.as_ref().map(ComponentId::as_str)), (BufferKind::Spec, Some("rust:a::save")));
    // Back in the graph, elsewhere (the card follows), then forward to the
    // card as it was: the graph comes along.
    ctrl(&ctx, &mut app, egui::Key::O);
    app.desk.graph_mut().select(Some("rust:a::run".into()));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.spec().target.as_ref().map(ComponentId::as_str), Some("rust:a::run"));
    ctrl(&ctx, &mut app, egui::Key::I);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.focused_kind(), app.desk.spec().target.as_ref().map(ComponentId::as_str), selected(&app)), (BufferKind::Spec, Some("rust:a::save"), "rust:a::save"));
}

#[test]
fn a_card_unpinned_catches_up_with_what_the_user_is_on() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Graph);
    app.desk.graph_mut().select(Some("rust:a::save".into()));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.spec().target.as_ref().map(ComponentId::as_str), Some("rust:a::step"), "pinned: it stays");
    focus(&ctx, &mut app, BufferKind::Spec);
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"spec.pin"}"#).unwrap();
    assert!(!app.desk.spec().pinned);
    assert_eq!(app.desk.spec().target.as_ref().map(ComponentId::as_str), Some("rust:a::save"), "unpinned: on what the user is on");
}

#[test]
fn what_buffers_share_reaches_each_one_shown_or_parked() {
    let (ctx, mut app) = full_app();
    // A second graph and a second card, parked.
    let shown = app.desk.panels.buffer(1);
    let second = app.desk.panels.show(2, BufferKind::Graph);
    app.use_buffer(second);
    app.use_buffer(shown);
    let card = BufferId::new(BufferKind::Spec, 7);
    app.use_buffer(card);
    app.use_buffer(BufferId::new(BufferKind::Spec, 0));
    // The review (its plan's asked for first), the types, the index, the
    // settings, the source.
    frame(&ctx, &mut app, vec![]);
    let save = ComponentId::new("rust:a::save");
    let on_save = crate::review::tests::on(&save, None);
    app.desk.shared.discussions.get_mut().apply(&harness_contracts::ClientEvent::DiscussionUpdated { discussion: crate::review::tests::discussion(1, on_save, harness_contracts::DiscussionStatus::Open) });
    app.desk.shared.types.set(Some(arch_graph::TypeSystem::defaults().unwrap()));
    app.desk.shared.index.set(arch_graph::IndexStatus { stale: vec!["src/a.rs".into()], ..Default::default() });
    app.config.graph.readable_zoom = 0.37;
    app.config.user.name = "ada".into();
    frame(&ctx, &mut app, vec![]);
    for graph in app.desk.all_of::<crate::graph::GraphState>(BufferKind::Graph) {
        assert_eq!((graph.marks[&save].open, graph.ui.readable_zoom), (1, 0.37), "every graph: the badges, the settings");
    }
    for spec in app.desk.all_of::<crate::spec::SpecState>(BufferKind::Spec) {
        assert!(spec.types.is_some() && spec.discussions.len() == 1 && spec.author == "ada", "every card: types, discussions, the user");
    }
    app.desk.code_mut().current_file = "src/a.rs".into();
    frame(&ctx, &mut app, vec![]);
    assert!(app.desk.code().out_of_date, "the code: its file changed since it was read");
    // The source the graph shows is every buffer's.
    app.desk.graph_mut().navigate(crate::graph::NavChange::Source(arch_graph::Source::Code));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.shared.source.get(), &arch_graph::Source::Code);
    focus(&ctx, &mut app, BufferKind::Queue);
    assert_eq!(app.desk.queue().plan, None, "the queue: no plan, no jobs");
}
