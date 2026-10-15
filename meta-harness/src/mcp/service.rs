//! MCP server over the architecture service. Every tool result ends with next
//! steps derived from the state, so the agent knows what the workflow expects
//! without relying on instructions it may drift from.

use std::collections::HashMap;
use std::sync::Arc;

use arch_service::{ArchitectureService, ServiceError};
use serde_json::{Value, json};

use crate::mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolResult};
use crate::mcp::sets::{ToolSet, all, list_tools};

#[derive(Clone)]
pub struct McpService {
    arch: Arc<ArchitectureService>,
    sets: Arc<Vec<Box<dyn ToolSet>>>,
    /// Which set each tool is in.
    index: Arc<HashMap<String, usize>>,
}

impl McpService {
    pub fn new(arch: Arc<ArchitectureService>, remote: Arc<crate::remote::Remote>) -> Self {
        let sets = all(&arch, &remote);
        let mut index = HashMap::new();
        let mut names = Vec::new();
        for (i, set) in sets.iter().enumerate() {
            for tool in set.tools(arch.trees()) {
                assert!(index.insert(tool.name.clone(), i).is_none(), "two tools are named {}", tool.name);
                names.push(tool.name);
            }
        }
        let listed: Vec<String> = list_tools(arch.trees()).into_iter().map(|t| t.name).collect();
        assert_eq!(names, listed, "the sets and the tool list name the same tools in the same order");
        Self { arch, sets: Arc::new(sets), index: Arc::new(index) }
    }

    /// Answers a request; notifications (no id) get no answer, as JSON-RPC requires.
    pub async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let req_id = req.id.clone();
        let response = match req.method.as_str() {
            "initialize" => JsonRpcResponse::success(
                req_id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "nah-architecture", "version": env!("CARGO_PKG_VERSION") }
                }),
            ),
            "notifications/initialized" | "initialized" | "ping" => JsonRpcResponse::success(req_id, json!({})),
            "tools/list" => JsonRpcResponse::success(req_id, json!({ "tools": list_tools(self.arch.trees()) })),
            "tools/call" => {
                let params = req.params.unwrap_or(Value::Null);
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let result = match self.call(&name, args).await {
                    Ok(text) => McpToolResult::text(text),
                    Err(err) => McpToolResult::error(err.to_string()),
                };
                match serde_json::to_value(&result) {
                    Ok(val) => JsonRpcResponse::success(req_id, val),
                    Err(e) => JsonRpcResponse::error(req_id, -32603, format!("Internal error serializing tool response: {e}"), None),
                }
            }
            other => JsonRpcResponse::error(req_id, -32601, format!("Method not found: '{other}'"), None),
        };
        req.id.is_some().then_some(response)
    }

    /// Runs a tool; the text is what the agent sees.
    pub async fn call(&self, name: &str, args: Value) -> Result<String, ServiceError> {
        match self.index.get(name) {
            Some(&set) => self.sets[set].call(name, args).await,
            None => Err(ServiceError::NotFound(format!("tool '{name}'"))),
        }
    }
}
