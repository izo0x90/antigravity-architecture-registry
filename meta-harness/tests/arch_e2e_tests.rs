//! The architecture REST API and its live events, end to end over HTTP and
//! the event WebSocket, on a copy of the Rust fixture repo.

mod common;

use std::time::Duration;

use arch_graph::{ArchEvent, ChangeReason, Requester};
use common::{http, http_anonymous, spawn_app};
use futures::StreamExt;
use harness_contracts::ClientEvent;
use serde_json::{Value, json};

fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

#[tokio::test]
async fn rest_api_covers_views_plans_checks_contracts_verification_and_approval() {
    let (addr, state) = spawn_app().await;
    let (status, body) = http(addr, "GET", "/api/graph/config", None).await;
    assert_eq!(status, 200);
    assert!(parse(&body)["views"]["calls"]["description"].is_string());

    let (status, body) = http(addr, "GET", "/api/graph/tree?view=calls&root=rust:sample::run&depth=1", None).await;
    assert_eq!(status, 200, "{body}");
    let view = parse(&body);
    assert!(view["cards"].as_array().unwrap().iter().any(|c| c["key"] == "rust:sample::run"));
    let (status, _) = http(addr, "GET", "/api/graph/tree?view=nope", None).await;
    assert_eq!(status, 400);

    let (status, body) = http(addr, "GET", "/api/arch/components/rust:sample::log", None).await;
    assert_eq!(status, 200);
    assert!(parse(&body)["code"].as_str().unwrap().contains("println!"));
    let (status, body) = http(addr, "GET", "/api/arch/at?file=src/store.rs&line=11", None).await;
    assert_eq!(status, 200);
    assert_eq!(parse(&body)["id"], "rust:sample::store::Store::save");
    let (status, body) = http(addr, "GET", "/api/arch/file?path=src/util.rs", None).await;
    assert_eq!(status, 200);
    assert_eq!(parse(&body)["components"].as_array().unwrap().len(), 3);
    // Calls the code makes that couldn't be tied to a component, never guessed.
    let (status, body) = http(addr, "GET", "/api/arch/unproven", None).await;
    assert_eq!(status, 200, "{body}");
    assert!(parse(&body).as_array().unwrap().iter().any(|c| c["caller"] == "rust:sample::store::store_each"), "{body}");
    let (status, body) = http(addr, "GET", "/api/search?q=shout", None).await;
    assert_eq!(status, 200);
    assert_eq!(parse(&body)["hits"][0]["key"], "rust:sample::util::shout");
    let (status, body) = http(addr, "GET", "/api/search?q=util&kind=files", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(parse(&body)["hits"][0]["label"], "src/util.rs");
    let (status, body) = http(addr, "GET", "/api/search?q=to_uppercase&kind=text", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(parse(&body)["hits"][0]["key"], "rust:sample::util::shout", "a line names what it's inside of: {body}");

    // The code viewer's analysis of a file: colours and named places, from what's kept.
    let (status, body) = http(addr, "GET", "/api/code/analysis?path=src/lib.rs", None).await;
    assert_eq!(status, 200, "{body}");
    let code = parse(&body);
    assert_eq!(code["stale"], false);
    assert!(code["highlights"]["names"].as_array().unwrap().iter().any(|n| n == "keyword"), "{body}");
    let named = code["occurrences"].as_array().unwrap();
    assert!(named.iter().any(|o| o["target"] == "rust:sample::run" && o["definition"] == true), "{body}");
    assert!(named.iter().any(|o| o["target"] == "rust:sample::store::Store::save" && o.get("definition").is_none()), "{body}");
    assert!(code["targets"].as_array().unwrap().iter().any(|t| t["id"] == "rust:sample::store::Store::save" && t["signature"].as_str().is_some_and(|s| s.contains("save("))), "{body}");
    // A code block's colours, by the language its fence names; none for a
    // fence no pack knows; too long, refused.
    let (status, body) = http(addr, "POST", "/api/code/highlight", Some(r#"{"language":"rs","text":"fn main() {}"}"#)).await;
    assert_eq!(status, 200, "{body}");
    assert!(parse(&body)["names"].as_array().unwrap().iter().any(|n| n == "keyword"), "{body}");
    let (status, body) = http(addr, "POST", "/api/code/highlight", Some(r#"{"language":"cobol","text":"MOVE A TO B."}"#)).await;
    assert_eq!((status, body.as_str()), (200, "null"));
    let long = json!({ "language": "rust", "text": "x".repeat(300 * 1024) }).to_string();
    let (status, _) = http(addr, "POST", "/api/code/highlight", Some(&long)).await;
    assert_eq!(status, 400);
    // Edited since it was read: nothing it says applies.
    let root = state.project.arch.root().to_path_buf();
    std::fs::write(root.join("src/util.rs"), std::fs::read_to_string(root.join("src/util.rs")).unwrap() + "\n// edited\n").unwrap();
    let (_, body) = http(addr, "GET", "/api/code/analysis?path=src/util.rs", None).await;
    let code = parse(&body);
    assert_eq!((code["stale"].as_bool(), code["occurrences"].as_array().map(Vec::len)), (Some(true), Some(0)), "{body}");

    let (status, body) = http(addr, "POST", "/api/arch/plans", Some(r#"{"id":"val","title":"Validate input"}"#)).await;
    assert_eq!(status, 200, "{body}");
    let ops = json!({ "ops": [
        { "op": "add_component", "name": "validate", "type": "function", "parent": "rust:sample", "description": "Rejects empty input.",
          "signature": { "params": [{ "name": "input", "ty": "&str" }], "returns": "bool" } },
        { "op": "relate", "kind": "calls", "source": "rust:sample::run", "target": "rust:sample::validate" }
    ]})
    .to_string();
    let (status, body) = http(addr, "POST", "/api/arch/plans/val/ops", Some(&ops)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(parse(&body)["components"][0]["proposed"]["file"], "src/lib.rs");
    let (status, body) = http(addr, "POST", "/api/arch/plans/val/ops", Some(r#"{"ops":[{"op":"remove","id":"rust:sample::run"}]}"#)).await;
    assert_eq!(status, 400);
    assert!(parse(&body)["error"].as_str().unwrap().contains("isn't part of this plan"));

    let (status, body) = http(addr, "POST", "/api/arch/plans/val/validate", None).await;
    assert_eq!(status, 200);
    assert_eq!(parse(&body)["findings"], json!([]), "{body}");
    let (status, body) = http(addr, "GET", "/api/arch/plans/val/components/rust:sample::validate/contract", None).await;
    assert_eq!(status, 200);
    assert!(parse(&body)["markdown"].as_str().unwrap().contains("Put it in src/lib.rs"));

    let approve = |to: &str, by: &str| json!({ "to": to, "by": by }).to_string();
    // Without the cockpit's session (an agent with a shell, another site), no approval.
    let (status, body) = http_anonymous(addr, "POST", "/api/arch/plans/val/components/rust:sample::validate/approve", Some(&approve("arch_approved", "izo"))).await;
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("only the user, in the cockpit"));
    // The API never hands out the cockpit's cookie: only the user's launch link does.
    for path in ["/api/ui/config", "/cockpit?nah=guess", "/health"] {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut stream, format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut response = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut stream, &mut response).await.unwrap();
        assert!(!response.to_ascii_lowercase().contains("set-cookie"), "{path} must not hand out the cookie");
    }
    for (method, path) in [
        ("DELETE", "/api/arch/plans/val"),
        ("POST", "/api/arch/plans/val/reviewed"),
        ("POST", "/api/discussions/d1/reply"),
        ("POST", "/api/threads/t1/requests/r1"),
    ] {
        let (status, _) = http_anonymous(addr, method, path, Some(r#"{"by":"izo"}"#)).await;
        assert_eq!(status, 403, "{method} {path}");
    }
    let (status, _) = http(addr, "POST", "/api/arch/plans/val/components/rust:sample::validate/approve", Some(&approve("arch_approved", ""))).await;
    assert_eq!(status, 400, "a user name is required");
    let (status, body) = http(addr, "POST", "/api/arch/plans/val/components/rust:sample::validate/approve", Some(&approve("arch_approved", "izo"))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(parse(&body)["by"], "izo");

    // Write a skeleton into the repo and verify it over REST.
    let lib = state.project.arch.root().join("src/lib.rs");
    let mut text = std::fs::read_to_string(&lib).unwrap();
    text.push_str("\npub fn validate(input: &str) -> bool {\n    todo!()\n}\n");
    std::fs::write(&lib, text).unwrap();
    let (status, body) = http(addr, "POST", "/api/arch/plans/val/components/rust:sample::validate/verify", Some(r#"{"goal":"skeleton"}"#)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(parse(&body)["advanced_to"], "skeleton");
    let (status, body) = http(addr, "POST", "/api/arch/plans/val/components/rust:sample::validate/verify", Some(r#"{"goal":"implemented"}"#)).await;
    assert_eq!(status, 200);
    assert_eq!(parse(&body)["report"]["findings"][0]["code"], "placeholder_remains");

    let (status, body) = http(addr, "POST", "/api/arch/ingest", Some(r#"{"scope":"src"}"#)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(parse(&body)["files"], 3);
    let (status, _) = http(addr, "POST", "/api/arch/ingest", Some(r#"{"scope":"../etc"}"#)).await;
    assert_eq!(status, 400);
    let (status, body) = http(addr, "GET", "/api/arch/plans", None).await;
    assert_eq!(status, 200);
    assert_eq!(parse(&body)[0]["id"], "val");
    let (status, body) = http(addr, "DELETE", "/api/arch/plans/nope", None).await;
    assert_eq!(status, 404, "deleting a plan that doesn't exist: {body}");
    let (status, _) = http(addr, "DELETE", "/api/arch/plans/val", None).await;
    assert_eq!(status, 204);
    let (status, _) = http(addr, "DELETE", "/api/arch/plans/val", None).await;
    assert_eq!(status, 404, "already gone");
}

async fn next_architecture_event(ws: &mut (impl StreamExt<Item = Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>> + Unpin)) -> ArchEvent {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("event in time").unwrap().unwrap();
        if let tokio_tungstenite::tungstenite::Message::Text(text) = msg
            && let Ok(ClientEvent::Architecture { event }) = serde_json::from_str::<ClientEvent>(&text)
        {
            return event;
        }
    }
}

#[tokio::test]
async fn changes_checks_and_navigation_arrive_on_the_event_socket() {
    let (addr, _) = spawn_app().await;
    let mut ws = common::events_socket(addr).await;
    http(addr, "POST", "/api/arch/plans", Some(r#"{"id":"ev","title":"Events"}"#)).await;
    assert!(matches!(next_architecture_event(&mut ws).await, ArchEvent::Changed { reason: ChangeReason::PlanEdited, .. }));
    http(addr, "POST", "/api/arch/plans/ev/validate", None).await;
    assert!(matches!(next_architecture_event(&mut ws).await, ArchEvent::Validated { errors: 0, .. }));
    http(addr, "POST", "/api/arch/ingest", Some("{}")).await;
    // Indexing announces itself (started, progress, done); nothing changed,
    // so no Changed follows.
    let mut index_states = Vec::new();
    loop {
        match next_architecture_event(&mut ws).await {
            ArchEvent::Index { status } => {
                index_states.push(status.state);
                if status.state == arch_graph::IndexState::Idle {
                    break;
                }
            }
            other => panic!("nothing changed, yet {other:?}"),
        }
    }
    assert_eq!(index_states.first(), Some(&arch_graph::IndexState::Indexing));
    let (status, _) = http(addr, "POST", "/api/arch/navigate", Some(r#"{"view":"calls","select":"rust:sample::run"}"#)).await;
    assert_eq!(status, 204);
    match next_architecture_event(&mut ws).await {
        ArchEvent::ViewRequested { request, by } => {
            assert_eq!(by, Requester::User);
            assert_eq!(request.select.as_deref(), Some("rust:sample::run"));
        }
        other => panic!("{other:?}"),
    }
    // A client that connects later lands on the last requested view.
    let mut late = common::events_socket(addr).await;
    assert!(matches!(next_architecture_event(&mut late).await, ArchEvent::ViewRequested { .. }));
}
