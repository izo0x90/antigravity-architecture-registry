//! Saved-session listing for any ACP agent (`session/list`, `session/delete`), over a
//! short-lived connection of its own.

use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{Agent, ByteStreams, ConnectionTo};
use async_trait::async_trait;
use harness_contracts::{
    NativeId, RuntimePolicy, SavedSession, SavedSessionPage, SessionConfig, ThreadId,
};
use harness_core::{HarnessError, Result, SessionCatalog};

use crate::launch::AcpLauncher;
use crate::session::acp_error;

pub struct AcpSessionCatalog {
    launcher: Arc<dyn AcpLauncher>,
}

impl AcpSessionCatalog {
    pub fn new(launcher: Arc<dyn AcpLauncher>) -> Self {
        Self { launcher }
    }

    /// Initializes a fresh connection and runs `op` on it.
    async fn with_connection<T: Send + 'static>(
        &self,
        cwd: &str,
        op: impl AsyncFnOnce(ConnectionTo<Agent>) -> agent_client_protocol::Result<T> + Send + 'static,
    ) -> Result<T> {
        let config = SessionConfig {
            thread_id: ThreadId::new("session-catalog"),
            policy: RuntimePolicy { cwd: Some(cwd.to_owned()), ..RuntimePolicy::default() },
            model: None,
            resume: None,
            mcp_servers: Vec::new(),
        };
        let transport = self.launcher.launch(&config).await?;
        let streams = ByteStreams::new(transport.writer, transport.reader);
        let (tx, rx) = tokio::sync::oneshot::channel();
        agent_client_protocol::Client
            .builder()
            .name("meta-harness-catalog")
            .connect_with(streams, async move |cx: ConnectionTo<Agent>| {
                cx.send_request(acp::InitializeRequest::new(ProtocolVersion::V1)).block_task().await?;
                let _ = tx.send(op(cx).await);
                Ok(())
            })
            .await
            .map_err(acp_error)?;
        drop(transport.guard);
        rx.await
            .map_err(|_| HarnessError::Provider { reason: "session catalog connection closed".into() })?
            .map_err(acp_error)
    }
}

#[async_trait]
impl SessionCatalog for AcpSessionCatalog {
    async fn list(&self, cwd: &str, cursor: Option<&str>) -> Result<SavedSessionPage> {
        let request = acp::ListSessionsRequest::new().cwd(PathBuf::from(cwd)).cursor(cursor.map(str::to_owned));
        let response = self
            .with_connection(cwd, async move |cx| cx.send_request(request).block_task().await)
            .await?;
        Ok(SavedSessionPage {
            sessions: response
                .sessions
                .into_iter()
                .map(|s| SavedSession {
                    native_id: NativeId::new(s.session_id.to_string()),
                    title: s.title,
                    cwd: Some(s.cwd.display().to_string()),
                    updated_at: s.updated_at,
                })
                .collect(),
            next_cursor: response.next_cursor,
        })
    }

    async fn delete(&self, id: &NativeId) -> Result<()> {
        let request = acp::DeleteSessionRequest::new(acp::SessionId::new(id.as_str().to_owned()));
        self.with_connection(".", async move |cx| cx.send_request(request).block_task().await.map(|_| ()))
            .await
    }
}
