use meta_harness::auth::{
    forward_antigravity_callback, is_logout_prompt, parse_antigravity_authorization_url,
    AntigravityAuth, AntigravityAuthRuntime, ProviderAuthState,
};
use meta_harness::error::HarnessError;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const INSTANCE_ID: &str = "antigravity-auth-test";
const OWNER: &str = "t3-auth-session-owner";
const OTHER_OWNER: &str = "t3-auth-session-other";
const AUTHORIZATION_URL: &str =
    "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&redirect_uri=http%3A%2F%2F127.0.0.1%3A51234%2F&state=test-state";
const CALLBACK_URL: &str = "http://127.0.0.1:51234/?state=test-state&code=test-code";

struct MockAuthRuntime {
    events: Arc<Mutex<Vec<String>>>,
    supports_logout: bool,
}

impl AntigravityAuthRuntime for MockAuthRuntime {
    fn initialize(&mut self) -> Result<serde_json::Value, HarnessError> {
        self.events.lock().unwrap().push("initialize".to_string());
        if self.supports_logout {
            Ok(json!({
                "protocolVersion": 1,
                "authMethods": [{"id": "oauth-personal", "name": "Log in with Google"}],
                "agentCapabilities": {"auth": {"logout": {}}}
            }))
        } else {
            Ok(json!({
                "protocolVersion": 1,
                "authMethods": [{"id": "oauth-personal", "name": "Log in with Google"}],
                "agentCapabilities": {}
            }))
        }
    }

    fn start(&mut self) -> Result<serde_json::Value, HarnessError> {
        self.events.lock().unwrap().push("authenticate".to_string());
        self.events.lock().unwrap().push("session-new".to_string());
        Ok(json!({
            "sessionId": "native-session",
            "models": {
                "currentModelId": "gemini-test",
                "availableModels": [{"modelId": "gemini-test", "name": "Gemini test"}]
            }
        }))
    }

    fn request(
        &mut self,
        method: &str,
        _params: serde_json::Value,
    ) -> Result<serde_json::Value, HarnessError> {
        self.events.lock().unwrap().push(method.to_string());
        Ok(json!({}))
    }
}

struct AuthTestHarness {
    auth: AntigravityAuth,
    events: Arc<Mutex<Vec<String>>>,
    catalog: Arc<Mutex<Vec<String>>>,
    forwarded: Arc<AtomicUsize>,
}

impl AuthTestHarness {
    fn new() -> Self {
        let auth = AntigravityAuth::new(INSTANCE_ID.to_string(), true);
        let events = Arc::new(Mutex::new(Vec::new()));
        let catalog = Arc::new(Mutex::new(vec!["previous-account-model".to_string()]));
        let forwarded = Arc::new(AtomicUsize::new(0));

        Self {
            auth,
            events,
            catalog,
            forwarded,
        }
    }

    fn get_catalog(&self) -> Vec<String> {
        self.catalog.lock().unwrap().clone()
    }

    fn get_events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    async fn wait_for_phase(&self, expected_phase: &str, session_id: &str) -> ProviderAuthState {
        let current = self.auth.get_state(session_id);
        if current.phase == expected_phase {
            return current;
        }

        let mut rx = self.auth.subscribe();
        let timeout = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                res = rx.recv() => {
                    if let Ok((_owner, _state)) = res {
                        let visible = self.auth.get_state(session_id);
                        if visible.phase == expected_phase {
                            return visible;
                        }
                    }
                }
                _ = &mut timeout => {
                    panic!("Timed out waiting for phase {expected_phase}, current is {}", self.auth.get_state(session_id).phase);
                }
            }
        }
    }
}

// Test 1: accepts the same authorization URL from stderr and stdout
#[tokio::test]
async fn test_accepts_the_same_authorization_url_from_stderr_and_stdout() {
    let harness = AuthTestHarness::new();

    let state = harness.auth.start(OWNER, || Ok(())).unwrap();
    assert_eq!(state.phase, "starting");

    let flow_id = state.flow_id.clone().unwrap();

    // Deliver URLs (simulating stdout and stderr both emitting it)
    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();
    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();

    let waiting = harness.wait_for_phase("waiting", OWNER).await;
    assert_eq!(
        waiting.authorization_url.as_deref(),
        Some(AUTHORIZATION_URL)
    );

    // Complete successfully
    harness.auth.finish_authentication(&flow_id, Ok(()));
    let succeeded = harness.wait_for_phase("succeeded", OWNER).await;
    assert_eq!(succeeded.phase, "succeeded");
}

// Test 2: accepts a delayed duplicate after callback completion starts
#[tokio::test]
async fn test_accepts_a_delayed_duplicate_after_callback_completion_starts() {
    let harness = AuthTestHarness::new();
    let state = harness.auth.start(OWNER, || Ok(())).unwrap();
    let flow_id = state.flow_id.clone().unwrap();

    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();
    let _waiting = harness.wait_for_phase("waiting", OWNER).await;

    let _verifying = harness
        .auth
        .complete(OWNER, &flow_id, CALLBACK_URL)
        .unwrap();

    // Delayed duplicate arrives while in verifying phase
    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();

    harness.auth.finish_authentication(&flow_id, Ok(()));
    let succeeded = harness.wait_for_phase("succeeded", OWNER).await;
    assert_eq!(succeeded.phase, "succeeded");
}

// Test 3: rejects a different second authorization URL
#[tokio::test]
async fn test_rejects_a_different_second_authorization_url() {
    let harness = AuthTestHarness::new();
    let _state = harness.auth.start(OWNER, || Ok(())).unwrap();

    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();
    let second_url = format!("{AUTHORIZATION_URL}&scope=another-request");
    let res = harness.auth.receive_authorization_url(&second_url);
    assert!(res.is_err());

    let failed = harness.wait_for_phase("failed", OWNER).await;
    assert!(failed.authorization_url.is_none());
    assert_eq!(harness.get_catalog(), vec!["previous-account-model"]);
}

// Test 4: keeps a remote flow private and waits for native auth and catalog discovery
#[tokio::test]
async fn test_keeps_a_remote_flow_private_and_waits_for_native_auth_and_catalog_discovery() {
    let harness = AuthTestHarness::new();
    let state = harness.auth.start(OWNER, || Ok(())).unwrap();
    assert!(state.flow_id.is_some());
    let flow_id = state.flow_id.unwrap();

    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();
    let waiting = harness.wait_for_phase("waiting", OWNER).await;
    assert_eq!(
        waiting.authorization_url.as_deref(),
        Some(AUTHORIZATION_URL)
    );

    // Other owner sees masked state
    let other = harness.auth.get_state(OTHER_OWNER);
    assert!(other.authorization_url.is_none());
    assert!(other.flow_id.is_none());
    assert_eq!(
        other.message.as_deref(),
        Some("Sign-in is in progress in another client.")
    );

    // Stolen attempt fails
    let stolen = harness
        .auth
        .complete(OTHER_OWNER, &flow_id, CALLBACK_URL);
    assert!(stolen.is_err());
    assert_eq!(harness.forwarded.load(Ordering::SeqCst), 0);

    // Legitimate completion succeeds
    let verifying = harness.auth.complete(OWNER, &flow_id, CALLBACK_URL).unwrap();
    assert_eq!(verifying.phase, "verifying");
    assert_eq!(harness.get_catalog(), vec!["previous-account-model"]);

    // Complete discovery
    *harness.catalog.lock().unwrap() = vec!["gemini-test".to_string()];
    harness.auth.finish_authentication(&flow_id, Ok(()));

    let succeeded = harness.wait_for_phase("succeeded", OWNER).await;
    assert_eq!(harness.get_catalog(), vec!["gemini-test"]);
    assert!(succeeded.authorization_url.is_none());
    assert!(succeeded.expires_at.is_none());
}

// Test 5: distinguishes a post-authentication session failure without exposing its payload
#[tokio::test]
async fn test_distinguishes_a_post_authentication_session_failure_without_exposing_its_payload() {
    let harness = AuthTestHarness::new();
    let state = harness.auth.start(OWNER, || Ok(())).unwrap();
    let flow_id = state.flow_id.unwrap();

    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();
    let _waiting = harness.wait_for_phase("waiting", OWNER).await;

    // Simulate session/new internal failure
    let err = HarnessError::ProviderDriver {
        detail: format!("session/new code: -32603 Internal error {CALLBACK_URL}"),
    };
    harness.auth.finish_authentication(&flow_id, Err(err));

    let failed = harness.wait_for_phase("failed", OWNER).await;
    assert_eq!(
        failed.message.as_deref(),
        Some("Antigravity authenticated, but could not initialize a session or load models.")
    );
    assert!(failed.authorization_url.is_none());
    assert_eq!(harness.get_catalog(), vec!["previous-account-model"]);
}

// Test 6: does not call callback HTTP success a successful Google sign-in
#[tokio::test]
async fn test_does_not_call_callback_http_success_a_successful_google_sign_in() {
    let harness = AuthTestHarness::new();
    let state = harness.auth.start(OWNER, || Ok(())).unwrap();
    let flow_id = state.flow_id.unwrap();

    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();
    let _waiting = harness.wait_for_phase("waiting", OWNER).await;
    let _verifying = harness.auth.complete(OWNER, &flow_id, CALLBACK_URL).unwrap();

    let err = HarnessError::ProviderDriver {
        detail: format!("access_denied {CALLBACK_URL}"),
    };
    harness.auth.finish_authentication(&flow_id, Err(err));

    let failed = harness.wait_for_phase("failed", OWNER).await;
    let msg = failed.message.unwrap_or_default();
    assert!(msg.contains("not approved"));
    assert!(!msg.contains("test-code"));
    assert!(failed.authorization_url.is_none());
    assert_eq!(harness.get_catalog(), vec!["previous-account-model"]);
}

// Test 7: accepts direct local or cached completion without a callback RPC
#[tokio::test]
async fn test_accepts_direct_local_or_cached_completion_without_a_callback_rpc() {
    let harness = AuthTestHarness::new();

    let state = harness.auth.start(OWNER, || Ok(())).unwrap();
    let flow_id = state.flow_id.unwrap();

    *harness.catalog.lock().unwrap() = vec!["gemini-test".to_string()];
    harness.auth.finish_authentication(&flow_id, Ok(()));

    let succeeded = harness.wait_for_phase("succeeded", OWNER).await;
    assert_eq!(succeeded.phase, "succeeded");
    assert_eq!(harness.forwarded.load(Ordering::SeqCst), 0);
    assert_eq!(harness.get_catalog(), vec!["gemini-test"]);
}

// Test 8: rejects mismatched callbacks without sending any HTTP request
#[tokio::test]
async fn test_rejects_mismatched_callbacks_without_sending_any_http_request() {
    let harness = AuthTestHarness::new();
    let state = harness.auth.start(OWNER, || Ok(())).unwrap();
    let flow_id = state.flow_id.unwrap();

    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();
    let _waiting = harness.wait_for_phase("waiting", OWNER).await;

    let invalid_urls = vec![
        CALLBACK_URL.replace("51234", "51235"),
        CALLBACK_URL.replace("test-state", "wrong-state"),
        CALLBACK_URL.replace("/?", "/other?"),
        format!("{CALLBACK_URL}&state=test-state"),
    ];

    for invalid in invalid_urls {
        let res = harness.auth.complete(OWNER, &flow_id, &invalid);
        assert!(res.is_err());
    }

    assert_eq!(harness.forwarded.load(Ordering::SeqCst), 0);
    let waiting = harness.auth.get_state(OWNER);
    assert_eq!(waiting.phase, "waiting");
    assert_eq!(
        waiting.authorization_url.as_deref(),
        Some(AUTHORIZATION_URL)
    );

    let _ = harness.auth.cancel(OWNER, &flow_id).unwrap();
}

// Test 9: fails the flow when delivery fails after the requesting client disconnects
#[tokio::test]
async fn test_fails_the_flow_when_delivery_fails_after_the_requesting_client_disconnects() {
    let harness = AuthTestHarness::new();
    let state = harness.auth.start(OWNER, || Ok(())).unwrap();
    let flow_id = state.flow_id.unwrap();

    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();
    let _waiting = harness.wait_for_phase("waiting", OWNER).await;

    let verifying = harness.auth.complete(OWNER, &flow_id, CALLBACK_URL).unwrap();
    assert_eq!(verifying.phase, "verifying");

    // Client disconnects; delivery fails asynchronously
    harness.auth.fail_delivery(&flow_id);

    let failed = harness.wait_for_phase("failed", OWNER).await;
    let msg = failed.message.unwrap_or_default();
    assert!(msg.contains("Could not deliver"));
}

// Test 10: cancel closes the owned process without forwarding a denial
#[tokio::test]
async fn test_cancel_closes_the_owned_process_without_forwarding_a_denial() {
    let harness = AuthTestHarness::new();
    let state = harness.auth.start(OWNER, || Ok(())).unwrap();
    let flow_id = state.flow_id.unwrap();

    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();
    let _waiting = harness.wait_for_phase("waiting", OWNER).await;

    // Wrong owner cannot cancel
    let wrong = harness.auth.cancel(OTHER_OWNER, &flow_id);
    assert!(wrong.is_err());

    let cancelled = harness.auth.cancel(OWNER, &flow_id).unwrap();
    assert_eq!(cancelled.phase, "cancelled");
    assert!(cancelled.authorization_url.is_none());
    assert_eq!(harness.forwarded.load(Ordering::SeqCst), 0);
    assert_eq!(harness.get_catalog(), vec!["previous-account-model"]);
}

// Test 11: expires the flow at the official deadline and removes its URL
#[tokio::test]
async fn test_expires_the_flow_at_the_official_deadline_and_removes_its_url() {
    let harness = AuthTestHarness::new();
    let state = harness.auth.start(OWNER, || Ok(())).unwrap();
    let flow_id = state.flow_id.unwrap();

    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();
    let _waiting = harness.wait_for_phase("waiting", OWNER).await;

    // Simulate expiration
    harness.auth.expire_flow(&flow_id);

    let failed = harness.wait_for_phase("failed", OWNER).await;
    let msg = failed.message.unwrap_or_default();
    assert!(msg.contains("expired"));
    assert!(failed.authorization_url.is_none());

    // Late completion fails
    let late = harness.auth.complete(OWNER, &flow_id, CALLBACK_URL);
    assert!(late.is_err());
    assert_eq!(harness.forwarded.load(Ordering::SeqCst), 0);
}

// Test 12: survives subscriber disconnect and does not replace a competing client's flow
#[tokio::test]
async fn test_survives_subscriber_disconnect_and_does_not_replace_a_competing_clients_flow() {
    let harness = AuthTestHarness::new();
    let first = harness.auth.start(OWNER, || Ok(())).unwrap();

    harness
        .auth
        .receive_authorization_url(AUTHORIZATION_URL)
        .unwrap();
    let _waiting = harness.wait_for_phase("waiting", OWNER).await;

    // Same owner re-submitting start gets existing flow
    let second = harness.auth.start(OWNER, || Ok(())).unwrap();
    assert_eq!(first.flow_id, second.flow_id);

    // Competing client gets rejected
    let competing = harness.auth.start(OTHER_OWNER, || Ok(()));
    assert!(competing.is_err());

    let resumed = harness.auth.get_state(OWNER);
    assert_eq!(resumed.flow_id, first.flow_id);

    let _ = harness.auth.cancel(OWNER, &first.flow_id.unwrap()).unwrap();
}

// Test 13: sign-out closes admission and every process before fresh native logout
#[tokio::test]
async fn test_sign_out_closes_admission_and_every_process_before_fresh_native_logout() {
    let harness = AuthTestHarness::new();
    let events = harness.events.clone();
    let catalog = harness.catalog.clone();

    // Start a mock active process
    let stopped_process = Arc::new(AtomicBool::new(false));
    let sp_clone = stopped_process.clone();

    let stop_sessions = move || {
        events.lock().unwrap().push("sessions-stop".to_string());
        sp_clone.store(true, Ordering::SeqCst);
        Ok(())
    };

    let events_rt = harness.events.clone();
    let make_runtime = move || {
        events_rt.lock().unwrap().push("process-open".to_string());
        let mock = MockAuthRuntime {
            events: events_rt.clone(),
            supports_logout: true,
        };
        // Hook catalog clear
        *catalog.lock().unwrap() = vec![];
        events_rt.lock().unwrap().push("catalog-cleared".to_string());
        Ok(Box::new(mock) as Box<dyn AntigravityAuthRuntime>)
    };

    let res = harness.auth.logout(stop_sessions, make_runtime).unwrap();
    assert_eq!(res.phase, "idle");
    assert!(stopped_process.load(Ordering::SeqCst));
    assert_eq!(harness.get_catalog(), Vec::<String>::new());

    let evs = harness.get_events();
    assert!(evs.contains(&"sessions-stop".to_string()));
    assert!(evs.contains(&"process-open".to_string()));
    assert!(evs.contains(&"initialize".to_string()));
    assert!(evs.contains(&"logout".to_string()));
    assert!(evs.contains(&"catalog-cleared".to_string()));
}

// Test 14: signs out after a slow packaged runtime starts
#[tokio::test]
async fn test_signs_out_after_a_slow_packaged_runtime_starts() {
    let harness = AuthTestHarness::new();
    let events = harness.events.clone();
    let catalog = harness.catalog.clone();

    let make_runtime = move || {
        let mock = MockAuthRuntime {
            events: events.clone(),
            supports_logout: true,
        };
        *catalog.lock().unwrap() = vec![];
        Ok(Box::new(mock) as Box<dyn AntigravityAuthRuntime>)
    };

    let res = harness.auth.logout(|| Ok(()), make_runtime).unwrap();
    assert_eq!(res.phase, "idle");
    assert_eq!(harness.get_catalog(), Vec::<String>::new());
    assert!(harness.get_events().contains(&"logout".to_string()));
}

// Test 15: closes a stalled sign-out process without clearing its account catalog
#[tokio::test]
async fn test_closes_a_stalled_sign_out_process_without_clearing_its_account_catalog() {
    let harness = AuthTestHarness::new();

    let make_runtime = || {
        Err(HarnessError::ProviderSetup {
            operation: "logout".to_string(),
            detail: "Antigravity sign-out timed out.".to_string(),
        })
    };

    let res = harness.auth.logout(|| Ok(()), make_runtime);
    assert!(res.is_err());
    assert!(!harness.get_events().contains(&"logout".to_string()));
    assert_eq!(harness.get_catalog(), vec!["previous-account-model"]);
}

// Test 16: sign-out interrupts startup without interrupting its caller after startup returns
#[tokio::test]
async fn test_sign_out_interrupts_startup_without_interrupting_its_caller_after_startup_returns() {
    let harness = AuthTestHarness::new();

    // When logout starts, concurrent process start is denied
    let events = harness.events.clone();
    let make_runtime = move || {
        let mock = MockAuthRuntime {
            events: events.clone(),
            supports_logout: true,
        };
        Ok(Box::new(mock) as Box<dyn AntigravityAuthRuntime>)
    };

    let logout_res = harness.auth.logout(|| Ok(()), make_runtime).unwrap();
    assert_eq!(logout_res.phase, "idle");

    // with_process works normally after logout finishes
    let proc_res = harness.auth.with_process(|| {}, || Ok(42)).unwrap();
    assert_eq!(proc_res, 42);
}

// Test 17: failed session stopping still closes owned processes and skips native logout
#[tokio::test]
async fn test_failed_session_stopping_still_closes_owned_processes_and_skips_native_logout() {
    let harness = AuthTestHarness::new();

    let stop_failed = || {
        Err(HarnessError::ProviderSetup {
            operation: "stopSessions".to_string(),
            detail: "Stop failed.".to_string(),
        })
    };

    let make_runtime = || -> Result<Box<dyn AntigravityAuthRuntime>, HarnessError> {
        panic!("make_runtime must not be called when stop_sessions fails");
    };

    let res = harness.auth.logout(stop_failed, make_runtime);
    assert!(res.is_err());
    assert_eq!(harness.get_catalog(), vec!["previous-account-model"]);
}

// Test 18: finishes sign-out when the requesting client disconnects
#[tokio::test]
async fn test_finishes_sign_out_when_the_requesting_client_disconnects() {
    let harness = AuthTestHarness::new();
    let catalog = harness.catalog.clone();
    let events = harness.events.clone();

    let make_runtime = move || {
        let mock = MockAuthRuntime {
            events: events.clone(),
            supports_logout: true,
        };
        *catalog.lock().unwrap() = vec![];
        Ok(Box::new(mock) as Box<dyn AntigravityAuthRuntime>)
    };

    let res = harness.auth.logout(|| Ok(()), make_runtime).unwrap();
    assert_eq!(res.phase, "idle");
    assert_eq!(res.message.as_deref(), Some("Signed out of Google."));
    assert_eq!(harness.get_catalog(), Vec::<String>::new());
    assert!(harness.get_events().contains(&"logout".to_string()));
}

// Test 19: does not call logout unless the official process advertises it
#[tokio::test]
async fn test_does_not_call_logout_unless_the_official_process_advertises_it() {
    let harness = AuthTestHarness::new();
    let events = harness.events.clone();

    let make_runtime = move || {
        let mock = MockAuthRuntime {
            events: events.clone(),
            supports_logout: false,
        };
        Ok(Box::new(mock) as Box<dyn AntigravityAuthRuntime>)
    };

    let res = harness.auth.logout(|| Ok(()), make_runtime);
    assert!(res.is_err());
    assert!(!harness.get_events().contains(&"logout".to_string()));
    assert_eq!(harness.get_catalog(), vec!["previous-account-model"]);
}

// Bonus unit tests verifying helper methods
#[test]
fn test_is_logout_prompt_detection() {
    assert!(is_logout_prompt("/logout", false));
    assert!(is_logout_prompt("  /logout  ", false));
    assert!(!is_logout_prompt("/logout", true));
    assert!(!is_logout_prompt("/other", false));
    assert!(!is_logout_prompt("hello /logout", false));
}

#[test]
fn test_authorization_url_parser() {
    let parsed = parse_antigravity_authorization_url(AUTHORIZATION_URL).unwrap();
    assert_eq!(parsed.authorization_url, AUTHORIZATION_URL);
    assert_eq!(parsed.state, "test-state");
    assert_eq!(parsed.redirect_port, 51234);
    assert_eq!(parsed.redirect_path, "/");
}

#[tokio::test]
async fn test_forward_antigravity_callback_over_tcp() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf[..n]).to_string()
    });

    let callback = url::Url::parse(&format!("http://127.0.0.1:{}/callback?code=xyz123&state=abc", port)).unwrap();
    let res = forward_antigravity_callback(&callback).await;
    assert!(res.is_ok());

    let received_http = server_task.await.unwrap();
    assert!(received_http.starts_with("GET /callback?code=xyz123&state=abc HTTP/1.1"));
}

