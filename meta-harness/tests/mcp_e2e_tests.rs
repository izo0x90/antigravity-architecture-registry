mod common;

use std::sync::Arc;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use architecture_model::{ComponentSpec, SystemArchitecture, UsageNode};

use meta_harness::mcp::protocol::JsonRpcRequest;
use meta_harness::mcp::{McpServerState, McpService};
use meta_harness::registry::{FsJsonRepository, RegistryService};

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
async fn test_mcp_service_tools() {
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let repo = Arc::new(FsJsonRepository::from_architecture(
        sample_test_architecture(),
        temp_file.path(),
    ));
    let registry = Arc::new(RegistryService::new(repo));
    let mcp = McpService::new(registry);

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
}


#[tokio::test]
async fn test_mcp_sse_http_server_flow() {
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let repo = Arc::new(FsJsonRepository::from_architecture(
        sample_test_architecture(),
        temp_file.path(),
    ));
    let registry = Arc::new(RegistryService::new(repo));

    let mcp_service = Arc::new(McpService::new(registry));
    let mcp_state = McpServerState::new(mcp_service);

    let (addr, _state) = common::spawn_app(mcp_state).await;

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
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let repo = Arc::new(FsJsonRepository::from_architecture(
        sample_test_architecture(),
        temp_file.path(),
    ));
    let registry = Arc::new(RegistryService::new(repo));
    let mcp_service = Arc::new(McpService::new(registry));
    let mcp_state = McpServerState::new(mcp_service);

    let (addr, _state) = common::spawn_app(mcp_state).await;

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

#[tokio::test]
async fn test_registry_architecture_endpoint_serves_registry_state() {
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let repo = Arc::new(FsJsonRepository::from_architecture(
        sample_test_architecture(),
        temp_file.path(),
    ));
    let registry = Arc::new(RegistryService::new(repo));
    let mcp_service = Arc::new(McpService::new(registry));
    let mcp_state = McpServerState::new(mcp_service);

    let (addr, _state) = common::spawn_app(mcp_state).await;

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /api/registry/architecture HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.contains("HTTP/1.1 200 OK"));

    let body = response.split("\r\n\r\n").nth(1).unwrap();
    let arch: SystemArchitecture = serde_json::from_str(body).unwrap();
    assert_eq!(arch, sample_test_architecture());
}
