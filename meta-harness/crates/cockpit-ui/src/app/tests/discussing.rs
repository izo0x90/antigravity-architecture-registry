//! Discussions from a card: what the spec view sends the server as you
//! start one, reply, settle and reopen (as the server's routes take them).

use super::*;

#[test]
fn a_cards_discussions_are_started_replied_to_settled_and_reopened_as_the_server_takes_them() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Spec);
    let step = ComponentId::new("rust:a::step");
    app.desk.spec_mut().show(step.clone());
    app.desk.spec_mut().detail = Some(crate::spec::tests::detail(&step, &ComponentId::new("rust:a::save")));
    let answered = crate::review::tests::discussion(1, crate::review::tests::on(&step, Some("invariants")), harness_contracts::DiscussionStatus::Answered);
    told(&app, harness_contracts::ClientEvent::DiscussionUpdated { discussion: answered });
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.spec().discussions.len(), 1, "the card has the plan's discussions, as the server told them");
    let _ = bodies(&app, "");
    // C on the logic section: a change request on it.
    app.desk.spec_mut().move_to(&crate::spec::Entry { section: crate::card::Section::Logic, item: crate::spec::SpecItem::Header }.key());
    keys(&ctx, &mut app, "C");
    keys(&ctx, &mut app, "Split step 2.");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let logic = harness_contracts::Subject::PlanSection { plan: PlanId::new("p").unwrap(), id: step.clone(), section: "logic".into() };
    assert_eq!(bodies(&app, "POST /api/discussions"), [serde_json::json!({ "subject": logic, "kind": "change_request", "text": "Split step 2." })]);
    // On d1: reply, settle (it's answered), reopen with why.
    let d1 = crate::spec::Entry { section: crate::card::Section::Invariants, item: crate::spec::SpecItem::Comment(harness_contracts::DiscussionId::new("d1")) }.key();
    app.desk.spec_mut().move_to(&d1);
    keys(&ctx, &mut app, "r");
    keys(&ctx, &mut app, "And the third?");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/discussions/d1/reply"), [serde_json::json!({ "text": "And the third?" })]);
    app.desk.spec_mut().move_to(&d1);
    keys(&ctx, &mut app, "a");
    assert_eq!(bodies(&app, "POST /api/discussions/d1/settle"), [serde_json::json!({})]);
    app.desk.spec_mut().move_to(&d1);
    keys(&ctx, &mut app, "x");
    keys(&ctx, &mut app, "Not yet.");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/discussions/d1/reopen"), [serde_json::json!({ "text": "Not yet." })]);
}
