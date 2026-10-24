use super::*;
use crate::net::NetClient;
use crate::shared::Shared;
use harness_contracts::{AdapterEvent, ClientEvent, FileLocation, InstanceId, ItemId, RuntimePolicy, Thread, ThreadStatus, Turn, TurnId, TurnStatus};

/// A chat over agents of its own: the server's events applied to them,
/// the chat reading them as it does each frame.
#[derive(Default)]
pub(crate) struct Fed {
    pub chat: ChatStreamState,
    pub shared: Shared,
}

impl Fed {
    pub(crate) fn apply(&mut self, event: ClientEvent) {
        self.shared.agents.get_mut().apply(event);
        self.read();
    }

    /// The chat reads the agents (as each frame).
    pub(crate) fn read(&mut self) {
        self.chat.share(&self.shared);
    }
}

impl std::ops::Deref for Fed {
    type Target = ChatStreamState;
    fn deref(&self) -> &ChatStreamState {
        &self.chat
    }
}

impl std::ops::DerefMut for Fed {
    fn deref_mut(&mut self) -> &mut ChatStreamState {
        &mut self.chat
    }
}

fn session(event: AdapterEvent) -> ClientEvent {
    ClientEvent::Session { event }
}

pub(crate) fn item(n: u32, body: ItemBody) -> ClientEvent {
    session(AdapterEvent::ItemUpdated {
        item: TurnItem {
            id: ItemId::new(format!("i{n}")),
            thread_id: ThreadId::new("t"),
            turn_id: TurnId::new("turn"),
            native_id: None,
            parent_id: None,
            ordinal: n,
            status: ItemStatus::Completed,
            title: None,
            body,
        },
    })
}

/// The lead slot, its one session running `thread` (when given).
pub(crate) fn lead(thread: Option<&str>) -> harness_contracts::Slot {
    let key = harness_contracts::SessionKey::new("s1");
    harness_contracts::Slot {
        id: SlotId::new(LEAD),
        role: LEAD.into(),
        name: LEAD.into(),
        instance_id: Some(InstanceId::new("i")),
        policy: RuntimePolicy::default(),
        model: None,
        sessions: vec![harness_contracts::SlotSession { key: key.clone(), native_id: None, title: None, thread: thread.map(ThreadId::new) }],
        current: Some(key),
        started_by: harness_contracts::Starter::User,
        status: harness_contracts::SlotStatus::Ready,
    }
}

/// A thread with a prompt, an answer, a tool call reading two files and a
/// file change: the server's events.
pub(crate) fn conversation_events() -> Vec<ClientEvent> {
    let mut chat = Vec::new();
    let thread = Thread { id: ThreadId::new("t"), instance_id: InstanceId::new("i"), native_id: None, title: None, status: ThreadStatus::Idle, policy: RuntimePolicy::default(), model: None, controls: Default::default(), mcp_servers: Vec::new() };
    chat.push(session(AdapterEvent::ThreadUpdated { thread }));
    chat.push(ClientEvent::SlotUpdated { slot: lead(Some("t")) });
    chat.push(session(AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new("turn"), thread_id: ThreadId::new("t"), ordinal: 1, status: TurnStatus::Completed, failure: None, usage: None } }));
    chat.push(item(0, ItemBody::UserMessage { text: "fix the parser".into() }));
    chat.push(item(1, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "Looking at it.".into() }], streaming: false }));
    let locations = vec![FileLocation { path: "src/parse.rs".into(), line: Some(12) }, FileLocation { path: "src/lex.rs".into(), line: None }];
    chat.push(item(2, ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output: vec![], locations }));
    chat.push(item(3, ItemBody::FileChange { path: "src/parse.rs".into(), diff: None }));
    chat
}

/// That conversation, in a chat of its own.
pub(crate) fn conversation() -> Fed {
    let mut fed = Fed::default();
    for event in conversation_events() {
        fed.apply(event);
    }
    fed
}

#[test]
fn messages_take_a_cursor_and_their_links_open() {
    let mut chat = conversation();
    assert_eq!(chat.target(), None, "no cursor: the stream follows the latest");
    chat.motion(Motion::Up, 1);
    assert_eq!(chat.ui.cursor, Some(3), "the first move starts at the latest message");
    chat.motion(Motion::Up, 2);
    assert_eq!(chat.ui.cursor, Some(1));
    chat.motion(Motion::Last, 1);
    assert_eq!(chat.ui.cursor, Some(3));
    chat.motion(Motion::First, 1);
    assert_eq!(chat.ui.cursor, Some(0));
    assert!(matches!(chat.enter(), Step::Nothing(_)), "a prompt has no file");
    chat.motion(Motion::Line(3), 1);
    assert_eq!(chat.target(), Some(Target::Message { thread: Some("t".into()), turn: Some("turn".into()), index: 2, link: Some(("src/parse.rs".into(), Some(12))) }));
    assert_eq!(chat.enter(), Step::GoTo(GoTo::Code), "Enter opens the file it points to");
    // A hint on the second link picks it.
    chat.move_to("2:1");
    assert_eq!(chat.target().and_then(|t| t.code()), Some(("src/lex.rs".into(), None)));
    // Search finds messages by what they say.
    let items = chat.items();
    assert_eq!(crate::nav::rank_matches(&items, "parser"), vec![0]);
    assert_eq!(crate::nav::rank_matches(&items, "lex.rs"), vec![2]);
    // Backspace lets go: the stream follows the latest again.
    assert_eq!(chat.leave(), Step::Done);
    assert_eq!(chat.ui.cursor, None);
    // What's drawn takes hints: each message, and each link.
    let ctx = egui::Context::default();
    let net = NetClient::new("http://127.0.0.1:9");
    let mut feedback = FeedbackInspectorState::default();
    for _ in 0..2 {
        let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 900.0))), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut mode = crate::keymap::InputMode::Normal;
                let mut note = None;
                let mut cx = RunCx { net: &net, ctx, input_mode: &mut mode, at: crate::keymap::Host::Panel(0), note: &mut note, asked: &mut None };
                chat.draw(ui, &mut feedback, false, &mut cx);
            });
        });
    }
    let keys: Vec<String> = chat.spots().into_iter().map(|s| s.key).collect();
    for key in ["0", "1", "2", "2:0", "2:1", "3", "3:0"] {
        assert!(keys.contains(&key.to_owned()), "{key} in {keys:?}");
    }
}

#[test]
fn its_skills_tool_servers_and_adding_to_the_running_turn_are_actions() {
    let mut chat = conversation();
    let ids = |chat: &Fed| chat.actions().into_iter().map(|a| a.id).collect::<Vec<_>>();
    assert!(!ids(&chat).iter().any(|a| a == "mcp.add" || a == "harness.skills" || a == "chat.inject"), "nothing known yet");
    let mut snapshot = ProviderSnapshot {
        status: ProviderStatus::Ready,
        auth: harness_contracts::AuthState { status: harness_contracts::AuthStatus::SignedIn, account: None, methods: Vec::new(), waiting: None, ended: None },
        skills: vec![harness_contracts::Skill { name: "review".into(), description: Some("Review".into()), path: None, scope: harness_contracts::Scope::User }],
        ..ProviderSnapshot::new(InstanceId::new("i"), harness_contracts::DriverKind::new("acp"), "Claude Code")
    };
    snapshot.capabilities.turns.active_steering = true;
    chat.apply(ClientEvent::InstanceUpdated { snapshot });
    chat.shared.agents.get_mut().take_tools_for_test(harness_contracts::ToolServers::default());
    chat.read();
    chat.apply(session(AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new("turn"), thread_id: ThreadId::new("t"), ordinal: 1, status: TurnStatus::Running, failure: None, usage: None } }));
    chat.ui.input.text = "also".into();
    let ids = ids(&chat);
    for id in ["mcp.add", "harness.skills", "chat.inject"] {
        assert!(ids.iter().any(|a| a == id), "{id} in {ids:?}");
    }
}

#[test]
fn a_view_shows_the_slot_picked_and_the_lead_when_its_slot_closes() {
    let mut chat = conversation();
    let mut sub = lead(None);
    (sub.id, sub.role) = (SlotId::new("sub-1"), "sub".into());
    chat.apply(ClientEvent::SlotUpdated { slot: sub });
    let pick = chat.actions().into_iter().find(|a| a.id == "agent.pick").expect("two slots: a pick");
    let ctx = egui::Context::default();
    let net = NetClient::new("http://127.0.0.1:9");
    let (mut mode, mut note) = (crate::keymap::InputMode::Normal, None);
    let mut cx = RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, at: crate::keymap::Host::Panel(0), note: &mut note, asked: &mut None };
    chat.run(&pick, Some("sub-1".into()), &mut cx);
    assert_eq!((chat.ui.slot.as_str(), chat.active_thread()), ("sub-1", None));
    chat.run(&pick, Some("gone".into()), &mut cx);
    assert_eq!(chat.ui.slot.as_str(), "sub-1", "a slot that isn't there isn't shown");
    chat.apply(ClientEvent::SlotClosed { slot_id: SlotId::new("sub-1") });
    assert_eq!((chat.ui.slot.as_str(), chat.active_thread().map(|t| t.to_string())), (LEAD, Some("t".into())));
}

#[test]
fn next_and_previous_go_through_the_slots_in_order_and_say_the_ends() {
    let mut chat = conversation();
    for (id, role, name) in [("w-1", "worker", "a"), ("sub-2", "sub", "z"), ("sub-1", "sub", "b")] {
        let mut slot = lead(None);
        (slot.id, slot.role, slot.name) = (SlotId::new(id), role.into(), name.into());
        chat.apply(ClientEvent::SlotUpdated { slot });
    }
    let action = |id: &str| chat.actions().into_iter().find(|a| a.id == id).expect(id);
    let (next, prev) = (action("chat.next_slot"), action("chat.prev_slot"));
    let ctx = egui::Context::default();
    let net = NetClient::new("http://127.0.0.1:9");
    let (mut mode, mut note) = (crate::keymap::InputMode::Normal, None);
    let mut cx = RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, at: crate::keymap::Host::Panel(0), note: &mut note, asked: &mut None };
    let mut seen = vec![chat.ui.slot.to_string()];
    for _ in 0..3 {
        chat.run(&next, None, &mut cx);
        seen.push(chat.ui.slot.to_string());
    }
    assert_eq!(seen, [LEAD, "sub-1", "sub-2", "w-1"], "the lead, sub-slots by name, then workers");
    chat.run(&next, None, &mut cx);
    assert_eq!(chat.ui.slot.as_str(), "w-1");
    assert_eq!(cx.note.take().as_deref(), Some("This is the last agent slot."));
    chat.run(&prev, None, &mut cx);
    assert_eq!(chat.ui.slot.as_str(), "sub-2");
}

#[test]
fn a_long_chat_moves_by_the_shared_rules() {
    let mut chat = conversation();
    for n in 4..40 {
        chat.apply(item(n, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: format!("answer {n}") }], streaming: false }));
    }
    let ctx = egui::Context::default();
    let net = NetClient::new("http://127.0.0.1:9");
    let mut feedback = FeedbackInspectorState::default();
    let mut now = 0.0;
    let mut draw = |chat: &mut Fed, frames: usize| {
        for _ in 0..frames {
            now += 0.05;
            let _ = ctx.run(egui::RawInput { time: Some(now), screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 500.0))), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut mode = crate::keymap::InputMode::Normal;
                    let mut note = None;
                    let mut cx = RunCx { net: &net, ctx, input_mode: &mut mode, at: crate::keymap::Host::Panel(0), note: &mut note, asked: &mut None };
                    chat.draw(ui, &mut feedback, false, &mut cx);
                });
            });
        }
        chat.ui.scrolling.viewport.offset.y
    };
    let bottom = draw(&mut chat, 3);
    assert!(bottom > 0.0, "following the latest: at the bottom");
    assert_eq!(chat.ui.scrolling.len(), 40, "each message measured once, following or not");
    // gg straight from following the latest: the top; Backspace, back.
    chat.motion(Motion::First, 1);
    assert_eq!((chat.ui.cursor, draw(&mut chat, 5)), (Some(0), 0.0));
    chat.leave();
    assert_eq!((chat.ui.cursor, draw(&mut chat, 5)), (None, bottom));
    chat.motion(Motion::Up, 1);
    assert_eq!((chat.ui.cursor, draw(&mut chat, 5)), (Some(39), bottom), "the first move lands on the newest, where it shows");
    chat.motion(Motion::First, 1);
    assert_eq!((chat.ui.cursor, draw(&mut chat, 5)), (Some(0), 0.0), "to the first: the top");
    let half = chat.ui.scrolling.viewport.window.y / 2.0;
    chat.motion(Motion::HalfDown, 1);
    let cursor = chat.ui.cursor.unwrap();
    assert_eq!(draw(&mut chat, 8), half, "half a page down: the view moves half a window");
    assert!(cursor > 0, "and the cursor with it");
    // Backspace: to the latest, and following it again.
    chat.leave();
    assert_eq!(draw(&mut chat, 5), bottom, "Backspace: at the latest");
    chat.apply(item(40, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "a new answer".into() }], streaming: false }));
    assert!(draw(&mut chat, 3) > bottom, "and following it: a new message shows");
}

/// A harness that runs its own Google login, signed out, its home kept for
/// the project; and the sign-in under way, as the server says it.
pub(crate) fn signed_out_harness(waiting: Option<harness_contracts::SignInWaiting>) -> ProviderSnapshot {
    let waiting = waiting.map(Box::new);
    ProviderSnapshot {
        status: ProviderStatus::Ready,
        auth: harness_contracts::AuthState {
            status: harness_contracts::AuthStatus::SignedOut,
            account: None,
            methods: vec![harness_contracts::AuthMethod { id: "oauth-personal".into(), name: "Google".into(), description: None, kind: harness_contracts::AuthMethodKind::Agent }],
            waiting,
            ended: None,
        },
        home: Some(harness_contracts::HarnessHome { path: "/work/p/.nah/harness/i".into(), kept: true }),
        ..ProviderSnapshot::new(InstanceId::new("i"), harness_contracts::DriverKind::new("acp"), "Antigravity")
    }
}

/// A sign-in under way, `left_s` seconds from now (on this clock).
pub(crate) fn waiting(id: u64, left_s: i64) -> harness_contracts::SignInWaiting {
    harness_contracts::SignInWaiting {
        id,
        url: Some("https://accounts.google.com/o/oauth2/auth?redirect_uri=http%3A%2F%2F127.0.0.1%3A4000%2F".into()),
        instructions: "Open the link and sign in.".into(),
        expects: Some("the address of the page that failed to load".into()),
        until_ms: Some((crate::attention::now_ms() as i64 + left_s * 1000) as u64),
    }
}

/// The chat's buttons on its top line, by id.
fn on_top(chat: &Fed) -> Vec<String> {
    chat.actions().into_iter().filter(|a| a.control.as_ref().is_some_and(|c| c.at == crate::actions::At::Header)).map(|a| a.id).collect()
}

/// The server's answers reach the agents (as each frame).
fn tick(chat: &mut Fed, ctx: &egui::Context, net: &NetClient) {
    chat.shared.agents.get_mut().tick(net, ctx);
    chat.read();
}

#[test]
fn signing_in_shows_the_servers_sign_in_and_a_paste_that_was_not_taken() {
    let (ctx, net) = (egui::Context::default(), NetClient::new("http://127.0.0.1:9"));
    let mut chat = conversation();
    chat.apply(ClientEvent::InstanceUpdated { snapshot: signed_out_harness(None) });
    let status: Vec<(String, Option<String>)> = crate::hosted::Hosted::status(&chat.chat).into_iter().map(|s| (s.text, s.detail)).collect();
    assert!(status.iter().any(|(_, detail)| detail.as_deref().is_some_and(|d| d.contains("/work/p/.nah/harness/i"))), "where its home is: {status:?}");
    assert!(on_top(&chat).contains(&"auth.login".to_owned()), "signed out: Sign in");

    // Asked for: not again until it's answered.
    chat.shared.agents.get_mut().act(SlotId::new(LEAD), Act::SignIn { instance: InstanceId::new("i"), method: "oauth-personal".into() }, &net, &ctx);
    chat.read();
    assert!(!on_top(&chat).contains(&"auth.login".to_owned()), "one sign-in at a time");
    assert!(chat.agents().signing_in(&InstanceId::new("i")), "said while it starts");

    // The server's sign-in under way (any device sees it): open it, paste back.
    chat.apply(ClientEvent::InstanceUpdated { snapshot: signed_out_harness(Some(waiting(7, 300))) });
    assert!(net.answer("POST /api/instances/i/auth/oauth-personal", 200, &serde_json::json!(waiting(7, 300))));
    tick(&mut chat, &ctx, &net);
    let top = on_top(&chat);
    assert!(top.contains(&"auth.page".to_owned()) && top.contains(&"auth.copy".to_owned()) && top.contains(&"auth.cancel".to_owned()), "{top:?}");

    // Pasted: being checked, then refused while it still waits.
    chat.shared.agents.get_mut().act(SlotId::new(LEAD), Act::FinishSignIn { instance: InstanceId::new("i"), answer: "4/abc".into() }, &net, &ctx);
    chat.read();
    assert_eq!(net.bodies.lock().unwrap().last().map(|(_, body)| body["id"].clone()), Some(serde_json::json!(7)), "the paste names its sign-in");
    assert!(net.answer("POST /api/instances/i/auth/complete", 400, &serde_json::json!({ "error": "that isn't the address" })));
    tick(&mut chat, &ctx, &net);
    assert!(chat.agents().pasted(&InstanceId::new("i"), 7).is_some_and(|p| p.refused.is_some() && !p.finishing));
    assert!(chat.error().is_none());

    // A new sign-in meanwhile: a late answer about the old one doesn't touch it.
    chat.shared.agents.get_mut().act(SlotId::new(LEAD), Act::FinishSignIn { instance: InstanceId::new("i"), answer: "http://127.0.0.1:4000/?code=x".into() }, &net, &ctx);
    chat.apply(ClientEvent::InstanceUpdated { snapshot: signed_out_harness(Some(waiting(8, 300))) });
    assert!(net.answer("POST /api/instances/i/auth/complete", 200, &serde_json::Value::Null));
    tick(&mut chat, &ctx, &net);
    assert!(chat.agents().waiting(&InstanceId::new("i")).is_some_and(|w| w.id == 8) && chat.agents().pasted(&InstanceId::new("i"), 8).is_none(), "the new one still waits, untouched");

    // Over on the server (taken, or ended): its answer is said as an error.
    chat.shared.agents.get_mut().act(SlotId::new(LEAD), Act::FinishSignIn { instance: InstanceId::new("i"), answer: "http://127.0.0.1:4000/?code=y".into() }, &net, &ctx);
    chat.apply(ClientEvent::InstanceUpdated { snapshot: signed_out_harness(None) });
    assert!(net.answer("POST /api/instances/i/auth/complete", 502, &serde_json::json!({ "error": "the sign-in failed: bad code" })));
    tick(&mut chat, &ctx, &net);
    assert!(chat.error().is_some_and(|e| e.contains("bad code")), "{:?}", chat.error());
    assert!(on_top(&chat).contains(&"auth.login".to_owned()), "sign in again");
}

#[test]
fn a_sign_in_that_stopped_waiting_offers_a_new_one_and_says_how_it_ended() {
    let mut chat = conversation();
    chat.apply(ClientEvent::InstanceUpdated { snapshot: signed_out_harness(Some(waiting(3, -1))) });
    let top = on_top(&chat);
    assert!(top.contains(&"auth.login".to_owned()) && !top.contains(&"auth.complete".to_owned()) && !top.contains(&"auth.page".to_owned()), "{top:?}");
    // The server ends it, saying how: every device reads the same.
    let mut over = signed_out_harness(None);
    over.auth.ended = Some(Box::new(harness_contracts::SignInEnded { id: 3, how: harness_contracts::SignInEnd::RanOut }));
    chat.apply(ClientEvent::InstanceUpdated { snapshot: over });
    assert_eq!(chat.agents().sign_in_ended(&InstanceId::new("i")).map(|e| e.how.clone()), Some(harness_contracts::SignInEnd::RanOut));
    assert!(on_top(&chat).contains(&"auth.login".to_owned()));
    // Said whatever the status (signed in, switching account, say), until put away.
    let mut unknown = signed_out_harness(None);
    unknown.auth.status = harness_contracts::AuthStatus::Unknown;
    unknown.auth.ended = Some(Box::new(harness_contracts::SignInEnded { id: 5, how: harness_contracts::SignInEnd::Failed { reason: "bad code".into() } }));
    chat.apply(ClientEvent::InstanceUpdated { snapshot: unknown.clone() });
    let top = on_top(&chat);
    assert!(chat.sign_in_end_to_say().is_some() && top.contains(&"auth.end.dismiss".to_owned()) && top.contains(&"auth.login".to_owned()), "{top:?}");
    // Signed in since (another way): nothing to say.
    let mut since = unknown.clone();
    since.auth.status = harness_contracts::AuthStatus::SignedIn;
    chat.apply(ClientEvent::InstanceUpdated { snapshot: since });
    assert!(chat.sign_in_end_to_say().is_none());
    chat.apply(ClientEvent::InstanceUpdated { snapshot: unknown });
    let (ctx, net) = (egui::Context::default(), NetClient::new("http://127.0.0.1:9"));
    chat.shared.agents.get_mut().act(SlotId::new(LEAD), Act::PutSignInEndAway { id: 5 }, &net, &ctx);
    chat.read();
    assert!(chat.sign_in_end_to_say().is_none() && !on_top(&chat).contains(&"auth.end.dismiss".to_owned()));
}
