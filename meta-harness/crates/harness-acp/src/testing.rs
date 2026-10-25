//! A mock ACP agent, in memory, set up to behave like a real one (its
//! modes, what it declares in `initialize`, its logins, who it says it's
//! signed in as). Every harness pack's contract tests run against one set
//! up from that pack.
//!
//! A prompt's words say what it does:
//! - `[tour]`: one of every kind of update, and a permission request;
//! - `[permission]`: asks to edit a file, then says which option it got;
//! - `[question]`: asks two questions in a form, then says the answers;
//! - `[hold]`: waits until [`MockAgent::release`];
//! - `[sub-agent]`: starts a sub-agent with a tool call; the sub-agent's
//!   message and call say whose they are in their `_meta`, where its
//!   settings look (`parent_call`);
//! - anything else: answers `echo: <prompt>`.
//!
//! A message steered in while a prompt runs is taken at once (as a real
//! agent queues it), and answered `steered: <text>` when the prompt goes
//! on.
//!
//! What was said in its session (prompts, messages steered in, its
//! answers) is kept, each message with an id: loading the session again
//! replays it, as a real agent replays a session's history (a session it
//! has none of: an earlier question and its answer).

use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{ByteStreams, Client, ConnectionTo, UntypedMessage};
use async_trait::async_trait;
use harness_contracts::SessionConfig;
use harness_core::Result;
use tokio::sync::Notify;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::launch::{AcpLauncher, AcpTransport};
use crate::settings::AcpSettings;

pub const SESSION: &str = "s1";

/// How the mock behaves.
#[derive(Debug, Clone)]
pub struct MockSetup {
    /// The session's modes (the first is current).
    pub modes: Vec<String>,
    /// Merged into its `initialize` answer (its `_meta`, its capabilities'
    /// `_meta`): what it declares beyond the protocol.
    pub declares: serde_json::Value,
    /// It queues prompts sent while one runs (else it refuses them).
    pub queues: bool,
    pub auth_methods: Vec<acp::AuthMethod>,
    /// A notification it sends after `initialize` (who it's signed in as).
    pub signed_in: Option<(String, serde_json::Value)>,
    pub load_session: bool,
    pub sse: bool,
    /// Its login runs itself as Antigravity's does: it prints a link on its
    /// error output, waits for the browser's callback on a local port, and,
    /// signed out, starts it when a session opens (and doesn't answer).
    pub loopback_login: bool,
    /// Where its sub-agents' updates name the call that started them, in
    /// their `_meta` (none: they don't say).
    pub parent_call: Option<String>,
}

impl Default for MockSetup {
    fn default() -> Self {
        Self {
            modes: vec!["default".into()],
            declares: serde_json::Value::Null,
            queues: false,
            auth_methods: Vec::new(),
            signed_in: None,
            load_session: false,
            sse: false,
            loopback_login: false,
            parent_call: None,
        }
    }
}

impl MockSetup {
    /// A mock of the agent `settings` describe: its modes, and everything
    /// they look for in `initialize` and after it, declared.
    pub fn like(settings: &AcpSettings) -> Self {
        let m = &settings.modes;
        let mut modes: Vec<String> = Vec::new();
        for mode in [&m.approval_required, &m.auto_accept_edits, &m.auto, &m.full_access, &m.plan].into_iter().flatten() {
            if !modes.contains(mode) {
                modes.push(mode.clone());
            }
        }
        if modes.is_empty() {
            modes.push("default".into());
        }
        let mut declares = serde_json::json!({});
        for pointer in [&settings.queueing, &settings.steering].into_iter().flatten() {
            set(&mut declares, pointer, serde_json::Value::Bool(true));
        }
        let signed_in = settings.sign_in.as_ref().map(|s| {
            if let Some(pointer) = &s.declared {
                set(&mut declares, pointer, serde_json::json!({}));
            }
            let mut said = serde_json::json!({});
            set(&mut said, &s.status, serde_json::json!("account"));
            for (pointer, value) in [(&s.label, "Test account"), (&s.email, "me@example.com"), (&s.plan, "team"), (&s.organization, "Example")] {
                if let Some(pointer) = pointer {
                    set(&mut said, pointer, serde_json::json!(value));
                }
            }
            (s.notification.clone(), said)
        });
        let auth_methods = vec![if settings.advertise.auth_terminal {
            acp::AuthMethod::Terminal(acp::AuthMethodTerminal::new("login", "Log in").args(vec!["login".into()]))
        } else {
            acp::AuthMethod::Agent(acp::AuthMethodAgent::new("login", "Log in"))
        }];
        Self { modes, declares, queues: settings.queueing.is_some(), auth_methods, signed_in, load_session: false, sse: true, loopback_login: false, parent_call: settings.parent_call.clone() }
    }
}

/// Sets the value at a JSON pointer, making the objects on the way.
fn set(json: &mut serde_json::Value, pointer: &str, value: serde_json::Value) {
    let mut at = json;
    let keys: Vec<String> = pointer.split('/').skip(1).map(|k| k.replace("~1", "/").replace("~0", "~")).collect();
    for (i, key) in keys.iter().enumerate() {
        if !at.is_object() {
            *at = serde_json::json!({});
        }
        let map = at.as_object_mut().expect("an object");
        if i + 1 == keys.len() {
            map.insert(key.clone(), value);
            return;
        }
        at = map.entry(key.clone()).or_insert(serde_json::json!({}));
    }
}

/// What the mock was sent.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub client: Option<acp::ClientCapabilities>,
    pub session_meta: Option<acp::Meta>,
    pub cwd: Option<std::path::PathBuf>,
    pub mcp_servers: Vec<acp::McpServer>,
    pub prompts: Vec<String>,
    pub steered: Vec<String>,
    pub modes: Vec<String>,
    pub permission: Option<String>,
    pub answers: Option<acp::CreateElicitationResponse>,
    /// Prompts that arrived while another ran.
    pub overlapping: usize,
}

#[derive(Clone)]
pub struct MockAgent {
    setup: MockSetup,
    seen: Arc<Mutex<Seen>>,
    hold: Arc<Notify>,
    /// Prompts sent and not answered, and those running (one at a time).
    in_flight: Arc<Mutex<usize>>,
    running: Arc<Mutex<usize>>,
    one_at_a_time: Arc<tokio::sync::Mutex<()>>,
    /// What was said in its session, in order: whether by the user, and what.
    said: Arc<Mutex<Vec<(bool, String)>>>,
    /// Messages steered in, not answered yet.
    steered: Arc<Mutex<Vec<String>>>,
    /// Its loopback login went through (the browser's callback reached it).
    pub login_done: Arc<std::sync::atomic::AtomicBool>,
}

/// The link a loopback login prints, its callback on `port`.
pub fn loopback_link(port: u16) -> String {
    format!("https://accounts.google.com/o/oauth2/auth?client_id=x&redirect_uri=http%3A%2F%2F127.0.0.1%3A{port}%2F&state=s")
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl MockAgent {
    pub fn new(setup: MockSetup) -> Self {
        Self { setup, seen: Arc::default(), hold: Arc::default(), in_flight: Arc::default(), running: Arc::default(), one_at_a_time: Arc::default(), said: Arc::default(), steered: Arc::default(), login_done: Arc::default() }
    }

    pub fn seen(&self) -> Seen {
        lock(&self.seen).clone()
    }

    /// Waits (up to 5 s) until `n` prompts have started running.
    pub async fn prompted(&self, n: usize) {
        let started = std::time::Instant::now();
        while lock(&self.seen).prompts.len() < n {
            assert!(started.elapsed() < std::time::Duration::from_secs(5), "{n} prompts didn't start");
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// Lets a `[hold]` prompt finish.
    pub fn release(&self) {
        self.hold.notify_one();
    }

    fn initialize(&self, req: &acp::InitializeRequest) -> acp::InitializeResponse {
        let caps = acp::AgentCapabilities::new()
            .load_session(self.setup.load_session)
            .mcp_capabilities(acp::McpCapabilities::new().http(true).sse(self.setup.sse))
            .session_capabilities(acp::SessionCapabilities::new().list(acp::SessionListCapabilities::new()));
        let base = acp::InitializeResponse::new(req.protocol_version).agent_capabilities(caps).auth_methods(self.setup.auth_methods.clone());
        let mut json = serde_json::to_value(base).unwrap_or_default();
        merge(&mut json, &self.setup.declares);
        serde_json::from_value(json).expect("the mock's declarations make an initialize answer")
    }

    fn session_modes(&self) -> acp::SessionModeState {
        let modes = self.setup.modes.iter().map(|m| acp::SessionMode::new(m.clone(), m.clone())).collect();
        acp::SessionModeState::new(self.setup.modes[0].clone(), modes)
    }

    async fn prompt(&self, cx: ConnectionTo<Client>, req: acp::PromptRequest) -> agent_client_protocol::Result<acp::PromptResponse> {
        let text = req.prompt.iter().filter_map(|b| if let acp::ContentBlock::Text(t) = b { Some(t.text.clone()) } else { None }).collect::<Vec<_>>().join("");
        {
            let mut in_flight = lock(&self.in_flight);
            if *in_flight > 0 {
                lock(&self.seen).overlapping += 1;
            }
            *in_flight += 1;
        }
        let result = {
            let _turn = self.one_at_a_time.lock().await;
            lock(&self.seen).prompts.push(text.clone());
            lock(&self.said).push((true, text.clone()));
            *lock(&self.running) += 1;
            let result = self.run(&cx, &text).await;
            *lock(&self.running) -= 1;
            result
        };
        *lock(&self.in_flight) -= 1;
        result.map(|()| acp::PromptResponse::new(acp::StopReason::EndTurn))
    }

    async fn run(&self, cx: &ConnectionTo<Client>, text: &str) -> agent_client_protocol::Result<()> {
        if text.contains("[tour]") {
            return tour(cx, &self.seen).await;
        }
        if text.contains("[sub-agent]") {
            return sub_agent(cx, self.setup.parent_call.as_deref());
        }
        if text.contains("[permission]") {
            notify(cx, acp::SessionUpdate::ToolCall(acp::ToolCall::new("edit-1", "Edit main.rs").kind(acp::ToolKind::Edit).status(acp::ToolCallStatus::Pending)))?;
            let options = vec![
                acp::PermissionOption::new("allow-once", "Allow", acp::PermissionOptionKind::AllowOnce),
                acp::PermissionOption::new("allow-always", "Always allow", acp::PermissionOptionKind::AllowAlways),
                acp::PermissionOption::new("reject", "Reject", acp::PermissionOptionKind::RejectOnce),
            ];
            let request = acp::RequestPermissionRequest::new(SESSION, acp::ToolCallUpdate::new("edit-1", acp::ToolCallUpdateFields::new().title("Edit main.rs")), options);
            let outcome = match cx.send_request(request).block_task().await?.outcome {
                acp::RequestPermissionOutcome::Selected(s) => s.option_id.to_string(),
                _ => "cancelled".into(),
            };
            lock(&self.seen).permission = Some(outcome.clone());
            return say(cx, &format!("permission: {outcome}"));
        }
        // As Google's Antigravity server asks a question (1.1.1, 1.3.0):
        // a made-up tool call, then a permission request whose options are
        // the answers, every one "allow once".
        if text.contains("[ask-by-permission]") {
            notify(cx, acp::SessionUpdate::ToolCall(acp::ToolCall::new("interaction_1a2b3c4d", "Which store?").status(acp::ToolCallStatus::Pending)))?;
            let options = vec![
                acp::PermissionOption::new("1", "Redis", acp::PermissionOptionKind::AllowOnce),
                acp::PermissionOption::new("2", "Postgres", acp::PermissionOptionKind::AllowOnce),
            ];
            let request = acp::RequestPermissionRequest::new(SESSION, acp::ToolCallUpdate::new("interaction_1a2b3c4d", acp::ToolCallUpdateFields::new().title("Which store?")), options);
            let outcome = match cx.send_request(request).block_task().await?.outcome {
                acp::RequestPermissionOutcome::Selected(s) => s.option_id.to_string(),
                _ => "cancelled".into(),
            };
            lock(&self.seen).permission = Some(outcome.clone());
            return say(cx, &format!("answer: {outcome}"));
        }
        if text.contains("[question]") {
            let schema: acp::ElicitationSchema = serde_json::from_value(serde_json::json!({
                "type": "object",
                "properties": {
                    "question_0": { "type": "string", "title": "Store", "description": "Which store?", "oneOf": [
                        { "const": "Redis", "title": "Redis" }, { "const": "Postgres", "title": "Postgres" } ] },
                    "question_0_custom": { "type": "string", "title": "Other" },
                    "question_1": { "type": "array", "title": "Checks", "description": "Run which?", "items": { "anyOf": [
                        { "const": "lint", "title": "lint" }, { "const": "test", "title": "test" } ] } },
                    "question_1_custom": { "type": "string", "title": "Other" }
                }
            }))
            .expect("a form");
            let mode = acp::ElicitationFormMode::new(acp::ElicitationSessionScope::new(SESSION), schema);
            let answer = cx.send_request(acp::CreateElicitationRequest::new(mode, "Please answer the following questions.")).block_task().await?;
            let said = serde_json::to_string(&answer.action).unwrap_or_default();
            lock(&self.seen).answers = Some(answer);
            return say(cx, &format!("answers: {said}"));
        }
        if text.contains("[hold]") {
            self.hold.notified().await;
        }
        let steered = std::mem::take(&mut *lock(&self.steered));
        for message in steered {
            self.answer(cx, &format!("steered: {message}"))?;
        }
        self.answer(cx, &format!("echo: {text}"))
    }

    /// Says `words` as its answer, kept with what was said.
    fn answer(&self, cx: &ConnectionTo<Client>, words: &str) -> agent_client_protocol::Result<()> {
        lock(&self.said).push((false, words.to_owned()));
        say(cx, words)
    }

    /// Its session's history, replayed as it was said: each message with
    /// an id of its own.
    fn replay(&self, cx: &ConnectionTo<Client>) -> agent_client_protocol::Result<()> {
        let said = lock(&self.said).clone();
        if said.is_empty() {
            notify(cx, acp::SessionUpdate::UserMessageChunk(text("earlier question")))?;
            return say(cx, "earlier answer");
        }
        for (n, (user, words)) in said.into_iter().enumerate() {
            let chunk = text(&words).message_id(acp::MessageId::new(format!("m{n}")));
            notify(cx, if user { acp::SessionUpdate::UserMessageChunk(chunk) } else { acp::SessionUpdate::AgentMessageChunk(chunk) })?;
        }
        Ok(())
    }

    fn steer(&self, params: &serde_json::Value) -> serde_json::Value {
        let text = params.pointer("/prompt/0/text").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
        if *lock(&self.running) == 0 {
            return serde_json::json!({ "outcome": "promptRequired", "reason": "noRunningTurn" });
        }
        lock(&self.seen).steered.push(text.clone());
        lock(&self.said).push((true, text.clone()));
        lock(&self.steered).push(text);
        serde_json::json!({ "outcome": "injected" })
    }
}

fn merge(into: &mut serde_json::Value, from: &serde_json::Value) {
    match (into, from) {
        (serde_json::Value::Object(into), serde_json::Value::Object(from)) => {
            for (k, v) in from {
                merge(into.entry(k.clone()).or_insert(serde_json::Value::Null), v);
            }
        }
        (_, serde_json::Value::Null) => {}
        (into, from) => *into = from.clone(),
    }
}

fn notify(cx: &ConnectionTo<Client>, update: acp::SessionUpdate) -> agent_client_protocol::Result<()> {
    cx.send_notification(acp::SessionNotification::new(SESSION, update))
}

fn text(t: &str) -> acp::ContentChunk {
    acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(t)))
}

fn say(cx: &ConnectionTo<Client>, words: &str) -> agent_client_protocol::Result<()> {
    notify(cx, acp::SessionUpdate::AgentMessageChunk(text(words)))
}

/// A sub-agent started by a tool call: its message and its call say whose
/// they are where `parent_call` points in their `_meta`.
fn sub_agent(cx: &ConnectionTo<Client>, parent_call: Option<&str>) -> agent_client_protocol::Result<()> {
    notify(cx, acp::SessionUpdate::ToolCall(acp::ToolCall::new("task-1", "Explore the parser").status(acp::ToolCallStatus::InProgress)))?;
    let meta = parent_call.map(|pointer| {
        let mut meta = serde_json::json!({});
        set(&mut meta, pointer, serde_json::json!("task-1"));
        serde_json::from_value::<acp::Meta>(meta).expect("an object")
    });
    notify(cx, acp::SessionUpdate::AgentMessageChunk(text("Reading the parser.").meta(meta.clone())))?;
    notify(cx, acp::SessionUpdate::ToolCall(acp::ToolCall::new("read-1", "Read parse.rs").kind(acp::ToolKind::Read).status(acp::ToolCallStatus::Completed).meta(meta)))?;
    notify(cx, acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new("task-1", acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed))))?;
    say(cx, "The helper read it.")
}

/// One of every kind of update, with a permission request in the middle.
async fn tour(cx: &ConnectionTo<Client>, seen: &Arc<Mutex<Seen>>) -> agent_client_protocol::Result<()> {
    notify(cx, acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(vec![acp::AvailableCommand::new("plan", "Plan before acting")])))?;
    notify(cx, acp::SessionUpdate::AgentThoughtChunk(text("thinking…")))?;
    notify(cx, acp::SessionUpdate::ToolCall(acp::ToolCall::new("tool-1", "Edit main.rs").kind(acp::ToolKind::Edit).status(acp::ToolCallStatus::Pending)))?;
    let options = vec![
        acp::PermissionOption::new("allow", "Allow once", acp::PermissionOptionKind::AllowOnce),
        acp::PermissionOption::new("always", "Allow for session", acp::PermissionOptionKind::AllowAlways),
        acp::PermissionOption::new("deny", "Deny", acp::PermissionOptionKind::RejectOnce),
    ];
    let request = acp::RequestPermissionRequest::new(SESSION, acp::ToolCallUpdate::new("tool-1", acp::ToolCallUpdateFields::new().title("Edit main.rs")), options);
    if let acp::RequestPermissionOutcome::Selected(s) = cx.send_request(request).block_task().await?.outcome {
        lock(seen).permission = Some(s.option_id.to_string());
    }
    notify(
        cx,
        acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
            "tool-1",
            acp::ToolCallUpdateFields::new()
                .status(acp::ToolCallStatus::Completed)
                .content(vec![acp::ToolCallContent::Diff(acp::Diff::new("main.rs", "fn main() {}").old_text(String::new()))]),
        )),
    )?;
    say(cx, "Hello ")?;
    say(cx, "world")?;
    notify(cx, acp::SessionUpdate::Plan(acp::Plan::new(vec![acp::PlanEntry::new("Write main", acp::PlanEntryPriority::High, acp::PlanEntryStatus::Completed)])))?;
    notify(cx, acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(1200, 200_000)))
}

#[async_trait]
impl AcpLauncher for MockAgent {
    async fn launch(&self, _config: &SessionConfig) -> Result<AcpTransport> {
        let (client_side, agent_side) = tokio::io::duplex(256 * 1024);
        // What it says outside the protocol (its error output).
        let (says_to, said) = tokio::sync::broadcast::channel(crate::launch::SAID_HELD);
        let (on_auth, on_new_says) = (self.clone(), says_to.clone());
        let (client_read, client_write) = tokio::io::split(client_side);
        let (agent_read, agent_write) = tokio::io::split(agent_side);
        let me = self.clone();
        let (on_init, on_new, on_load, on_list, on_mode, on_prompt, on_steer) = (me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone());
        tokio::spawn(async move {
            let _ = agent_client_protocol::Agent
                .builder()
                .on_receive_request(
                    async move |req: acp::InitializeRequest, responder, cx: ConnectionTo<Client>| {
                        lock(&on_init.seen).client = Some(req.client_capabilities.clone());
                        let answer = on_init.initialize(&req);
                        let signed_in = on_init.setup.signed_in.clone();
                        responder.respond(answer)?;
                        if let Some((method, params)) = signed_in {
                            cx.send_notification(UntypedMessage::new(&method, params)?)?;
                        }
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |_req: acp::AuthenticateRequest, responder, cx: ConnectionTo<Client>| {
                        if !on_auth.setup.loopback_login {
                            return responder.respond(acp::AuthenticateResponse::new());
                        }
                        // As Antigravity: a listener on a local port, its link said,
                        // the browser's callback awaited.
                        let failed = |e: std::io::Error| agent_client_protocol::Error::internal_error().data(e.to_string());
                        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.map_err(failed)?;
                        let port = listener.local_addr().map_err(failed)?.port();
                        let _ = says_to.send(format!("Open the following link to authenticate the ACP server: {}", loopback_link(port)));
                        let agent = on_auth.clone();
                        cx.spawn(async move {
                            use tokio::io::{AsyncReadExt, AsyncWriteExt};
                            let Ok((mut stream, _)) = listener.accept().await else { return Ok(()) };
                            let mut asked = [0u8; 1024];
                            let n = stream.read(&mut asked).await.unwrap_or(0);
                            let request = String::from_utf8_lossy(&asked[..n]).into_owned();
                            let fine = request.starts_with("GET /?code=");
                            let reply = if fine { "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok" } else { "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n" };
                            let _ = stream.write_all(reply.as_bytes()).await;
                            if fine {
                                agent.login_done.store(true, std::sync::atomic::Ordering::SeqCst);
                                responder.respond(acp::AuthenticateResponse::new())
                            } else {
                                responder.respond_with_error(agent_client_protocol::Error::invalid_request().data("bad callback"))
                            }
                        })
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |req: acp::NewSessionRequest, responder, _cx| {
                        // Signed out, as Antigravity: its login starts, and no answer.
                        if on_new.setup.loopback_login && !on_new.login_done.load(std::sync::atomic::Ordering::SeqCst) {
                            let _ = on_new_says.send(format!("Open the following link to authenticate the ACP server: {}", loopback_link(1)));
                            std::mem::forget(responder);
                            return Ok(());
                        }
                        {
                            let mut seen = lock(&on_new.seen);
                            seen.session_meta = req.meta.clone();
                            seen.cwd = Some(req.cwd.clone());
                            seen.mcp_servers = req.mcp_servers.clone();
                        }
                        responder.respond(
                            acp::NewSessionResponse::new(SESSION).modes(on_new.session_modes()).config_options(vec![
                                acp::SessionConfigOption::select("model", "Model", "flash", vec![acp::SessionConfigSelectOption::new("flash", "Flash"), acp::SessionConfigSelectOption::new("pro", "Pro")])
                                    .category(acp::SessionConfigOptionCategory::Model),
                                acp::SessionConfigOption::boolean("verbose", "Verbose", false),
                            ]),
                        )
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |req: acp::LoadSessionRequest, responder, cx: ConnectionTo<Client>| {
                        lock(&on_load.seen).cwd = Some(req.cwd.clone());
                        on_load.replay(&cx)?;
                        responder.respond(acp::LoadSessionResponse::new().modes(on_load.session_modes()))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |req: acp::ListSessionsRequest, responder, _cx| {
                        let cwd = req.cwd.unwrap_or_default();
                        lock(&on_list.seen).cwd = Some(cwd.clone());
                        responder.respond(acp::ListSessionsResponse::new(vec![acp::SessionInfo::new("old-1", cwd).title("Earlier work".to_string())]))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |_req: acp::SetSessionConfigOptionRequest, responder, _cx| responder.respond(acp::SetSessionConfigOptionResponse::new(Vec::new())),
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |req: acp::SetSessionModeRequest, responder, _cx| {
                        lock(&on_mode.seen).modes.push(req.mode_id.to_string());
                        responder.respond(acp::SetSessionModeResponse::new())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |req: acp::PromptRequest, responder, cx: ConnectionTo<Client>| {
                        if !on_prompt.setup.queues && *lock(&on_prompt.in_flight) > 0 {
                            return responder.respond_with_error(agent_client_protocol::Error::invalid_request().data("a prompt is already running"));
                        }
                        let agent = on_prompt.clone();
                        cx.spawn({
                            let cx = cx.clone();
                            async move { responder.respond(agent.prompt(cx, req).await?) }
                        })
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |req: UntypedMessage, responder, _cx: ConnectionTo<Client>| match req.method.as_str() {
                        "_session/steering" => responder.respond(on_steer.steer(&req.params)),
                        _ => responder.respond_with_error(agent_client_protocol::Error::method_not_found()),
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(ByteStreams::new(agent_write.compat_write(), agent_read.compat()))
                .await;
        });
        Ok(AcpTransport { reader: Box::new(client_read.compat()), writer: Box::new(client_write.compat_write()), guard: Box::new(()), said })
    }
}
