//! Saved-session listing for any ACP agent (`session/list`, `session/delete`).

use std::sync::Arc;

use agent_client_protocol::schema::v1 as acp;
use async_trait::async_trait;
use harness_contracts::{NativeId, SavedSession, SavedSessionPage};
use harness_core::{Result, SessionCatalog};

use crate::control::with_connection;
use crate::launch::AcpLauncher;
use crate::profile::Profile;

pub struct AcpSessionCatalog {
    launcher: Arc<dyn AcpLauncher>,
    profile: Arc<Profile>,
}

impl AcpSessionCatalog {
    pub fn new(launcher: Arc<dyn AcpLauncher>, profile: Arc<Profile>) -> Self {
        Self { launcher, profile }
    }
}

#[async_trait]
impl SessionCatalog for AcpSessionCatalog {
    /// The sessions saved in `cwd` (a relative one is in the project).
    async fn list(&self, cwd: &str, cursor: Option<&str>) -> Result<SavedSessionPage> {
        // As the agent saved it: no `.` parts (Antigravity matches the text).
        let cwd: std::path::PathBuf = self.profile.project.join(cwd).components().collect();
        let request = acp::ListSessionsRequest::new().cwd(cwd).cursor(cursor.map(str::to_owned));
        let response =
            with_connection(&self.launcher, &self.profile, async move |cx, _| cx.send_request(request).block_task().await).await?;
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
        with_connection(&self.launcher, &self.profile, async move |cx, _| cx.send_request(request).block_task().await.map(|_| ())).await
    }
}
