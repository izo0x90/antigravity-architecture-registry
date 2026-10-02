//! A live ACP session (one agent process/connection per session) behind
//! `SessionRuntime`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{Agent, ConnectionTo};
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
use crate::flavor::AcpFlavor;
use crate::state::SessionState;

/// State shared between the session handle and the connection's handlers.
pub(crate) struct Shared {
    pub state: Mutex<SessionState>,
    /// Pending permission prompts: reply with the chosen option id (None = cancel).
    pub pending: Mutex<HashMap<RequestId, oneshot::Sender<Option<String>>>>,
    pub policy: Mutex<RuntimePolicy>,
    pub flavor: Arc<dyn AcpFlavor>,
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

pub struct AcpSession {
    pub(crate) thread_id: ThreadId,
    pub(crate) native_id: acp::SessionId,
    pub(crate) cx: ConnectionTo<Agent>,
    pub(crate) shared: Arc<Shared>,
    /// Native mode ids the session offers.
    pub(crate) modes: Vec<String>,
    /// Config option id that selects the model, if the agent exposes one.
    pub(crate) model_option: Option<acp::SessionConfigId>,
    pub(crate) connection: tokio::task::JoinHandle<()>,
    pub(crate) _guard: Box<dyn Send + Sync>,
}

impl AcpSession {
    pub(crate) async fn apply_policy(&self, policy: RuntimePolicy) -> Result<()> {
        if let Some(mode) = self.shared.flavor.native_mode(&policy, &self.modes) {
            self.cx
                .send_request(acp::SetSessionModeRequest::new(self.native_id.clone(), mode))
                .block_task()
                .await
                .map_err(acp_error)?;
        }
        *lock(&self.shared.policy) = policy.clone();
        lock(&self.shared.state).set_thread(|t| t.policy = policy);
        Ok(())
    }
}

fn prompt_blocks(input: TurnInput) -> Vec<acp::ContentBlock> {
    let mut blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(input.text))];
    for attachment in input.attachments {
        blocks.push(match attachment {
            Attachment::File { path } => {
                let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                acp::ContentBlock::ResourceLink(acp::ResourceLink::new(name, format!("file://{path}")))
            }
            Attachment::Image { mime_type, data_base64 } => {
                acp::ContentBlock::Image(acp::ImageContent::new(data_base64, mime_type))
            }
        });
    }
    blocks
}

#[async_trait]
impl SessionRuntime for AcpSession {
    fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    fn events(&self) -> BoxStream<'static, AdapterEvent> {
        let (replay, rx) = {
            let state = lock(&self.shared.state);
            (state.replay(), state.subscribe())
        };
        let live = BroadcastStream::new(rx).filter_map(|e| async move { e.ok() });
        futures::stream::iter(replay).chain(live).boxed()
    }

    async fn start_turn(&self, input: TurnInput) -> Result<TurnId> {
        let turn_id = {
            let mut state = lock(&self.shared.state);
            if state.turn_running() {
                return Err(HarnessError::Unsupported { operation: "queued messages" });
            }
            state.begin_turn(&input.text)
        };
        let request = acp::PromptRequest::new(self.native_id.clone(), prompt_blocks(input));
        let cx = self.cx.clone();
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let result = cx.send_request(request).block_task().await;
            {
                let mut state = lock(&shared.state);
                match result {
                    Ok(done) => state.end_turn(convert::stop_reason(&done.stop_reason), None),
                    Err(err) => state.end_turn(TurnStatus::Failed, Some(Failure { message: acp_error(err).to_string(), retryable: false })),
                }
            }
            // Unanswered prompts resolve as cancelled once their senders drop.
            lock(&shared.pending).clear();
        });
        Ok(turn_id)
    }

    async fn steer(&self, input: SteerInput) -> Result<()> {
        if let Some(model) = input.model {
            let Some(option) = self.model_option.clone() else {
                return Err(HarnessError::Unsupported { operation: "model switch" });
            };
            self.cx
                .send_request(acp::SetSessionConfigOptionRequest::new(self.native_id.clone(), option, model.model.as_str()))
                .block_task()
                .await
                .map_err(acp_error)?;
            lock(&self.shared.state).set_thread(|t| t.model = Some(model));
        }
        if let Some(policy) = input.policy {
            self.apply_policy(policy).await?;
        }
        Ok(())
    }

    async fn interrupt(&self) -> Result<()> {
        self.cx
            .send_notification(acp::CancelNotification::new(self.native_id.clone()))
            .map_err(acp_error)
    }

    async fn respond(&self, request: &RequestId, response: RequestResponse) -> Result<()> {
        let mut state = lock(&self.shared.state);
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
        let Some(reply) = lock(&self.shared.pending).remove(request) else {
            return Err(HarnessError::NotFound { what: format!("request {request}") });
        };
        let _ = reply.send(response.option_id.clone());
        state.resolve_request(request, response);
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        self.connection.abort();
        Ok(())
    }
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        self.connection.abort();
    }
}
