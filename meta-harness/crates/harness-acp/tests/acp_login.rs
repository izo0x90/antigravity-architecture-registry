//! Logins the agent runs itself, as Antigravity's does: a link said on its
//! error output, the browser's callback to a local port handed back.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness_acp::testing::{MockAgent, MockSetup};
use harness_acp::{AcpAdapter, AcpAuthController, AcpLauncher, AcpSettings, AuthChanged, Profile, probe};
use harness_contracts::*;
use harness_core::{AuthController, HarnessError, ProviderAdapter};

const PROJECT: &str = "/work/project";

fn profile(toml_text: &str) -> Arc<Profile> {
    let table: toml::Table = toml::from_str(toml_text).unwrap();
    Arc::new(Profile::new(AcpSettings::parse(&serde_json::to_value(table).unwrap()).unwrap(), Path::new(PROJECT), None))
}

const GOOGLE: &str = r#"
[login]
link = 'https://accounts\.google\.com/\S+'
within_s = 300
on_session = true
"#;

fn loopback_agent() -> MockAgent {
    MockAgent::new(MockSetup { loopback_login: true, ..MockSetup::default() })
}

/// What the login said of the status, in order.
fn changes() -> (AuthChanged, Arc<Mutex<Vec<Option<bool>>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let to = seen.clone();
    (Arc::new(move |s| to.lock().unwrap().push(s)), seen)
}

fn controller(agent: &MockAgent, on_change: AuthChanged) -> AcpAuthController {
    AcpAuthController::new(Arc::new(agent.clone()), profile(GOOGLE), None, on_change)
}

fn port_of(link: &str) -> u16 {
    let at = link.find("127.0.0.1%3A").unwrap() + "127.0.0.1%3A".len();
    link[at..].split('%').next().unwrap().parse().unwrap()
}

#[tokio::test]
async fn the_login_shows_its_link_and_the_callback_handed_back_signs_in() {
    let agent = loopback_agent();
    let (on_change, seen) = changes();
    let auth = controller(&agent, on_change);
    let challenge = auth.begin("oauth-personal").await.unwrap();
    let link = challenge.url.clone().unwrap();
    assert!(link.starts_with("https://accounts.google.com/o/oauth2/auth?"), "{link}");
    assert_eq!(challenge.within_s, Some(300));
    assert_eq!(challenge.expects.as_deref(), Some("the address of the page that failed to load"));
    // What the browser, on another machine, couldn't reach.
    let port = port_of(&link);
    auth.complete(&format!("http://localhost:{port}/?code=4/abc&scope=email&state=s")).await.unwrap();
    assert!(agent.signed_in.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(*seen.lock().unwrap(), vec![Some(true)]);
}

#[tokio::test]
async fn a_wrong_paste_is_refused_and_the_login_keeps_waiting() {
    let agent = loopback_agent();
    let (on_change, _) = changes();
    let auth = controller(&agent, on_change);
    let link = auth.begin("oauth-personal").await.unwrap().url.unwrap();
    let port = port_of(&link);
    for wrong in ["4/abc", "https://accounts.google.com/x", &format!("http://127.0.0.1:{}/?code=x", port.wrapping_add(1))] {
        let err = auth.complete(wrong).await.unwrap_err();
        assert!(matches!(&err, HarnessError::Refused { reason } if reason.contains("isn't the address")), "{wrong}: {err}");
    }
    assert!(!agent.signed_in.load(std::sync::atomic::Ordering::SeqCst));
    // Still waiting: the right one goes through.
    auth.complete(&format!("http://127.0.0.1:{port}/?code=4/abc")).await.unwrap();
    assert!(agent.signed_in.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn a_paste_with_no_login_waiting_says_so() {
    let agent = loopback_agent();
    let (on_change, _) = changes();
    let auth = controller(&agent, on_change);
    let err = auth.complete("http://127.0.0.1:4000/?code=x").await.unwrap_err();
    assert!(err.to_string().contains("no sign-in is waiting"), "{err}");
}

#[tokio::test]
async fn a_session_opened_signed_out_is_refused_at_once() {
    let agent = loopback_agent();
    let (on_change, seen) = changes();
    let adapter = AcpAdapter::new(InstanceId::new("mock"), profile(GOOGLE), Arc::new(agent.clone()), on_change);
    let config = SessionConfig { thread_id: ThreadId::new("t"), policy: RuntimePolicy::default(), model: None, resume: None, mcp_servers: Vec::new() };
    let refused = tokio::time::timeout(Duration::from_secs(5), adapter.open_session(config)).await.expect("refused at once, not left waiting");
    let err = refused.err().expect("refused");
    assert!(err.to_string().contains("authentication"), "the cockpit offers to sign in on it: {err}");
    assert_eq!(*seen.lock().unwrap(), vec![Some(false)]);
}

#[tokio::test]
async fn status_comes_from_its_login_file_when_it_cant_say() {
    let home = tempfile::tempdir().unwrap();
    let kept = home.path().join("acp_token.json");
    let profile = profile(&format!("{GOOGLE}kept = '{}'\n", kept.display()));
    let launcher: Arc<dyn AcpLauncher> = Arc::new(loopback_agent());
    let status = async || probe(&launcher, &profile).await.unwrap().sign_in.unwrap().status;
    assert_eq!(status().await, AuthStatus::SignedOut);
    std::fs::write(&kept, "{}").unwrap();
    assert_eq!(status().await, AuthStatus::SignedIn);
}
