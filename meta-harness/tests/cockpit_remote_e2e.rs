//! Agents driving the cockpit: the cockpit tools reach the open cockpit
//! over its socket (only the cockpit can open it) and bring its answers back.

mod common;

use std::time::Duration;

use common::{Mcp, spawn_app};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

async fn tool(mcp: &mut Mcp, name: &str, arguments: Value) -> (bool, String) {
    let result = mcp.rpc("tools/call", json!({ "name": name, "arguments": arguments })).await;
    let result = &result["result"];
    (result["isError"] == true, result["content"][0]["text"].as_str().unwrap_or_default().to_owned())
}

#[tokio::test]
async fn the_cockpit_tools_reach_the_cockpit_only_while_it_lets_agents_drive() {
    let (addr, _) = spawn_app().await;
    let mut mcp = Mcp::connect(addr).await;
    let (failed, text) = tool(&mut mcp, "cockpit_read", json!({})).await;
    assert!(failed && text.contains("Let agents drive the cockpit"), "no cockpit yet: {text}");

    // Only the cockpit opens its socket: with a ticket only its session gets.
    let refused = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/cockpit")).await;
    assert!(refused.is_err(), "no ticket, no socket");
    let (status, _) = common::http_anonymous(addr, "POST", "/api/cockpit/ticket", None).await;
    assert_eq!(status, 403, "tickets are the cockpit's");
    let ticket = common::ticket(addr).await;
    let (mut cockpit, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/cockpit?ticket={ticket}")).await.unwrap();
    assert!(tokio_tungstenite::connect_async(format!("ws://{addr}/ws/cockpit?ticket={ticket}")).await.is_err(), "a ticket is good once");

    // The cockpit answers: the call it got, and the screen.
    let answering = tokio::spawn(async move {
        let mut calls = Vec::new();
        while let Some(Ok(Message::Text(text))) = cockpit.next().await {
            let message: Value = serde_json::from_str(&text).unwrap();
            calls.push(message["call"].clone());
            let answer = match message["call"]["op"].as_str() {
                Some("read") => json!({ "id": message["id"], "ok": {
                    "mode": "Normal", "palette": false, "note": null, "leader": null,
                    "columns": [{ "column": 1, "view": "chat · the agent's session", "focused": true }],
                    "texts": [{ "text": "Connect", "x": 520.0, "y": 90.0 }]
                } }),
                _ => json!({ "id": message["id"], "error": "\"Nowhere\" isn't on screen (read shows what is)" }),
            };
            cockpit.send(Message::Text(answer.to_string().into())).await.unwrap();
            if calls.len() == 2 {
                return calls;
            }
        }
        calls
    });
    let (failed, text) = tool(&mut mcp, "cockpit_read", json!({})).await;
    assert!(!failed && text.contains("column 1: chat · the agent's session (focused)") && text.contains("Connect @520"), "{text}");
    let (failed, text) = tool(&mut mcp, "cockpit_click", json!({ "text": "Nowhere" })).await;
    assert!(failed && text.contains("isn't on screen"), "the cockpit's own reason: {text}");
    let calls = tokio::time::timeout(Duration::from_secs(5), answering).await.unwrap().unwrap();
    assert_eq!(calls, [json!({ "op": "read" }), json!({ "op": "click", "text": "Nowhere" })]);

    let tools = mcp.rpc("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).filter(|n| n.starts_with("cockpit_")).collect();
    assert_eq!(names, ["cockpit_read", "cockpit_actions", "cockpit_run", "cockpit_click", "cockpit_type", "cockpit_keys"]);
}
