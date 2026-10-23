//! Claude Code through the ACP driver, for real, signed out: its own
//! folder is an empty one, so the machine's login (if any) isn't touched
//! or used. Machine-dependent: it needs the `claude-code` pack installed
//! and passing its check; without it, it says so and skips.

use std::time::Duration;

use harness_contracts::{AuthMethodKind, AuthStatus, HarnessLaunch, ProviderStatus, RuntimeMode, RuntimePolicy, SessionConfig, ThreadId};
use harness_core::ProviderDriver;

#[tokio::test]
async fn signed_out_it_says_so_offers_its_logins_and_shows_a_sign_in_link_needs_the_pack() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog = meta_harness::packs::catalog(root).unwrap();
    let Some(mut entry) = meta_harness::packs::harness_launches(&catalog).into_iter().find(|e| e.profile.instance.as_str() == "claude-code") else {
        panic!("the claude-code pack ships");
    };
    let HarnessLaunch::Ready { launch } = &mut entry.launch else {
        return eprintln!("skipped: {:?}", entry.launch);
    };
    let home = tempfile::tempdir().unwrap();
    launch.env.push(("CLAUDE_CONFIG_DIR".into(), home.path().display().to_string()));

    let driver = harness_acp::AcpDriver::new(root.to_path_buf(), Default::default());
    let instance = driver.create(entry.profile.instance.clone(), driver.default_config(&entry)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(120), instance.refresh()).await.expect("checked in time").unwrap();
    let snapshot = instance.snapshot().borrow().clone();
    println!("{:#?}", (&snapshot.status, &snapshot.auth, &snapshot.capabilities.turns, &snapshot.runtime_modes));
    assert_eq!(snapshot.status, ProviderStatus::Ready);
    assert_eq!(snapshot.auth.status, AuthStatus::SignedOut, "it says it's signed out");
    assert!(snapshot.auth.methods.iter().any(|m| m.id == "claude-ai-login" && m.kind == AuthMethodKind::Command), "{:?}", snapshot.auth.methods);
    assert!(snapshot.capabilities.turns.active_steering, "it takes messages into the running turn");
    assert!(snapshot.supports_plan_mode && snapshot.runtime_modes.contains(&RuntimeMode::FullAccess));

    // Signing in starts its login command, which shows a link.
    let auth = harness_core::ProviderInstance::auth(instance.as_ref()).unwrap();
    let challenge = tokio::time::timeout(Duration::from_secs(60), auth.begin("claude-ai-login")).await.expect("a link in time").unwrap();
    println!("{challenge:#?}");
    let url = challenge.url.expect("a link to open");
    assert!(url.starts_with("https://") && url.contains("oauth"), "{url}");
    assert!(challenge.expects.is_some(), "it takes a code back");

    // Signed out, a session isn't opened: it says sign in.
    let config = SessionConfig { thread_id: ThreadId::new("e2e"), policy: RuntimePolicy::default(), model: None, resume: None, mcp_servers: Vec::new() };
    match tokio::time::timeout(Duration::from_secs(120), instance.adapter().open_session(config)).await.expect("an answer in time") {
        Err(err) => println!("session refused: {err}"),
        Ok(session) => {
            println!("a session opened signed out (it asks at the first prompt)");
            session.close().await.unwrap();
        }
    }
}
