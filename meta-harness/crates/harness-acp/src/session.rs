//! A live ACP session (one agent process/connection per session) behind
//! `SessionRuntime`. Messages sent while a turn runs queue: at the agent
//! when it queues them itself, else here, sent when the one before ends.
//! A message can also go into the running turn, when the agent takes
//! steering.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{Agent, ConnectionTo, UntypedMessage};
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use harness_contracts::{
    AdapterEvent, Attachment, Failure, RequestId, RequestResponse, RequestStatus, RuntimePolicy, SteerInput,
    ThreadId, TurnId, TurnInput, TurnStatus,
};
use harness_core::{HarnessError, Result, SessionRuntime};
use tokio::sync::oneshot;
use tokio_stream::wrappers::BroadcastStream;

use crate::convert;
use crate::profile::{Declared, Profile};
use crate::state::SessionState;

/// The request that adds a message to the running turn, and what it says
/// when none is running (we then send it as a prompt).
const STEERING_METHOD: &str = "_session/steering";
const INJECTED: &str = "injected";
const PROMPT_REQUIRED: &str = "promptRequired";

/// State shared between the session handle and the connection's handlers.
pub(crate) struct Shared {
    pub state: Mutex<SessionState>,
    /// Requests the agent waits on, answered with the user's response
    /// (dropped unanswered: cancelled).
    pub pending: Mutex<HashMap<RequestId, oneshot::Sender<RequestResponse>>>,
    pub policy: Mutex<RuntimePolicy>,
    pub profile: Arc<Profile>,
    /// Turns waiting here for the one before them to end.
    pub waiting: Mutex<VecDeque<(TurnId, TurnInput)>>,
}

impl Shared {
    /// Ends a turn; what it still asked is dropped (so cancelled).
    fn end_turn(&self, turn: &TurnId, status: TurnStatus, failure: Option<Failure>) {
        let cancelled = lock(&self.state).end_turn(turn, status, failure);
        let mut pending = lock(&self.pending);
        for id in cancelled {
            pending.remove(&id);
        }
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn acp_error(err: agent_client_protocol::Error) -> HarnessError {
    if err.code == agent_client_protocol::ErrorCode::AuthRequired {
        HarnessError::AuthRequired { reason: err.message }
    } else {
        HarnessError::Provider { reason: format!("{}: {}", i32::from(err.code), err.message) }
    }
}

/// The session at the agent: where prompts go.
pub(crate) struct Link {
    pub cx: ConnectionTo<Agent>,
    pub native_id: acp::SessionId,
    pub shared: Arc<Shared>,
    /// How it's sent a message's pieces.
    pub sending: crate::pieces::Sending,
}

/// Sends `input` as turn `turn`'s prompt; when it ends, the next waiting
/// turn goes. Its end is taken in order with what the agent sends: what
/// it says after answering this prompt is the next turn's.
fn send_prompt(link: Arc<Link>, turn: TurnId, input: TurnInput) {
    let request = acp::PromptRequest::new(link.native_id.clone(), prompt_blocks(input, link.sending));
    let (on_end, ended) = (link.clone(), turn.clone());
    let sent = link.cx.send_request(request).on_receiving_result(move |result| async move {
        match result {
            Ok(done) => on_end.shared.end_turn(&ended, convert::stop_reason(&done.stop_reason), None),
            Err(err) => on_end.shared.end_turn(&ended, TurnStatus::Failed, Some(Failure { message: acp_error(err).to_string(), retryable: false })),
        }
        let next = lock(&on_end.shared.waiting).pop_front();
        if let Some((turn, input)) = next {
            send_prompt(on_end, turn, input);
        }
        Ok(())
    });
    if let Err(err) = sent {
        link.shared.end_turn(&turn, TurnStatus::Failed, Some(Failure { message: acp_error(err).to_string(), retryable: true }));
    }
}

pub struct AcpSession {
    pub(crate) thread_id: ThreadId,
    pub(crate) link: Arc<Link>,
    /// What the agent does beyond the protocol (queueing, steering).
    pub(crate) declared: Declared,
    /// The agent ends sessions via `session/close`.
    pub(crate) supports_close: bool,
    pub(crate) connection: tokio::task::JoinHandle<()>,
    pub(crate) _guard: Box<dyn Send + Sync>,
}

impl AcpSession {
    fn shared(&self) -> &Arc<Shared> {
        &self.link.shared
    }

    async fn set_mode(&self, mode: String) -> Result<()> {
        self.link
            .cx
            .send_request(acp::SetSessionModeRequest::new(self.link.native_id.clone(), mode.clone()))
            .block_task()
            .await
            .map_err(acp_error)?;
        lock(&self.shared().state).set_thread(|t| t.controls.current_mode = Some(mode));
        Ok(())
    }

    async fn set_config(&self, id: &str, value: &serde_json::Value) -> Result<()> {
        let value: acp::SessionConfigOptionValue = match value {
            serde_json::Value::Bool(b) => (*b).into(),
            serde_json::Value::String(s) => s.as_str().into(),
            other => return Err(HarnessError::InvalidConfig { reason: format!("unsupported value for {id}: {other}") }),
        };
        self.link
            .cx
            .send_request(acp::SetSessionConfigOptionRequest::new(self.link.native_id.clone(), id.to_owned(), value))
            .block_task()
            .await
            .map_err(acp_error)?;
        Ok(())
    }

    pub(crate) async fn apply_policy(&self, policy: RuntimePolicy) -> Result<()> {
        let modes: Vec<String> = lock(&self.shared().state).controls().modes.iter().map(|m| m.id.clone()).collect();
        if let Some(mode) = self.shared().profile.native_mode(&policy, &modes) {
            self.set_mode(mode).await?;
        }
        *lock(&self.shared().policy) = policy.clone();
        lock(&self.shared().state).set_thread(|t| t.policy = policy);
        Ok(())
    }
}

/// A turn's input as the agent gets it: its text and pieces (as pieces,
/// when it takes them), then its attachments.
fn prompt_blocks(input: TurnInput, sending: crate::pieces::Sending) -> Vec<acp::ContentBlock> {
    let TurnInput { text, attachments, pieces } = input;
    let mut blocks = crate::pieces::blocks(text, pieces, sending);
    for attachment in attachments {
        blocks.push(match attachment {
            Attachment::File { path } => {
                let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                acp::ContentBlock::ResourceLink(acp::ResourceLink::new(name, format!("file://{path}")))
            }
            Attachment::Image { mime_type, data_base64 } => acp::ContentBlock::Image(acp::ImageContent::new(data_base64, mime_type)),
        });
    }
    blocks
}

#[async_trait]
impl SessionRuntime for AcpSession {
    fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    fn native_id(&self) -> Option<harness_contracts::NativeId> {
        Some(harness_contracts::NativeId::new(self.link.native_id.to_string()))
    }

    fn events(&self) -> BoxStream<'static, AdapterEvent> {
        let (replay, rx) = {
            let state = lock(&self.shared().state);
            (state.replay(), state.subscribe())
        };
        let live = BroadcastStream::new(rx).filter_map(|e| async move { e.ok() });
        futures::stream::iter(replay).chain(live).boxed()
    }

    async fn start_turn(&self, input: TurnInput) -> Result<TurnId> {
        let (turn, now) = {
            let mut state = lock(&self.shared().state);
            let turn = state.begin_turn(&input);
            let now = self.declared.queueing || state.active_turn() == Some(&turn);
            (turn, now)
        };
        if now {
            send_prompt(self.link.clone(), turn.clone(), input);
        } else {
            lock(&self.shared().waiting).push_back((turn.clone(), input));
        }
        Ok(turn)
    }

    async fn inject(&self, input: TurnInput) -> Result<TurnId> {
        if !self.declared.steering {
            return Err(HarnessError::Unsupported { operation: "adding to a running turn" });
        }
        // In the turn before it's sent: what the agent says to it comes after.
        let Some((turn, item)) = lock(&self.shared().state).inject(&input) else {
            return self.start_turn(input).await;
        };
        let params = serde_json::json!({
            "sessionId": self.link.native_id.to_string(),
            "prompt": prompt_blocks(input.clone(), self.link.sending),
            "_meta": { "steering": { "idleBehavior": PROMPT_REQUIRED } },
        });
        let request = UntypedMessage::new(STEERING_METHOD, params).map_err(acp_error)?;
        let answer = self.link.cx.send_request(request).block_task().await;
        let injected = answer.as_ref().is_ok_and(|a| a.get("outcome").and_then(serde_json::Value::as_str) == Some(INJECTED));
        if injected {
            // It starts the next turn (as the session's history has it).
            return Ok(lock(&self.shared().state).steered(&item).unwrap_or(turn));
        }
        lock(&self.shared().state).withdraw(&item);
        answer.map_err(acp_error)?;
        // Not taken (the turn ended meanwhile): it's a turn of its own.
        self.start_turn(input).await
    }

    async fn steer(&self, input: SteerInput) -> Result<()> {
        if let Some(model) = input.model {
            let control = lock(&self.shared().state).controls().model_control().map(|c| c.id.clone());
            let Some(control) = control else {
                return Err(HarnessError::Unsupported { operation: "model switch" });
            };
            let value = serde_json::Value::String(model.model.clone());
            self.set_config(&control, &value).await?;
            let mut state = lock(&self.shared().state);
            state.set_config_value(&control, &value);
            state.set_thread(|t| t.model = Some(model));
        }
        for option in &input.config {
            self.set_config(&option.id, &option.value).await?;
            lock(&self.shared().state).set_config_value(&option.id, &option.value);
        }
        if let Some(mode) = input.mode {
            self.set_mode(mode).await?;
        }
        if let Some(policy) = input.policy {
            self.apply_policy(policy).await?;
        }
        Ok(())
    }

    async fn interrupt(&self) -> Result<()> {
        self.link.cx.send_notification(acp::CancelNotification::new(self.link.native_id.clone())).map_err(acp_error)
    }

    async fn cancel_subagent(&self, id: &harness_contracts::SubagentId) -> Result<()> {
        // Its own session, when its harness lets it be stopped alone.
        let Some(session) = lock(&self.shared().state).cancellable(id) else {
            return Err(HarnessError::Unsupported { operation: "stopping this sub-agent alone" });
        };
        self.link.cx.send_notification(acp::CancelNotification::new(acp::SessionId::new(session))).map_err(acp_error)
    }

    async fn respond(&self, request: &RequestId, response: RequestResponse) -> Result<()> {
        let mut state = lock(&self.shared().state);
        let Some(pending) = state.request(request) else {
            return Err(HarnessError::NotFound { what: format!("request {request}") });
        };
        if pending.status != RequestStatus::Pending {
            return Err(HarnessError::InvalidConfig { reason: format!("request {request} is no longer pending") });
        }
        if let Some(option) = &response.option_id
            && !pending.options.iter().any(|o| &o.id == option)
        {
            return Err(HarnessError::InvalidConfig { reason: format!("unknown option {option}") });
        }
        let Some(reply) = lock(&self.shared().pending).remove(request) else {
            return Err(HarnessError::NotFound { what: format!("request {request}") });
        };
        let _ = reply.send(response.clone());
        state.resolve_request(request, response);
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        if self.supports_close {
            let request = acp::CloseSessionRequest::new(self.link.native_id.clone());
            if let Err(err) = self.link.cx.send_request(request).block_task().await {
                tracing::debug!(target: "harness_acp", "session/close failed: {}", acp_error(err));
            }
        }
        self.connection.abort();
        Ok(())
    }
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        self.connection.abort();
    }
}
