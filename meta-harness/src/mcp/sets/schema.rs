//! What several tools' schemas share, built once per list.

use arch_graph::tree::{TreeConfig, ViewKind};
use serde_json::{Value, json};

use crate::mcp::protocol::McpTool;

pub(super) fn tool(name: &str, description: &str, input_schema: Value) -> McpTool {
    McpTool { name: name.into(), description: description.into(), input_schema }
}

pub(super) struct Fragments {
    pub(super) tree_help: String,
    pub(super) card: Value,
    pub(super) id: Value,
    pub(super) plan: Value,
    pub(super) source: Value,
    pub(super) op: Value,
}

impl Fragments {
    pub(super) fn new(trees: &TreeConfig) -> Self {
    let tree_list: Vec<String> = ViewKind::ALL.iter().map(|v| format!("- {}: {}", v.name(), trees.view(*v).description)).collect();
    let tree_help = format!("Views (each a tree from one root, cut to a size limit that it reports):\n{}", tree_list.join("\n"));
    let card = json!({ "type": "string", "description": "A card: a component id (rust:my_crate::server::run), in data_flow a data type as type:<Name> or a flow's start or end as start:<id> / end:<id>" });
    let id = json!({ "type": "string", "description": "Component id: <language>:<symbol>, e.g. rust:my_crate::server::run" });
    let plan = json!({ "type": "string", "description": "Plan id" });
    let source = json!({ "type": "string", "description": "What to read: \"code\" (the code alone, the default) or \"plan:<id>\" (the code with that plan)" });
    let kind = json!({ "type": "string", "enum": ["contains", "implements", "calls", "data_flow"] });
    let signature = json!({
        "type": "object",
        "properties": {
            "params": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" }, "ty": { "type": "string" } }, "required": ["name"] } },
            "returns": { "type": "string" }
        }
    });
    let spec = json!({
        "type": "object",
        "description": "How it works: the primary planning artifact.",
        "properties": {
            "pattern": { "type": "string" },
            "logic_steps": { "type": "array", "description": "Numbered 1..n.", "items": { "type": "object", "properties": {
                "sequence": { "type": "integer" }, "name": { "type": "string" }, "description": { "type": "string" },
                "algorithm": { "type": "string" }, "complexity": { "type": "string" },
                "check": { "type": "string", "description": "Shell snippet run from the repo root during validation; exit 0 = the step holds" }
            }, "required": ["sequence", "name"] } },
            "invariants": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" }, "kind": { "type": "string", "enum": ["pre_condition", "post_condition", "state_invariant", "complexity"] }, "description": { "type": "string" } }, "required": ["name"] } },
            "tasks": { "type": "array", "items": { "type": "object", "properties": { "text": { "type": "string" }, "done": { "type": "boolean" } }, "required": ["text"] } },
            "validation": { "type": "array", "description": "Project tools to run on it (see .nah/tools.toml)", "items": { "type": "object", "properties": { "tool": { "type": "string" }, "targets": { "type": "array", "items": { "type": "string" } }, "args": { "type": "array", "items": { "type": "string" } } }, "required": ["tool"] } }
        }
    });
    let shape = json!({ "description": "JSON Schema, or shorthand: a type string (\"int\", \"str[]\", \"User?\" where User is a data type) or a map of field -> type (fields ending in ? are optional)." });
    let effects = json!({ "type": "array", "items": { "type": "object", "properties": { "target": { "type": "string", "description": "What it changes: db, fs, network..." }, "description": { "type": "string" } }, "required": ["target"] } });
    let schemas = json!({
        "type": "object",
        "description": "Data shapes: properties (data types), inputs (one field per parameter), outputs, and side effects.",
        "properties": { "properties": shape, "inputs": shape, "outputs": shape, "side_effects": effects }
    });
    let op = json!({
        "type": "object",
        "description": "One plan operation, selected by `op`.",
        "oneOf": [
            { "properties": { "op": { "const": "add_component" }, "name": { "type": "string" }, "type": { "type": "string", "description": "A component type (see list_types)" }, "description": { "type": "string" }, "parent": id, "file": { "type": "string", "description": "Repo-relative file; defaults to the parent's file" }, "signature": signature, "schemas": schemas, "spec": spec }, "required": ["op", "name", "type"] },
            { "properties": { "op": { "const": "modify" }, "id": id, "description": { "type": "string" }, "signature": signature, "schemas": schemas, "spec": spec }, "required": ["op", "id"] },
            { "properties": { "op": { "const": "reference" }, "id": id, "kinds": { "type": "array", "items": kind }, "depth": { "type": "integer" } }, "required": ["op", "id"] },
            { "properties": { "op": { "const": "relate" }, "kind": kind, "source": id, "target": id, "order": { "type": "integer" }, "ports": { "type": "array", "items": { "type": "string" }, "minItems": 2, "maxItems": 2 }, "note": { "type": "string" }, "expects": { "description": "Calls only: what the caller passes (inputs), gets back (outputs) and expects changed (side_effects); checked against the callee's real signature.", "type": "object", "properties": { "inputs": shape, "outputs": shape, "side_effects": effects } } }, "required": ["op", "kind", "source", "target"] },
            { "properties": { "op": { "const": "unrelate" }, "kind": kind, "source": id, "target": id }, "required": ["op", "kind", "source", "target"] },
            { "properties": { "op": { "const": "remove" }, "id": id }, "required": ["op", "id"] },
            { "properties": { "op": { "const": "set_greenfield" }, "greenfield": { "type": "boolean" } }, "required": ["op", "greenfield"] },
            { "properties": { "op": { "const": "describe" }, "title": { "type": "string" }, "description": { "type": "string" } }, "required": ["op"] }
        ]
    });
        Self { tree_help, card, id, plan, source, op }
    }
}
