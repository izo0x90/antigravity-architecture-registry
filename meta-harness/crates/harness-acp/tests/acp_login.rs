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
    assert!(waiting.url.as_deref().is_some_and(|u| u.starts_with("https://accounts.google.com/")), "{waiting:?}");
    let left = waiting.until_ms.unwrap().saturating_sub(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64);
    assert!((295_000..=300_000).contains(&left), "it waits 300 s: {left}");
    assert_eq!(last(&said), Some(AuthEvent::Waiting(Some(waiting.clone()))), "every device sees it");
    let port = port_of(&waiting);

    // Not its callback: refused, and it still waits.
    for wrong in ["4/abc", "https://accounts.google.com/x", &format!("http://127.0.0.1:{}/?code=x", port.wrapping_add(1))] {
        assert!(matches!(auth.complete(wrong).await, Err(HarnessError::Refused { .. })), "{wrong}");
    }
    assert!(!agent.login_done.load(std::sync::atomic::Ordering::SeqCst));

    // What the browser on another machine couldn't reach.
    auth.complete(&format!("http://localhost:{port}/?code=4/abc&scope=email&state=s")).await.unwrap();
    assert!(agent.login_done.load(std::sync::atomic::Ordering::SeqCst));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let events = said.lock().unwrap().clone();
    assert!(events.contains(&AuthEvent::Status(Some(true))) && events.last() == Some(&AuthEvent::Waiting(None)), "{events:?}");
    assert!(matches!(auth.complete(&format!("http://127.0.0.1:{port}/?code=4/abc")).await, Err(HarnessError::Conflict { .. })), "taken: nothing waits");

    // Signed in: a session opens (its link isn't said any more).
    let adapter = AcpAdapter::new(InstanceId::new("mock"), profile(antigravity()), Arc::new(agent.clone()), Arc::new(|_| {}));
    adapter.open_session(config()).await.expect("a session");
}

#[tokio::test]
async fn a_callback_the_agent_turns_down_ends_the_sign_in_saying_why() {
    let agent = agent();
    let (auth, said) = controller(&agent, antigravity());
    let port = port_of(&auth.begin("oauth-personal").await.unwrap().unwrap());
    // Cancelled on Google's page: the address says so, the agent's login fails.
    let err = auth.complete(&format!("http://127.0.0.1:{port}/?error=access_denied")).await.unwrap_err();
    assert!(matches!(&err, HarnessError::Provider { reason } if reason.starts_with("the sign-in failed")), "{err}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let events = said.lock().unwrap().clone();
    assert!(events.contains(&AuthEvent::Status(Some(false))) && events.last() == Some(&AuthEvent::Waiting(None)), "over: {events:?}");
}

#[tokio::test]
async fn a_new_sign_in_replaces_the_one_under_way_and_cancel_stops_it() {
    let agent = agent();
    let (auth, said) = controller(&agent, antigravity());
    let first = auth.begin("oauth-personal").await.unwrap().unwrap();
    let second = auth.begin("oauth-personal").await.unwrap().unwrap();
    assert_ne!(first.id, second.id);
    // The first's address goes nowhere now: only the second's port is taken.
    assert!(matches!(auth.complete(&format!("http://127.0.0.1:{}/?code=x", port_of(&first))).await, Err(HarnessError::Refused { .. })));
    assert_eq!(last(&said), Some(AuthEvent::Waiting(Some(second.clone()))));

    auth.cancel().await.unwrap();
    assert_eq!(last(&said), Some(AuthEvent::Waiting(None)));
    assert!(matches!(auth.complete(&format!("http://127.0.0.1:{}/?code=x", port_of(&second))).await, Err(HarnessError::Conflict { .. })));
}

#[tokio::test]
async fn the_server_stops_a_sign_in_when_its_time_is_up() {
    let agent = agent();
    let mut settings = antigravity();
    settings.login.within_s = Some(1);
    let (auth, said) = controller(&agent, settings);
    let port = port_of(&auth.begin("oauth-personal").await.unwrap().unwrap());
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let events = said.lock().unwrap().clone();
    assert!(events.contains(&AuthEvent::Status(Some(false))) && events.last() == Some(&AuthEvent::Waiting(None)), "{events:?}");
    assert!(matches!(auth.complete(&format!("http://127.0.0.1:{port}/?code=x")).await, Err(HarnessError::Conflict { .. })));
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
    let transport = ProcessLauncher::from(&launch).launch(&config()).await.unwrap();
    let pattern = regex::Regex::new(r"https://accounts\.google\.com/\S+").unwrap();
    let link = tokio::time::timeout(Duration::from_secs(5), harness_acp::says(transport.said, pattern)).await.unwrap();
    assert_eq!(link.as_deref(), Some("https://accounts.google.com/x?y=1"));
}
