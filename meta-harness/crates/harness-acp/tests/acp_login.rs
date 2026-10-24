//! Signing in from any device, the sign-in held on the project's server:
//! Antigravity's login (its pack's own settings) against a mock that does
//! as it does: a link said on its error output, the browser's callback
//! awaited on a local port.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness_acp::testing::{MockAgent, MockSetup};
use harness_acp::{AcpAdapter, AcpAuthController, AcpInstance, AcpLauncher, AcpSettings, AuthEvent, InstanceParts, Profile, ProcessLauncher};
use harness_contracts::*;
use harness_core::{AuthController, HarnessError, ProviderAdapter, ProviderInstance};

const PROJECT: &str = "/work/project";

/// The shipped Antigravity pack's settings.
fn antigravity() -> AcpSettings {
    let pack: toml::Table = toml::from_str(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/harness/antigravity-acp/pack.toml")).unwrap()).unwrap();
    AcpSettings::parse(&serde_json::to_value(&pack["provides"]["settings"]).unwrap()).unwrap()
}

fn profile(settings: AcpSettings) -> Arc<Profile> {
    Arc::new(Profile::new(settings, Path::new(PROJECT), None))
}

fn agent() -> MockAgent {
    MockAgent::new(MockSetup { loopback_login: true, ..MockSetup::default() })
}

/// What the sign-in said, in order.
type Said = Arc<Mutex<Vec<AuthEvent>>>;

fn controller(agent: &MockAgent, settings: AcpSettings) -> (AcpAuthController, Said) {
    let said = Said::default();
    let to = said.clone();
    (AcpAuthController::new(Arc::new(agent.clone()), profile(settings), None, Arc::new(move |e| to.lock().unwrap().push(e))), said)
}

fn last(said: &Said) -> Option<AuthEvent> {
    said.lock().unwrap().last().cloned()
}

fn port_of(waiting: &SignInWaiting) -> u16 {
    let link = url::Url::parse(waiting.url.as_deref().unwrap()).unwrap();
    let back = link.query_pairs().find(|(k, _)| k == "redirect_uri").unwrap().1.into_owned();
    url::Url::parse(&back).unwrap().port().unwrap()
}

fn config() -> SessionConfig {
    SessionConfig { thread_id: ThreadId::new("t"), policy: RuntimePolicy::default(), model: None, resume: None, mcp_servers: Vec::new() }
}

#[tokio::test]
async fn the_sign_in_shows_its_link_takes_the_callback_and_then_sessions_open() {
    let agent = agent();
    let (auth, said) = controller(&agent, antigravity());
    let waiting = auth.begin("oauth-personal").await.unwrap().expect("a link to open");
    let id = waiting.id;
    assert!(waiting.url.as_deref().is_some_and(|u| u.starts_with("https://accounts.google.com/")), "{waiting:?}");
    let left = waiting.until_ms.unwrap().saturating_sub(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64);
    assert!((295_000..=300_000).contains(&left), "it waits 300 s: {left}");
    let events = said.lock().unwrap().clone();
    assert!(matches!(&events[0], AuthEvent::Waiting(w) if w.id == id && w.url.is_none()), "starting, from the start: {events:?}");
    assert_eq!(last(&said), Some(AuthEvent::Waiting(waiting.clone())), "every device sees it");
    let port = port_of(&waiting);

    // Not its callback: refused, and it still waits.
    for wrong in ["4/abc", "https://accounts.google.com/x", &format!("http://127.0.0.1:{}/?code=x", port.wrapping_add(1))] {
        assert!(matches!(auth.complete(id, wrong).await, Err(HarnessError::Refused { .. })), "{wrong}");
    }
    assert!(!agent.login_done.load(std::sync::atomic::Ordering::SeqCst));

    // What the browser on another machine couldn't reach.
    // Named for another sign-in: not this one's.
    assert!(matches!(auth.complete(id + 1000, &format!("http://127.0.0.1:{port}/?code=4/abc")).await, Err(HarnessError::Conflict { .. })));
    auth.complete(id, &format!("http://localhost:{port}/?code=4/abc&scope=email&state=s")).await.unwrap();
    assert!(agent.login_done.load(std::sync::atomic::Ordering::SeqCst));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let events = said.lock().unwrap().clone();
    assert!(events.contains(&AuthEvent::Ended(SignInEnded { id, how: SignInEnd::SignedIn })) && events.contains(&AuthEvent::Status(Some(true))), "{events:?}");
    assert!(matches!(auth.complete(id, &format!("http://127.0.0.1:{port}/?code=4/abc")).await, Err(HarnessError::Conflict { .. })), "taken: nothing waits");

    // Signed in: a session opens (its link isn't said any more).
    let adapter = AcpAdapter::new(InstanceId::new("mock"), profile(antigravity()), Arc::new(agent.clone()), Arc::new(|_| {}));
    adapter.open_session(config()).await.expect("a session");
}

#[tokio::test]
async fn a_callback_the_agent_turns_down_ends_the_sign_in_saying_why() {
    let agent = agent();
    let (auth, said) = controller(&agent, antigravity());
    let waiting = auth.begin("oauth-personal").await.unwrap().unwrap();
    let port = port_of(&waiting);
    // Cancelled on Google's page: the address says so, the agent's login fails.
    let err = auth.complete(waiting.id, &format!("http://127.0.0.1:{port}/?error=access_denied")).await.unwrap_err();
    assert!(matches!(&err, HarnessError::Provider { reason } if reason.starts_with("the sign-in failed")), "{err}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let events = said.lock().unwrap().clone();
    assert!(matches!(events.last(), Some(AuthEvent::Ended(SignInEnded { how: SignInEnd::Failed { .. }, .. }))), "over, saying why: {events:?}");
    assert!(!events.iter().any(|e| matches!(e, AuthEvent::Status(_))), "a failed sign-in says nothing of the login there is: {events:?}");
}

#[tokio::test]
async fn a_new_sign_in_replaces_the_one_under_way_and_cancel_stops_it() {
    let agent = agent();
    let (auth, said) = controller(&agent, antigravity());
    let first = auth.begin("oauth-personal").await.unwrap().unwrap();
    let second = auth.begin("oauth-personal").await.unwrap().unwrap();
    assert_ne!(first.id, second.id);
    assert!(said.lock().unwrap().contains(&AuthEvent::Ended(SignInEnded { id: first.id, how: SignInEnd::Stopped })));
    // A paste or a Cancel for the first doesn't touch the second.
    assert!(matches!(auth.complete(first.id, &format!("http://127.0.0.1:{}/?code=x", port_of(&first))).await, Err(HarnessError::Conflict { .. })));
    auth.cancel(first.id).await.unwrap();
    assert_eq!(last(&said), Some(AuthEvent::Waiting(second.clone())));

    auth.cancel(second.id).await.unwrap();
    assert_eq!(last(&said), Some(AuthEvent::Ended(SignInEnded { id: second.id, how: SignInEnd::Stopped })));
    assert!(matches!(auth.complete(second.id, &format!("http://127.0.0.1:{}/?code=x", port_of(&second))).await, Err(HarnessError::Conflict { .. })));
}

#[tokio::test]
async fn the_server_stops_a_sign_in_when_its_time_is_up() {
    let agent = agent();
    let mut settings = antigravity();
    settings.login.within_s = Some(1);
    let (auth, said) = controller(&agent, settings);
    let waiting = auth.begin("oauth-personal").await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let events = said.lock().unwrap().clone();
    assert_eq!(events.last(), Some(&AuthEvent::Ended(SignInEnded { id: waiting.id, how: SignInEnd::RanOut })), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, AuthEvent::Status(_))), "running out says nothing of the login there is");
    assert!(matches!(auth.complete(waiting.id, &format!("http://127.0.0.1:{}/?code=x", port_of(&waiting))).await, Err(HarnessError::Conflict { .. })));
}

#[tokio::test]
async fn a_session_opened_signed_out_is_refused_at_once() {
    let said = Said::default();
    let to = said.clone();
    let adapter = AcpAdapter::new(InstanceId::new("mock"), profile(antigravity()), Arc::new(agent()), Arc::new(move |e| to.lock().unwrap().push(e)));
    let refused = tokio::time::timeout(Duration::from_secs(5), adapter.open_session(config())).await.expect("refused at once, not left waiting");
    assert!(matches!(refused.err(), Some(HarnessError::AuthRequired { .. })));
    assert_eq!(*said.lock().unwrap(), vec![AuthEvent::Status(Some(false))]);
}

#[tokio::test]
async fn its_login_file_says_signed_in_only_while_nothing_says_more() {
    let home = tempfile::tempdir().unwrap();
    let mut settings = antigravity();
    settings.login.kept = Some(home.path().join("acp_token.json").display().to_string());
    let agent = agent();
    let harness = HarnessProfile { instance: InstanceId::new("antigravity"), driver: DriverKind::new("acp"), display_name: "Antigravity".into(), settings: serde_json::Value::Null };
    let instance = AcpInstance::create(InstanceParts { id: InstanceId::new("antigravity"), harness, profile: profile(settings), launcher: Ok((Arc::new(agent.clone()) as Arc<dyn AcpLauncher>, None)) });
    let status = |i: &Arc<AcpInstance>| i.snapshot().borrow().auth.status;
    instance.refresh().await.unwrap();
    assert_eq!(status(&instance), AuthStatus::Unknown, "no file: not known");
    std::fs::write(home.path().join("acp_token.json"), "{}").unwrap();
    instance.refresh().await.unwrap();
    assert_eq!(status(&instance), AuthStatus::SignedIn);
    // Its token no good any more: a session is refused, and the file
    // (still there) doesn't say otherwise.
    assert!(instance.adapter().open_session(config()).await.is_err());
    instance.refresh().await.unwrap();
    assert_eq!(status(&instance), AuthStatus::SignedOut);
}

#[tokio::test]
async fn what_a_real_agent_prints_on_its_error_output_reaches_the_driver() {
    let launch = Launch { program: "sh".into(), args: vec!["-c".into(), "echo 'Open the following link to authenticate the ACP server: https://accounts.google.com/x?y=1' >&2; sleep 2".into()], env: Vec::new(), env_remove: Vec::new() };
    let mut transport = ProcessLauncher::from(&launch).launch(&config()).await.unwrap();
    let line = tokio::time::timeout(Duration::from_secs(5), transport.said.recv()).await.unwrap().unwrap();
    assert!(line.ends_with("https://accounts.google.com/x?y=1"), "{line}");
}

#[tokio::test]
async fn a_sign_in_goes_on_when_the_request_that_began_it_goes_away() {
    let agent = agent();
    let (auth, said) = controller(&agent, antigravity());
    // A reload, a dropped connection: the request is gone before the link.
    let _ = tokio::time::timeout(Duration::from_millis(1), auth.begin("oauth-personal")).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(matches!(last(&said), Some(AuthEvent::Waiting(w)) if w.url.is_some()), "its link is shown all the same: {:?}", last(&said));
}

#[tokio::test]
async fn a_login_command_taking_a_paste_is_stopped_by_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let done = dir.path().join("signed-in");
    // Prints its link, takes the code, and is slow to finish.
    let launch = Launch { program: "sh".into(), args: vec!["-c".into(), format!("echo 'visit https://claude.ai/oauth?x=1'; read code; sleep 3; touch {}", done.display())], env: Vec::new(), env_remove: Vec::new() };
    let said = Said::default();
    let to = said.clone();
    let auth = Arc::new(AcpAuthController::new(Arc::new(agent()), Arc::new(Profile::new(AcpSettings::default(), dir.path(), None)), Some(launch), Arc::new(move |e| to.lock().unwrap().push(e))));
    auth.set_logins(vec![harness_acp::CommandLogin { method: "login".into(), args: Vec::new(), env: Vec::new() }]);
    let waiting = auth.begin("login").await.unwrap().unwrap();
    let pasting = { let auth = auth.clone(); tokio::spawn(async move { auth.complete(waiting.id, "the-code").await }) };
    tokio::time::sleep(Duration::from_millis(300)).await;
    auth.cancel(waiting.id).await.unwrap();
    let answered = tokio::time::timeout(Duration::from_secs(2), pasting).await.expect("answered at once").unwrap();
    assert!(matches!(answered, Err(HarnessError::Conflict { .. })), "{answered:?}");
    assert_eq!(last(&said), Some(AuthEvent::Ended(SignInEnded { id: waiting.id, how: SignInEnd::Stopped })));
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(!done.exists(), "the login was stopped, not left to finish");
}

#[tokio::test]
async fn a_login_command_starting_is_stopped_by_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let ran_on = dir.path().join("ran-on");
    // Slow to show its link.
    let launch = Launch { program: "sh".into(), args: vec!["-c".into(), format!("sleep 2; touch {}; echo 'visit https://claude.ai/oauth?x=1'; read code", ran_on.display())], env: Vec::new(), env_remove: Vec::new() };
    let said = Said::default();
    let to = said.clone();
    let auth = Arc::new(AcpAuthController::new(Arc::new(agent()), Arc::new(Profile::new(AcpSettings::default(), dir.path(), None)), Some(launch), Arc::new(move |e| to.lock().unwrap().push(e))));
    auth.set_logins(vec![harness_acp::CommandLogin { method: "login".into(), args: Vec::new(), env: Vec::new() }]);
    let beginning = { let auth = auth.clone(); tokio::spawn(async move { auth.begin("login").await }) };
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The sign-in every device sees starting.
    let Some(AuthEvent::Waiting(starting)) = last(&said) else { panic!("{:?}", last(&said)) };
    assert!(starting.url.is_none());
    auth.cancel(starting.id).await.unwrap();
    let answered = tokio::time::timeout(Duration::from_secs(1), beginning).await.expect("answered at once").unwrap();
    assert!(matches!(answered, Err(HarnessError::Conflict { .. })), "{answered:?}");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!ran_on.exists(), "the login command was stopped while it started");
}
