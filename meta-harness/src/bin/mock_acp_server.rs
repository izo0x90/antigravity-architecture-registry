use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out_handle = stdout.lock();

    let mut last_mcp_servers = serde_json::Value::Null;

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(msg) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        let msg_id = msg.get("id");
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        match method {
            "initialize" => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": { "protocolVersion": 1 }
                });
                let _ = writeln!(out_handle, "{}", resp);
                let _ = out_handle.flush();
            }
            "authenticate" => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": {}
                });
                let _ = writeln!(out_handle, "{}", resp);
                let _ = out_handle.flush();
            }
            "session/new" => {
                if let Some(params) = msg.get("params")
                    && let Some(servers) = params.get("mcpServers")
                {
                    last_mcp_servers = servers.clone();
                }
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": { "sessionId": "real-rust-proc-sess-100" }
                });
                let _ = writeln!(out_handle, "{}", resp);
                let _ = out_handle.flush();
            }
            "session/set_mode" | "session/set_model" => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": {}
                });
                let _ = writeln!(out_handle, "{}", resp);
                let _ = out_handle.flush();
            }
            "session/prompt" => {
                // Stream a text delta event first
                let stream_text = if !last_mcp_servers.is_null()
                    && last_mcp_servers.as_array().map(|a| !a.is_empty()).unwrap_or(false)
                {
                    format!("Real compiled Rust process with MCP: {}", last_mcp_servers)
                } else {
                    "Real compiled Rust process streaming response".to_string()
                };

                let evt = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": "real-rust-proc-sess-100",
                        "update": {
                            "_tag": "ContentDelta",
                            "text": stream_text
                        }
                    }
                });
                let _ = writeln!(out_handle, "{}", evt);
                let _ = out_handle.flush();

                // Then complete the prompt request
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": { "stopReason": "end_turn" }
                });
                let _ = writeln!(out_handle, "{}", resp);
                let _ = out_handle.flush();
            }
            "ping" => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": { "status": "ok" },
                    "error": null
                });
                let _ = writeln!(out_handle, "{}", resp);
                let _ = out_handle.flush();
            }
            "session/cancel" => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": {}
                });
                let _ = writeln!(out_handle, "{}", resp);
                let _ = out_handle.flush();
            }
            _ => {
                if let Some(id) = msg_id {
                    let resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {}
                    });
                    let _ = writeln!(out_handle, "{}", resp);
                    let _ = out_handle.flush();
                }
            }
        }
    }
}
