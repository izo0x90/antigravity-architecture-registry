use std::fs;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tempfile::tempdir;
use tokio::process::Command;

use meta_harness::process::TokioProcessSpawner;
use meta_harness::transport::JsonRpcTransport;

#[tokio::test]
async fn test_json_rpc_transport_and_reverse_rpc() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let test_file = root.join("hello.txt");
    fs::write(&test_file, "Line 1\nLine 2\nLine 3\n").unwrap();

    let mut cmd = Command::new("cat");
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn cat");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take();

    let auth_detected = Arc::new(AtomicBool::new(false));
    let auth_detected_clone = auth_detected.clone();

    let transport = JsonRpcTransport::new(
        stdin,
        stdout,
        stderr,
        vec![root.clone()],
        Some(Arc::new(move |_url| {
            auth_detected_clone.store(true, Ordering::SeqCst);
        })),
    );

    assert_eq!(transport.allowed_roots().len(), 1);
    assert_eq!(transport.allowed_roots()[0], root);
}

#[tokio::test]
async fn test_tokio_process_spawner_real_execution() {
    use meta_harness::driver::{DriverProcessSpawner, ProcessLaunchRecord};

    let spawner = TokioProcessSpawner::new();
    let record = ProcessLaunchRecord {
        command: "echo".to_string(),
        args: vec!["hello from real process".to_string()],
        cwd: None,
        extend_env: true,
        profile_directory: None,
        harness_path: None,
        force_file_storage: None,
        credential_keys: vec![],
        gemini_api_key: None,
        temp_directory: None,
        env: vec![],
    };

    let mut handle = spawner.spawn(record).expect("spawner should succeed");
    // Wait for echo to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    assert!(!handle.is_running());
    handle.close();
}

#[tokio::test]
async fn test_eof_drains_pending_requests_without_hanging() {
    let mut cmd = Command::new("true"); // Exits immediately
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn true");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    let transport = JsonRpcTransport::new(stdin, stdout, None, vec![], None);

    // Give child time to exit
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Sending a request should fail promptly with an error, not hang!
    let res = tokio::time::timeout(
        tokio::time::Duration::from_millis(500),
        transport.send_request("test_method", serde_json::json!({})),
    )
    .await;

    assert!(res.is_ok(), "Request timed out, hanging on EOF!");
    assert!(res.unwrap().is_err(), "Request should fail when process exits");
}

#[tokio::test]
async fn test_error_null_accepted_as_success() {
    // Spawn python or sh that echoes a response with "error": null
    let bin_path = env!("CARGO_BIN_EXE_mock_acp_server");
    let mut cmd = Command::new(bin_path);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn mock_acp_server");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    let transport = JsonRpcTransport::new(stdin, stdout, None, vec![], None);

    let res = tokio::time::timeout(
        tokio::time::Duration::from_secs(1),
        transport.send_request("ping", serde_json::json!({})),
    )
    .await
    .expect("did not timeout")
    .expect("RPC should succeed with error: null");

    assert_eq!(res.get("status").and_then(|s| s.as_str()), Some("ok"));
}

#[tokio::test]
async fn test_vertical_native_runtime_end_to_end() {
    use meta_harness::adapter::AntigravityAdapter;
    use meta_harness::driver::AuthMethod;
    use meta_harness::process::make_native_runtime_factory;
    use meta_harness::protocol::StartSessionInput;

    let tmp = tempdir().unwrap();
    let bin_path = env!("CARGO_BIN_EXE_mock_acp_server");

    let spawner = Arc::new(TokioProcessSpawner::new());
    let factory = make_native_runtime_factory(
        spawner,
        bin_path.to_string(),
        vec![],
        AuthMethod::OAuthPersonal,
        None,
    );

    let adapter = AntigravityAdapter::with_runtime_factory(factory);
    let mut event_rx = adapter.subscribe();

    let started = adapter
        .start_session(StartSessionInput {
            thread_id: "thread-real-os-proc".to_string(),
            cwd: tmp.path().to_str().unwrap().to_string(),
            runtime_mode: "approval-required".to_string(),
            model: None,
            resume_cursor: None,
            resume_session_id: None,
        })
        .await
        .expect("start_session should succeed with real compiled Rust OS process");

    assert_eq!(started.session_id, "real-rust-proc-sess-100");

    let _turn_res = adapter
        .send_turn(meta_harness::protocol::SendTurnInput {
            thread_id: "thread-real-os-proc".to_string(),
            input: "Hello real ACP process".to_string(),
            model: None,
        })
        .await
        .expect("send_turn should succeed over real OS stdio");

    // Check that we received the streamed event
    let mut got_streamed_delta = false;
    while let Ok(evt) = event_rx.try_recv() {
        if let meta_harness::protocol::ProviderRuntimeEvent::ContentDelta { payload, .. } = evt
            && payload.delta.contains("Real compiled Rust process streaming response")
        {
            got_streamed_delta = true;
            break;
        }
    }
    assert!(got_streamed_delta, "Should have received streamed text delta from real subprocess");
}

#[tokio::test]
async fn test_vertical_native_runtime_with_injector_mcp_servers() {
    use meta_harness::adapter::AntigravityAdapter;
    use meta_harness::driver::AuthMethod;
    use meta_harness::injector::AgyToolInjector;
    use meta_harness::process::make_native_runtime_factory_with_injector;
    use meta_harness::protocol::StartSessionInput;

    let tmp = tempdir().unwrap();
    let bin_path = env!("CARGO_BIN_EXE_mock_acp_server");

    let spawner = Arc::new(TokioProcessSpawner::new());
    let injector = Arc::new(AgyToolInjector::default());
    let base_url = "http://127.0.0.1:54321".to_string();
    let base_url_provider = Arc::new(move || Some(base_url.clone()));

    let factory = make_native_runtime_factory_with_injector(
        spawner,
        bin_path.to_string(),
        vec![],
        AuthMethod::OAuthPersonal,
        None,
        Some(injector),
        Some(base_url_provider),
    );

    let adapter = AntigravityAdapter::with_runtime_factory(factory);
    let mut event_rx = adapter.subscribe();

    let started = adapter
        .start_session(StartSessionInput {
            thread_id: "thread-wire-mcp".to_string(),
            cwd: tmp.path().to_str().unwrap().to_string(),
            runtime_mode: "approval-required".to_string(),
            model: None,
            resume_cursor: None,
            resume_session_id: None,
        })
        .await
        .expect("start_session should succeed with injected wire MCP");

    assert_eq!(started.session_id, "real-rust-proc-sess-100");

    let _turn_res = adapter
        .send_turn(meta_harness::protocol::SendTurnInput {
            thread_id: "thread-wire-mcp".to_string(),
            input: "Hello ACP with MCP".to_string(),
            model: None,
        })
        .await
        .expect("send_turn should succeed");

    // Check that the subprocess received the MCP servers in session/new
    let mut got_mcp_stream = false;
    while let Ok(evt) = event_rx.try_recv() {
        if let meta_harness::protocol::ProviderRuntimeEvent::ContentDelta { payload, .. } = evt
            && payload.delta.contains("Real compiled Rust process with MCP:")
            && payload.delta.contains("architecture-registry")
            && payload.delta.contains("http://127.0.0.1:54321/api/mcp/sse")
        {
            got_mcp_stream = true;
            break;
        }
    }
    assert!(
        got_mcp_stream,
        "Subprocess should have received dynamic MCP servers over ACP wire in session/new"
    );
}


