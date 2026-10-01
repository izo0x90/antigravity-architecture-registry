use std::sync::Arc;
use tokio::sync::broadcast;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use harness_protocol::arch::{ComponentSpec, SystemArchitecture, UsageNode};
use harness_protocol::ProviderRuntimeEvent;

use meta_harness::adapter::AntigravityAdapter;
use meta_harness::driver::{
    AntigravityDriver, AntigravityDriverConfig, AuthMethod, DriverCreateOptions,
    ProcessLaunchRecord,
};
use meta_harness::injector::{AgyToolInjector, HarnessToolInjector};
use meta_harness::installation::{AntigravityInstallation, AntigravityInstallationOptions};
use meta_harness::mcp::protocol::JsonRpcRequest;
use meta_harness::mcp::{McpServerState, McpService};
use meta_harness::process::TokioProcessSpawner;
use meta_harness::registry::{FsJsonRepository, RegistryService};
use meta_harness::server::{create_router, AppState};

fn sample_test_architecture() -> SystemArchitecture {
    let mut arch = SystemArchitecture::default();

    let auth = ComponentSpec {
        id: "auth_service".to_string(),
        name: "Authentication Service".to_string(),
        comp_type: "service".to_string(),
        description: "Handles user authentication".to_string(),
        status: "active".to_string(),
        stage: "draft".to_string(),
        parent_id: None,
        implements_id: None,
        location: None,
        inputs: Some(json!({"credentials": "UserCredentials"})),
        outputs: Some(json!({"jwt": "JwtToken"})),
        properties: Some(json!({"tags": ["security"]})),
        side_effects: vec![],
        implementation_spec: None,
        modification_tasks: vec![],
    };

    let token = ComponentSpec {
        id: "token_service".to_string(),
        name: "Token Service".to_string(),
        comp_type: "service".to_string(),
        description: "Mints and signs JWT tokens".to_string(),
        status: "implemented".to_string(),
        stage: "arch_approved".to_string(),
        parent_id: None,
        implements_id: None,
        location: None,
        inputs: None,
        outputs: None,
        properties: Some(json!({"tags": ["crypto"]})),
        side_effects: vec![],
        implementation_spec: None,
        modification_tasks: vec![],
    };

    let usage = UsageNode {
        node_id: "root_usage".to_string(),
        caller_id: "auth_service".to_string(),
        component_id: "token_service".to_string(),
        description: "Auth calls token service to mint JWT".to_string(),
        expected_inputs: None,
        expected_outputs: None,
        expected_side_effects: vec![],
        dependencies: vec![],
    };

    arch.components.insert("auth_service".to_string(), auth);
    arch.components.insert("token_service".to_string(), token);
    arch.usage_trees.insert("auth_flow".to_string(), usage);
    arch
}

#[tokio::test]
async fn test_mcp_service_tools_and_events() {
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let repo = Arc::new(FsJsonRepository::from_architecture(
        sample_test_architecture(),
        temp_file.path(),
    ));
    let registry = Arc::new(RegistryService::new(repo));
    let (event_tx, mut event_rx) = broadcast::channel::<ProviderRuntimeEvent>(32);
    let mcp = McpService::new(registry, Some(event_tx));

    // 1. Test initialize
    let init_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: "initialize".to_string(),
        params: None,
    };
    let init_resp = mcp.handle_request(init_req).await;
    assert!(init_resp.error.is_none());
    assert_eq!(
        init_resp.result.unwrap()["serverInfo"]["name"],
        "architecture-registry"
    );

    // 2. Test tools/list
    let list_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(2)),
        method: "tools/list".to_string(),
        params: None,
    };
    let list_resp = mcp.handle_request(list_req).await;
    let tools = list_resp.result.unwrap()["tools"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(tools.len(), 7);
    assert!(tools.iter().any(|t| t["name"] == "focus_architecture"));
    assert!(tools.iter().any(|t| t["name"] == "search_components"));

    // 3. Test tools/call search_components
    let search_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(3)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "search_components",
            "arguments": { "query": "auth" }
        })),
    };
    let search_resp = mcp.handle_request(search_req).await;
    let text = search_resp.result.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.contains("auth_service"));

    // 4. Test tools/call focus_architecture
    let focus_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(4)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "focus_architecture",
            "arguments": {
                "root_id": "auth_service",
                "perspective": "call_flow",
                "depth": 2
            }
        })),
    };
    let focus_resp = mcp.handle_request(focus_req).await;
    assert!(focus_resp.error.is_none());
    let tree_text = focus_resp.result.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(tree_text.contains("auth_service"));

    // Verify ProviderRuntimeEvent::ArchitectureFocused was emitted
    let ev = event_rx.recv().await.unwrap();
    match ev {
        ProviderRuntimeEvent::ArchitectureFocused {
            root_id,
            depth,
            direction: _,
            affected_components,
        } => {
            assert_eq!(root_id, "auth_service");
            assert_eq!(depth, 2);
            assert!(affected_components.contains(&"auth_service".to_string()));
        }
        other => panic!("Expected ArchitectureFocused, got {:?}", other),
    }

    // 5. Test tools/call approve_component
    let approve_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(5)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "approve_component",
            "arguments": { "component_id": "auth_service" }
        })),
    };
    let approve_resp = mcp.handle_request(approve_req).await;
    assert!(approve_resp.error.is_none());

    // Verify ProviderRuntimeEvent::ArchitectureUpdated was emitted
    let ev = event_rx.recv().await.unwrap();
    match ev {
        ProviderRuntimeEvent::ArchitectureUpdated {
            component_id,
            status,
        } => {
            assert_eq!(component_id, "auth_service");
            assert_eq!(status, "arch_approved");
        }
        other => panic!("Expected ArchitectureUpdated, got {:?}", other),
    }
}

#[test]
fn test_agy_tool_injector_zero_config() {
    let injector = AgyToolInjector::default();
    let profile_temp = tempfile::tempdir().unwrap();
    let mut record = ProcessLaunchRecord {
        command: "agy".to_string(),
        args: vec![],
        cwd: Some("/test/dir".to_string()),
        extend_env: true,
        profile_directory: Some(profile_temp.path().to_string_lossy().to_string()),
        harness_path: None,
        force_file_storage: None,
        credential_keys: vec![],
        gemini_api_key: None,
        temp_directory: None,
        env: vec![],
    };

    let base_url = "http://127.0.0.1:49876";

    // 1. Prepare launch does not touch the filesystem or pollute the profile directory
    injector.prepare_launch(base_url, &mut record).unwrap();
    assert_eq!(std::fs::read_dir(profile_temp.path()).unwrap().count(), 0);

    // 2. Dynamic MCP server entries are constructed in-memory for the ACP wire handshake
    let servers = injector.session_mcp_servers(base_url);
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["type"], "http");
    assert_eq!(servers[0]["name"], "architecture-registry");
    assert_eq!(servers[0]["url"], "http://127.0.0.1:49876/api/mcp/sse");
    assert_eq!(servers[0]["headers"], serde_json::json!([]));
}

#[tokio::test]
async fn test_mcp_sse_http_server_flow() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let repo = Arc::new(FsJsonRepository::from_architecture(
        sample_test_architecture(),
        temp_file.path(),
    ));
    let registry = Arc::new(RegistryService::new(repo));

    let (event_tx, _) = broadcast::channel(32);
    let mcp_service = Arc::new(McpService::new(registry, Some(event_tx)));
    let mcp_state = McpServerState::new(mcp_service);

    let spawner = Arc::new(TokioProcessSpawner::new());
    let profile_dir = tempfile::tempdir().unwrap();
    let installation = Arc::new(
        AntigravityInstallation::new(AntigravityInstallationOptions {
            base_dir: profile_dir.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap(),
    );
    let driver = AntigravityDriver::create(DriverCreateOptions {
        instance_id: "test-driver".to_string(),
        display_name: "Test Driver".to_string(),
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

    let adapter = Arc::new(AntigravityAdapter::new());
    let state = AppState::with_mcp_state(adapter, Arc::new(tokio::sync::Mutex::new(driver)), mcp_state);
    let app = create_router(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // 1. Connect to GET /api/mcp/sse
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /api/mcp/sse HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\nConnection: keep-alive\r\n\r\n")
        .await
        .unwrap();

    let mut reader = BufReader::new(stream);

    // Read headers until \r\n\r\n
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        if line == "\r\n" {
            break;
        }
    }

    let endpoint_url = loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        if line.starts_with("data: /api/mcp/messages?session_id=") {
            break line.trim_start_matches("data: ").trim().to_string();
        }
    };

    assert!(!endpoint_url.is_empty(), "Failed to receive endpoint event");
    assert!(endpoint_url.contains("session_id="));

    // 2. Post a JSON-RPC request to the received endpoint
    let rpc_body = json!({
        "jsonrpc": "2.0",
        "id": 100,
        "method": "tools/call",
        "params": {
            "name": "get_architecture_summary",
            "arguments": {}
        }
    })
    .to_string();

    let mut post_stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let post_req = format!(
        "POST {} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        endpoint_url,
        rpc_body.len(),
        rpc_body
    );
    post_stream.write_all(post_req.as_bytes()).await.unwrap();

    // Read POST response (should be 202 Accepted)
    let mut post_reader = BufReader::new(post_stream);
    let mut status_line = String::new();
    post_reader.read_line(&mut status_line).await.unwrap();
    assert!(
        status_line.contains("202 Accepted") || status_line.contains("200 OK"),
        "Unexpected status: {}",
        status_line
    );

    // 3. Read the JSON-RPC response from the SSE stream
    let response_data = loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        if line.starts_with("data: ") {
            let data = line.trim_start_matches("data: ").trim();
            if data.contains("\"jsonrpc\":\"2.0\"") || data.contains("\"result\"") {
                break data.to_string();
            }
        }
    };

    assert!(!response_data.is_empty(), "Expected response over SSE stream");
    let parsed: serde_json::Value = serde_json::from_str(&response_data).unwrap();
    assert_eq!(parsed["id"], 100);
    let text = parsed["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("total_components"));
}

#[tokio::test]
async fn test_mcp_streamable_http_direct_post() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let repo = Arc::new(FsJsonRepository::from_architecture(
        sample_test_architecture(),
        temp_file.path(),
    ));
    let registry = Arc::new(RegistryService::new(repo));
    let mcp_service = Arc::new(McpService::new(registry, None));
    let mcp_state = McpServerState::new(mcp_service);

    let spawner = Arc::new(TokioProcessSpawner::new());
    let profile_dir = tempfile::tempdir().unwrap();
    let installation = Arc::new(
        AntigravityInstallation::new(AntigravityInstallationOptions {
            base_dir: profile_dir.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap(),
    );
    let driver = AntigravityDriver::create(DriverCreateOptions {
        instance_id: "test-driver-http".to_string(),
        display_name: "Test Driver HTTP".to_string(),
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

    let adapter = Arc::new(AntigravityAdapter::new());
    let state = AppState::with_mcp_state(adapter, Arc::new(tokio::sync::Mutex::new(driver)), mcp_state);
    let app = create_router(state);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // 1. Direct POST to /api/mcp/sse with 'initialize'
    let mut stream1 = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body1 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0.0" }
        }
    }).to_string();
    let req1 = format!(
        "POST /api/mcp/sse HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body1.len(),
        body1
    );
    stream1.write_all(req1.as_bytes()).await.unwrap();

    let mut resp1 = String::new();
    stream1.read_to_string(&mut resp1).await.unwrap();
    assert!(resp1.contains("HTTP/1.1 200 OK"));
    assert!(resp1.contains("\"name\":\"architecture-registry\""));

    // 2. Direct POST to /api/mcp/sse with 'tools/list'
    let mut stream2 = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body2 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    }).to_string();
    let req2 = format!(
        "POST /api/mcp/sse HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body2.len(),
        body2
    );
    stream2.write_all(req2.as_bytes()).await.unwrap();

    let mut resp2 = String::new();
    stream2.read_to_string(&mut resp2).await.unwrap();
    assert!(resp2.contains("HTTP/1.1 200 OK"));
    assert!(resp2.contains("\"tools\":["));
}
