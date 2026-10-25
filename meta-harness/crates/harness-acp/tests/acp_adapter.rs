//! The adapter, sessions, checks and saved sessions against the mock agent.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use harness_acp::testing::{MockAgent, MockSetup};
use harness_acp::{AcpAdapter, AcpSessionCatalog, AcpSettings, Profile, acp, probe};
use harness_contracts::*;
use harness_core::{ProviderAdapter, SessionCatalog, SessionRuntime};

const PROJECT: &str = "/work/project";

fn settings(toml_text: &str) -> AcpSettings {
    let table: toml::Table = toml::from_str(toml_text).unwrap();
    AcpSettings::parse(&serde_json::to_value(table).unwrap()).unwrap()
}

fn profile(toml_text: &str) -> Arc<Profile> {
    Arc::new(Profile::new(settings(toml_text), Path::new(PROJECT), None))
}

const MODES: &str = r#"
[modes]
approval_required = "default"
full_access = "yolo"
"#;

fn agent(setup: MockSetup) -> MockAgent {
    MockAgent::new(MockSetup { modes: vec!["default".into(), "yolo".into()], ..setup })
}

fn config(mode: RuntimeMode) -> SessionConfig {
    SessionConfig {
        thread_id: ThreadId::new("thread-1"),
        policy: RuntimePolicy { runtime_mode: mode, ..RuntimePolicy::default() },
        model: None,
        resume: None,
        mcp_servers: Vec::new(),
    }
}

async fn open_with(agent: &MockAgent, profile: Arc<Profile>, config: SessionConfig) -> Box<dyn SessionRuntime> {
    let adapter = AcpAdapter::new(InstanceId::new("mock"), profile, Arc::new(agent.clone()), Arc::new(|_| {}));
    adapter.open_session(config).await.expect("open session")
}

async fn open(mode: RuntimeMode) -> (Box<dyn SessionRuntime>, MockAgent) {
    let agent = agent(MockSetup::default());
    (open_with(&agent, profile(MODES), config(mode)).await, agent)
}

fn say(text: &str) -> TurnInput {
    TurnInput::of(text)
}

/// Collects events until `done` matches one (or times out).
async fn collect_until(events: &mut (impl futures::Stream<Item = AdapterEvent> + Unpin), seen: &mut Vec<AdapterEvent>, done: impl Fn(&AdapterEvent) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.next().await {
            let stop = done(&event);
            seen.push(event);
            if stop {
                return;
            }
        }
    })
    .await
    .expect("timed out waiting for event");
}

fn turn_ended(ordinal: u32) -> impl Fn(&AdapterEvent) -> bool {
    move |e| matches!(e, AdapterEvent::TurnUpdated { turn } if turn.status.is_terminal() && turn.ordinal == ordinal)
}

fn pending_request(e: &AdapterEvent) -> bool {
    matches!(e, AdapterEvent::RequestUpdated { request } if request.status == RequestStatus::Pending)
}

fn last_request(seen: &[AdapterEvent]) -> RuntimeRequest {
    seen.iter().rev().find_map(|e| match e { AdapterEvent::RequestUpdated { request } => Some(request.clone()), _ => None }).unwrap()
}

fn latest_items(seen: &[AdapterEvent]) -> Vec<TurnItem> {
    let mut items: Vec<TurnItem> = Vec::new();
    for e in seen {
        if let AdapterEvent::ItemUpdated { item } = e {
            match items.iter_mut().find(|i| i.id == item.id) {
                Some(existing) => *existing = item.clone(),
                None => items.push(item.clone()),
            }
        }
    }
    items
}

fn said(seen: &[AdapterEvent], turn: &TurnId) -> Vec<String> {
    latest_items(seen)
        .into_iter()
        .filter(|i| &i.turn_id == turn)
        .filter_map(|i| match i.body {
            ItemBody::UserMessage { text, .. } => Some(format!("> {text}")),
            ItemBody::AssistantMessage { content, .. } => Some(content.iter().map(|c| if let ContentBlock::Text { text } = c { text.clone() } else { String::new() }).collect()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn full_turn_maps_every_update_and_waits_for_approval() {
    let (session, agent) = open(RuntimeMode::ApprovalRequired).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    session.start_turn(say("[tour] write main")).await.unwrap();

    collect_until(&mut events, &mut seen, pending_request).await;
    let request = last_request(&seen);
    assert_eq!(request.kind, RequestKind::Approval { action: harness_contracts::ToolAction::FileChange });
    assert_eq!(request.options.iter().map(|o| o.decision).collect::<Vec<_>>(), vec![ApprovalDecision::Accept, ApprovalDecision::AcceptForSession, ApprovalDecision::Decline]);

    session.respond(&request.id, RequestResponse { option_id: Some("always".into()), ..Default::default() }).await.unwrap();
    collect_until(&mut events, &mut seen, turn_ended(1)).await;
    assert_eq!(agent.seen().permission.as_deref(), Some("always"));

    let items = latest_items(&seen);
    let bodies: Vec<&ItemBody> = items.iter().map(|i| &i.body).collect();
    assert!(matches!(bodies[0], ItemBody::UserMessage { text, .. } if text == "[tour] write main"));
    assert!(bodies.iter().any(|b| matches!(b, ItemBody::Reasoning { text, streaming: false } if text == "thinking…")));
    assert!(items.iter().any(|i| i.status == ItemStatus::Completed
        && matches!(&i.body, ItemBody::ToolCall { kind: ToolKind::Edit, output, .. } if matches!(output.as_slice(), [ContentBlock::Diff(d)] if d.new_text == "fn main() {}"))));
    assert!(bodies.iter().any(|b| matches!(b, ItemBody::AssistantMessage { content, streaming: false } if content == &vec![ContentBlock::Text { text: "Hello world".into() }])));
    assert!(seen.iter().any(|e| matches!(e, AdapterEvent::CatalogUpdated { update } if update.commands.as_ref().is_some_and(|c| c[0].name == "plan"))));
    assert!(seen.iter().any(|e| matches!(e, AdapterEvent::PlanUpdated { plan } if plan.status == PlanStatus::Completed)));
    assert!(seen.iter().any(|e| matches!(e, AdapterEvent::UsageUpdated { usage, .. } if usage.context_used_tokens == Some(1200))));
    assert!(seen.iter().any(|e| matches!(e, AdapterEvent::RequestUpdated { request } if request.status == RequestStatus::Resolved)));
}

/// A permission request is answered once, and the agent goes on with the answer.
#[tokio::test]
async fn a_permission_answered_once_lets_the_agent_go_on() {
    let (session, agent) = open(RuntimeMode::ApprovalRequired).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    let turn = session.start_turn(say("[permission]")).await.unwrap();
    collect_until(&mut events, &mut seen, pending_request).await;
    let request = last_request(&seen);
    assert_eq!((request.kind, request.turn_id.as_ref()), (RequestKind::Approval { action: harness_contracts::ToolAction::FileChange }, Some(&turn)));
    let once = request.options.iter().find(|o| o.decision == ApprovalDecision::Accept).expect("a yes-once option");
    session.respond(&request.id, RequestResponse { option_id: Some(once.id.clone()), ..Default::default() }).await.unwrap();
    collect_until(&mut events, &mut seen, turn_ended(1)).await;
    assert_eq!(agent.seen().permission.as_deref(), Some("allow-once"));
    assert!(said(&seen, &turn).iter().any(|t| t.contains("permission: allow-once")), "{:?}", said(&seen, &turn));
    let again = session.respond(&request.id, RequestResponse { option_id: Some(once.id.clone()), ..Default::default() }).await;
    assert!(again.is_err(), "an answered request can't be answered again");
}

/// Two asked at once (two sub-agents in parallel) are both waiting, and
/// each answer goes to its own, whichever is answered first.
#[tokio::test]
async fn two_permissions_asked_at_once_are_each_answered() {
    let (session, agent) = open(RuntimeMode::ApprovalRequired).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    session.start_turn(say("[two-asks]")).await.unwrap();
    let both_waiting = |seen: &[AdapterEvent]| {
        let mut waiting = std::collections::BTreeMap::new();
        for e in seen {
            if let AdapterEvent::RequestUpdated { request } = e {
                waiting.insert(request.id.clone(), request.status == RequestStatus::Pending);
            }
        }
        waiting.values().filter(|w| **w).count() == 2
    };
    while !both_waiting(&seen) {
        collect_until(&mut events, &mut seen, pending_request).await;
    }
    let asked: Vec<harness_contracts::RuntimeRequest> = seen.iter().filter_map(|e| if let AdapterEvent::RequestUpdated { request } = e { Some(request.clone()) } else { None }).collect();
    let titled = |t: &str| asked.iter().find(|r| r.title.contains(t)).cloned().unwrap_or_else(|| panic!("{t}: {asked:?}"));
    let (a, b) = (titled("a.rs"), titled("b.rs"));
    let option = |r: &harness_contracts::RuntimeRequest, d: ApprovalDecision| r.options.iter().find(|o| o.decision == d).unwrap().id.clone();
    session.respond(&b.id, RequestResponse { option_id: Some(option(&b, ApprovalDecision::Decline)), ..Default::default() }).await.unwrap();
    session.respond(&a.id, RequestResponse { option_id: Some(option(&a, ApprovalDecision::Accept)), ..Default::default() }).await.unwrap();
    collect_until(&mut events, &mut seen, turn_ended(1)).await;
    assert_eq!(agent.seen().permission.as_deref(), Some("a: allow-once, b: reject"));
}

#[tokio::test]
async fn full_access_auto_approves_without_asking() {
    let (session, agent) = open(RuntimeMode::FullAccess).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    session.start_turn(say("[tour]")).await.unwrap();
    collect_until(&mut events, &mut seen, turn_ended(1)).await;
    assert_eq!(agent.seen().permission.as_deref(), Some("allow"));
    assert_eq!(agent.seen().modes, ["yolo"], "its mode for full access");
    assert!(!seen.iter().any(|e| matches!(e, AdapterEvent::RequestUpdated { .. })));
}

#[tokio::test]
async fn late_subscriber_gets_replay() {
    let (session, _) = open(RuntimeMode::FullAccess).await;
    {
        let mut events = session.events();
        let mut seen = Vec::new();
        session.start_turn(say("go")).await.unwrap();
        collect_until(&mut events, &mut seen, turn_ended(1)).await;
    }
    let replay: Vec<AdapterEvent> = session.events().take(3).collect().await;
    assert!(matches!(&replay[0], AdapterEvent::ThreadUpdated { thread } if thread.native_id.as_ref().map(NativeId::as_str) == Some("s1") && thread.status == ThreadStatus::Idle));
    assert!(matches!(&replay[1], AdapterEvent::TurnUpdated { turn } if turn.status == TurnStatus::Completed));
}

#[tokio::test]
async fn sessions_and_saved_sessions_work_in_the_project() {
    let agent = agent(MockSetup::default());
    let _session = open_with(&agent, profile(MODES), config(RuntimeMode::ApprovalRequired)).await;
    assert_eq!(agent.seen().cwd.as_deref(), Some(Path::new(PROJECT)), "no folder asked for: the project");

    let catalog = AcpSessionCatalog::new(Arc::new(agent.clone()), profile(MODES));
    let page = catalog.list(".", None).await.unwrap();
    assert_eq!(agent.seen().cwd.as_deref(), Some(Path::new("/work/project/.")));
    assert_eq!((page.sessions[0].native_id.as_str(), page.sessions[0].title.as_deref()), ("old-1", Some("Earlier work")));
    catalog.list("/elsewhere", None).await.unwrap();
    assert_eq!(agent.seen().cwd.as_deref(), Some(Path::new("/elsewhere")));
}

fn controls_of(seen: &[AdapterEvent]) -> SessionControls {
    seen.iter().rev().find_map(|e| match e { AdapterEvent::ThreadUpdated { thread } => Some(thread.controls.clone()), _ => None }).unwrap()
}

#[tokio::test]
async fn session_controls_expose_and_steer_every_config_option() {
    let (session, _) = open(RuntimeMode::ApprovalRequired).await;
    let mut events = session.events();
    let mut replay = Vec::new();
    collect_until(&mut events, &mut replay, |e| matches!(e, AdapterEvent::CatalogUpdated { update } if update.models.is_some())).await;

    let controls = controls_of(&replay);
    assert_eq!(controls.modes.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["default", "yolo"]);
    let model = controls.model_control().expect("model control");
    assert!(matches!(&model.kind, ConfigKind::Select { choices, current } if choices.len() == 2 && current == "flash"));
    assert!(controls.config.iter().any(|c| c.id == "verbose" && c.kind == ConfigKind::Boolean { current: false }));

    session
        .steer(SteerInput {
            model: Some(ModelSelection { instance_id: InstanceId::new("mock"), model: "pro".into(), options: Vec::new() }),
            config: vec![OptionValue { id: "verbose".into(), value: serde_json::Value::Bool(true) }],
            mode: Some("yolo".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut seen = Vec::new();
    collect_until(&mut events, &mut seen, |e| matches!(e, AdapterEvent::ThreadUpdated { thread } if thread.controls.current_mode.as_deref() == Some("yolo"))).await;
    let controls = controls_of(&seen);
    assert!(matches!(&controls.model_control().unwrap().kind, ConfigKind::Select { current, .. } if current == "pro"));
    assert!(controls.config.iter().any(|c| c.id == "verbose" && c.kind == ConfigKind::Boolean { current: true }));
}

#[tokio::test]
async fn messages_sent_while_a_turn_runs_wait_here_for_an_agent_that_doesnt_queue() {
    let (session, agent) = open(RuntimeMode::ApprovalRequired).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    let first = session.start_turn(say("[hold] first")).await.unwrap();
    let second = session.start_turn(say("second")).await.unwrap();
    collect_until(&mut events, &mut seen, |e| matches!(e, AdapterEvent::TurnUpdated { turn } if turn.status == TurnStatus::Queued)).await;
    agent.prompted(1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(agent.seen().prompts, ["[hold] first"], "the second waits for the first");
    agent.release();
    collect_until(&mut events, &mut seen, turn_ended(2)).await;
    assert_eq!(agent.seen().prompts, ["[hold] first", "second"]);
    assert_eq!(agent.seen().overlapping, 0);
    assert_eq!(said(&seen, &first), ["> [hold] first", "echo: [hold] first"]);
    assert_eq!(said(&seen, &second), ["> second", "echo: second"]);
}

#[tokio::test]
async fn an_agent_that_queues_gets_them_at_once() {
    let agent = agent(MockSetup { queues: true, declares: serde_json::json!({ "agentCapabilities": { "_meta": { "queue": { "on": true } } } }), ..MockSetup::default() });
    let session = open_with(&agent, profile(&format!("queueing = \"/agentCapabilities/_meta/queue/on\"\n{MODES}")), config(RuntimeMode::ApprovalRequired)).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    session.start_turn(say("[hold] first")).await.unwrap();
    let second = session.start_turn(say("second")).await.unwrap();
    agent.prompted(1).await;
    let started = std::time::Instant::now();
    while agent.seen().overlapping == 0 {
        assert!(started.elapsed() < Duration::from_secs(5), "the second was sent while the first ran");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    agent.release();
    collect_until(&mut events, &mut seen, turn_ended(2)).await;
    assert_eq!(said(&seen, &second), ["> second", "echo: second"]);
}

#[tokio::test]
async fn a_message_the_agent_takes_while_it_runs_starts_the_next_turn_which_the_prompt_goes_on_in() {
    let agent = agent(MockSetup { declares: serde_json::json!({ "_meta": { "steering": { "supported": true } } }), ..MockSetup::default() });
    let session = open_with(&agent, profile(&format!("steering = \"/_meta/steering/supported\"\n{MODES}")), config(RuntimeMode::ApprovalRequired)).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    let first = session.start_turn(say("[hold] first")).await.unwrap();
    agent.prompted(1).await;
    let went_on = session.inject(say("and this")).await.unwrap();
    assert_ne!(went_on, first, "a turn of its own, as a replay of the session numbers it");
    agent.release();
    collect_until(&mut events, &mut seen, turn_ended(2)).await;
    assert_eq!(agent.seen().steered, ["and this"]);
    assert_eq!(said(&seen, &first), ["> [hold] first"]);
    assert_eq!(said(&seen, &went_on), ["> and this", "steered: and thisecho: [hold] first"], "one reply, streamed, in the turn the prompt went on in");
    // Nothing running: it's a turn of its own.
    let next = session.inject(say("later")).await.unwrap();
    collect_until(&mut events, &mut seen, turn_ended(3)).await;
    assert_eq!(said(&seen, &next), ["> later", "echo: later"]);
}

/// Each turn's messages, by its number: what a tag on a turn or a message
/// points at.
fn numbered(seen: &[AdapterEvent]) -> Vec<(u32, Vec<String>)> {
    let mut turns: Vec<Turn> = Vec::new();
    for e in seen {
        if let AdapterEvent::TurnUpdated { turn } = e {
            turns.retain(|t| t.id != turn.id);
            turns.push(turn.clone());
        }
    }
    turns.sort_by_key(|t| t.ordinal);
    turns.into_iter().map(|t| (t.ordinal, said(seen, &t.id))).collect()
}

/// A session loaded again replays its history: its turns and their
/// messages have the numbers they had live, a message the agent took
/// while it ran and a prompt answered after it included, so what's tagged
/// in it is what was tagged.
#[tokio::test]
async fn a_session_loaded_again_numbers_its_turns_and_messages_as_they_were_live() {
    let agent = agent(MockSetup { declares: serde_json::json!({ "_meta": { "steering": { "supported": true } } }), load_session: true, ..MockSetup::default() });
    let profile = profile(&format!("steering = \"/_meta/steering/supported\"\n{MODES}"));
    let session = open_with(&agent, profile.clone(), config(RuntimeMode::ApprovalRequired)).await;
    let mut events = session.events();
    let mut live = Vec::new();
    session.start_turn(say("[hold] first")).await.unwrap();
    agent.prompted(1).await;
    session.inject(say("and this")).await.unwrap();
    agent.release();
    collect_until(&mut events, &mut live, turn_ended(2)).await;
    session.start_turn(say("second")).await.unwrap();
    collect_until(&mut events, &mut live, turn_ended(3)).await;
    let live = numbered(&live);
    assert_eq!(live.len(), 3, "{live:?}");
    // Loaded again (the server started again, say): replayed.
    let again = open_with(&agent, profile, SessionConfig { resume: Some(NativeId::new("s1")), ..config(RuntimeMode::ApprovalRequired) }).await;
    let mut replayed = Vec::new();
    let mut events = again.events();
    collect_until(&mut events, &mut replayed, |e| matches!(e, AdapterEvent::ItemUpdated { item } if matches!(&item.body, ItemBody::AssistantMessage { content, .. } if content.iter().any(|c| matches!(c, ContentBlock::Text { text } if text == "echo: second"))))).await;
    assert_eq!(numbered(&replayed), live);
}

#[tokio::test]
async fn steering_an_agent_that_doesnt_take_it_is_refused() {
    let (session, _) = open(RuntimeMode::ApprovalRequired).await;
    assert!(matches!(session.inject(say("x")).await, Err(harness_core::HarnessError::Unsupported { .. })));
}

#[tokio::test]
async fn questions_are_asked_and_their_answers_go_back_in_the_form() {
    let agent = agent(MockSetup::default());
    let session = open_with(&agent, profile(&format!("[questions]\ncustom_suffix = \"_custom\"\n[advertise]\nelicitation_form = true\n{MODES}")), config(RuntimeMode::FullAccess)).await;
    assert!(agent.seen().client.unwrap().elicitation.is_some_and(|e| e.form.is_some()), "we say we take forms");
    let mut events = session.events();
    let mut seen = Vec::new();
    session.start_turn(say("[question]")).await.unwrap();
    collect_until(&mut events, &mut seen, pending_request).await;
    let request = last_request(&seen);
    assert_eq!(request.kind, RequestKind::Question);
    assert_eq!(request.questions.iter().map(|q| (q.header.as_str(), q.multi_select)).collect::<Vec<_>>(), [("Store", false), ("Checks", true)]);
    let answers = [("question_0", vec!["Postgres"]), ("question_1", vec!["test", "fmt"])].map(|(k, v)| (k.to_owned(), v.into_iter().map(String::from).collect())).into();
    session.respond(&request.id, RequestResponse { option_id: Some("submit".into()), answers }).await.unwrap();
    collect_until(&mut events, &mut seen, turn_ended(1)).await;
    let answer = serde_json::to_value(agent.seen().answers.unwrap()).unwrap();
    assert_eq!(answer, serde_json::json!({ "action": "accept", "content": { "question_0": "Postgres", "question_1": ["test"], "question_1_custom": "fmt" } }));
}

#[tokio::test]
async fn a_skipped_form_is_declined() {
    let (session, agent) = open(RuntimeMode::FullAccess).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    session.start_turn(say("[question]")).await.unwrap();
    collect_until(&mut events, &mut seen, pending_request).await;
    session.respond(&last_request(&seen).id, RequestResponse { option_id: Some("decline".into()), ..Default::default() }).await.unwrap();
    collect_until(&mut events, &mut seen, turn_ended(1)).await;
    assert!(matches!(agent.seen().answers.unwrap().action, acp::ElicitationAction::Decline));
}

const SIGN_IN: &str = r#"
[sign_in]
notification = "_auth/status_update"
declared = "/agentCapabilities/_meta/authStatus"
wait_s = 2
status = "/authStatus/kind"
signed_out = ["none"]
label = "/authStatus/label"
email = "/authStatus/account/email"
"#;

fn signing_agent() -> MockAgent {
    agent(MockSetup {
        declares: serde_json::json!({ "agentCapabilities": { "_meta": { "authStatus": {} } } }),
        signed_in: Some(("_auth/status_update".into(), serde_json::json!({ "authStatus": { "kind": "account", "label": "Claude Team", "account": { "email": "me@x.io" } } }))),
        auth_methods: vec![acp::AuthMethod::Terminal(acp::AuthMethodTerminal::new("claude-ai-login", "Claude account").args(vec!["--cli".into(), "auth".into(), "login".into()]))],
        ..MockSetup::default()
    })
}

#[tokio::test]
async fn a_check_finds_who_the_agent_is_signed_in_as_and_its_login_commands() {
    let agent = signing_agent();
    let profile = profile(&format!("[advertise]\nauth_terminal = true\n{SIGN_IN}"));
    let found = probe(&(Arc::new(agent.clone()) as Arc<dyn harness_acp::AcpLauncher>), &profile).await.unwrap();
    assert!(agent.seen().client.unwrap().auth.terminal, "we say we run login commands");
    let sign_in = found.sign_in.unwrap();
    let account = sign_in.account.unwrap();
    assert_eq!((sign_in.status, account.label.as_str(), account.email.as_deref()), (AuthStatus::SignedIn, "Claude Team", Some("me@x.io")));
    assert_eq!(found.auth_methods.iter().map(|m| (m.id.as_str(), m.kind)).collect::<Vec<_>>(), [("claude-ai-login", AuthMethodKind::Command)]);
    assert_eq!(found.logins[0].args, ["--cli", "auth", "login"]);
}

#[tokio::test]
async fn a_session_reports_who_the_agent_is_signed_in_as() {
    let agent = signing_agent();
    let session = open_with(&agent, profile(&format!("{SIGN_IN}{MODES}")), config(RuntimeMode::ApprovalRequired)).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    collect_until(&mut events, &mut seen, |e| matches!(e, AdapterEvent::CatalogUpdated { update } if update.sign_in.is_some())).await;
}

#[tokio::test]
async fn session_meta_and_tool_servers_go_to_the_agent_as_it_takes_them() {
    let servers = vec![McpServer { name: "nah".into(), transport: McpTransport::Sse { url: "http://127.0.0.1:8080/api/mcp/sse".into(), headers: Vec::new() } }];
    let with_servers = SessionConfig { mcp_servers: servers, ..config(RuntimeMode::ApprovalRequired) };
    let takes_sse = agent(MockSetup { sse: true, ..MockSetup::default() });
    let meta = "[session_meta.claudeCode.options]\nsettingSources = [\"user\"]\n";
    open_with(&takes_sse, profile(&format!("{meta}{MODES}")), with_servers.clone()).await;
    assert!(matches!(takes_sse.seen().mcp_servers.as_slice(), [acp::McpServer::Sse(s)] if s.name == "nah"));
    assert_eq!(takes_sse.seen().session_meta, Some(serde_json::json!({ "claudeCode": { "options": { "settingSources": ["user"] } } }).as_object().unwrap().clone()));
    let http_only = agent(MockSetup::default());
    open_with(&http_only, profile(MODES), with_servers).await;
    assert!(matches!(http_only.seen().mcp_servers.as_slice(), [acp::McpServer::Http(_)]));
    assert_eq!(http_only.seen().session_meta, None);
}

/// A session's key reaches its agent, and no one else: the session as
/// shown to clients has its tool servers without it.
#[tokio::test]
async fn a_sessions_key_goes_to_its_agent_and_not_to_clients() {
    let servers = vec![McpServer { name: "nah".into(), transport: McpTransport::Sse { url: "http://127.0.0.1:8080/api/mcp/sse?key=secret".into(), headers: vec![("Authorization".into(), "Bearer hidden".into())] } }];
    let agent = agent(MockSetup { sse: true, ..MockSetup::default() });
    let session = open_with(&agent, profile(MODES), SessionConfig { mcp_servers: servers, ..config(RuntimeMode::ApprovalRequired) }).await;
    assert!(matches!(agent.seen().mcp_servers.as_slice(), [acp::McpServer::Sse(s)] if s.url.ends_with("?key=secret")), "the agent connects with its key");
    let mut events = session.events();
    let mut seen = Vec::new();
    collect_until(&mut events, &mut seen, |e| matches!(e, AdapterEvent::ThreadUpdated { .. })).await;
    let shown = seen.iter().rev().find_map(|e| if let AdapterEvent::ThreadUpdated { thread } = e { Some(thread.mcp_servers.clone()) } else { None }).unwrap();
    let text = serde_json::to_string(&shown).unwrap();
    assert!(!text.contains("secret") && !text.contains("hidden"), "{text}");
    assert!(text.contains("http://127.0.0.1:8080/api/mcp/sse"), "still says where: {text}");
}

/// Antigravity asks questions through permission requests (its pack says
/// how to tell them); one is a question, and Full access never answers it.
#[tokio::test]
async fn a_question_asked_through_a_permission_request_waits_for_the_user_even_in_full_access() {
    let agent = agent(MockSetup::default());
    let antigravity = profile(&format!("{MODES}\n[questions]\npermission_tool_prefix = \"interaction_\"\n"));
    let session = open_with(&agent, antigravity, config(RuntimeMode::FullAccess)).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    session.start_turn(say("[ask-by-permission]")).await.unwrap();
    collect_until(&mut events, &mut seen, pending_request).await;
    let request = last_request(&seen);
    assert_eq!(request.kind, RequestKind::Question, "a question, not an approval");
    assert_eq!(request.options.iter().map(|o| (o.label.as_str(), o.decision)).collect::<Vec<_>>(), [("Redis", ApprovalDecision::Answer), ("Postgres", ApprovalDecision::Answer)]);
    assert_eq!(agent.seen().permission, None, "not answered for the user");
    session.respond(&request.id, RequestResponse { option_id: Some("2".into()), ..Default::default() }).await.unwrap();
    collect_until(&mut events, &mut seen, turn_ended(1)).await;
    assert_eq!(agent.seen().permission.as_deref(), Some("2"), "the user's answer goes back as its option");
    // The same in Full access without the pack's rule: still a question (no deny among them).
    let session = open_with(&agent, profile(MODES), config(RuntimeMode::FullAccess)).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    session.start_turn(say("[ask-by-permission]")).await.unwrap();
    collect_until(&mut events, &mut seen, pending_request).await;
    assert_eq!(last_request(&seen).kind, RequestKind::Question);
}
