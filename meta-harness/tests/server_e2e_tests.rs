mod common;

use std::time::Duration;

use common::{FAKE, http, http_anonymous, spawn_app};
use harness_contracts::{AdapterEvent, ClientEvent, ContentBlock, ItemBody, ProviderSnapshot};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn app() -> (std::net::SocketAddr, meta_harness::modules::AppState) {
    spawn_app().await
}

#[tokio::test]
async fn health_and_instance_listing() {
    let (addr, _) = app().await;
    let (status, body) = http(addr, "GET", "/health", None).await;
    assert_eq!(status, 200);
    assert!(body.contains("\"status\":\"ok\""));

    let (status, body) = http(addr, "GET", "/api/instances", None).await;
    assert_eq!(status, 200);
    let instances: Vec<ProviderSnapshot> = serde_json::from_str(&body).unwrap();
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].instance_id.as_str(), FAKE);
}

#[tokio::test]
async fn a_slots_session_routes_through_the_instance_adapter() {
    let (addr, state) = app().await;

    let (status, body) = http(addr, "POST", "/api/slots/lead/sessions", Some(r#"{"instanceId":"fake"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let opened: serde_json::Value = serde_json::from_str(&body).unwrap();
    let (thread_id, session) = (opened["threadId"].as_str().unwrap().to_owned(), opened["session"].as_str().unwrap().to_owned());

    let (status, body) = http(addr, "POST", "/api/slots/lead/prompt", Some(r#"{"text":"hi"}"#)).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"turnId\":"), "{body}");

    // Unsupported operations surface as 501, unknown threads and slots as 404.
    let (status, _) = http(addr, "POST", &format!("/api/threads/{thread_id}/steer"), Some("{}")).await;
    assert_eq!(status, 501);
    let (status, _) = http(addr, "POST", "/api/threads/nope/interrupt", None).await;
    assert_eq!(status, 404);
    let (status, _) = http(addr, "POST", "/api/slots/nope/prompt", Some(r#"{"text":"x"}"#)).await;
    assert_eq!(status, 404);

    // The hub replays the session's events and the catalog folded into the snapshot.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (replay, _) = state.harness.hub.subscribe();
    assert!(replay.iter().any(|e| matches!(&e.event, ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } }
        if matches!(&item.body, ItemBody::AssistantMessage { content, .. }
            if content == &vec![ContentBlock::Text { text: "echo: hi".into() }]))));
    assert!(replay.iter().any(|e| matches!(&e.event, ClientEvent::InstanceUpdated { snapshot }
        if snapshot.commands.iter().any(|c| c.name == "plan"))));

    let (status, _) = http(addr, "DELETE", &format!("/api/slots/lead/sessions/{session}"), None).await;
    assert_eq!(status, 204);
    let (status, _) = http(addr, "POST", &format!("/api/threads/{thread_id}/interrupt"), None).await;
    assert_eq!(status, 404, "the session ended");
}

#[tokio::test]
async fn opening_a_session_on_an_unknown_instance_is_404() {
    let (addr, _) = app().await;
    let (status, _) = http(addr, "POST", "/api/slots/lead/sessions", Some(r#"{"instanceId":"missing"}"#)).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn websocket_upgrade_and_replay() {
    let (addr, _) = app().await;
    let ticket = common::ticket(addr).await;
    let ws_req = format!("GET /ws/events?ticket={ticket} HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n");
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(ws_req.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 8192];
    let mut received = Vec::new();
    // Handshake plus the first replayed frame (the instance snapshot).
    while !String::from_utf8_lossy(&received).contains("instance_updated") {
        let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await.unwrap().unwrap();
        assert!(n > 0, "connection closed early");
        received.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&received);
    assert!(text.contains("HTTP/1.1 101 Switching Protocols"));
}

#[tokio::test]
async fn fs_read_endpoint_reads_only_inside_the_repository() {
    let (addr, state) = app().await;
    let (status, body) = http(addr, "GET", "/api/fs/read?path=Cargo.toml", None).await;
    assert_eq!(status, 200);
    assert!(body.contains("sample"), "the repository's own manifest: {body}");
    assert!(body.contains("\"lines\":"));
    let absolute = state.project.arch.root().join("src/lib.rs");
    let (status, _) = http(addr, "GET", &format!("/api/fs/read?path={}", absolute.display()), None).await;
    assert_eq!(status, 200, "absolute paths inside the repo are fine");
    for outside in ["/etc/passwd", "../../../../etc/passwd", "../sample/../../outside.txt"] {
        let (status, _) = http(addr, "GET", &format!("/api/fs/read?path={outside}"), None).await;
        assert!(status == 403 || status == 404, "{outside}: {status}");
    }
    let (status, body) = http(addr, "GET", "/api/fs/read?path=/etc/hostname", None).await;
    assert_eq!(status, 403, "{body}");
    // The server's own: a harness's kept login, its state.
    let root = state.project.arch.root();
    std::fs::create_dir_all(root.join(".nah/harness/antigravity/antigravity-acp")).unwrap();
    std::fs::write(root.join(".nah/harness/antigravity/antigravity-acp/acp_token.json"), "{\"refresh_token\":\"x\"}").unwrap();
    for private in [".nah/harness/antigravity/antigravity-acp/acp_token.json", ".nah/harness/../harness/antigravity/antigravity-acp/acp_token.json"] {
        let (status, body) = http(addr, "GET", &format!("/api/fs/read?path={private}"), None).await;
        assert_eq!(status, 403, "{private}: {body}");
        assert!(!body.contains("refresh_token"));
    }
}

#[tokio::test]
async fn dev_feedback_endpoint() {
    let (addr, _) = app().await;
    let feedback = r#"{"category":"Bug","comment":"Testing feedback collector","target":{"type":"code","file":"Cargo.toml"}}"#;
    let (status, body) = http(addr, "POST", "/api/dev/feedback", Some(feedback)).await;
    assert_eq!(status, 200);
    assert!(body.contains("\"status\":\"saved\""));
    assert!(body.contains("\"total_feedback_count\":1"), "{body}");
    let id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"].as_str().unwrap().to_owned();
    let updated = format!(r#"{{"id":"{id}","category":"Bug","comment":"Fixed","target":{{}},"status":"done"}}"#);
    assert_eq!(http(addr, "PUT", "/api/dev/feedback", Some(&updated)).await.0, 200);
    let (_, list) = http(addr, "GET", "/api/dev/feedback", None).await;
    let list: Vec<serde_json::Value> = serde_json::from_str(&list).unwrap();
    assert_eq!((list.len(), list[0]["status"].as_str()), (1, Some("done")), "kept in the project's state, updated in place");
    assert!(!std::path::Path::new("dev_feedback.json").exists(), "no file wherever the server was started");
}

#[tokio::test]
async fn static_index_boots_the_wasm_bundle() {
    let (addr, _) = app().await;
    let (status, body) = http(addr, "GET", "/index.html", None).await;
    assert_eq!(status, 200);
    assert!(body.contains("<title>NAH</title>"));
    assert!(body.contains("/wasm/cockpit_ui.js"));
}

#[tokio::test]
async fn ui_config_endpoint_answers_with_the_user_file_or_null() {
    let (addr, _) = app().await;
    let (status, body) = http(addr, "GET", "/api/ui/config", None).await;
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(value["text"].is_null() || value["text"].is_string());
}

#[tokio::test]
async fn the_projects_tool_servers_are_kept_in_its_file_and_only_the_cockpit_changes_them() {
    let (addr, state) = app().await;
    let list = r#"[{"name":"docs","transport":{"type":"http","url":"https://h/mcp","headers":[]}}]"#;
    let (status, _) = http_anonymous(addr, "PUT", "/api/mcp/servers", Some(list)).await;
    assert!(status == 401 || status == 403, "not the cockpit: {status}");
    let (status, body) = common::http(addr, "PUT", "/api/mcp/servers", Some(list)).await;
    assert_eq!(status, 200, "{body}");
    let (_, body) = http(addr, "GET", "/api/mcp/servers", None).await;
    let servers: harness_contracts::ToolServers = serde_json::from_str(&body).unwrap();
    assert_eq!(servers.project.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["docs"]);
    let file = std::fs::read_to_string(state.project.arch.root().join(".nah/mcp.toml")).unwrap();
    assert!(file.contains("name = \"docs\"") && file.contains("url = \"https://h/mcp\""), "{file}");
    common::http(addr, "PUT", "/api/mcp/servers", Some("[]")).await;
}

#[tokio::test]
async fn adding_to_a_running_turn_is_refused_by_a_harness_that_cant() {
    let (addr, _) = app().await;
    let (status, body) = http(addr, "POST", "/api/slots/lead/sessions", Some(r#"{"instanceId":"fake"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = http(addr, "POST", "/api/slots/lead/prompt", Some(r#"{"text":"also","addToRunning":true}"#)).await;
    assert_eq!(status, 501);
}
