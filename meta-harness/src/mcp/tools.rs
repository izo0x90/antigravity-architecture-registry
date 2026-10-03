//! The agent's architecture tools: read the code's real structure, plan on top
//! of it, check plans, get contracts, verify implementations, and point the
//! user's graph at things. There is deliberately no approval tool: the user
//! approves in the cockpit.

use arch_graph::ViewCatalog;
use serde_json::json;

use crate::mcp::protocol::McpTool;

fn tool(name: &str, description: &str, input_schema: serde_json::Value) -> McpTool {
    McpTool { name: name.into(), description: description.into(), input_schema }
}

pub fn list_tools(views: &ViewCatalog) -> Vec<McpTool> {
    let view_list: Vec<String> = views.views.iter().map(|(name, v)| format!("- {name}: {}", v.description)).collect();
    let view_help = format!("Views (each a lens on the same graph):\n{}", view_list.join("\n"));
    let id = json!({ "type": "string", "description": "Component id: <language>:<symbol>, e.g. rust:my_crate::server::run" });
    let plan = json!({ "type": "string", "description": "Plan id" });
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
            "logic_steps": { "type": "array", "items": { "type": "object", "properties": { "sequence": { "type": "integer" }, "name": { "type": "string" }, "description": { "type": "string" } }, "required": ["sequence", "name"] } },
            "invariants": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" }, "kind": { "type": "string" }, "description": { "type": "string" } }, "required": ["name"] } }
        }
    });
    let op = json!({
        "type": "object",
        "description": "One plan operation, selected by `op`.",
        "oneOf": [
            { "properties": { "op": { "const": "add_component" }, "name": { "type": "string" }, "type": { "type": "string", "description": "A component type (see list_types)" }, "description": { "type": "string" }, "parent": id, "file": { "type": "string", "description": "Repo-relative file; defaults to the parent's file" }, "signature": signature, "spec": spec }, "required": ["op", "name", "type"] },
            { "properties": { "op": { "const": "modify" }, "id": id, "description": { "type": "string" }, "signature": signature, "spec": spec }, "required": ["op", "id"] },
            { "properties": { "op": { "const": "reference" }, "id": id, "kinds": { "type": "array", "items": kind }, "depth": { "type": "integer" } }, "required": ["op", "id"] },
            { "properties": { "op": { "const": "relate" }, "kind": kind, "source": id, "target": id, "order": { "type": "integer" }, "ports": { "type": "array", "items": { "type": "string" }, "minItems": 2, "maxItems": 2 }, "note": { "type": "string" } }, "required": ["op", "kind", "source", "target"] },
            { "properties": { "op": { "const": "unrelate" }, "kind": kind, "source": id, "target": id }, "required": ["op", "kind", "source", "target"] },
            { "properties": { "op": { "const": "remove" }, "id": id }, "required": ["op", "id"] },
            { "properties": { "op": { "const": "set_greenfield" }, "greenfield": { "type": "boolean" } }, "required": ["op", "greenfield"] },
            { "properties": { "op": { "const": "describe" }, "title": { "type": "string" }, "description": { "type": "string" } }, "required": ["op"] }
        ]
    });
    vec![
        tool(
            "ingest",
            "Read the code into the architecture graph (incremental: only changed files are parsed). Run it after editing code, or with a path to refresh just that part.",
            json!({ "type": "object", "properties": { "scope": { "type": "string", "description": "Repo-relative path; the whole repo when absent" } } }),
        ),
        tool("list_types", "The component types available (generic defaults and any specializations), with their rules.", json!({ "type": "object", "properties": {} })),
        tool(
            "search",
            "Find components by name, symbol or file. Use it to find the existing code your work will touch.",
            json!({ "type": "object", "properties": { "query": { "type": "string" }, "plan": plan, "limit": { "type": "integer" } }, "required": ["query"] }),
        ),
        tool(
            "inspect",
            "One component: type, stage, location (file:symbol), signature, relations in and out, and its source code.",
            json!({ "type": "object", "properties": { "id": id, "plan": plan }, "required": ["id"] }),
        ),
        tool(
            "view",
            &format!("A view of the graph as text: nodes with locations and stage, edges with whether plan and code agree.\n{view_help}"),
            json!({ "type": "object", "properties": { "name": { "type": "string", "enum": views.views.keys().collect::<Vec<_>>() }, "plan": plan, "roots": { "type": "array", "items": id }, "depth": { "type": "integer" } }, "required": ["name"] }),
        ),
        tool(
            "navigate",
            &format!("Show something in the user's graph (moves their view; they can go back). Use it to point at what you're discussing.\n{view_help}"),
            json!({ "type": "object", "properties": { "view": { "type": "string", "enum": views.views.keys().collect::<Vec<_>>() }, "plan": plan, "roots": { "type": "array", "items": id }, "depth": { "type": "integer" }, "focus": id } }),
        ),
        tool("plan_list", "All plans, with their components and stages.", json!({ "type": "object", "properties": {} })),
        tool(
            "plan_create",
            "Start a plan for one change (a feature, a fix). Plans hold only what's new or changing; existing code is referenced, never re-described.",
            json!({ "type": "object", "properties": { "id": { "type": "string", "description": "letters, digits, - and _" }, "title": { "type": "string" }, "description": { "type": "string" } }, "required": ["id", "title"] }),
        ),
        tool(
            "plan_import",
            "Create a plan from the old registry document (system_architecture.json): components are matched to the code where possible, everything else is reported. Approvals aren't carried over.",
            json!({ "type": "object", "properties": {
                "id": { "type": "string", "description": "new plan id" },
                "title": { "type": "string" },
                "file": { "type": "string", "description": "repo-relative path (default system_architecture.json)" },
                "language": { "type": "string", "description": "language for components with no location and no code to match" }
            }, "required": ["id"] }),
        ),
        tool(
            "plan_edit",
            "Apply operations to a plan, in order (stops at the first that fails). Add new components under a real parent (they get a proposed location), modify existing ones, reference existing code you build on, and relate components (calls, data flow, implements). New work must connect to existing code.",
            json!({ "type": "object", "properties": { "plan": plan, "ops": { "type": "array", "items": op } }, "required": ["plan", "ops"] }),
        ),
        tool(
            "plan_validate",
            "Check a plan before any code is written: references, proposed locations, location rules, and touch points with existing code.",
            json!({ "type": "object", "properties": { "plan": plan }, "required": ["plan"] }),
        ),
        tool(
            "contract",
            "Everything needed to implement one component: where it goes, its signature and spec, what it must call (with the callees' real signatures and locations) and who calls it.",
            json!({ "type": "object", "properties": { "plan": plan, "id": id }, "required": ["plan", "id"] }),
        ),
        tool(
            "verify",
            "Check the code against the plan for one component and, when it passes, move it to the goal stage. goal=skeleton accepts placeholder bodies (todo!(), raise NotImplementedError...); goal=implemented requires them gone.",
            json!({ "type": "object", "properties": { "plan": plan, "id": id, "goal": { "type": "string", "enum": ["skeleton", "implemented"] } }, "required": ["plan", "id", "goal"] }),
        ),
        tool(
            "run_tools",
            "Run the project's validation tools (linters, type checkers, tests) on one planned component, or on every planned component already in the code. Shows each command, whether it passed, and the end of its output.",
            json!({ "type": "object", "properties": { "plan": plan, "id": id }, "required": ["plan"] }),
        ),
        tool(
            "review",
            "Run the reviewers that apply to one planned component (faithfulness to the plan, anti-patterns, logic...). Blocking points must be fixed before it can be verified as implemented.",
            json!({ "type": "object", "properties": { "plan": plan, "id": id }, "required": ["plan", "id"] }),
        ),
    ]
}
