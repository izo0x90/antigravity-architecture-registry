//! Signing in, the agent's way. A login the agent runs itself goes through
//! ACP `authenticate` (it may open a browser and wait on a local callback;
//! `complete` hands it that callback's URL when the browser couldn't reach
//! it). A login that's a command of its own, we run (see `login`): its
//! link is shown, and `complete` hands it what the page gave back.

use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1 as acp;
use async_trait::async_trait;
use harness_contracts::Launch;
use harness_core::{AuthChallenge, AuthController, HarnessError, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::control::{CommandLogin, with_connection};
use crate::launch::AcpLauncher;
use crate::login::LoginRun;
use crate::profile::Profile;

/// Called after a login or logout: `Some(true)` signed in, `Some(false)`
/// signed out, `None` unknown (check again).
pub type AuthChanged = Arc<dyn Fn(Option<bool>) + Send + Sync>;

enum Running {
    Agent(tokio::task::JoinHandle<()>),
    Command(LoginRun),
}

pub struct AcpAuthController {
    launcher: Arc<dyn AcpLauncher>,
    profile: Arc<Profile>,
    /// How the agent is started (its login commands are run the same way).
    launch: Option<Launch>,
    /// Its login commands, from its last check.
    logins: Mutex<Vec<CommandLogin>>,
    on_change: AuthChanged,
    running: Mutex<Option<Running>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl AcpAuthController {
    pub fn new(launcher: Arc<dyn AcpLauncher>, profile: Arc<Profile>, launch: Option<Launch>, on_change: AuthChanged) -> Self {
        Self { launcher, profile, launch, logins: Mutex::default(), on_change, running: Mutex::default() }
    }

    /// The agent's login commands, as its last check found them.
    pub fn set_logins(&self, logins: Vec<CommandLogin>) {
        *lock(&self.logins) = logins;
    }

    fn replace(&self, next: Running) {
        if let Some(Running::Agent(task)) = lock(&self.running).replace(next) {
            task.abort();
        }
    }

    async fn begin_command(&self, login: CommandLogin) -> Result<AuthChallenge> {
        let launch = self.launch.as_ref().ok_or_else(|| HarnessError::Provider { reason: "the agent can't be started to sign in".into() })?;
        let settings = &self.profile.settings.login;
        let started = LoginRun::start(launch, &login, &self.profile.project, settings.link.as_deref()).await?;
        if let Some(ok) = started.exited {
            (self.on_change)(None);
            return match ok {
                true => Ok(AuthChallenge { url: None, instructions: "Signed in.".into(), expects: None }),
                false => Err(HarnessError::Provider { reason: format!("the login failed: {}", started.run.last_words()) }),
            };
        }
        let Some(link) = started.link else {
            return Err(HarnessError::Provider { reason: format!("the login showed no link: {}", started.run.last_words()) });
        };
        let expects = settings.expects.clone().unwrap_or_else(|| "the code the page shows".into());
        self.replace(Running::Command(started.run));
        Ok(AuthChallenge {
            url: Some(link),
            instructions: format!("Open the link, sign in, then paste {expects} here."),
            expects: Some(expects),
        })
    }

    fn begin_agent(&self, method: &str) -> AuthChallenge {
        let (launcher, profile, on_change) = (self.launcher.clone(), self.profile.clone(), self.on_change.clone());
        let method = acp::AuthMethodId::new(method.to_owned());
        let task = tokio::spawn(async move {
            let result = with_connection(&launcher, &profile, async move |cx, _| cx.send_request(acp::AuthenticateRequest::new(method)).block_task().await).await;
            if let Err(err) = &result {
                tracing::warn!(target: "harness_acp", "authentication failed: {err}");
            }
            on_change(Some(result.is_ok()));
        });
        self.replace(Running::Agent(task));
        AuthChallenge {
            url: None,
            instructions: "Finish signing in in the browser window that opened. If its last page fails to load, paste that page's address here.".into(),
            expects: Some("the address of the page that failed to load".into()),
        }
    }
}

#[async_trait]
impl AuthController for AcpAuthController {
    async fn begin(&self, method_id: &str) -> Result<AuthChallenge> {
        let login = lock(&self.logins).iter().find(|l| l.method == method_id).cloned();
        match login {
            Some(login) => self.begin_command(login).await,
            None => Ok(self.begin_agent(method_id)),
        }
    }

    async fn complete(&self, response: &str) -> Result<()> {
        let command = match lock(&self.running).take() {
            Some(Running::Command(run)) => Some(run),
            other => {
                *lock(&self.running) = other;
                None
            }
        };
        match command {
            Some(run) => {
                let result = run.finish(response).await;
                (self.on_change)(if result.is_ok() { None } else { Some(false) });
                result
            }
            None => hand_back_callback(response).await,
        }
    }

    async fn logout(&self) -> Result<()> {
        with_connection(&self.launcher, &self.profile, async move |cx, _| cx.send_request(acp::LogoutRequest::new()).block_task().await.map(|_| ())).await?;
        (self.on_change)(Some(false));
        Ok(())
    }
}

/// Opens the agent's local callback URL (`http://127.0.0.1:<port>/…`) for
/// it, as the browser would have.
async fn hand_back_callback(response: &str) -> Result<()> {
    let url = response.trim();
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .or_else(|| url.strip_prefix("http://localhost:"))
        .ok_or_else(|| HarnessError::InvalidConfig { reason: "expected the http://127.0.0.1:<port>/… address".into() })?;
    let (port, path) = rest.split_once('/').map(|(p, q)| (p, format!("/{q}"))).unwrap_or((rest, "/".into()));
    let port: u16 = port.parse().map_err(|_| HarnessError::InvalidConfig { reason: format!("bad port in {url}") })?;
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await?;
    if reply.starts_with("HTTP/1.1 2") || reply.starts_with("HTTP/1.0 2") {
        Ok(())
    } else {
        Err(HarnessError::Provider { reason: format!("callback rejected: {}", reply.lines().next().unwrap_or("")) })
    }
}
