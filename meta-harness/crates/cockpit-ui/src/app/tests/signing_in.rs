//! Signing a harness in from the chat, as a user does: its keys, its link
//! copied, the typing box the sign-in's while it waits (Enter included),
//! the draft kept and given back.

use super::*;
use crate::chat::tests::{signed_out_harness, waiting};
use harness_contracts::{ClientEvent, SlotId};

/// The lead's chat alone on screen, with a draft saved for it.
fn chat_app(draft: &str) -> (egui::Context, CockpitApp) {
    let (ctx, mut app) = full_app();
    app.desk.shared.drafts.get_mut().arrived([(SlotId::new("lead"), draft.into())].into());
    crate::chat::tests::conversation_events().into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    let chat = BufferId::new(BufferKind::Chat, 0);
    app.desk.panels = crate::panels::Panels::new(vec![chat], 0);
    app.use_buffer(chat);
    frame(&ctx, &mut app, vec![]);
    (ctx, app)
}

fn instance(app: &mut CockpitApp, waiting: Option<harness_contracts::SignInWaiting>) {
    app.desk.shared.agents.get_mut().apply(ClientEvent::InstanceUpdated { snapshot: signed_out_harness(waiting) });
}

/// The leader, then `keys` (an Account step: `ay`, `ac`, `ax`).
fn leader(ctx: &egui::Context, app: &mut CockpitApp, keys_after: &str) {
    // A tap: pressed, then let go (held, it's a modifier).
    frame(ctx, app, press(egui::Key::Space, false));
    frame(ctx, app, vec![egui::Event::Key { key: egui::Key::Space, physical_key: None, pressed: false, repeat: false, modifiers: Default::default() }]);
    keys(ctx, app, keys_after);
}

fn box_text(app: &CockpitApp) -> String {
    app.desk.chat().ui.input.text.clone()
}

fn saved(app: &CockpitApp) -> Option<String> {
    app.desk.shared.drafts.get().to_save(&SlotId::new("lead")).map(str::to_owned)
}

#[test]
fn a_sign_in_has_the_typing_box_while_it_waits_enter_and_keys_included() {
    let (ctx, mut app) = chat_app("half a message ");
    assert_eq!(box_text(&app), "half a message ", "the slot's draft, as written");

    // A sign-in starts (on any device): the box is its, empty; the draft kept.
    instance(&mut app, Some(waiting(7, 300)));
    frame(&ctx, &mut app, vec![]);
    assert!(box_text(&app).is_empty());
    assert!(app.note.as_deref().is_some_and(|n| n.contains("kept")), "said: {:?}", app.note);

    // Its keys: copy the link.
    sent(&app);
    leader(&ctx, &mut app, "ay");
    assert!(app.note.as_deref().is_some_and(|n| n.contains("Copied")), "{:?}", app.note);

    // Space a c with nothing pasted: into the box. Pasted, then Enter: to the sign-in.
    leader(&ctx, &mut app, "ac");
    keys(&ctx, &mut app, "http://127.0.0.1:4000/?code=4/abc");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let calls = sent(&app);
    assert!(calls.iter().any(|c| c == "POST /api/instances/i/auth/complete"), "{calls:?}");
    assert!(!calls.iter().any(|c| c.ends_with("/prompt")), "nothing typed went to the agent: {calls:?}");
    let body = bodies(&app, "POST /api/instances/i/auth/complete").pop().unwrap();
    assert_eq!((body["id"].clone(), body["response"].clone()), (serde_json::json!(7), serde_json::json!("http://127.0.0.1:4000/?code=4/abc")));
    // Nothing typed for it was ever saved as a draft.
    assert_eq!(saved(&app).as_deref(), Some("half a message "));

    // While that paste is checked, another isn't sent, and stays, said.
    keys(&ctx, &mut app, "again");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert!(!sent(&app).iter().any(|c| c.contains("auth/complete")));
    assert_eq!(box_text(&app), "again");

    // It ends: the box is the chat's again, the draft back as it was.
    instance(&mut app, None);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(box_text(&app), "half a message ");
}

#[test]
fn after_cancel_what_is_typed_goes_to_the_agent() {
    let (ctx, mut app) = chat_app("");
    instance(&mut app, Some(waiting(7, 300)));
    frame(&ctx, &mut app, vec![]);
    sent(&app);
    leader(&ctx, &mut app, "ax");
    assert!(bodies(&app, "POST /api/instances/i/auth/cancel").pop().is_some_and(|b| b["id"] == 7), "Cancel names the sign-in");
    // The server says it ended (stopped).
    instance(&mut app, None);
    frame(&ctx, &mut app, vec![]);
    keys(&ctx, &mut app, "i");
    keys(&ctx, &mut app, "hello");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let calls = sent(&app);
    assert!(calls.iter().any(|c| c.ends_with("/prompt")) && !calls.iter().any(|c| c.contains("auth/complete")), "{calls:?}");
}

#[test]
fn the_draft_that_comes_back_is_the_one_written_last() {
    let (ctx, mut app) = chat_app("from before");
    // Written after the desk opened (saved a moment later).
    app.desk.chat_mut().ui.input.text = "written since".into();
    frame(&ctx, &mut app, vec![]);
    instance(&mut app, Some(waiting(7, 300)));
    frame(&ctx, &mut app, vec![]);
    assert!(box_text(&app).is_empty());
    // Finish signing in is the box's; not while a paste is checked.
    assert!(app.desk.chat().actions().iter().any(|a| a.id == "auth.complete"));
    instance(&mut app, None);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(box_text(&app), "written since");
}

#[test]
fn before_the_drafts_are_read_the_box_stays_the_chats() {
    let (ctx, mut app) = full_app();
    crate::chat::tests::conversation_events().into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    let chat = BufferId::new(BufferKind::Chat, 0);
    app.desk.panels = crate::panels::Panels::new(vec![chat], 0);
    app.use_buffer(chat);
    app.desk.chat_mut().ui.input.text = "typed before they came".into();
    instance(&mut app, Some(waiting(7, 300)));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(box_text(&app), "typed before they came", "nothing taken it couldn't keep");
    // Read: then it's the sign-in's, what was typed kept.
    app.desk.shared.drafts.get_mut().arrived(Default::default());
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert!(box_text(&app).is_empty());
    instance(&mut app, None);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(box_text(&app), "typed before they came");
}
