use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use meta_harness::adapter::AntigravityAdapter;
use meta_harness::driver::{
    AntigravityDriver, AntigravityDriverConfig, AuthMethod, DriverCreateOptions,
};
use meta_harness::installation::{AntigravityInstallation, AntigravityInstallationOptions};
use meta_harness::process::TokioProcessSpawner;
use meta_harness::server::{create_router, AppState};

#[tokio::test]
async fn test_server_http_health_and_snapshot_endpoints() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let spawner = Arc::new(TokioProcessSpawner::new());
    let adapter = Arc::new(AntigravityAdapter::new());
    let profile_dir = tempfile::tempdir().unwrap();
    let installation = Arc::new(
        AntigravityInstallation::new(AntigravityInstallationOptions {
            base_dir: profile_dir.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap(),
    );

    let driver = AntigravityDriver::create(DriverCreateOptions {
        instance_id: "test-instance".to_string(),
        display_name: "Test Instance".to_string(),
        enabled: true,
        config: AntigravityDriverConfig {
            auth_method: AuthMethod::OAuthPersonal,
            api_key: None,
        },
        environment: vec![],
        profile_directory: profile_dir.path().to_path_buf(),
        installation,
        spawner,
    })
    .unwrap();

    let state = AppState::new(adapter, Arc::new(Mutex::new(driver)));
    let app = create_router(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // 1. Test /health
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.contains("HTTP/1.1 200 OK"));
    assert!(response.contains("\"status\":\"ok\""));

    // 2. Test /api/driver/snapshot
    let mut stream2 = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream2
        .write_all(b"GET /api/driver/snapshot HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response2 = String::new();
    stream2.read_to_string(&mut response2).await.unwrap();
    assert!(response2.contains("HTTP/1.1 200 OK"));
    assert!(response2.contains("\"status\":\"ready\""));
}

#[tokio::test]
async fn test_server_http_session_lifecycle() {
    let mock_server_bin = env!("CARGO_BIN_EXE_mock_acp_server");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let spawner = Arc::new(TokioProcessSpawner::new());
    let runtime_factory = meta_harness::process::make_native_runtime_factory(
        spawner.clone(),
        mock_server_bin.to_string(),
        vec![],
        AuthMethod::OAuthPersonal,
        None,
    );

    let adapter = Arc::new(AntigravityAdapter::with_runtime_factory(runtime_factory));
    let profile_dir = tempfile::tempdir().unwrap();
    let installation = Arc::new(
        AntigravityInstallation::new(AntigravityInstallationOptions {
            base_dir: profile_dir.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap(),
    );

    let driver = AntigravityDriver::create(DriverCreateOptions {
        instance_id: "test-instance".to_string(),
        display_name: "Test Instance".to_string(),
        enabled: true,
        config: AntigravityDriverConfig {
            auth_method: AuthMethod::OAuthPersonal,
            api_key: None,
        },
        environment: vec![],
        profile_directory: profile_dir.path().to_path_buf(),
        installation,
        spawner,
    })
    .unwrap();

    let state = AppState::new(adapter, Arc::new(Mutex::new(driver)));
    let app = create_router(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // 1. Start Session via POST /api/sessions/start
    let session_body = r#"{"thread_id":"http-thread-1","cwd":"/tmp","runtime_mode":"auto-accept-edits"}"#;
    let request = format!(
        "POST /api/sessions/start HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        session_body.len(),
        session_body
    );

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.contains("HTTP/1.1 200 OK"));
    assert!(response.contains("\"sessionId\":\"real-rust-proc-sess-100\""));

    // 2. Stop Session via POST /api/sessions/http-thread-1/stop
    let stop_req = "POST /api/sessions/http-thread-1/stop HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let mut stream_stop = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream_stop.write_all(stop_req.as_bytes()).await.unwrap();

    let mut stop_resp = String::new();
    stream_stop.read_to_string(&mut stop_resp).await.unwrap();
    assert!(stop_resp.contains("HTTP/1.1 200 OK"));
    assert!(stop_resp.contains("\"status\":\"stopped\""));
}

#[tokio::test]
async fn test_server_websocket_upgrade() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let spawner = Arc::new(TokioProcessSpawner::new());
    let adapter = Arc::new(AntigravityAdapter::new());
    let profile_dir = tempfile::tempdir().unwrap();
    let installation = Arc::new(
        AntigravityInstallation::new(AntigravityInstallationOptions {
            base_dir: profile_dir.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap(),
    );

    let driver = AntigravityDriver::create(DriverCreateOptions {
        instance_id: "test-instance".to_string(),
        display_name: "Test Instance".to_string(),
        enabled: true,
        config: AntigravityDriverConfig {
            auth_method: AuthMethod::OAuthPersonal,
            api_key: None,
        },
        environment: vec![],
        profile_directory: profile_dir.path().to_path_buf(),
        installation,
        spawner,
    })
    .unwrap();

    let state = AppState::new(adapter, Arc::new(Mutex::new(driver)));
    let app = create_router(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let ws_req = "GET /ws/events HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(ws_req.as_bytes()).await.unwrap();

    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf[..n]);

    assert!(response.contains("HTTP/1.1 101 Switching Protocols"));
    assert!(response.contains("Upgrade: websocket") || response.contains("upgrade: websocket"));
    assert!(response.contains("Sec-WebSocket-Accept:") || response.contains("sec-websocket-accept:"));
}

#[tokio::test]
async fn test_server_fs_read_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let spawner = Arc::new(TokioProcessSpawner::new());
    let adapter = Arc::new(AntigravityAdapter::new());
    let profile_dir = tempfile::tempdir().unwrap();
    let installation = Arc::new(
        AntigravityInstallation::new(AntigravityInstallationOptions {
            base_dir: profile_dir.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap(),
    );

    let driver = AntigravityDriver::create(DriverCreateOptions {
        instance_id: "test-instance".to_string(),
        display_name: "Test Instance".to_string(),
        enabled: true,
        config: AntigravityDriverConfig {
            auth_method: AuthMethod::OAuthPersonal,
            api_key: None,
        },
        environment: vec![],
        profile_directory: profile_dir.path().to_path_buf(),
        installation,
        spawner,
    })
    .unwrap();

    let state = AppState::new(adapter, Arc::new(Mutex::new(driver)));
    let app = create_router(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /api/fs/read?path=Cargo.toml HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.contains("HTTP/1.1 200 OK"));
    assert!(response.contains("meta-harness"));
    assert!(response.contains("\"lines\":"));
}

#[tokio::test]
async fn test_server_dev_feedback_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let spawner = Arc::new(TokioProcessSpawner::new());
    let adapter = Arc::new(AntigravityAdapter::new());
    let profile_dir = tempfile::tempdir().unwrap();
    let installation = Arc::new(
        AntigravityInstallation::new(AntigravityInstallationOptions {
            base_dir: profile_dir.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap(),
    );

    let driver = AntigravityDriver::create(DriverCreateOptions {
        instance_id: "test-instance".to_string(),
        display_name: "Test Instance".to_string(),
        enabled: true,
        config: AntigravityDriverConfig {
            auth_method: AuthMethod::OAuthPersonal,
            api_key: None,
        },
        environment: vec![],
        profile_directory: profile_dir.path().to_path_buf(),
        installation,
        spawner,
    })
    .unwrap();

    let state = AppState::new(adapter, Arc::new(Mutex::new(driver)));
    let app = create_router(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let feedback_body = r#"{"category":"Bug","comment":"Testing feedback collector","target":{"type":"code","file":"Cargo.toml"}}"#;
    let request = format!(
        "POST /api/dev/feedback HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        feedback_body.len(),
        feedback_body
    );

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.contains("HTTP/1.1 200 OK"));
    assert!(response.contains("\"status\":\"saved\""));
    assert!(response.contains("\"total_feedback_count\":"));

    // Cleanup dev_feedback.json if created in current working dir
    let _ = tokio::fs::remove_file("dev_feedback.json").await;
}

#[tokio::test]
async fn test_server_static_file_serving() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let spawner = Arc::new(TokioProcessSpawner::new());
    let adapter = Arc::new(AntigravityAdapter::new());
    let profile_dir = tempfile::tempdir().unwrap();
    let installation = Arc::new(
        AntigravityInstallation::new(AntigravityInstallationOptions {
            base_dir: profile_dir.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap(),
    );

    let driver = AntigravityDriver::create(DriverCreateOptions {
        instance_id: "test-instance".to_string(),
        display_name: "Test Instance".to_string(),
        enabled: true,
        config: AntigravityDriverConfig {
            auth_method: AuthMethod::OAuthPersonal,
            api_key: None,
        },
        environment: vec![],
        profile_directory: profile_dir.path().to_path_buf(),
        installation,
        spawner,
    })
    .unwrap();

    let state = AppState::new(adapter, Arc::new(Mutex::new(driver)));
    let app = create_router(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Request index.html
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /index.html HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.contains("HTTP/1.1 200 OK"));
    assert!(response.contains("Antigravity Meta-Harness Cockpit"));

    // Request app.js
    let mut stream_js = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream_js
        .write_all(b"GET /app.js HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response_js = String::new();
    stream_js.read_to_string(&mut response_js).await.unwrap();
    assert!(response_js.contains("HTTP/1.1 200 OK"));
    assert!(response_js.contains("Antigravity Meta-Harness Cockpit Frontend Controller"));
}



