//! A desk per server, fed by home's socket: switching keeps each desk as
//! it was, what waits is counted across them, the next one waiting crosses
//! servers, and each desk's calls go to its own server.

use super::*;
use harness_contracts::{ClientEvent, HomeEvent, LinkState, Notification, NotificationId, NotificationKind, NotificationState, ServerView, SlotId};

fn view(name: &str) -> ServerView {
    ServerView { name: name.into(), url: if name == "here" { String::new() } else { format!("http://{name}:8080") }, link: LinkState::Connected, project: true }
}

fn from_home(app: &CockpitApp, events: Vec<HomeEvent>) {
    app.home.from_home.lock().unwrap().extend(events);
}

fn event(server: &str, event: ClientEvent) -> HomeEvent {
    HomeEvent::Event { server: server.into(), event: Box::new(event) }
}

fn note(id: &str, kind: NotificationKind, at_ms: u64) -> ClientEvent {
    let notification = Notification { id: NotificationId::new(id), server: None, level: kind.level(), kind, slot: Some(SlotId::new("lead")), thread: None, request: None, title: format!("{id} title"), detail: None, at_ms, state: NotificationState::Open };
    ClientEvent::NotificationUpdated { notification }
}

/// The app with home's own desk and a "cloud" one; one item waiting on
/// cloud, an update here.
fn two_desks() -> (egui::Context, CockpitApp) {
    let (ctx, mut app) = full_app();
    from_home(&app, vec![HomeEvent::Servers { servers: vec![view("here"), view("cloud")] }, event("cloud", note("req:c1", NotificationKind::Approval, 5)), event("here", note("turn:h1", NotificationKind::TurnFinished, 3))]);
    frame(&ctx, &mut app, vec![]);
    (ctx, app)
}

#[test]
fn each_server_gets_a_desk_fed_its_own_events_and_whats_waiting_is_counted_across_them() {
    let (ctx, mut app) = two_desks();
    assert_eq!((app.desk.server.as_str(), app.away.keys().cloned().collect::<Vec<_>>()), ("here", vec!["cloud".to_owned()]));
    assert_eq!(app.away["cloud"].chat.store.notifications.len(), 1, "taken while not shown");
    assert!(app.desk.chat.store.notifications.contains_key(&NotificationId::new("turn:h1")));
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains(&format!("{} 1", egui_phosphor::regular::BELL_RINGING)) && text.contains(&format!("{} 1", egui_phosphor::regular::BELL)), "{text}");
    assert!(text.contains("here ●"), "the chip says something waits elsewhere: {text}");
    // A Resync for one server drops only its desk's state.
    from_home(&app, vec![event("cloud", ClientEvent::Resync)]);
    frame(&ctx, &mut app, vec![]);
    assert!(app.away["cloud"].chat.store.notifications.is_empty() && app.away["cloud"].net.reconnected, "dropped; it refetches when shown");
    assert_eq!(app.desk.chat.store.notifications.len(), 1, "here's untouched");
}

#[test]
fn switching_keeps_each_desk_as_it_was_and_its_calls_go_to_its_server() {
    let (ctx, mut app) = two_desks();
    app.desk.graph.select(Some("rust:a::step".into()));
    app.desk.net.sent.lock().unwrap().clear();
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"server.switch","value":"cloud"}"#).unwrap();
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.server, "cloud");
    let sent = app.desk.net.sent.lock().unwrap().clone();
    assert!(sent.iter().any(|s| s == "GET /s/cloud/api/ui/state"), "shown the first time: its layout, from its server: {sent:?}");
    assert!(sent.iter().all(|s| s.contains("/s/cloud/")), "every call through home to it: {sent:?}");
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"server.switch","value":"here"}"#).unwrap();
    assert_eq!((app.desk.server.as_str(), selected(&app)), ("here", "rust:a::step"), "as it was");
}

#[test]
fn the_next_waiting_crosses_servers_and_a_removed_server_takes_its_desk_away() {
    let (ctx, mut app) = two_desks();
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"attention.next_waiting"}"#).unwrap();
    assert_eq!((app.desk.server.as_str(), app.looked_at().map(|id| id.as_str())), ("cloud", Some("req:c1")), "its desk shown, the look on it");
    keys(&ctx, &mut app, "n");
    assert_eq!((app.desk.server.as_str(), app.looked_at().map(|id| id.as_str())), ("here", Some("turn:h1")), "n crosses to here's update");
    let actions = from_outside(&ctx, &mut app, r#"{"op":"actions"}"#).unwrap();
    let waiting = actions.as_array().unwrap().iter().find(|a| a["id"] == "attention.waiting").expect("listed");
    assert!(waiting.to_string().contains("\"cloud/req:c1\""), "{waiting}");
    // Shown, then removed: back to here's.
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"server.switch","value":"cloud"}"#).unwrap();
    from_home(&app, vec![HomeEvent::Servers { servers: vec![view("here")] }]);
    // Said as home's message is taken (a reply landing later may say more).
    app.take_from_home(&ctx);
    assert!(app.desk.server == "here" && app.away.is_empty());
    assert!(app.note.as_deref().unwrap_or_default().contains("cloud was removed"), "{:?}", app.note);
}

#[test]
fn a_home_with_no_project_shows_a_servers_desk_or_says_how_to_link_one() {
    let (ctx, mut app) = full_app();
    let mut here = view("here");
    here.project = false;
    from_home(&app, vec![HomeEvent::Servers { servers: vec![here.clone()] }]);
    frame(&ctx, &mut app, vec![]);
    assert!(app.note.as_deref().unwrap_or_default().contains("link a server"), "{:?}", app.note);
    from_home(&app, vec![HomeEvent::Servers { servers: vec![here, view("cloud")] }]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.server, "cloud", "linked: shown");
    let actions = from_outside(&ctx, &mut app, r#"{"op":"actions"}"#).unwrap();
    assert!(!actions.as_array().unwrap().iter().any(|a| a["id"] == "server.switch"), "nothing else to switch to");
}
