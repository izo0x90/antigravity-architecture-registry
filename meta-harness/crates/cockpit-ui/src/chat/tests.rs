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
                let mut cx = RunCx { net: &net, ctx, input_mode: &mut mode, panel: 0, note: &mut note, asked: &mut None };
                chat.draw(ui, &mut feedback, true, &mut cx);
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
    chat.apply(ClientEvent::InstanceUpdated { snapshot });
    chat.shared.agents.get_mut().take_tools_for_test(harness_contracts::ToolServers::default());
    chat.read();
    chat.apply(session(AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new("turn"), thread_id: ThreadId::new("t"), ordinal: 1, status: TurnStatus::Running, failure: None, usage: None } }));
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
    chat.apply(ClientEvent::SlotUpdated { slot: sub });
    let pick = chat.actions().into_iter().find(|a| a.id == "agent.pick").expect("two slots: a pick");
    let ctx = egui::Context::default();
    let net = NetClient::new("http://127.0.0.1:9");
    let (mut mode, mut note) = (crate::keymap::InputMode::Normal, None);
    let mut cx = RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, panel: 0, note: &mut note, asked: &mut None };
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
    let mut cx = RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, panel: 0, note: &mut note, asked: &mut None };
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
                    let mut cx = RunCx { net: &net, ctx, input_mode: &mut mode, panel: 0, note: &mut note, asked: &mut None };
                    chat.draw(ui, &mut feedback, true, &mut cx);
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
