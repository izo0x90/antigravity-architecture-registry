use super::*;
use harness_contracts::{AdapterEvent, FileLocation, InstanceId, ItemId, RuntimePolicy, Thread, ThreadStatus, Turn, TurnId, TurnStatus};

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

/// A thread with a prompt, an answer, a tool call reading two files and a file change.
pub(crate) fn conversation() -> ChatStreamState {
    let mut chat = ChatStreamState::default();
    let thread = Thread { id: ThreadId::new("t"), instance_id: InstanceId::new("i"), native_id: None, title: None, status: ThreadStatus::Idle, policy: RuntimePolicy::default(), model: None, controls: Default::default(), mcp_servers: Vec::new() };
    chat.process_event(session(AdapterEvent::ThreadUpdated { thread }));
    chat.process_event(ClientEvent::SlotUpdated { slot: lead(Some("t")) });
    chat.process_event(session(AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new("turn"), thread_id: ThreadId::new("t"), ordinal: 1, status: TurnStatus::Completed, failure: None, usage: None } }));
    chat.process_event(item(0, ItemBody::UserMessage { text: "fix the parser".into() }));
    chat.process_event(item(1, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "Looking at it.".into() }], streaming: false }));
    let locations = vec![FileLocation { path: "src/parse.rs".into(), line: Some(12) }, FileLocation { path: "src/lex.rs".into(), line: None }];
    chat.process_event(item(2, ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output: vec![], locations }));
    chat.process_event(item(3, ItemBody::FileChange { path: "src/parse.rs".into(), diff: None }));
    chat
}

#[test]
fn a_harness_that_cant_start_says_why_and_the_fix() {
    let mut chat = ChatStreamState::default();
    assert_eq!(chat.harness_problem(), None);
    let snapshot = |status: ProviderStatus| ProviderSnapshot {
        instance_id: InstanceId::new("antigravity"),
        driver: harness_contracts::DriverKind::new("antigravity"),
        display_name: "Antigravity".into(),
        status,
        auth: harness_contracts::AuthState { status: harness_contracts::AuthStatus::Unknown, account: None, methods: Vec::new() },
        capabilities: Default::default(),
        models: Vec::new(),
        commands: Vec::new(),
        skills: Vec::new(),
        runtime_modes: Vec::new(),
        supports_plan_mode: false,
        tool_servers: Vec::new(),
        problems: Vec::new(),
    };
    let reason = "antigravity-acp isn't installed (fix: meta-harness pack install antigravity-acp)";
    chat.process_event(ClientEvent::InstanceUpdated { snapshot: snapshot(ProviderStatus::Unavailable { reason: reason.into() }) });
    assert_eq!(chat.harness_problem().as_deref(), Some(format!("Antigravity: {reason}").as_str()), "the AGENT light's hint");
    chat.process_event(ClientEvent::InstanceUpdated { snapshot: snapshot(ProviderStatus::Ready) });
    assert_eq!(chat.harness_problem(), None);
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
    assert_eq!(chat.target(), Some(Target::Message { thread: Some("t".into()), index: 2, link: Some(("src/parse.rs".into(), Some(12))) }));
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
                let mut cx = RunCx { net: &net, ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[], hints: &[] };
                chat.render(ui, &mut feedback, true, &mut cx);
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
    let ids = |chat: &ChatStreamState| chat.actions().into_iter().map(|a| a.id).collect::<Vec<_>>();
    assert!(!ids(&chat).iter().any(|a| a == "mcp.add" || a == "harness.skills" || a == "chat.inject"), "nothing known yet");
    let mut snapshot = ProviderSnapshot {
        instance_id: InstanceId::new("i"),
        driver: harness_contracts::DriverKind::new("acp"),
        display_name: "Claude Code".into(),
        status: ProviderStatus::Ready,
        auth: harness_contracts::AuthState { status: harness_contracts::AuthStatus::SignedIn, account: None, methods: Vec::new() },
        capabilities: Default::default(),
        models: Vec::new(),
        commands: Vec::new(),
        skills: vec![harness_contracts::Skill { name: "review".into(), description: Some("Review".into()), path: None, scope: harness_contracts::Scope::User }],
        runtime_modes: Vec::new(),
        supports_plan_mode: false,
        tool_servers: Vec::new(),
        problems: Vec::new(),
    };
    snapshot.capabilities.turns.active_steering = true;
    chat.process_event(ClientEvent::InstanceUpdated { snapshot });
    chat.tools = Some(ToolServers::default());
    chat.process_event(session(AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new("turn"), thread_id: ThreadId::new("t"), ordinal: 1, status: TurnStatus::Running, failure: None, usage: None } }));
    chat.ui.input_buffer = "also".into();
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
    chat.process_event(ClientEvent::SlotUpdated { slot: sub });
    let pick = chat.actions().into_iter().find(|a| a.id == "agent.pick").expect("two slots: a pick");
    let ctx = egui::Context::default();
    let net = NetClient::new("http://127.0.0.1:9");
    let (mut mode, mut note) = (crate::keymap::InputMode::Normal, None);
    let mut cx = RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[], hints: &[] };
    chat.run(&pick, Some("sub-1".into()), &mut cx);
    assert_eq!((chat.ui.slot.as_str(), chat.active_thread()), ("sub-1", None));
    chat.run(&pick, Some("gone".into()), &mut cx);
    assert_eq!(chat.ui.slot.as_str(), "sub-1", "a slot that isn't there isn't shown");
    chat.process_event(ClientEvent::SlotClosed { slot_id: SlotId::new("sub-1") });
    assert_eq!((chat.ui.slot.as_str(), chat.active_thread().map(|t| t.to_string())), (LEAD, Some("t".into())));
}

#[test]
fn a_prompt_with_no_harness_anywhere_says_so() {
    let mut chat = ChatStreamState::default();
    let (ctx, net) = (egui::Context::default(), NetClient::new("http://127.0.0.1:9"));
    chat.say(&net, &ctx, "hello".into());
    assert!(matches!(chat.session_status(), SessionStatus::Failed(e) if e.contains("No harness instance")));
}
