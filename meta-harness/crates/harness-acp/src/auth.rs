//! Login through ACP `authenticate`: the agent itself runs the flow (e.g. opens a
//! browser and waits on a local callback). `complete` lets a user hand back a
//! callback URL by hand when the browser could not reach the agent's listener.

use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1 as acp;
use async_trait::async_trait;
use harness_core::{AuthChallenge, AuthController, HarnessError, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::control::with_connection;
use crate::launch::AcpLauncher;

/// Called with `true` when a login succeeds and `false` after logout or failure.
pub type AuthChanged = Arc<dyn Fn(bool) + Send + Sync>;

pub struct AcpAuthController {
    launcher: Arc<dyn AcpLauncher>,
    on_change: AuthChanged,
    running: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl AcpAuthController {
    pub fn new(launcher: Arc<dyn AcpLauncher>, on_change: AuthChanged) -> Self {
        Self { launcher, on_change, running: Mutex::new(None) }
    }
}

#[async_trait]
impl AuthController for AcpAuthController {
    async fn begin(&self, method_id: &str) -> Result<AuthChallenge> {
        let launcher = self.launcher.clone();
        let on_change = self.on_change.clone();
        let method = acp::AuthMethodId::new(method_id.to_owned());
        let task = tokio::spawn(async move {
            let result = with_connection(&launcher, ".", async move |cx, _init| {
                cx.send_request(acp::AuthenticateRequest::new(method)).block_task().await
            })
            .await;
            if let Err(err) = &result {
                tracing::warn!(target: "harness_acp", "authentication failed: {err}");
            }
            on_change(result.is_ok());
        });
        if let Some(previous) = self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).replace(task) {
            previous.abort();
        }
        Ok(AuthChallenge {
            url: None,
            instructions: "Finish signing in in the browser window that opened. If its last page fails to load, paste that page's URL to complete the login.".into(),
        })
    }

    async fn complete(&self, response: &str) -> Result<()> {
        let url = response.trim();
        let rest = url
            .strip_prefix("http://127.0.0.1:")
            .or_else(|| url.strip_prefix("http://localhost:"))
            .ok_or_else(|| HarnessError::InvalidConfig { reason: "expected the http://127.0.0.1:<port>/… callback URL".into() })?;
        let (port, path) = rest.split_once('/').map(|(p, q)| (p, format!("/{q}"))).unwrap_or((rest, "/".into()));
        let port: u16 = port.parse().map_err(|_| HarnessError::InvalidConfig { reason: format!("bad port in {url}") })?;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes())
            .await?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await?;
        if reply.starts_with("HTTP/1.1 2") || reply.starts_with("HTTP/1.0 2") {
            Ok(())
        } else {
            Err(HarnessError::Provider { reason: format!("callback rejected: {}", reply.lines().next().unwrap_or("")) })
        }
    }

    async fn logout(&self) -> Result<()> {
        with_connection(&self.launcher, ".", async move |cx, _init| {
            cx.send_request(acp::LogoutRequest::new()).block_task().await.map(|_| ())
        })
        .await?;
        (self.on_change)(false);
        Ok(())
    }
}
