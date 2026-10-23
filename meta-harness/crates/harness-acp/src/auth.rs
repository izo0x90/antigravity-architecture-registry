//! Signing in, the agent's way. A login the agent runs itself goes through
//! ACP `authenticate`: it prints a link (on its error output) and waits on
//! a local callback; the link is shown to open on any device, and
//! `complete` hands the agent that callback's address, which the browser
//! (elsewhere) couldn't reach. A login that's a command of its own, we run
//! (see `login`): its link is shown, and `complete` hands it what the page
//! gave back.

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

/// How a login the agent runs itself ended (why, if it failed).
type Ended = tokio::sync::oneshot::Receiver<std::result::Result<(), String>>;

enum Running {
    /// Its login, waiting for the browser's callback on `port`.
    Agent { task: tokio::task::JoinHandle<()>, port: Option<u16>, ended: Ended },
    Command(Box<LoginRun>),
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
        if let Some(Running::Agent { task, .. }) = lock(&self.running).replace(next) {
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
                true => Ok(AuthChallenge { url: None, instructions: "Signed in.".into(), expects: None, within_s: None }),
                false => Err(HarnessError::Provider { reason: format!("the login failed: {}", started.run.last_words()) }),
            };
        }
        let Some(link) = started.link else {
            return Err(HarnessError::Provider { reason: format!("the login showed no link: {}", started.run.last_words()) });
        };
        let expects = settings.expects.clone().unwrap_or_else(|| "the code the page shows".into());
        self.replace(Running::Command(Box::new(started.run)));
        Ok(AuthChallenge {
            url: Some(link),
            instructions: format!("Open the link, sign in, then paste {expects} here."),
            expects: Some(expects),
            within_s: settings.within_s,
        })
    }

    /// A login the agent runs itself: started, and its link (once it
    /// prints it) shown. It may finish at once (a key in its environment).
    async fn begin_agent(&self, method: &str) -> Result<AuthChallenge> {
        let settings = &self.profile.settings.login;
        let wait_s = settings.wait_s.unwrap_or(30);
        let pattern = settings.link_pattern().map_err(|reason| HarnessError::InvalidConfig { reason })?;
        // A login started again replaces the one waiting.
        if let Some(Running::Agent { task, .. }) = lock(&self.running).take() {
            task.abort();
        }
        let (launcher, profile, on_change) = (self.launcher.clone(), self.profile.clone(), self.on_change.clone());
        let method = acp::AuthMethodId::new(method.to_owned());
        let (link_to, link) = tokio::sync::oneshot::channel::<String>();
        let (done_to, mut done) = tokio::sync::oneshot::channel::<std::result::Result<(), String>>();
        let task = tokio::spawn(async move {
            let result = with_connection(&launcher, &profile, async move |cx, opened| {
                // Its link, from what it says while it waits for the browser.
                tokio::spawn(async move {
                    if let Some(found) = crate::launch::says(opened.said, pattern).await {
                        let _ = link_to.send(found);
                    }
                });
                cx.send_request(acp::AuthenticateRequest::new(method)).block_task().await
            })
            .await;
            if let Err(err) = &result {
                tracing::warn!(target: "harness_acp", "authentication failed: {err}");
            }
            on_change(Some(result.is_ok()));
            let _ = done_to.send(result.map(|_| ()).map_err(|e| e.to_string()));
        });
        let expects = "the address of the page that failed to load".to_owned();
        let instructions = "Open the link and sign in. The last page won't load: that's expected. Copy its address and paste it here.".to_owned();
        let ended = tokio::select! {
            // Its end first, when both are in: why it ended is what's said.
            biased;
            ended = &mut done => ended,
            found = link => {
                let Ok(link) = found else {
                    // It ended before printing one: as its end says.
                    return Err(HarnessError::Provider { reason: "the login ended before it showed a link".into() });
                };
                let port = callback_port(&link);
                self.replace(Running::Agent { task, port, ended: done });
                return Ok(AuthChallenge { url: Some(link), instructions, expects: Some(expects), within_s: settings.within_s });
            },
            () = tokio::time::sleep(std::time::Duration::from_secs(wait_s)) => {
                task.abort();
                return Err(HarnessError::Provider { reason: format!("the login showed no link within {wait_s} seconds") });
            },
        };
        match ended {
                Ok(Ok(())) => Ok(AuthChallenge { url: None, instructions: "Signed in.".into(), expects: None, within_s: None }),
                Ok(Err(why)) => Err(HarnessError::Provider { reason: format!("the login failed: {why}") }),
                Err(_) => Err(HarnessError::Provider { reason: "the login stopped".into() }),
        }
    }

    /// Its login command, if one is waiting (a login the agent runs itself
    /// stays).
    fn take_command(&self) -> Option<Box<LoginRun>> {
        let mut running = lock(&self.running);
        match running.take() {
            Some(Running::Command(run)) => Some(run),
            other => {
                *running = other;
                None
            }
        }
    }

    /// The browser's callback, handed to the agent's login waiting for it;
    /// then how the login ended. A paste that isn't its callback leaves
    /// the login waiting.
    async fn finish_agent(&self, response: &str) -> Result<()> {
        let port = match &*lock(&self.running) {
            Some(Running::Agent { port, .. }) => *port,
            _ => return Err(HarnessError::Conflict { reason: "no sign-in is waiting: sign in again".into() }),
        };
        let port = port.ok_or_else(|| HarnessError::Provider { reason: "the login's link names no address to hand back to".into() })?;
        hand_back_callback(response, port).await?;
        let Some(Running::Agent { ended, .. }) = lock(&self.running).take() else {
            return Err(HarnessError::Conflict { reason: "the sign-in was started again meanwhile".into() });
        };
        match tokio::time::timeout(END_WAIT, ended).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(why))) => Err(HarnessError::Provider { reason: format!("the login failed: {why}") }),
            Ok(Err(_)) => Err(HarnessError::Provider { reason: "the login stopped".into() }),
            Err(_) => Err(HarnessError::Provider { reason: format!("the login didn't finish within {} seconds of the address", END_WAIT.as_secs()) }),
        }
    }
}

/// How long a login, handed its callback, is given to finish.
const END_WAIT: std::time::Duration = std::time::Duration::from_secs(60);


#[async_trait]
impl AuthController for AcpAuthController {
    async fn begin(&self, method_id: &str) -> Result<AuthChallenge> {
        let login = lock(&self.logins).iter().find(|l| l.method == method_id).cloned();
        match login {
            Some(login) => self.begin_command(login).await,
            None => self.begin_agent(method_id).await,
        }
    }

    async fn complete(&self, response: &str) -> Result<()> {
        match self.take_command() {
            Some(run) => {
                let result = run.finish(response).await;
                (self.on_change)(if result.is_ok() { None } else { Some(false) });
                result
            }
            None => self.finish_agent(response).await,
        }
    }

    async fn logout(&self) -> Result<()> {
        with_connection(&self.launcher, &self.profile, async move |cx, _| cx.send_request(acp::LogoutRequest::new()).block_task().await.map(|_| ())).await?;
        (self.on_change)(Some(false));
        Ok(())
    }
}

/// The local port a login's link sends the browser back to (its
/// `redirect_uri`, `http://127.0.0.1:<port>/…` or `localhost`).
fn callback_port(link: &str) -> Option<u16> {
    let (_, after) = link.split_once("redirect_uri=")?;
    let encoded = after.split('&').next()?;
    let back = encoded.replace("%3A", ":").replace("%3a", ":").replace("%2F", "/").replace("%2f", "/");
    let rest = back.strip_prefix("http://127.0.0.1:").or_else(|| back.strip_prefix("http://localhost:"))?;
    rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// Opens the agent's local callback address (`http://127.0.0.1:<port>/…`)
/// for it, as the browser would have; only the port its login waits on.
async fn hand_back_callback(response: &str, waits_on: u16) -> Result<()> {
    let url = response.trim();
    let wrong = || HarnessError::Refused {
        reason: format!("that isn't the address of the page that failed to load: it starts with http://localhost:{waits_on}/ or http://127.0.0.1:{waits_on}/"),
    };
    let rest = url.strip_prefix("http://127.0.0.1:").or_else(|| url.strip_prefix("http://localhost:")).ok_or_else(wrong)?;
    let (port, path) = rest.split_once('/').map(|(p, q)| (p, format!("/{q}"))).unwrap_or((rest, "/".into()));
    if port.parse::<u16>().ok() != Some(waits_on) || path.contains(char::is_whitespace) {
        return Err(wrong());
    }
    let port = waits_on;
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|_| HarnessError::Conflict { reason: "the sign-in isn't waiting any more (its time ran out?): sign in again".into() })?;
    stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await?;
    if reply.starts_with("HTTP/1.1 2") || reply.starts_with("HTTP/1.0 2") {
        Ok(())
    } else {
        Err(HarnessError::Provider { reason: format!("callback rejected: {}", reply.lines().next().unwrap_or("")) })
    }
}
