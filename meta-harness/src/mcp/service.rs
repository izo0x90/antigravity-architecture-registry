use std::sync::Arc;
use tokio::sync::broadcast;
use serde_json::{json, Value};
use harness_protocol::ProviderRuntimeEvent;
use harness_protocol::arch::{
    ComponentFilter, GraphPerspective, SubtreeQuery, TaskSpec,
};
use crate::registry::service::RegistryService;
use crate::mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolResult};
use crate::mcp::tools::list_registry_tools;

/// In-process MCP server executing against RegistryService and dispatching
/// architecture lifecycle and focus events to the UI.
#[derive(Clone)]
pub struct McpService {
    registry: Arc<RegistryService>,
    event_tx: Option<broadcast::Sender<ProviderRuntimeEvent>>,
}

impl McpService {
    pub fn new(
        registry: Arc<RegistryService>,
        event_tx: Option<broadcast::Sender<ProviderRuntimeEvent>>,
    ) -> Self {
        Self { registry, event_tx }
    }

    pub fn event_sender(&self) -> Option<&broadcast::Sender<ProviderRuntimeEvent>> {
        self.event_tx.as_ref()
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let req_id = req.id.clone();

        match req.method.as_str() {
            "initialize" => {
                JsonRpcResponse::success(
                    req_id,
                    json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {
                            "tools": {}
                        },
                        "serverInfo": {
                            "name": "architecture-registry",
                            "version": "0.1.0"
                        }
                    }),
                )
            }
            "notifications/initialized" | "initialized" => {
                JsonRpcResponse::success(req_id, json!({}))
            }
            "ping" => {
                JsonRpcResponse::success(req_id, json!({}))
            }
            "tools/list" => {
                let tools = list_registry_tools();
                JsonRpcResponse::success(req_id, json!({ "tools": tools }))
            }
            "tools/call" => {
                let params = req.params.unwrap_or(Value::Null);
                let tool_name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));

                let result = self.execute_tool(tool_name, args).await;
                match serde_json::to_value(&result) {
                    Ok(val) => JsonRpcResponse::success(req_id, val),
                    Err(e) => JsonRpcResponse::error(
                        req_id,
                        -32603,
                        format!("Internal error serializing tool response: {e}"),
                        None,
                    ),
                }
            }
            other => JsonRpcResponse::error(
                req_id,
                -32601,
                format!("Method not found: '{other}'"),
                None,
            ),
        }
    }

    async fn execute_tool(&self, name: &str, args: Value) -> McpToolResult {
        match name {
            "search_components" => {
                let mut filter = ComponentFilter::default();
                if let Some(q) = args.get("query").and_then(|v| v.as_str()) {
                    filter.query = Some(q.to_string());
                }
                if let Some(s) = args.get("status").and_then(|v| v.as_str()) {
                    filter.status = Some(s.to_string());
                }
                if let Some(st) = args.get("stage").and_then(|v| v.as_str()) {
                    filter.stage = Some(st.to_string());
                }
                if let Some(ct) = args.get("comp_type").and_then(|v| v.as_str()) {
                    filter.comp_type = Some(ct.to_string());
                }
                if let Some(t) = args.get("tag").and_then(|v| v.as_str()) {
                    filter.tag = Some(t.to_string());
                }

                match self.registry.find_components(&filter).await {
                    Ok(summaries) => match serde_json::to_string_pretty(&summaries) {
                        Ok(json_str) => McpToolResult::text(json_str),
                        Err(e) => McpToolResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpToolResult::error(format!("Registry search error: {e}")),
                }
            }
            "inspect_component" => {
                let comp_id = match args.get("component_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return McpToolResult::error("Missing required parameter 'component_id'"),
                };

                match self.registry.inspect_component(comp_id).await {
                    Ok(spec) => match serde_json::to_string_pretty(&spec) {
                        Ok(json_str) => McpToolResult::text(json_str),
                        Err(e) => McpToolResult::error(format!("Serialization error: {e}")),
                    },
                    Err(e) => McpToolResult::error(format!("Component inspection error: {e}")),
                }
            }
            "get_architecture_summary" => match self.registry.get_summary().await {
                Ok(summary) => match serde_json::to_string_pretty(&summary) {
                    Ok(json_str) => McpToolResult::text(json_str),
                    Err(e) => McpToolResult::error(format!("Serialization error: {e}")),
                },
                Err(e) => McpToolResult::error(format!("Summary error: {e}")),
            },
            "focus_architecture" => {
                let root_id = match args.get("root_id").and_then(|v| v.as_str()) {
                    Some(id) => id.to_string(),
                    None => return McpToolResult::error("Missing required parameter 'root_id'"),
                };

                let perspective = match args.get("perspective").and_then(|v| v.as_str()) {
                    Some("component_hierarchy") => GraphPerspective::ComponentHierarchy,
                    Some("data_flow") => GraphPerspective::DataFlow,
                    Some("unified") => GraphPerspective::Unified,
                    _ => GraphPerspective::CallFlow,
                };

                let depth = args
                    .get("depth")
                    .and_then(|v| v.as_u64())
                    .map(|d| d as usize)
                    .unwrap_or(2);

                let query = SubtreeQuery {
                    root_id: root_id.clone(),
                    perspective,
                    depth,
                };

                match self.registry.get_subtree(&query).await {
                    Ok((sub_graph, textual_tree)) => {
                        let affected_components: Vec<String> = sub_graph.nodes.keys().cloned().collect();

                        // Emit ArchitectureFocused event to the shared event bus
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx.send(ProviderRuntimeEvent::ArchitectureFocused {
                                root_id: root_id.clone(),
                                depth,
                                direction: format!("{:?}", perspective),
                                affected_components,
                            });
                        }

                        let response_text = format!(
                            "{}\n\n[Subtree slice extracted: {} nodes, {} edges]",
                            textual_tree,
                            sub_graph.nodes.len(),
                            sub_graph.edges.len()
                        );
                        McpToolResult::text(response_text)
                    }
                    Err(e) => McpToolResult::error(format!("Focus architecture error: {e}")),
                }
            }
            "approve_component" => {
                let comp_id = match args.get("component_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return McpToolResult::error("Missing required parameter 'component_id'"),
                };

                match self.registry.approve_architecture(comp_id).await {
                    Ok(comp) => {
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx.send(ProviderRuntimeEvent::ArchitectureUpdated {
                                component_id: comp.id.clone(),
                                status: comp.stage.clone(),
                            });
                        }
                        McpToolResult::text(format!(
                            "Component '{}' approved successfully (stage: '{}').",
                            comp.id, comp.stage
                        ))
                    }
                    Err(e) => McpToolResult::error(format!("Approval failed: {e}")),
                }
            }
            "plan_component" => {
                let comp_id = match args.get("component_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return McpToolResult::error("Missing required parameter 'component_id'"),
                };
                let plan = match args.get("plan") {
                    Some(p) => p.clone(),
                    None => return McpToolResult::error("Missing required parameter 'plan'"),
                };

                let mut tasks = Vec::new();
                if let Some(steps) = plan.get("steps").and_then(|v| v.as_array()) {
                    for (i, step) in steps.iter().enumerate() {
                        if let Some(s) = step.as_str() {
                            tasks.push(TaskSpec {
                                id: Some(format!("task_{}", i + 1)),
                                task: s.to_string(),
                                completed: false,
                                subtasks: vec![],
                            });
                        }
                    }
                }

                match self.registry.plan_component(comp_id, plan, tasks).await {
                    Ok(comp) => {
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx.send(ProviderRuntimeEvent::ArchitectureUpdated {
                                component_id: comp.id.clone(),
                                status: comp.stage.clone(),
                            });
                        }
                        McpToolResult::text(format!(
                            "Plan recorded for component '{}' (stage: '{}').",
                            comp.id, comp.stage
                        ))
                    }
                    Err(e) => McpToolResult::error(format!("Planning failed: {e}")),
                }
            }
            "implement_component" => {
                let comp_id = match args.get("component_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return McpToolResult::error("Missing required parameter 'component_id'"),
                };

                match self.registry.implement_component(comp_id).await {
                    Ok(comp) => {
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx.send(ProviderRuntimeEvent::ArchitectureUpdated {
                                component_id: comp.id.clone(),
                                status: comp.stage.clone(),
                            });
                        }
                        McpToolResult::text(format!(
                            "Component '{}' successfully marked as implemented (stage: '{}').",
                            comp.id, comp.stage
                        ))
                    }
                    Err(e) => McpToolResult::error(format!("Implementation transition failed: {e}")),
                }
            }
            other => McpToolResult::error(format!("Unknown tool: '{other}'")),
        }
    }
}
