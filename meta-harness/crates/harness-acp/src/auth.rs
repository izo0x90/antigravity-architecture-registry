//! Signing in, the agent's way, held on the project's server: one sign-in
//! under way at a time, said in the snapshot (for any device to finish)
//! until it's taken, stopped, replaced or runs out.
//!
//! A login the agent runs itself goes through ACP `authenticate`: it prints
//! a link (on its error output) and waits on a local callback; the link is
//! shown to open on any device, and `complete` hands the agent that
//! callback's address, which the browser (elsewhere) couldn't reach. A
//! login that's a command of its own, we run (see `login`): its link is
//! shown, and `complete` hands it what the page gave back.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1 as acp;
use async_trait::async_trait;
use harness_contracts::{Launch, SignInWaiting};
use harness_core::{AuthController, HarnessError, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

use crate::control::{CommandLogin, with_connection};
use crate::launch::AcpLauncher;
use crate::login::{FINISH_WAIT, LoginRun};
use crate::profile::Profile;

/// What a sign-in changed.
#[derive(Clone, Debug, PartialEq)]
pub enum AuthEvent {
    /// `Some(true)` signed in, `Some(false)` signed out, `None` unknown
    /// (check again).
    Status(Option<bool>),
    /// The sign-in under way, or none any more.
    Waiting(Option<SignInWaiting>),
}

pub type AuthChanged = Arc<dyn Fn(AuthEvent) + Send + Sync>;

/// How a login the agent runs itself ended (why, if it failed), once it has.
type Ended = watch::Receiver<Option<std::result::Result<(), String>>>;

enum Running {
    /// The agent's login: its callback's port once it's shown its link.
    Agent { task: tokio::task::JoinHandle<()>, port: Option<u16>, ended: Ended },
    Command(Box<LoginRun>),
}

/// The sign-in under way.
struct Under {
    id: u64,
    running: Running,
}

type Slot = Arc<Mutex<Option<Under>>>;

pub struct AcpAuthController {
    launcher: Arc<dyn AcpLauncher>,
    profile: Arc<Profile>,
    /// How the agent is started (its login commands are run the same way).
    launch: Option<Launch>,
    /// Its login commands, from its last check.
    logins: Mutex<Vec<CommandLogin>>,
    on_change: AuthChanged,
    under: Slot,
    ids: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Stops a sign-in: the agent's login aborted (its process with it), a
/// login command killed.
fn stop(running: Running) {
    if let Running::Agent { task, .. } = running {
        task.abort();
    }
}

/// Ends sign-in `id` if it's still the one under way, saying so.
fn end(under: &Slot, id: u64, on_change: &AuthChanged) {
    let taken = {
        let mut under = lock(under);
        if under.as_ref().is_some_and(|u| u.id == id) { under.take() } else { None }
    };
    if let Some(taken) = taken {
        stop(taken.running);
        on_change(AuthEvent::Waiting(None));
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl AcpAuthController {
    pub fn new(launcher: Arc<dyn AcpLauncher>, profile: Arc<Profile>, launch: Option<Launch>, on_change: AuthChanged) -> Self {
        Self { launcher, profile, launch, logins: Mutex::default(), on_change, under: Slot::default(), ids: AtomicU64::new(0) }
    }

    /// The agent's login commands, as its last check found them.
    pub fn set_logins(&self, logins: Vec<CommandLogin>) {
        *lock(&self.logins) = logins;
    }

    /// The pattern of its login's link.
    fn link_pattern(&self) -> Result<regex::Regex> {
        self.profile.login_link.clone().ok_or_else(|| HarnessError::InvalidConfig { reason: "the login link pattern is wrong".into() })
    }

    /// Stops the sign-in under way, if any, saying so.
    fn stop_under_way(&self) {
        let taken = lock(&self.under).take();
        if let Some(taken) = taken {
            stop(taken.running);
            (self.on_change)(AuthEvent::Waiting(None));
        }
    }

    /// Ends sign-in `id` when its time is up.
    fn expire(&self, id: u64, within_s: u64) {
        let (under, on_change) = (self.under.clone(), self.on_change.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(within_s)).await;
            end(&under, id, &on_change);
        });
    }

    async fn begin_command(&self, id: u64, login: CommandLogin) -> Result<Option<SignInWaiting>> {
        let launch = self.launch.as_ref().ok_or_else(|| HarnessError::Provider { reason: "the agent can't be started to sign in".into() })?;
        let settings = &self.profile.settings.login;
        let started = LoginRun::start(launch, &login, &self.profile.project, &self.link_pattern()?, Duration::from_secs(settings.wait_s)).await?;
        if let Some(ok) = started.exited {
            (self.on_change)(AuthEvent::Status(None));
            return match ok {
                true => Ok(None),
                false => Err(HarnessError::Provider { reason: format!("the sign-in failed: {}", started.run.last_words()) }),
            };
        }
        let Some(link) = started.link else {
            return Err(HarnessError::Provider { reason: format!("the sign-in showed no link: {}", started.run.last_words()) });
        };
        let expects = settings.expects.clone().unwrap_or_else(|| "the code the page shows".into());
        let waiting = SignInWaiting {
            id,
            url: Some(link),
            instructions: format!("Open the link and sign in, then paste {expects} in Finish signing in."),
            expects: Some(expects),
            until_ms: settings.within_s.map(|s| now_ms() + s * 1000),
        };
        *lock(&self.under) = Some(Under { id, running: Running::Command(Box::new(started.run)) });
        (self.on_change)(AuthEvent::Waiting(Some(waiting.clone())));
        if let Some(within_s) = settings.within_s {
            self.expire(id, within_s);
        }
        Ok(Some(waiting))
    }

    /// A login the agent runs itself: started, held as the sign-in under
    /// way (so a request that goes away doesn't lose it), and its link,
    /// once it prints it, shown. It may finish at once (a key in its
    /// environment). The server gives it `within_s` from its start.
    async fn begin_agent(&self, id: u64, method: &str) -> Result<Option<SignInWaiting>> {
        let settings = &self.profile.settings.login;
        let (launcher, profile, on_change, under) = (self.launcher.clone(), self.profile.clone(), self.on_change.clone(), self.under.clone());
        let method = acp::AuthMethodId::new(method.to_owned());
        let (within_s, pattern) = (settings.within_s, self.link_pattern()?);
        let until_ms = within_s.map(|s| now_ms() + s * 1000);
        let (link_to, link) = tokio::sync::oneshot::channel::<std::result::Result<SignInWaiting, String>>();
        let (ended_to, ended) = watch::channel(None);
        let task = tokio::spawn(async move {
            let shown = (under.clone(), on_change.clone());
            let login = with_connection(&launcher, &profile, async move |cx, opened| {
                // Its link, from what it says while it waits for the browser:
                // shown once it names the callback to hand back to.
                tokio::spawn(async move {
                    let Some(link) = crate::launch::says(opened.said, pattern).await else { return };
                    let Some(port) = callback_port(&link) else {
                        let _ = link_to.send(Err(format!("its link names no local address to hand the page's address back to: {link}")));
                        return;
                    };
                    let waiting = SignInWaiting {
                        id,
                        url: Some(link),
                        instructions: "Open the link and sign in. The last page won't load: that's expected. Copy its address and paste it in Finish signing in.".into(),
                        expects: Some("the address of the page that failed to load".into()),
                        until_ms,
                    };
                    let (under, on_change) = shown;
                    let current = {
                        let mut under = lock(&under);
                        match under.as_mut() {
                            Some(Under { id: at, running: Running::Agent { port: p, .. } }) if *at == id => {
                                *p = Some(port);
                                true
                            }
                            _ => false,
                        }
                    };
                    if current {
                        on_change(AuthEvent::Waiting(Some(waiting.clone())));
                    }
                    let _ = link_to.send(Ok(waiting));
                });
                cx.send_request(acp::AuthenticateRequest::new(method)).block_task().await
            });
            let result = match within_s {
                Some(s) => tokio::time::timeout(Duration::from_secs(s), login).await.unwrap_or_else(|_| Err(HarnessError::Provider { reason: "its time ran out".into() })),
                None => login.await,
            };
            if let Err(err) = &result {
                tracing::warn!(target: "harness_acp", "sign-in failed: {err}");
            }
            ended_to.send_replace(Some(result.as_ref().map(|_| ()).map_err(|e| match e {
                HarnessError::Provider { reason } => reason.clone(),
                e => e.to_string(),
            })));
            on_change(AuthEvent::Status(Some(result.is_ok())));
            end(&under, id, &on_change);
        });
        *lock(&self.under) = Some(Under { id, running: Running::Agent { task, port: None, ended: ended.clone() } });
        tokio::select! {
            found = link => match found {
                Ok(Ok(waiting)) => Ok(Some(waiting)),
                Ok(Err(why)) => {
                    end(&self.under, id, &self.on_change);
                    Err(HarnessError::Provider { reason: why })
                }
                // It stopped saying anything before its link: as it ended.
                Err(_) => match how_it_ended(ended, Duration::from_secs(5)).await {
                    Some(Ok(())) => Ok(None),
                    Some(Err(why)) => Err(HarnessError::Provider { reason: format!("the sign-in failed: {why}") }),
                    None => {
                        end(&self.under, id, &self.on_change);
                        Err(HarnessError::Provider { reason: "the sign-in ended before it showed a link".into() })
                    }
                },
            },
            () = tokio::time::sleep(Duration::from_secs(settings.wait_s)) => {
                end(&self.under, id, &self.on_change);
                Err(HarnessError::Provider { reason: format!("the sign-in showed no link within {} seconds", settings.wait_s) })
            },
        }
    }

    /// The browser's callback, handed to the agent's login waiting for it
    /// (only to its port); then how the login ended. A paste that isn't its
    /// callback leaves it waiting.
    async fn finish_agent(&self, id: u64, port: Option<u16>, ended: Ended, response: &str) -> Result<()> {
        let port = port.ok_or_else(|| HarnessError::Conflict { reason: "the sign-in hasn't shown its link yet".into() })?;
        let delivered = hand_back_callback(response, port).await?;
        if !delivered {
            return Err(HarnessError::Conflict {
                reason: match &*ended.borrow() {
                    Some(Err(why)) => format!("the sign-in ended ({why}): sign in again"),
                    _ => "the sign-in isn't waiting any more: sign in again".into(),
                },
            });
        }
        match how_it_ended(ended, FINISH_WAIT).await {
            Some(Ok(())) => Ok(()),
            Some(Err(why)) => Err(HarnessError::Provider { reason: format!("the sign-in failed: {why}") }),
            None => {
                end(&self.under, id, &self.on_change);
                Err(HarnessError::Provider { reason: format!("the sign-in didn't finish within {} seconds of the address", FINISH_WAIT.as_secs()) })
            }
        }
    }
}

/// How a login the agent runs itself ended, waiting up to `limit`; `None`:
/// not within it, or it was stopped.
async fn how_it_ended(mut ended: Ended, limit: Duration) -> Option<std::result::Result<(), String>> {
    match tokio::time::timeout(limit, ended.wait_for(Option::is_some)).await {
        Ok(Ok(value)) => value.clone(),
        _ => None,
    }
}

/// What `complete` acts on: the sign-in under way, as it is now.
enum Finishing {
    Agent { id: u64, port: Option<u16>, ended: Ended },
    Command(Box<LoginRun>),
}

#[async_trait]
impl AuthController for AcpAuthController {
    async fn begin(&self, method_id: &str) -> Result<Option<SignInWaiting>> {
        let id = self.ids.fetch_add(1, Ordering::Relaxed) + 1;
        // A sign-in started again replaces the one under way.
        self.stop_under_way();
        let login = lock(&self.logins).iter().find(|l| l.method == method_id).cloned();
        match login {
            Some(login) => self.begin_command(id, login).await,
            None => self.begin_agent(id, method_id).await,
        }
    }

    async fn complete(&self, response: &str) -> Result<()> {
        let finishing = {
            let mut under = lock(&self.under);
            match under.take() {
                // A login command takes one answer.
                Some(Under { running: Running::Command(run), .. }) => Some(Finishing::Command(run)),
                Some(Under { id, running: Running::Agent { task, port, ended } }) => {
                    let finishing = Finishing::Agent { id, port, ended: ended.clone() };
                    *under = Some(Under { id, running: Running::Agent { task, port, ended } });
                    Some(finishing)
                }
                None => None,
            }
        };
        match finishing {
            None => Err(HarnessError::Conflict { reason: "no sign-in is waiting: sign in again".into() }),
            Some(Finishing::Command(run)) => {
                (self.on_change)(AuthEvent::Waiting(None));
                let result = run.finish(response).await;
                (self.on_change)(AuthEvent::Status(if result.is_ok() { None } else { Some(false) }));
                result
            }
            Some(Finishing::Agent { id, port, ended }) => self.finish_agent(id, port, ended, response).await,
        }
    }

    async fn cancel(&self) -> Result<()> {
        self.stop_under_way();
        Ok(())
    }

    async fn logout(&self) -> Result<()> {
        with_connection(&self.launcher, &self.profile, async move |cx, _| cx.send_request(acp::LogoutRequest::new()).block_task().await.map(|_| ())).await?;
        (self.on_change)(AuthEvent::Status(Some(false)));
        Ok(())
    }
}

/// Whether `url` is the loopback address of this machine, as a login's
/// callback is (`127.0.0.1`, `localhost`, `[::1]`).
fn loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// The local port a login's link sends the browser back to (its
/// `redirect_uri`).
pub(crate) fn callback_port(link: &str) -> Option<u16> {
    let link = url::Url::parse(link).ok()?;
    let (_, back) = link.query_pairs().find(|(name, _)| name == "redirect_uri")?;
    let back = url::Url::parse(&back).ok()?;
    (back.scheme() == "http" && loopback(&back)).then(|| back.port_or_known_default()).flatten()
}

/// How long handing the address back may take, and how much of the reply
/// is read.
const HAND_BACK_WAIT: Duration = Duration::from_secs(10);
const REPLY_MOST: u64 = 64 * 1024;

/// Opens the agent's local callback address (`http://127.0.0.1:<port>/…`)
/// for it, as the browser would have; only the port its login waits on.
/// Whether it was delivered (something took it): how the login ended says
/// whether it was right.
async fn hand_back_callback(response: &str, waits_on: u16) -> Result<bool> {
    let wrong = || HarnessError::Refused {
        reason: format!("that isn't the address of the page that failed to load: it starts with http://localhost:{waits_on}/ or http://127.0.0.1:{waits_on}/"),
    };
    let url = url::Url::parse(response.trim()).map_err(|_| wrong())?;
    if url.scheme() != "http" || !loopback(&url) || url.port() != Some(waits_on) {
        return Err(wrong());
    }
    let target = &url[url::Position::BeforePath..url::Position::AfterQuery];
    let delivered = tokio::time::timeout(HAND_BACK_WAIT, async {
        let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", waits_on)).await else { return Ok(false) };
        stream.write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{waits_on}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
        let mut reply = Vec::new();
        stream.take(REPLY_MOST).read_to_end(&mut reply).await?;
        Ok::<_, std::io::Error>(true)
    })
    .await;
    match delivered {
        Ok(Ok(delivered)) => Ok(delivered),
        // It took the address and didn't answer in time, or broke off: taken.
        Ok(Err(_)) | Err(_) => Ok(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_callback_port_is_read_from_the_links_redirect() {
        // Antigravity's, as it printed it (1.1.1, checked live).
        let live = "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id=1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com&redirect_uri=http%3A%2F%2F127.0.0.1%3A53677%2F&scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcloud-platform&state=blSe&access_type=offline";
        assert_eq!(callback_port(live), Some(53677));
        assert_eq!(callback_port("https://x.test/a?post_redirect_uri=http%3a%2f%2f127.0.0.1%3a1%2f&redirect_uri=http%3a%2f%2flocalhost%3a4000%2fcb"), Some(4000));
        assert_eq!(callback_port("https://x.test/a?redirect_uri=http%3A%2F%2F%5B%3A%3A1%5D%3A4001%2F"), Some(4001));
        // Not a callback on this machine: nothing to hand back to.
        assert_eq!(callback_port("https://x.test/a?redirect_uri=https%3A%2F%2Fexample.com%2Fcb"), None);
        assert_eq!(callback_port("https://x.test/a"), None);
    }
}
