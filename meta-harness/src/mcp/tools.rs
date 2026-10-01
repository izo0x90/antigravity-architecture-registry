use serde_json::json;
use crate::mcp::protocol::McpTool;

pub fn list_registry_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "search_components".to_string(),
            description: "Search for architectural components by keyword, lifecycle status, subsystem, or tags.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Optional search term for component name or description" },
                    "status": { "type": "string", "description": "Optional filter by lifecycle status: Draft, Approved, Planned, Implemented" },
                    "subsystem": { "type": "string", "description": "Optional filter by subsystem name" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional list of tags to filter by" }
                }
            }),
        },
        McpTool {
            name: "inspect_component".to_string(),
            description: "Inspect the full specification, inputs, outputs, side-effects, requirements, and invariants of a specific component.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "component_id": { "type": "string", "description": "The unique ID of the component to inspect" }
                },
                "required": ["component_id"]
            }),
        },
        McpTool {
            name: "get_architecture_summary".to_string(),
            description: "Retrieve high-level architecture registry summary metrics, component counts by status, and subsystems.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "focus_architecture".to_string(),
            description: "Extract and inspect a targeted architectural subtree rooted at a component. Returns an ASCII hierarchy tree for reasoning, and navigates the human operator's Cockpit graph directly to this focused slice.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "root_id": { "type": "string", "description": "The root component ID from which to traverse the subtree" },
                    "perspective": {
                        "type": "string",
                        "enum": ["call_flow", "component_hierarchy", "data_flow", "unified"],
                        "description": "Architectural perspective: call_flow, component_hierarchy, data_flow, or unified (default call_flow)"
                    },
                    "depth": { "type": "integer", "description": "Maximum traversal depth (default 2)" }
                },
                "required": ["root_id"]
            }),
        },
        McpTool {
            name: "approve_component".to_string(),
            description: "Transition a component from Draft to Approved lifecycle status, locking its high-level contract.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "component_id": { "type": "string", "description": "The unique ID of the component to approve" }
                },
                "required": ["component_id"]
            }),
        },
        McpTool {
            name: "plan_component".to_string(),
            description: "Submit an implementation plan with internal steps and invariants for an Approved component, transitioning it to Planned.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "component_id": { "type": "string", "description": "The unique ID of the component to plan" },
                    "plan": {
                        "type": "object",
                        "properties": {
                            "steps": { "type": "array", "items": { "type": "string" }, "description": "Sequential steps to implement" },
                            "invariants": { "type": "array", "items": { "type": "string" }, "description": "Critical architectural invariants that must hold" },
                            "rationale": { "type": "string", "description": "Explanation for the chosen design" }
                        },
                        "required": ["steps", "invariants"]
                    }
                },
                "required": ["component_id", "plan"]
            }),
        },
        McpTool {
            name: "implement_component".to_string(),
            description: "Mark a Planned component as Implemented after its code and tests have been verified.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "component_id": { "type": "string", "description": "The unique ID of the component to mark implemented" }
                },
                "required": ["component_id"]
            }),
        },
    ]
}
