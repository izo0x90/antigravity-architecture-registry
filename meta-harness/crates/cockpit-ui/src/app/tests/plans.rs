//! A plan in the cockpit: its items in the plan tree (nested, each with its
//! state), an item's card, and its state set on it.

use super::*;

/// Plan `p` as a review: Decide (with FFF skipped under it, decided), then Open.
fn review() -> serde_json::Value {
    let mut plan = arch_graph::Plan::new("p", "Findings");
    plan.kind = arch_graph::PlanKind::Review;
    let item = |id: &str, parent: Option<&str>, title: &str, state| arch_graph::Item { id: arch_graph::ItemId(id.into()), parent: parent.map(|p| arch_graph::ItemId(p.into())), title: title.into(), text: format!("What {title} is about."), state };
    plan.items = vec![item("i1", None, "Decide", arch_graph::ItemState::Open), item("i2", Some("i1"), "FFF skipped", arch_graph::ItemState::Decided), item("i3", None, "Open", arch_graph::ItemState::Open)];
    serde_json::to_value(plan).unwrap()
}

#[test]
fn a_plans_items_show_nested_in_the_plan_tree_open_as_cards_and_take_their_state_there() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Plan);
    assert!(app.desk.net.answer("GET /api/arch/plans/p", 200, &review()), "it reads the plan the desk reads: {:?}", sent(&app));
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    let rows = |app: &CockpitApp| app.desk.view(BufferKind::Plan).items().into_iter().map(|i| i.key).collect::<Vec<_>>();
    assert_eq!(rows(&app), ["item:i1", "item:i2", "item:i3"], "in order");
    let depths = |app: &CockpitApp| app.desk.all_of::<crate::tree::TreeBuffer>(BufferKind::Plan)[0].rows().into_iter().map(|r| (r.id, r.depth)).collect::<Vec<_>>();
    assert_eq!(depths(&app), [("item:i1".to_owned(), 0), ("item:i2".to_owned(), 1), ("item:i3".to_owned(), 0)], "nested: FFF skipped under Decide");
    // A discussion on an item: a row under it, and the item says whose move it is.
    let on_i2 = harness_contracts::Subject::PlanItem { plan: PlanId::new("p").unwrap(), item: "i2".into() };
    told(&app, harness_contracts::ClientEvent::DiscussionUpdated { discussion: crate::review::tests::discussion(1, on_i2, harness_contracts::DiscussionStatus::Open) });
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(depths(&app)[2], ("discussion:d1".to_owned(), 2), "under FFF skipped: {:?}", depths(&app));
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("decided · 1 waiting on lead") && drawn.contains("d1 question: point 1"), "{drawn}");
    let things = app.desk.view(BufferKind::Plan).things("discussion:d1");
    assert_eq!(things.at.iter().map(|t| t.words.clone()).collect::<Vec<_>>(), ["discussion d1", "plan p item i2", "plan p"], "the discussion, what it's on, the plan");
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    for words in ["Decide", "FFF skipped", "decided", "Open", "open"] {
        assert!(drawn.contains(words), "{words}: {drawn}");
    }
    // Enter on an item: its card (the spec view), its text.
    app.desk.view_mut(BufferKind::Plan).move_to("item:i2");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.focused_kind(), BufferKind::Spec);
    assert!(!app.desk.net.answer("GET /api/arch/plans/p", 200, &review()), "the card has the plan the desk read: not read again");
    frame(&ctx, &mut app, vec![]);
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("What FFF skipped is about.") && drawn.contains("Under i1 Decide"), "{drawn}");
    assert!(drawn.contains("OPEN") && drawn.contains("point 1") && drawn.contains("d1 · asks lead"), "its discussions, as every conversation draws them: {drawn}");
    // A discussion is started on the item from its card (G.7).
    keys(&ctx, &mut app, "C");
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("Discuss plan p item i2"), "{drawn}");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    frame(&ctx, &mut app, vec![]);
    // A component's card's controls don't come with it.
    let offered: Vec<String> = app.bound_actions().into_iter().filter(|b| b.action.when.holds(&app.situation())).map(|b| b.action.id).collect();
    assert!(!offered.iter().any(|id| id.starts_with("spec.") && id != "spec.pin"), "{offered:?}");
    assert_eq!(crate::buffer::Buffer::describe(app.desk.view(BufferKind::Spec)), "i2 FFF skipped (pinned)", "(the fixture pins the card)");
    // Kept as the card's place: after a reload, the item's card again.
    let place = app.desk.view(BufferKind::Spec).kept_place();
    app.desk.view_mut(BufferKind::Spec).restore(&crate::nav::Place::of(&serde_json::json!({ "component": null, "item": null })));
    app.desk.view_mut(BufferKind::Spec).restore(&place);
    assert_eq!(cursor(&app).as_deref(), Some("item#i2"));
    // Its state, set there (the thing at the cursor is the item).
    bodies(&app, "");
    app.run_by_id("plan.item_state", Some("done".into()), &ctx);
    assert_eq!(bodies(&app, "POST /api/arch/plans/p/ops"), [serde_json::json!({ "ops": [{ "op": "set_item", "id": "i2", "state": "done" }] })]);
}

/// Gone to from elsewhere, an item of a plan the desk isn't reading: the
/// desk reads that plan, and its card shows the item (SP3 faithfulness 7).
#[test]
fn an_item_of_another_plan_gone_to_is_read_from_that_plan() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Spec);
    app.desk.spec_mut().pinned = false;
    let mut q = review();
    q["id"] = "q".into();
    let jump = app.jump_here();
    app.land(None, &crate::nav::Target::Plan { plan: PlanId::new("q").unwrap(), part: crate::nav::PlanPart::Item("i2".into()) }, jump, &ctx);
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(app.desk.shared.source.get(), &arch_graph::Source::Plan(PlanId::new("q").unwrap()), "the desk reads it");
    assert!(app.desk.net.answer("GET /api/arch/plans/q", 200, &q), "asked for: {:?}", sent(&app));
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("What FFF skipped is about."), "{drawn}");
}
