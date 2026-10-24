//! Signing in, the agent's way, held on the project's server: one sign-in
//! under way at a time, said in the snapshot (for any device to finish)
//! from its start until it ends, and then how it ended. Every sign-in has
//! an id: what's pasted back or cancelled names it, and only the one under
//! way is touched.
//!
//! A login the agent runs itself goes through ACP `authenticate`: it prints
//! a link (on its error output) and waits on a local callback; the link is
//! shown to open on any device, and `complete` hands the agent that
//! callback's address, which the browser (elsewhere) couldn't reach. A
//! login that's a command of its own, we run (see `login`): its link is
//! shown, and `complete` hands it what the page gave back.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1 as acp;
use async_trait::async_trait;
use harness_contracts::{Launch, SignInEnd, SignInEnded, SignInWaiting};
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
    /// A sign-in under way, as it is now.
    Waiting(SignInWaiting),
    /// The sign-in under way ended, so.
    Ended(SignInEnded),
}

pub type AuthChanged = Arc<dyn Fn(AuthEvent) + Send + Sync>;

/// How a login the agent runs itself ended (why, if it failed), once it has.
type Ended = watch::Receiver<Option<std::result::Result<(), String>>>;

/// Where a login the agent runs itself waits for the browser.
#[derive(Clone, Debug, PartialEq)]
struct Callback {
    host: url::Host<String>,
    port: u16,
}

enum Running {
    /// Starting, its link not shown yet.
    Starting,
    /// The agent's login: its callback once it's shown its link.
    Agent { task: tokio::task::JoinHandle<()>, callback: Option<Callback>, ended: Ended },
    Command(Box<LoginRun>),
}

/// The sign-in under way.
struct Under {
    id: u64,
    running: Running,
}

/// The sign-in under way (one controller's), and where to say what happens to it.
#[derive(Clone)]
struct Slot {
    under: Arc<Mutex<Option<Under>>>,
    on_change: AuthChanged,
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

impl Slot {
    fn current(&self) -> Option<u64> {
        lock(&self.under).as_ref().map(|u| u.id)
    }

    /// Sign-in `id`, if it's still the one under way, now `running`.
    fn put(&self, id: u64, running: Running) -> std::result::Result<(), Running> {
        let mut under = lock(&self.under);
        match under.as_mut() {
            Some(at) if at.id == id => {
                stop(std::mem::replace(&mut at.running, running));
                Ok(())
            }
            _ => Err(running),
        }
    }

    /// Ends sign-in `id`, if it's still the one under way, saying how.
    fn end(&self, id: u64, how: SignInEnd) {
        let taken = {
            let mut under = lock(&self.under);
            if under.as_ref().is_some_and(|u| u.id == id) { under.take() } else { None }
        };
        if let Some(taken) = taken {
            stop(taken.running);
            self.ended(id, how);
        }
    }

    /// Says how sign-in `id` ended (taken already).
    fn ended(&self, id: u64, how: SignInEnd) {
        let signed_in = how == SignInEnd::SignedIn;
        (self.on_change)(AuthEvent::Ended(SignInEnded { id, how }));
        if signed_in {
            (self.on_change)(AuthEvent::Status(Some(true)));
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A sign-in's id: never one before it, a restart or a new instance included.
fn next_id() -> u64 {
    static NEXT: LazyLock<AtomicU64> = LazyLock::new(|| AtomicU64::new(now_ms()));
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// What sign-in `id` being stopped says to whoever was waiting on it.
fn stopped() -> HarnessError {
    HarnessError::Conflict { reason: "that sign-in was stopped (cancelled, or a new one started): sign in again".into() }
}

pub struct AcpAuthController {
    launcher: Arc<dyn AcpLauncher>,
    profile: Arc<Profile>,
    /// How the agent is started (its login commands are run the same way).
    launch: Option<Launch>,
    /// Its login commands, from its last check.
    logins: Mutex<Vec<CommandLogin>>,
    slot: Slot,
}

impl Drop for AcpAuthController {
    fn drop(&mut self) {
        if let Some(under) = lock(&self.slot.under).take() {
            stop(under.running);
        }
    }
}

impl AcpAuthController {
    pub fn new(launcher: Arc<dyn AcpLauncher>, profile: Arc<Profile>, launch: Option<Launch>, on_change: AuthChanged) -> Self {
        Self { launcher, profile, launch, logins: Mutex::default(), slot: Slot { under: Arc::default(), on_change } }
    }

    /// The agent's login commands, as its last check found them.
    pub fn set_logins(&self, logins: Vec<CommandLogin>) {
        *lock(&self.logins) = logins;
    }

    async fn begin_command(&self, id: u64, until_ms: Option<u64>, login: CommandLogin) -> Result<Option<SignInWaiting>> {
        let fail = |reason: String| {
            self.slot.end(id, SignInEnd::Failed { reason: reason.clone() });
            Err(HarnessError::Provider { reason: format!("the sign-in failed: {reason}") })
        };
        let Some(launch) = &self.launch else { return fail("the agent can't be started to sign in".into()) };
        let settings = &self.profile.settings.login;
        let started = match LoginRun::start(launch, &login, &self.profile.project, settings.link_pattern(), Duration::from_secs(settings.wait_s)).await {
            Ok(started) => started,
            Err(err) => return fail(err.to_string()),
        };
        if self.slot.current() != Some(id) {
            return Err(stopped());
        }
        match (started.exited, started.link) {
            (Some(true), _) => {
                self.slot.end(id, SignInEnd::SignedIn);
                Ok(None)
            }
            (Some(false), _) => fail(started.run.last_words()),
            (None, None) => fail(format!("it showed no link: {}", started.run.last_words())),
            (None, Some(link)) => {
                let expects = settings.expects.clone().unwrap_or_else(|| "the code the page shows".into());
                let waiting = SignInWaiting {
                    id,
                    url: Some(link),
                    instructions: format!("Open the link and sign in, then paste {expects} in Finish signing in."),
                    expects: Some(expects),
                    until_ms,
                };
                self.slot.put(id, Running::Command(Box::new(started.run))).map_err(|_| stopped())?;
                (self.slot.on_change)(AuthEvent::Waiting(waiting.clone()));
                Ok(Some(waiting))
            }
        }
    }

    /// A login the agent runs itself: held as sign-in `id` from its start
    /// (a request that goes away doesn't lose it), and its link, once it
    /// prints it, shown. It may finish at once (a key in its environment).
    async fn begin_agent(&self, id: u64, until_ms: Option<u64>, method: &str) -> Result<Option<SignInWaiting>> {
        let settings = &self.profile.settings.login;
        let (launcher, profile, slot) = (self.launcher.clone(), self.profile.clone(), self.slot.clone());
        let method = acp::AuthMethodId::new(method.to_owned());
        let pattern = settings.link_pattern();
        let (link_to, link) = tokio::sync::oneshot::channel::<std::result::Result<SignInWaiting, String>>();
        let (ended_to, ended) = watch::channel(None);
        let task = tokio::spawn(async move {
            let shown = slot.clone();
            let result = with_connection(&launcher, &profile, async move |cx, opened| {
                // Its link, from what it says while it waits for the browser:
                // shown once it names the callback to hand back to.
                tokio::spawn(async move {
                    let Some(link) = crate::launch::says(opened.said, pattern).await else { return };
                    let Some(callback) = callback_of(&link) else {
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
                    let current = {
                        let mut under = lock(&shown.under);
                        match under.as_mut() {
                            Some(Under { id: at, running: Running::Agent { callback: c, .. } }) if *at == id => {
                                *c = Some(callback);
                                true
                            }
                            _ => false,
                        }
                    };
                    if current {
                        (shown.on_change)(AuthEvent::Waiting(waiting.clone()));
                    }
                    let _ = link_to.send(Ok(waiting));
                });
                cx.send_request(acp::AuthenticateRequest::new(method)).block_task().await
            })
            .await;
            let how = match &result {
                Ok(_) => SignInEnd::SignedIn,
                Err(HarnessError::Provider { reason }) => SignInEnd::Failed { reason: reason.clone() },
                Err(err) => SignInEnd::Failed { reason: err.to_string() },
            };
            if let SignInEnd::Failed { reason } = &how {
                tracing::warn!(target: "harness_acp", "sign-in failed: {reason}");
            }
            ended_to.send_replace(Some(match &how {
                SignInEnd::Failed { reason } => Err(reason.clone()),
                _ => Ok(()),
            }));
            slot.end(id, how);
        });
        if let Err(running) = self.slot.put(id, Running::Agent { task, callback: None, ended: ended.clone() }) {
            stop(running);
            return Err(stopped());
        }
        tokio::select! {
            found = link => match found {
                Ok(Ok(waiting)) => Ok(Some(waiting)),
                Ok(Err(why)) => {
                    self.slot.end(id, SignInEnd::Failed { reason: why.clone() });
                    Err(HarnessError::Provider { reason: format!("the sign-in failed: {why}") })
                }
                // It stopped saying anything before its link: as it ended.
                Err(_) => match how_it_ended(ended, Duration::from_secs(5)).await {
                    Some(Ok(())) => Ok(None),
                    Some(Err(why)) => Err(HarnessError::Provider { reason: format!("the sign-in failed: {why}") }),
                    None if self.slot.current() == Some(id) => {
                        let why = "it ended before it showed a link".to_owned();
                        self.slot.end(id, SignInEnd::Failed { reason: why.clone() });
                        Err(HarnessError::Provider { reason: format!("the sign-in failed: {why}") })
                    }
                    None => Err(stopped()),
                },
            },
            () = tokio::time::sleep(Duration::from_secs(settings.wait_s)) => {
                let why = format!("it showed no link within {} seconds", settings.wait_s);
                self.slot.end(id, SignInEnd::Failed { reason: why.clone() });
                Err(HarnessError::Provider { reason: format!("the sign-in failed: {why}") })
            },
        }
    }

    /// The browser's callback, handed to the agent's login waiting for it
    /// (only to its own address); then how the login ended. A paste that
    /// isn't its callback leaves it waiting.
    async fn finish_agent(&self, id: u64, callback: Option<Callback>, ended: Ended, response: &str) -> Result<()> {
        let callback = callback.ok_or_else(|| HarnessError::Conflict { reason: "the sign-in hasn't shown its link yet".into() })?;
        if let Err(why) = hand_back_callback(response, &callback).await? {
            return Err(HarnessError::Conflict {
                reason: match &*ended.borrow() {
                    Some(Err(failed)) => format!("the sign-in ended ({failed}): sign in again"),
                    _ => format!("the sign-in isn't waiting at its address any more ({why}): sign in again"),
                },
            });
        }
        match how_it_ended(ended, FINISH_WAIT).await {
            Some(Ok(())) => Ok(()),
            Some(Err(why)) => Err(HarnessError::Provider { reason: format!("the sign-in failed: {why}") }),
            None if self.slot.current() == Some(id) => {
                let why = format!("it didn't finish within {} seconds of the address", FINISH_WAIT.as_secs());
                self.slot.end(id, SignInEnd::Failed { reason: why.clone() });
                Err(HarnessError::Provider { reason: format!("the sign-in failed: {why}") })
            }
            None => Err(stopped()),
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
    Agent { callback: Option<Callback>, ended: Ended },
    Command(Box<LoginRun>),
}

#[async_trait]
impl AuthController for AcpAuthController {
    async fn begin(&self, method_id: &str) -> Result<Option<SignInWaiting>> {
        let id = next_id();
        let within_s = self.profile.settings.login.within_s;
        let until_ms = within_s.map(|s| now_ms() + s * 1000);
        // A sign-in started again replaces the one under way.
        let replaced = lock(&self.slot.under).replace(Under { id, running: Running::Starting });
        if let Some(replaced) = replaced {
            stop(replaced.running);
            self.slot.ended(replaced.id, SignInEnd::Stopped);
        }
        let starting = SignInWaiting { id, url: None, instructions: "Starting the sign-in…".into(), expects: None, until_ms };
        (self.slot.on_change)(AuthEvent::Waiting(starting));
        // Its time, counted from now, the same for every kind: then it ends.
        if let Some(within_s) = within_s {
            let slot = self.slot.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(within_s)).await;
                slot.end(id, SignInEnd::RanOut);
            });
        }
        let login = lock(&self.logins).iter().find(|l| l.method == method_id).cloned();
        match login {
            Some(login) => self.begin_command(id, until_ms, login).await,
            None => self.begin_agent(id, until_ms, method_id).await,
        }
    }

    async fn complete(&self, id: u64, response: &str) -> Result<()> {
        let finishing = {
            let mut under = lock(&self.slot.under);
            match under.take() {
                Some(Under { id: at, running }) if at != id => {
                    *under = Some(Under { id: at, running });
                    None
                }
                // A login command takes one answer.
                Some(Under { running: Running::Command(run), .. }) => Some(Ok(Finishing::Command(run))),
                Some(Under { running: Running::Starting, .. }) => {
                    *under = Some(Under { id, running: Running::Starting });
                    Some(Err(HarnessError::Conflict { reason: "the sign-in hasn't shown its link yet".into() }))
                }
                Some(Under { running: Running::Agent { task, callback, ended }, .. }) => {
                    let finishing = Finishing::Agent { callback: callback.clone(), ended: ended.clone() };
                    *under = Some(Under { id, running: Running::Agent { task, callback, ended } });
                    Some(Ok(finishing))
                }
                None => None,
            }
        };
        match finishing {
            None => Err(HarnessError::Conflict { reason: "that sign-in isn't under way any more (it ended, was cancelled, or a new one started): sign in again".into() }),
            Some(Err(err)) => Err(err),
            Some(Ok(Finishing::Command(run))) => {
                let result = run.finish(response).await;
                self.slot.ended(id, match &result {
                    Ok(()) => SignInEnd::SignedIn,
                    Err(HarnessError::Provider { reason }) => SignInEnd::Failed { reason: reason.clone() },
                    Err(err) => SignInEnd::Failed { reason: err.to_string() },
                });
                result
            }
            Some(Ok(Finishing::Agent { callback, ended })) => self.finish_agent(id, callback, ended, response).await,
        }
    }

    async fn cancel(&self, id: u64) -> Result<()> {
        self.slot.end(id, SignInEnd::Stopped);
        Ok(())
    }

    async fn logout(&self) -> Result<()> {
        with_connection(&self.launcher, &self.profile, async move |cx, _| cx.send_request(acp::LogoutRequest::new()).block_task().await.map(|_| ())).await?;
        (self.slot.on_change)(AuthEvent::Status(Some(false)));
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

/// Where a login's link sends the browser back to (its `redirect_uri`), on
/// this machine.
fn callback_of(link: &str) -> Option<Callback> {
    let link = url::Url::parse(link).ok()?;
    let (_, back) = link.query_pairs().find(|(name, _)| name == "redirect_uri")?;
    let back = url::Url::parse(&back).ok()?;
    if back.scheme() != "http" || !loopback(&back) {
        return None;
    }
    Some(Callback { host: back.host()?.to_owned(), port: back.port_or_known_default()? })
}

/// How long handing the address back may take, and how much of the reply
/// is read.
const HAND_BACK_WAIT: Duration = Duration::from_secs(10);
const REPLY_MOST: u64 = 64 * 1024;

/// Opens the agent's local callback address for it, as the browser would
/// have: only at the address its link named (a `localhost` one at either
/// loopback). `Err` inside: nothing took it, and why.
async fn hand_back_callback(response: &str, callback: &Callback) -> Result<std::result::Result<(), String>> {
    let port = callback.port;
    let wrong = || HarnessError::Refused {
        reason: format!("that isn't the address of the page that failed to load: it starts with http://{}:{port}/", callback.host),
    };
    let url = url::Url::parse(response.trim()).map_err(|_| wrong())?;
    if url.scheme() != "http" || !loopback(&url) || url.port() != Some(port) {
        return Err(wrong());
    }
    let target = &url[url::Position::BeforePath..url::Position::AfterQuery];
    let addresses: Vec<std::net::SocketAddr> = match &callback.host {
        url::Host::Ipv4(ip) => vec![(*ip, port).into()],
        url::Host::Ipv6(ip) => vec![(*ip, port).into()],
        url::Host::Domain(_) => vec![(std::net::Ipv4Addr::LOCALHOST, port).into(), (std::net::Ipv6Addr::LOCALHOST, port).into()],
    };
    let connected = tokio::time::timeout(HAND_BACK_WAIT, tokio::net::TcpStream::connect(&addresses[..])).await;
    let mut stream = match connected {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => return Ok(Err(e.to_string())),
        Err(_) => return Ok(Err(format!("no answer within {} seconds", HAND_BACK_WAIT.as_secs()))),
    };
    // Taken: what it answers (or doesn't) is said by how the login ends.
    let _ = tokio::time::timeout(HAND_BACK_WAIT, async {
        stream.write_all(format!("GET {target} HTTP/1.1\r\nHost: {}:{port}\r\nConnection: close\r\n\r\n", callback.host).as_bytes()).await?;
        let mut reply = Vec::new();
        stream.take(REPLY_MOST).read_to_end(&mut reply).await
    })
    .await;
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(link: &str) -> Option<u16> {
        callback_of(link).map(|c| c.port)
    }

    #[test]
    fn the_callback_is_read_from_the_links_redirect() {
        // Antigravity's, as it printed it (1.1.1, checked live).
        let live = "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id=1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com&redirect_uri=http%3A%2F%2F127.0.0.1%3A53677%2F&scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcloud-platform&state=blSe&access_type=offline";
        assert_eq!(callback_of(live), Some(Callback { host: url::Host::Ipv4(std::net::Ipv4Addr::LOCALHOST), port: 53677 }));
        assert_eq!(port("https://x.test/a?post_redirect_uri=http%3a%2f%2f127.0.0.1%3a1%2f&redirect_uri=http%3a%2f%2flocalhost%3a4000%2fcb"), Some(4000));
        assert_eq!(callback_of("https://x.test/a?redirect_uri=http%3A%2F%2F%5B%3A%3A1%5D%3A4001%2F"), Some(Callback { host: url::Host::Ipv6(std::net::Ipv6Addr::LOCALHOST), port: 4001 }));
        // Not a callback on this machine: nothing to hand back to.
        assert_eq!(port("https://x.test/a?redirect_uri=https%3A%2F%2Fexample.com%2Fcb"), None);
        assert_eq!(port("https://x.test/a"), None);
    }

    #[tokio::test]
    async fn the_address_goes_back_only_where_the_link_said() {
        // A listener on [::1] only: the address goes there, not to 127.0.0.1.
        let Ok(v6) = tokio::net::TcpListener::bind("[::1]:0").await else { return };
        let port = v6.local_addr().unwrap().port();
        let taken = tokio::spawn(async move {
            let (mut stream, _) = v6.accept().await.unwrap();
            let mut asked = [0u8; 256];
            let n = stream.read(&mut asked).await.unwrap();
            String::from_utf8_lossy(&asked[..n]).into_owned()
        });
        let callback = Callback { host: url::Host::Ipv6(std::net::Ipv6Addr::LOCALHOST), port };
        hand_back_callback(&format!("http://[::1]:{port}/?code=x"), &callback).await.unwrap().unwrap();
        assert!(taken.await.unwrap().starts_with("GET /?code=x HTTP/1.1"));
        // Nothing there: said, not taken.
        let gone = Callback { host: url::Host::Ipv4(std::net::Ipv4Addr::LOCALHOST), port: 1 };
        assert!(hand_back_callback("http://127.0.0.1:1/?code=x", &gone).await.unwrap().is_err());
    }
}
