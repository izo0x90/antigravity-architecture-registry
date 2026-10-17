//! The quick look.

use super::*;

#[test]
fn the_quick_look_shows_the_full_card_over_the_graph_and_says_how_links_are_known() {
    let (ctx, mut app) = app_with_graph();
    crate::graph::tests::with_detail(app.desk.graph_mut(), "rust:a::step", "src/a.rs", 10, 20);
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    frame_at(&ctx, &mut app, vec![], 1.0);
    // Tapped: it stays open, and the keys go to the card.
    peek_key(&ctx, &mut app, true, 1.0);
    peek_key(&ctx, &mut app, false, 1.05);
    assert!(app.quick_look.is_some(), "a tap keeps it open");
    assert!(app.bound_actions().iter().all(|b| b.action.id != "spec.pin"), "held on its card: nothing to pin");
    assert_eq!((app.focused_kind(), app.desk.panels.focused_kind(), app.desk.spec().target.as_ref().map(|t| t.as_str())), (BufferKind::Spec, BufferKind::Graph, Some("rust:a::step")));
    // Its links say how they're known: a data flow and a call only
    // matched by name, a call confirmed by the language's analyzer.
    let id = ComponentId::new("rust:a::step");
    let mut detail = crate::spec::tests::detail(&id, &ComponentId::new("rust:a::save"));
    detail.outgoing.push(Relation::new(RelationKind::DataFlow, id.clone(), ComponentId::new("rust:a::log"), Provenance::OBSERVED));
    let mut confirmed = Relation::new(RelationKind::Calls, ComponentId::new("rust:a::run"), id.clone(), Provenance::OBSERVED);
    confirmed.proven_by = Some("rust-analyzer 1.90.0".into());
    detail.incoming.push(confirmed);
    app.desk.spec_mut().detail = Some(detail);
    let text = texts(&frame_at(&ctx, &mut app, vec![], 1.1));
    assert!(text.contains("Quick look") && text.contains("Does a step."), "{text}");
    // Over the side away from its panel (the graph, on the right), so the graph stays in sight.
    let shown = ctx.memory(|m| m.area_rect(egui::Id::new("quick_look"))).expect("drawn");
    assert!(app.desk.panels.len() > 1 && app.panel_rects[1].center().x > SCREEN.x / 2.0 && shown.center().x < SCREEN.x / 2.0, "{shown:?}");
    assert!(text.contains("log (matched by name)"), "a data flow: {text}");
    assert!(text.contains("run") && !text.contains("run (matched by name)"), "the confirmed one reads plainly: {text}");
    let before = (app.desk.spec_mut().cursor, app.desk.graph_mut().selection.clone());
    frame_at(&ctx, &mut app, key("j"), 1.2);
    assert_eq!((app.desk.spec().cursor, app.desk.graph().selection.clone()), (before.0 + 1, before.1), "j moves in the card, not the graph");
    // Escape closes it; the spec follows the graph again.
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 1.3);
    assert!(app.quick_look.is_none() && !app.desk.spec().pinned);
    assert_eq!(app.focused_kind(), BufferKind::Graph);
    // Held: a peek, closed when let go.
    peek_key(&ctx, &mut app, true, 2.0);
    assert!(app.quick_look.is_some());
    peek_key(&ctx, &mut app, true, 2.2);
    assert!(app.quick_look.is_some(), "the key repeating doesn't close it");
    peek_key(&ctx, &mut app, false, 2.5);
    assert!(app.quick_look.is_none(), "let go after a hold: closed");
}

#[test]
fn the_quick_look_key_is_shown_wherever_keys_are_listed() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    let bound = app.bound_actions();
    let bar: Vec<String> = app.bar_keys(&bound).iter().map(|b| format!("{} {}", b.key_hint(), b.action.id)).collect();
    assert!(bar.iter().any(|k| k.starts_with("q ") && k.ends_with(" quicklook.toggle")), "the key bar offers it on a card: {bar:?}");
    app.key_sheet = Some(String::new());
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Quick look: the full card"), "the ? sheet lists it: {text}");
    app.key_sheet = None;
    let menu = leader_entries(&bound, &['q']);
    assert!(menu.iter().any(|e| matches!(e, LeaderEntry::Action { key: 'q', index } if bound[*index].action.id == "quicklook.toggle")), "Space q q opens it");
    // On nothing, it isn't offered.
    app.desk.graph_mut().select(None);
    frame(&ctx, &mut app, vec![]);
    let bound = app.bound_actions();
    assert!(app.bar_keys(&bound).iter().all(|b| b.action.id != "quicklook.toggle"));
}

#[test]
fn the_quick_look_goes_to_a_column_and_says_when_theres_nothing_to_look_at() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph_mut().select(None);
    frame_at(&ctx, &mut app, vec![], 1.0);
    peek_key(&ctx, &mut app, true, 1.0);
    assert!(app.quick_look.is_none() && app.note.as_deref().unwrap_or_default().contains("Nothing here to look at"), "{:?}", app.note);
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    peek_key(&ctx, &mut app, false, 1.1);
    peek_key(&ctx, &mut app, true, 1.2);
    peek_key(&ctx, &mut app, false, 1.25);
    let to_panel = app.bound_actions().into_iter().find(|b| b.action.id == "quicklook.to_panel").expect("offered while open");
    assert_eq!(to_panel.key_hint(), "Space q c");
    app.invoke(Invocation { action: to_panel.action, origin: to_panel.origin, value: None }, &ctx);
    assert!(app.quick_look.is_none());
    assert_eq!((app.focused_kind(), app.desk.spec().target.as_ref().map(|t| t.as_str())), (BufferKind::Spec, Some("rust:a::step")), "the same card, in a panel, focused");
    assert!(app.desk.panels.panel_of_kind(BufferKind::Graph).is_some(), "beside the graph");
}

/// In the quick look, its own actions are where you are: the key bar offers
/// them with their keys, its controls show those keys, the `?` sheet has a
/// section for it.
#[test]
fn the_quick_looks_own_actions_are_offered_with_their_keys() {
    let (ctx, mut app) = app_with_graph();
    crate::graph::tests::with_detail(app.desk.graph_mut(), "rust:a::step", "src/a.rs", 10, 20);
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"quicklook.toggle"}"#).unwrap();
    let bound = app.bound_actions();
    let panel = bound.iter().find(|b| b.action.id == "quicklook.to_panel").expect("offered");
    assert_eq!((panel.layer, panel.key_hint().as_str()), (crate::actions::Layer::Host, "Space q c"));
    let bar: Vec<String> = app.bar_keys(&bound).iter().map(|b| b.action.id.clone()).collect();
    assert!(bar.contains(&"quicklook.to_panel".to_owned()) && bar.contains(&"quicklook.toggle".to_owned()), "{bar:?}");
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Open in a panel  Space q c"), "its control says its key: {text}");
    app.key_sheet = Some("quick".into());
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Quick look") && text.contains("Close the quick look"), "{text}");
}
