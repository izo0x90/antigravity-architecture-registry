//! AcpAdapter against an in-process mock ACP agent (official crate, agent side),
//! connected through an in-memory pipe.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{ByteStreams, Client, ConnectionTo};
use async_trait::async_trait;
use futures::StreamExt;
use harness_acp::{AcpAdapter, AcpFlavor, AcpLauncher, AcpSessionCatalog, AcpTransport};
use harness_contracts::*;
use harness_core::{ProviderAdapter, Result, SessionCatalog, SessionRuntime};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Records which permission option the client selected.
type Selection = Arc<Mutex<Option<String>>>;

struct MockLauncher {
    selection: Selection,
}

fn notify(cx: &ConnectionTo<Client>, update: acp::SessionUpdate) -> agent_client_protocol::Result<()> {
    cx.send_notification(acp::SessionNotification::new(acp::SessionId::new("s1"), update))
}

fn text(t: &str) -> acp::ContentChunk {
    acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(t)))
}

async fn run_turn(cx: ConnectionTo<Client>, selection: Selection) -> agent_client_protocol::Result<acp::PromptResponse> {
    notify(&cx, acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(vec![
        acp::AvailableCommand::new("plan", "Plan before acting"),
    ])))?;
    notify(&cx, acp::SessionUpdate::AgentThoughtChunk(text("thinking…")))?;
    notify(&cx, acp::SessionUpdate::ToolCall(
        acp::ToolCall::new("tool-1", "Edit main.rs").kind(acp::ToolKind::Edit).status(acp::ToolCallStatus::Pending),
    ))?;
    let options = vec![
        acp::PermissionOption::new("allow", "Allow once", acp::PermissionOptionKind::AllowOnce),
        acp::PermissionOption::new("always", "Allow for session", acp::PermissionOptionKind::AllowAlways),
        acp::PermissionOption::new("deny", "Deny", acp::PermissionOptionKind::RejectOnce),
    ];
    let request = acp::RequestPermissionRequest::new(
        acp::SessionId::new("s1"),
        acp::ToolCallUpdate::new("tool-1", acp::ToolCallUpdateFields::new().title("Edit main.rs")),
        options,
    );
    let response = cx.send_request(request).block_task().await?;
    if let acp::RequestPermissionOutcome::Selected(s) = &response.outcome {
        *selection.lock().unwrap() = Some(s.option_id.to_string());
    }
    notify(&cx, acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        "tool-1",
        acp::ToolCallUpdateFields::new()
            .status(acp::ToolCallStatus::Completed)
            .content(vec![acp::ToolCallContent::Diff(acp::Diff::new("main.rs", "fn main() {}").old_text("".to_string()))]),
    )))?;
    notify(&cx, acp::SessionUpdate::AgentMessageChunk(text("Hello ")))?;
    notify(&cx, acp::SessionUpdate::AgentMessageChunk(text("world")))?;
    notify(&cx, acp::SessionUpdate::Plan(acp::Plan::new(vec![acp::PlanEntry::new(
        "Write main",
        acp::PlanEntryPriority::High,
        acp::PlanEntryStatus::Completed,
    )])))?;
    notify(&cx, acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(1200, 200_000)))?;
    Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
}

#[async_trait]
impl AcpLauncher for MockLauncher {
    async fn launch(&self, _config: &SessionConfig) -> Result<AcpTransport> {
        let (client_side, agent_side) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_side);
        let (agent_read, agent_write) = tokio::io::split(agent_side);
        let selection = self.selection.clone();
        tokio::spawn(async move {
            let _ = agent_client_protocol::Agent
                .builder()
                .on_receive_request(
                    async move |req: acp::InitializeRequest, responder, _cx| {
                        responder.respond(acp::InitializeResponse::new(req.protocol_version))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |_req: acp::NewSessionRequest, responder, _cx| {
                        responder.respond(acp::NewSessionResponse::new("s1").modes(acp::SessionModeState::new(
                            "default",
                            vec![acp::SessionMode::new("default", "Default"), acp::SessionMode::new("yolo", "YOLO")],
                        )))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |req: acp::ListSessionsRequest, responder, _cx| {
                        let cwd = req.cwd.unwrap_or_default();
                        responder.respond(acp::ListSessionsResponse::new(vec![
                            acp::SessionInfo::new("old-1", cwd).title("Earlier work".to_string()),
                        ]))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |_req: acp::SetSessionModeRequest, responder, _cx| {
                        responder.respond(acp::SetSessionModeResponse::new())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |_req: acp::PromptRequest, responder, cx: ConnectionTo<Client>| {
                        let selection = selection.clone();
                        cx.spawn({
                            let cx = cx.clone();
                            async move { responder.respond(run_turn(cx, selection).await?) }
                        })
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(ByteStreams::new(agent_write.compat_write(), agent_read.compat()))
                .await;
        });
        Ok(AcpTransport {
            reader: Box::new(client_read.compat()),
            writer: Box::new(client_write.compat_write()),
            guard: Box::new(()),
        })
    }
}

struct TestFlavor;

impl AcpFlavor for TestFlavor {
    fn native_mode(&self, policy: &RuntimePolicy, available: &[String]) -> Option<String> {
        let want = if policy.runtime_mode == RuntimeMode::FullAccess { "yolo" } else { "default" };
        available.iter().find(|m| *m == want).cloned()
    }
}

fn config(mode: RuntimeMode) -> SessionConfig {
    SessionConfig {
        thread_id: ThreadId::new("thread-1"),
        policy: RuntimePolicy { runtime_mode: mode, cwd: Some("/tmp".into()), ..RuntimePolicy::default() },
        model: None,
        resume: None,
        mcp_servers: Vec::new(),
    }
}

async fn open(mode: RuntimeMode) -> (Box<dyn SessionRuntime>, Selection) {
    let selection: Selection = Arc::default();
    let adapter = AcpAdapter::new(
        InstanceId::new("mock"),
        Arc::new(TestFlavor),
        Arc::new(MockLauncher { selection: selection.clone() }),
    );
    let session = adapter.open_session(config(mode)).await.expect("open session");
    (session, selection)
}

/// Collects events until `done` matches one (or times out).
async fn collect_until(
    events: &mut (impl futures::Stream<Item = AdapterEvent> + Unpin),
    seen: &mut Vec<AdapterEvent>,
    done: impl Fn(&AdapterEvent) -> bool,
) {
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

fn turn_finished(e: &AdapterEvent) -> bool {
    matches!(e, AdapterEvent::TurnUpdated { turn } if turn.status.is_terminal() && turn.ordinal == 1)
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

#[tokio::test]
async fn full_turn_maps_every_update_and_waits_for_approval() {
    let (session, selection) = open(RuntimeMode::ApprovalRequired).await;
    let mut events = session.events();
    let mut seen = Vec::new();

    session.start_turn(TurnInput { text: "write main".into(), attachments: Vec::new() }).await.unwrap();

    collect_until(&mut events, &mut seen, |e| matches!(e, AdapterEvent::RequestUpdated { request } if request.status == RequestStatus::Pending)).await;
    let request = seen
        .iter()
        .find_map(|e| match e { AdapterEvent::RequestUpdated { request } => Some(request.clone()), _ => None })
        .unwrap();
    assert_eq!(request.kind, RequestKind::FileChange);
    assert_eq!(request.options.iter().map(|o| o.decision).collect::<Vec<_>>(), vec![
        ApprovalDecision::Accept,
        ApprovalDecision::AcceptForSession,
        ApprovalDecision::Decline
    ]);

    session.respond(&request.id, RequestResponse { option_id: Some("always".into()), ..Default::default() }).await.unwrap();
    collect_until(&mut events, &mut seen, turn_finished).await;
    assert_eq!(selection.lock().unwrap().as_deref(), Some("always"));

    let items = latest_items(&seen);
    let bodies: Vec<&ItemBody> = items.iter().map(|i| &i.body).collect();
    assert!(matches!(bodies[0], ItemBody::UserMessage { text } if text == "write main"));
    assert!(bodies.iter().any(|b| matches!(b, ItemBody::Reasoning { text, streaming: false } if text == "thinking…")));
    assert!(items.iter().any(|i| i.status == ItemStatus::Completed
        && matches!(&i.body, ItemBody::ToolCall { kind: ToolKind::Edit, output, .. }
            if matches!(output.as_slice(), [ContentBlock::Diff(d)] if d.new_text == "fn main() {}"))));
    assert!(bodies.iter().any(|b| matches!(b, ItemBody::AssistantMessage { content, streaming: false }
        if content == &vec![ContentBlock::Text { text: "Hello world".into() }])));

    assert!(seen.iter().any(|e| matches!(e, AdapterEvent::CatalogUpdated { update }
        if update.commands.as_ref().is_some_and(|c| c[0].name == "plan"))));
    assert!(seen.iter().any(|e| matches!(e, AdapterEvent::PlanUpdated { plan } if plan.status == PlanStatus::Completed)));
    assert!(seen.iter().any(|e| matches!(e, AdapterEvent::UsageUpdated { usage, .. } if usage.context_used_tokens == Some(1200))));
    assert!(seen.iter().any(|e| matches!(e, AdapterEvent::RequestUpdated { request } if request.status == RequestStatus::Resolved)));
}

#[tokio::test]
async fn full_access_auto_approves_without_asking() {
    let (session, selection) = open(RuntimeMode::FullAccess).await;
    let mut events = session.events();
    let mut seen = Vec::new();
    session.start_turn(TurnInput { text: "go".into(), attachments: Vec::new() }).await.unwrap();
    collect_until(&mut events, &mut seen, turn_finished).await;
    assert_eq!(selection.lock().unwrap().as_deref(), Some("allow"));
    assert!(!seen.iter().any(|e| matches!(e, AdapterEvent::RequestUpdated { .. })));
}

#[tokio::test]
async fn late_subscriber_gets_replay() {
    let (session, _) = open(RuntimeMode::FullAccess).await;
    {
        let mut events = session.events();
        let mut seen = Vec::new();
        session.start_turn(TurnInput { text: "go".into(), attachments: Vec::new() }).await.unwrap();
        collect_until(&mut events, &mut seen, turn_finished).await;
    }
    let replay: Vec<AdapterEvent> = session.events().take(3).collect().await;
    assert!(matches!(&replay[0], AdapterEvent::ThreadUpdated { thread }
        if thread.native_id.as_ref().map(NativeId::as_str) == Some("s1") && thread.status == ThreadStatus::Idle));
    assert!(matches!(&replay[1], AdapterEvent::TurnUpdated { turn } if turn.status == TurnStatus::Completed));
}

#[tokio::test]
async fn catalog_lists_saved_sessions() {
    let catalog = AcpSessionCatalog::new(Arc::new(MockLauncher { selection: Arc::default() }));
    let page = catalog.list("/work", None).await.unwrap();
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].native_id.as_str(), "old-1");
    assert_eq!(page.sessions[0].title.as_deref(), Some("Earlier work"));
    assert_eq!(page.sessions[0].cwd.as_deref(), Some("/work"));
}
