use super::*;
use crate::net::NetClient;
use crate::shared::Shared;
use harness_contracts::{AdapterEvent, ClientEvent, FileLocation, InstanceId, ItemId, RuntimePolicy, Thread, ThreadStatus, ToolKind, Turn, TurnId, TurnStatus};

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
    chat.push(item(0, ItemBody::UserMessage { text: "fix the parser".into(), pieces: Vec::new() }));
    chat.push(item(1, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "Looking at it.".into() }], streaming: false }));
    let locations = vec![FileLocation { path: "src/parse.rs".into(), line: Some(12) }, FileLocation { path: "src/lex.rs".into(), line: None }];
    chat.push(item(2, ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output: vec![], locations }));
    chat.push(item(3, ItemBody::FileChange { path: "src/parse.rs".into(), diff: None }));
    // (All of it the state as it was: what comes after is news.)
    chat.push(ClientEvent::Replayed);
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
    // Its rows: the prompt, the answer, and the run of two calls folded.
    chat.motion(Motion::Up, 1);
    assert_eq!(chat.ui.cursor, Some(2), "the first move starts at the latest row: the run");
    chat.motion(Motion::Up, 2);
    assert_eq!(chat.ui.cursor, Some(0));
    chat.motion(Motion::Last, 1);
    assert_eq!(chat.ui.cursor, Some(2));
    chat.motion(Motion::First, 1);
    assert_eq!(chat.ui.cursor, Some(0));
    assert!(matches!(chat.enter(), Step::Nothing(_)), "a prompt has no file");
    // Message 3 is the run's first: folded, the run (no file of its own),
    // and Enter unfolds it.
    chat.motion(Motion::Line(3), 1);
    assert_eq!(chat.target(), Some(Target::Message { thread: Some("t".into()), turn: Some("turn".into()), index: 2, link: None }));
    assert_eq!((chat.enter(), chat.shape().rows.len()), (Step::Done, 4), "Enter on a folded run opens it: each call a row");
    assert_eq!(chat.target().and_then(|t| t.code()), Some(("src/parse.rs".into(), Some(12))));
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
    assert_eq!(chat.ui.scrolling.len(), 39, "each row measured once, following or not (the run of two calls is one)");
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

/// What's under its banners (their ways out, while they're where you
/// are), by id.
fn on_banners(chat: &Fed) -> Vec<String> {
    chat.actions().into_iter().filter(|a| a.at() == Some(crate::actions::At::Item)).map(|a| a.id).collect()
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
    assert!(on_banners(&chat).contains(&"auth.login".to_owned()), "signed out: Sign in");

    // Asked for: not again until it's answered.
    chat.shared.agents.get_mut().act(SlotId::new(LEAD), Act::SignIn { instance: InstanceId::new("i"), method: "oauth-personal".into() }, &net, &ctx);
    chat.read();
    assert!(!on_banners(&chat).contains(&"auth.login".to_owned()), "one sign-in at a time");
    assert!(chat.agents().signing_in(&InstanceId::new("i")), "said while it starts");

    // The server's sign-in under way (any device sees it): open it, paste back.
    chat.apply(ClientEvent::InstanceUpdated { snapshot: signed_out_harness(Some(waiting(7, 300))) });
    assert!(net.answer("POST /api/instances/i/auth/oauth-personal", 200, &serde_json::json!(waiting(7, 300))));
    tick(&mut chat, &ctx, &net);
    let top = on_banners(&chat);
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
    assert!(on_banners(&chat).contains(&"auth.login".to_owned()), "sign in again");
}

#[test]
fn a_sign_in_that_stopped_waiting_offers_a_new_one_and_says_how_it_ended() {
    let mut chat = conversation();
    chat.apply(ClientEvent::InstanceUpdated { snapshot: signed_out_harness(Some(waiting(3, -1))) });
    let top = on_banners(&chat);
    assert!(top.contains(&"auth.login".to_owned()) && !top.contains(&"auth.complete".to_owned()) && !top.contains(&"auth.page".to_owned()), "{top:?}");
    // The server ends it, saying how: every device reads the same.
    let mut over = signed_out_harness(None);
    over.auth.ended = Some(Box::new(harness_contracts::SignInEnded { id: 3, how: harness_contracts::SignInEnd::RanOut }));
    chat.apply(ClientEvent::InstanceUpdated { snapshot: over });
    assert_eq!(chat.agents().sign_in_ended(&InstanceId::new("i")).map(|e| e.how.clone()), Some(harness_contracts::SignInEnd::RanOut));
    assert!(on_banners(&chat).contains(&"auth.login".to_owned()));
    // Said whatever the status (signed in, switching account, say), until put away.
    let mut unknown = signed_out_harness(None);
    unknown.auth.status = harness_contracts::AuthStatus::Unknown;
    unknown.auth.ended = Some(Box::new(harness_contracts::SignInEnded { id: 5, how: harness_contracts::SignInEnd::Failed { reason: "bad code".into() } }));
    chat.apply(ClientEvent::InstanceUpdated { snapshot: unknown.clone() });
    let top = on_banners(&chat);
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
    assert!(chat.sign_in_end_to_say().is_none() && !on_banners(&chat).contains(&"auth.end.dismiss".to_owned()));
}

/// A run of calls folds and opens with the fold keys every view has, and
/// so does a call's output; everything at once; the cursor inside a run
/// folded again is on the run's row.
#[test]
fn a_run_of_calls_and_a_calls_output_fold_with_the_shared_keys() {
    let mut chat = conversation();
    let output = vec![ContentBlock::Text { text: "fn parse() {}".into() }];
    chat.apply(item(4, ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output, locations: vec![] }));
    let rows = |chat: &Fed| chat.shape().rows.len();
    assert_eq!(rows(&chat), 3, "the prompt, the answer, the run of three calls");
    chat.motion(Motion::Up, 1);
    chat.fold(Fold::Toggle);
    assert_eq!((rows(&chat), chat.ui.cursor), (5, Some(2)), "opened: each call a row");
    chat.motion(Motion::Last, 1);
    assert_eq!(chat.ui.cursor, Some(4));
    let open = |chat: &Fed| chat.thread().is_some_and(|v| chat.ui.folds.item_open(v.in_stream(4).unwrap()));
    assert!(!open(&chat), "its output folded");
    chat.fold(Fold::Open);
    assert!(open(&chat), "z o: its output open");
    chat.fold(Fold::CloseAll);
    assert_eq!((rows(&chat), chat.cursor_key().as_deref(), open(&chat)), (3, Some("2"), false), "everything folded: the cursor on the run");
    chat.fold(Fold::OpenAll);
    assert_eq!((rows(&chat), open(&chat)), (5, true));
}


/// `event` (an item's) in turn `turn`.
fn in_turn(mut event: ClientEvent, turn: &str) -> ClientEvent {
    if let ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } } = &mut event {
        item.turn_id = TurnId::new(turn);
    }
    event
}

/// The cursor stays on its item when another comes before it (the
/// running turn's, placed before a queued turn's message); what was
/// opened by hand stays open through a reconnect, a frame read before the
/// thread came again included.
#[test]
fn the_cursor_stays_on_its_item_and_what_was_opened_through_a_reconnect() {
    let mut chat = conversation();
    let turn = |id: &str, ordinal, status| session(AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new(id), thread_id: ThreadId::new("t"), ordinal, status, failure: None, usage: None } });
    chat.apply(turn("turn", 1, TurnStatus::Running));
    chat.apply(turn("next", 2, TurnStatus::Queued));
    chat.apply(in_turn(item(10, ItemBody::UserMessage { text: "then the lexer".into(), pieces: Vec::new() }), "next"));
    let on = |chat: &Fed| chat.thread().and_then(|v| v.in_stream(chat.ui.cursor?).map(|i| i.id.to_string()));
    chat.motion(Motion::Up, 1);
    chat.motion(Motion::Last, 1);
    assert_eq!(on(&chat).as_deref(), Some("i10"));
    chat.apply(item(4, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "Done.".into() }], streaming: false }));
    assert_eq!(on(&chat).as_deref(), Some("i10"), "the running turn's item came before it: the cursor stayed on the message");
    // The run opened by hand; the server gone and back.
    chat.motion(Motion::Line(3), 1);
    chat.fold(Fold::Toggle);
    let rows = chat.shape().rows.len();
    chat.apply(ClientEvent::Resync);
    for event in conversation_events() {
        chat.apply(event);
    }
    chat.apply(turn("turn", 1, TurnStatus::Running));
    chat.apply(turn("next", 2, TurnStatus::Queued));
    chat.apply(in_turn(item(10, ItemBody::UserMessage { text: "then the lexer".into(), pieces: Vec::new() }), "next"));
    chat.apply(item(4, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "Done.".into() }], streaming: false }));
    assert_eq!(chat.shape().rows.len(), rows, "the run still open, its rows as they were");
}

/// Quiet too long, the live line says whether the agent can still be
/// reached: the server reaches it, its harness starting again or failed,
/// its slot unable to run.
#[test]
fn quiet_too_long_says_whether_the_agent_can_be_reached() {
    use crate::indicator::{Activity, Liveness};
    let mut chat = conversation();
    let quiet = Activity { liveness: Liveness::Quiet, updates: Vec::new(), since_last: Some(130.0), doing: "Reading".into() };
    let said = |chat: &mut Fed, status: harness_contracts::ProviderStatus| {
        let mut snapshot = crate::agents::tests::snapshot(status);
        snapshot.instance_id = InstanceId::new("i");
        chat.apply(ClientEvent::InstanceUpdated { snapshot });
        chat.live_words(&quiet).0
    };
    assert_eq!(said(&mut chat, harness_contracts::ProviderStatus::Ready), "No word for 2 min · the server still reaches it");
    assert_eq!(said(&mut chat, harness_contracts::ProviderStatus::Starting), "No word for 2 min · its harness is starting");
    assert_eq!(said(&mut chat, harness_contracts::ProviderStatus::Error { message: "it crashed".into() }), "No word for 2 min · its harness failed: it crashed");
    let mut slot = lead(Some("t"));
    slot.status = harness_contracts::SlotStatus::Unavailable { reason: "no claude on the PATH".into(), fix: None };
    chat.apply(ClientEvent::SlotUpdated { slot });
    assert_eq!(chat.live_words(&quiet).0, "No word for 2 min · it can't run: no claude on the PATH");
}

/// A chat not picked for starts as the config says.
#[test]
fn a_chat_starts_as_the_config_says() {
    let mut chat = conversation();
    let config = crate::config::UiConfig::with_overrides("[chat]\nshows = \"quiet\"\nindicator = \"none\"").unwrap();
    chat.shared.settings.set(crate::shared::Settings::of(&config));
    chat.read();
    assert_eq!((chat.ui.folds.verbosity(), chat.ui.indicator.kind()), (crate::config::Verbosity::Quiet, crate::config::IndicatorKind::None));
}

/// A place restored before its thread's all here (the layout comes before
/// the replay): the cursor goes on what's at that place once it is, not
/// on what was there midway.
#[test]
fn a_place_restored_before_the_thread_is_all_here_lands_once_it_is() {
    let mut chat = Fed::default();
    crate::nav::Navigable::restore(&mut chat.chat, &crate::nav::Place::of(&ChatPlace { message: Some(2), slot: None }));
    let mut events = conversation_events();
    let replayed = events.pop();
    // (Its sub-agent's step, last to come, shown after its call: second.)
    let mut sub = item(9, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "the helper's".into() }], streaming: false });
    if let ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } } = &mut sub {
        item.parent_id = Some(ItemId::new("i1"));
    }
    events.push(sub);
    events.extend(replayed);
    for event in events {
        chat.apply(event);
    }
    let on = chat.thread().and_then(|v| v.in_stream(chat.ui.cursor?).map(|i| i.id.to_string()));
    assert_eq!(on.as_deref(), Some("i9"), "on what's at its place, all here");
}

/// Opened on an agent that's said nothing since: quiet too long, counted
/// from when it became known, not waiting for ever.
#[test]
fn an_agent_silent_since_it_became_known_reads_quiet() {
    let mut chat = conversation();
    chat.shared.socket.set(crate::net::WsState::Open);
    chat.read();
    let view = chat.thread().unwrap().clone();
    let activity = chat.activity(&view, 130.0);
    assert_eq!(activity.liveness, crate::indicator::Liveness::Quiet);
    assert!(chat.live_words(&activity).0.starts_with("No word for 2 min"), "{:?}", chat.live_words(&activity).0);
    // A sub-agent of its at work: it waits on it, not gone quiet.
    let sub = harness_contracts::Subagent { id: harness_contracts::SubagentId::new("a1"), thread_id: ThreadId::new("t"), item_id: ItemId::new("i9"), title: "Helper".into(), kind: None, description: None, prompt: None, doing: None, status: harness_contracts::SubagentStatus::Running, can_cancel: false };
    chat.apply(session(AdapterEvent::SubagentUpdated { subagent: sub }));
    let view = chat.thread().unwrap().clone();
    let activity = chat.activity(&view, 130.0);
    assert_eq!(activity.liveness, crate::indicator::Liveness::Waiting);
    assert!(chat.live_words(&activity).0.ends_with("1 sub-agent working"), "{:?}", chat.live_words(&activity).0);
}

/// What's picked is each chat's: another slot's chat shows as the config
/// says till picked for, and back on the first it's as picked.
#[test]
fn what_is_picked_is_each_chats_own() {
    let mut chat = conversation();
    chat.ui.picked.insert(SlotId::new(LEAD), crate::chat::Picked { shows: Some(crate::config::Verbosity::Quiet), at_work: None });
    chat.read();
    assert_eq!(chat.ui.folds.verbosity(), crate::config::Verbosity::Quiet);
    chat.ui.slot = SlotId::new("coder");
    chat.read();
    assert_eq!(chat.ui.folds.verbosity(), crate::config::Verbosity::Normal, "another chat: the config's");
    chat.ui.slot = SlotId::new(LEAD);
    chat.read();
    assert_eq!(chat.ui.folds.verbosity(), crate::config::Verbosity::Quiet, "back: as picked");
}
