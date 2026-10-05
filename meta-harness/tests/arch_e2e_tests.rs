//! The architecture REST API and its live events, end to end over HTTP and
//! the event WebSocket, on a copy of the Rust fixture repo.

mod common;

use std::time::Duration;

use arch_graph::{ArchEvent, ChangeReason, Requester};
use common::{http, http_cockpit, spawn_app};
use futures::StreamExt;
use harness_contracts::ClientEvent;
use serde_json::{Value, json};

fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

// req: P8.1
#[tokio::test]
async fn rest_api_covers_views_plans_checks_contracts_verification_and_approval() {
    let (addr, state) = spawn_app().await;
    let (status, body) = http(addr, "GET", "/api/arch/views", None).await;
    assert_eq!(status, 200);
    assert!(parse(&body)["views"]["calls"]["description"].is_string());

    let (status, body) = http(addr, "GET", "/api/arch/view?name=calls&roots=rust:sample::run&depth=1", None).await;
    assert_eq!(status, 200, "{body}");
    let view = parse(&body);
    assert!(view["text"].as_str().unwrap().contains("run [function"));
    assert!(view["layout"]["rects"]["rust:sample::run"]["w"].is_number());
    let (status, _) = http(addr, "GET", "/api/arch/view?name=nope", None).await;
    assert_eq!(status, 404);

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
    let (status, body) = http(addr, "GET", "/api/arch/search?q=shout", None).await;
    assert_eq!(status, 200);
    assert_eq!(parse(&body)[0]["id"], "rust:sample::util::shout");

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
    let (status, body) = http(addr, "POST", "/api/arch/plans/val/components/rust:sample::validate/approve", Some(&approve("arch_approved", "izo"))).await;
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("only the user, in the cockpit"));
    // The API never hands out the cockpit's cookie: only the user's launch link does.
    for path in ["/api/ui/config", "/cockpit?nah=guess", "/health"] {
        let (status, _) = http(addr, "GET", path, None).await;
        assert_ne!(status, 303, "{path}");
    }
    for (method, path) in [
        ("DELETE", "/api/arch/plans/val"),
        ("POST", "/api/arch/plans/val/reviewed"),
        ("POST", "/api/arch/plans/val/comments/c1/replies"),
        ("POST", "/api/threads/t1/requests/r1"),
    ] {
        let (status, _) = http(addr, method, path, Some(r#"{"by":"izo"}"#)).await;
        assert_eq!(status, 403, "{method} {path}");
    }
    let (status, _) = http_cockpit(addr, "POST", "/api/arch/plans/val/components/rust:sample::validate/approve", Some(&approve("arch_approved", ""))).await;
    assert_eq!(status, 400, "a user name is required");
    let (status, body) = http_cockpit(addr, "POST", "/api/arch/plans/val/components/rust:sample::validate/approve", Some(&approve("arch_approved", "izo"))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(parse(&body)["by"], "izo");

    // Write a skeleton into the repo and verify it over REST.
    let lib = state.arch.root().join("src/lib.rs");
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
    let (status, body) = http_cockpit(addr, "DELETE", "/api/arch/plans/nope", None).await;
    assert_eq!(status, 404, "deleting a plan that doesn't exist: {body}");
    let (status, _) = http_cockpit(addr, "DELETE", "/api/arch/plans/val", None).await;
    assert_eq!(status, 204);
    let (status, _) = http_cockpit(addr, "DELETE", "/api/arch/plans/val", None).await;
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

// req: P8.2
#[tokio::test]
async fn changes_checks_and_navigation_arrive_on_the_event_socket() {
    let (addr, _) = spawn_app().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events")).await.unwrap();
    http(addr, "POST", "/api/arch/plans", Some(r#"{"id":"ev","title":"Events"}"#)).await;
    assert!(matches!(next_architecture_event(&mut ws).await, ArchEvent::Changed { reason: ChangeReason::PlanEdited, .. }));
    http(addr, "POST", "/api/arch/plans/ev/validate", None).await;
    assert!(matches!(next_architecture_event(&mut ws).await, ArchEvent::Validated { errors: 0, .. }));
    http(addr, "POST", "/api/arch/ingest", Some("{}")).await;
    assert!(matches!(next_architecture_event(&mut ws).await, ArchEvent::Changed { reason: ChangeReason::Ingested, .. }));
    let (status, _) = http(addr, "POST", "/api/arch/navigate", Some(r#"{"view":"calls","focus":"rust:sample::run"}"#)).await;
    assert_eq!(status, 204);
    match next_architecture_event(&mut ws).await {
        ArchEvent::ViewRequested { request, by } => {
            assert_eq!(by, Requester::User);
            assert_eq!(request.focus.unwrap().as_str(), "rust:sample::run");
        }
        other => panic!("{other:?}"),
    }
    // A client that connects later lands on the last requested view.
    let (mut late, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events")).await.unwrap();
    assert!(matches!(next_architecture_event(&mut late).await, ArchEvent::ViewRequested { .. }));
}
