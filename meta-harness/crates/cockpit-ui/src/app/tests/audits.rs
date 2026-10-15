//! What the audits found, kept fixed.

use super::*;

#[test]
fn what_the_audit_found_stays_fixed() {
    let (ctx, mut app) = full_app();
    // A tree filter clears from the keys: / then Enter on nothing.
    focus(&ctx, &mut app, BufferKind::Tree);
    keys(&ctx, &mut app, "/lo");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(app.desk.tree.search_filter, "lo");
    frame(&ctx, &mut app, key("/"));
    assert_eq!(app.desk.tree.search_filter, "", "an empty search empties the filter");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!((app.desk.tree.search_filter.as_str(), app.desk.tree.rows().len()), ("", 6));
    // A graph search that finds nothing puts the view back as it was.
    focus(&ctx, &mut app, BufferKind::Graph);
    app.desk.graph.set_tree(crate::graph::tests::boxes_tree());
    let before = app.desk.graph.place();
    keys(&ctx, &mut app, "/zzz");
    assert_eq!(app.desk.graph.place(), before);
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!(app.desk.graph.place(), before);
    // c in the spec without a plan says why.
    focus(&ctx, &mut app, BufferKind::Spec);
    app.desk.spec.source = Source::Code;
    frame(&ctx, &mut app, key("c"));
    assert!(app.note.as_deref().unwrap_or_default().contains("pick one in the graph"), "{:?}", app.note);
    assert_eq!(app.input_mode, InputMode::Normal);
    // z o on a card with nothing more under it says why too.
    focus(&ctx, &mut app, BufferKind::Graph);
    app.desk.graph.set_tree(sample_tree());
    app.desk.graph.select(Some("rust:a::save".into()));
    keys(&ctx, &mut app, "zo");
    assert!(app.desk.graph.note.as_deref().unwrap_or_default().contains("nothing more"), "{:?}", app.desk.graph.note);
    // A key that runs clears the last note.
    frame(&ctx, &mut app, key("j"));
    assert!(app.note.is_none());
    // Fewer columns keep the focused view on screen.
    app.desk.columns.focus(2);
    app.desk.columns.show(2, BufferKind::Spec);
    app.run_shell(ShellCommand::SetLayout(1), None, None, &ctx);
    assert_eq!((app.desk.columns.focused(), app.desk.columns.kind(0)), (0, BufferKind::Spec));
}

#[test]
fn what_the_final_audit_found_stays_fixed() {
    let (ctx, mut app) = full_app();
    // Escape after a search puts the selection back.
    focus(&ctx, &mut app, BufferKind::Graph);
    app.desk.graph.select(Some("rust:a::run".into()));
    keys(&ctx, &mut app, "/sa");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!(selected(&app), "rust:a::run");
    keys(&ctx, &mut app, "/sazz");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(selected(&app), "rust:a::run", "no match: nothing changed");
    assert!(!app.desk.searches.contains_key(&BufferKind::Graph), "and nothing to step through with n");
    frame(&ctx, &mut app, key("n"));
    assert!(app.note.as_deref().unwrap_or_default().contains("No match for sazz"), "n says why: {:?}", app.note);
    // The review point belongs to its component: moving on forgets it.
    focus(&ctx, &mut app, BufferKind::Spec);
    let step = ComponentId::new("rust:a::step");
    app.desk.spec.comments = vec![crate::review::tests::comment(1, arch_graph::CommentAnchor::Section { id: step.clone(), section: "logic".into() }, arch_graph::CommentStatus::Open)];
    app.desk.spec.step_point(true);
    assert!(app.desk.spec.actions().iter().any(|a| a.id == "spec.reply"));
    app.desk.spec.show(ComponentId::new("rust:a::save"));
    assert!(app.desk.spec.point.is_none() && !app.desk.spec.actions().iter().any(|a| a.id == "spec.reply"), "no replying to another component's point");
    // Back to a relation item.
    app.desk.spec.show(step.clone());
    let relation = crate::spec::Entry { section: crate::spec::Section::Relations, item: crate::nav::SpecItem::Relation { kind: RelationKind::Calls, other: ComponentId::new("rust:a::save"), outgoing: true } }.key();
    app.desk.spec.detail = Some(crate::spec::tests::detail(&step, &ComponentId::new("rust:a::save")));
    app.desk.spec.restore(&Place::Spec { component: Some(step.clone()), item: Some(relation.clone()) });
    assert_eq!(app.desk.spec.cursor_key().as_deref(), Some(relation.as_str()));
    // A second Escape throws a draft away.
    app.desk.spec.source = Source::Plan(PlanId::new("p").unwrap());
    frame(&ctx, &mut app, key("c"));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert!(app.desk.spec.composer.is_some() && app.input_mode == InputMode::Normal, "the first leaves the field");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert!(app.desk.spec.composer.is_none(), "the second discards it");
    // Go-tos are offered only where they lead, once the relations are known.
    focus(&ctx, &mut app, BufferKind::Graph);
    app.desk.graph.select(Some("rust:a::save".into()));
    app.desk.details.insert(crate::goto::tests::detail("rust:a::save", vec![], vec![call("rust:a::step", "rust:a::save", 0, 5)]), &app.desk.graph.nav.source);
    let situation = app.situation();
    assert!(When::Reaches(GoTo::Callers).holds(&situation) && !When::Reaches(GoTo::Callees).holds(&situation));
    // The queue's verbs are always in its key sheet, applying or not.
    focus(&ctx, &mut app, BufferKind::Queue);
    app.desk.queue.move_to("3");
    let approve = app.bound_actions().into_iter().find(|b| b.action.id == "queue.approve").expect("listed");
    assert_eq!(approve.action.when, When::Unavailable, "a queued job can't be approved");
    frame(&ctx, &mut app, key("a"));
    assert!(app.note.as_deref().unwrap_or_default().contains("waiting for your approval"), "and says why: {:?}", app.note);
}
