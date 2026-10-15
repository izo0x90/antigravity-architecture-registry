//! MCP server over the architecture service. Every tool result ends with next
//! steps derived from the state, so the agent knows what the workflow expects
//! without relying on instructions it may drift from.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use arch_service::{ArchitectureService, ServiceError};
use nah_access::{Access, Capability, Principal};
use serde_json::{Value, json};

use crate::mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpTool, McpToolResult};
use crate::mcp::sets::{ToolDef, ToolSet, all, tool_defs};

/// Who a tool call comes from: a slot's agent (by its session's key), or
/// an agent NAH didn't start.
#[derive(Clone, Debug, PartialEq)]
pub struct McpCaller {
    pub principal: Principal,
    pub slot: Option<harness_contracts::SlotId>,
    /// How its writing is signed: the slot's name, or "external".
    pub label: String,
}

impl McpCaller {
    pub fn external() -> Self {
        Self { principal: Principal::agent("external"), slot: None, label: "external".into() }
    }
}

#[derive(Clone)]
pub struct McpService {
    sets: Arc<Vec<Box<dyn ToolSet>>>,
    /// Every tool with what it needs, in the list's order.
    defs: Arc<Vec<ToolDef>>,
    /// Which set each tool is in, and what it needs.
    index: Arc<HashMap<String, (usize, Capability)>>,
    access: Arc<Access>,
    /// The list each set of capabilities sees, made the first time it's asked for.
    lists: Arc<Mutex<HashMap<u16, Arc<Vec<McpTool>>>>>,
}

impl McpService {
    pub fn new(arch: Arc<ArchitectureService>, remote: Arc<crate::remote::Remote>, access: Arc<Access>, slots: Arc<harness_core::SlotManager>) -> Self {
        let sets = all(&arch, &remote, &slots);
        let mut index = HashMap::new();
        let mut names = Vec::new();
        for (i, set) in sets.iter().enumerate() {
            for def in set.tools(arch.trees()) {
                assert!(index.insert(def.tool.name.clone(), (i, def.needs)).is_none(), "two tools are named {}", def.tool.name);
                names.push(def.tool.name);
            }
        }
        let defs = tool_defs(arch.trees());
        let listed: Vec<&str> = defs.iter().map(|d| d.tool.name.as_str()).collect();
        assert_eq!(names, listed, "the sets and the tool list name the same tools in the same order");
        Self { sets: Arc::new(sets), defs: Arc::new(defs), index: Arc::new(index), access, lists: Arc::default() }
    }

    /// The tools a caller may use, in the list's order.
    pub fn tools_for(&self, caller: &Principal) -> Arc<Vec<McpTool>> {
        let can = self.access.of(caller);
        let mut lists = self.lists.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        lists.entry(can.bits()).or_insert_with(|| Arc::new(self.defs.iter().filter(|d| can.contains(d.needs)).map(|d| d.tool.clone()).collect())).clone()
    }

    /// Answers a request; notifications (no id) get no answer, as JSON-RPC requires.
    pub async fn handle_request(&self, req: JsonRpcRequest, caller: &McpCaller) -> Option<JsonRpcResponse> {
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
            "tools/list" => JsonRpcResponse::success(req_id, json!({ "tools": *self.tools_for(&caller.principal) })),
            "tools/call" => {
                let params = req.params.unwrap_or(Value::Null);
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let result = match self.call(&name, args, caller).await {
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

    /// Runs a tool for a caller allowed to; the text is what the agent sees.
    pub async fn call(&self, name: &str, args: Value, caller: &McpCaller) -> Result<String, ServiceError> {
        let Some(&(set, needs)) = self.index.get(name) else {
            return Err(ServiceError::NotFound(format!("tool '{name}'")));
        };
        if !self.access.allows(&caller.principal, needs) {
            let role = match &caller.principal {
                Principal::Agent { role } => role.to_string(),
                other => format!("{other:?}").to_lowercase(),
            };
            return Err(ServiceError::Forbidden(format!("this session's role ({role}) can't use {name} (it needs {needs})")));
        }
        self.sets[set].call(name, args, caller).await
    }
}
