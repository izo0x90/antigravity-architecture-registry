//! The agent's architecture tools: read the code's real structure, plan on top
//! of it, check plans, get contracts, verify implementations, and point the
//! user's graph at things. There is deliberately no approval tool: the user
//! approves in the cockpit.

use arch_graph::tree::{TreeConfig, ViewKind};
use serde_json::json;

use crate::mcp::protocol::McpTool;

fn tool(name: &str, description: &str, input_schema: serde_json::Value) -> McpTool {
    McpTool { name: name.into(), description: description.into(), input_schema }
}

pub fn list_tools(trees: &TreeConfig) -> Vec<McpTool> {
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
    vec![
        tool(
            "ingest",
            "Read the code into the architecture graph (incremental: only changed files are parsed). Run it after editing code, or with a path to refresh just that part.",
            json!({ "type": "object", "properties": { "scope": { "type": "string", "description": "Repo-relative path; the whole repo when absent" } } }),
        ),
        tool("list_types", "The component types available (generic defaults and any specializations), with their rules.", json!({ "type": "object", "properties": {} })),
        tool(
            "search",
            "Find what your work will touch: symbols by name (fuzzy, a typo forgiven; `a::b` matches whole symbols), files by path, or lines by text (as written; `*.rs` or `src/` narrows the files). Each hit gives the card to inspect or view. Query words all must match; 'word matches as written, ^word starts, word$ ends, !word excludes.",
            json!({ "type": "object", "properties": {
                "query": { "type": "string" },
                "kind": { "type": "string", "enum": ["symbols", "files", "text"], "description": "symbols (the default), files or text" },
                "source": source,
                "limit": { "type": "integer" }
            }, "required": ["query"] }),
        ),
        tool(
            "inspect",
            "One card's component: type, stage, location (file:symbol), signature, relations in and out (each link found in the code says whether the language's analyzer confirmed it or it was matched by name), and its source code. Any card a view shows: a component, a data type (type:<Name>), a flow's start or end (start:<id>, end:<id>).",
            json!({ "type": "object", "properties": { "id": card, "source": source }, "required": ["id"] }),
        ),
        tool(
            "view",
            &format!("One of the three views as a text tree, the same one the user sees: each card with where it is, what it hides (+N more), what sits on its line, and lines back to cards shown elsewhere. With a plan: its own items first, marked NEW / CHANGED / DONE / DOESN'T MATCH. Give `expand` to get only what opening one card adds, or `context` to see what holds a card and what's beside it.\n{tree_help}"),
            json!({ "type": "object", "properties": {
                "view": { "type": "string", "enum": ViewKind::ALL.iter().map(|v| v.name()).collect::<Vec<_>>() },
                "root": card,
                "way": { "type": "string", "enum": ["down", "up"], "description": "down: what it calls, holds, turns into; up: what calls it, holds it, it came from" },
                "depth": { "type": "integer" },
                "source": source,
                "full_code": { "type": "boolean", "description": "With a plan: show it in the whole codebase, not only its items and what they plug into" },
                "open": { "type": "array", "items": card, "description": "Cards to open one level further" },
                "expand": card,
                "levels": { "type": "integer", "description": "With expand: how many levels (default 1)" },
                "context": card
            }, "required": ["view"] }),
        ),
        tool(
            "navigate",
            &format!("Show something in the user's graph (moves their view; they can go back). Use it to point at what you're discussing. Leave out what should stay as the user has it.\n{tree_help}"),
            json!({ "type": "object", "properties": {
                "view": { "type": "string", "enum": ViewKind::ALL.iter().map(|v| v.name()).collect::<Vec<_>>() },
                "source": source,
                "root": card,
                "way": { "type": "string", "enum": ["down", "up"] },
                "depth": { "type": "integer" },
                "select": card
            } }),
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
            "Apply operations to a plan, in order (stops at the first that fails). Add new components under a real parent (they get a proposed location), modify planned or existing ones, reference existing code you build on, and relate components (calls with expectations, data flow, implements). New work must connect to existing code. Changing an approved component's design sends it, and what depends on it, back for approval.",
            json!({ "type": "object", "properties": { "plan": plan, "ops": { "type": "array", "items": op } }, "required": ["plan", "ops"] }),
        ),
        tool(
            "plan_validate",
            "Check a plan before any code is written: references, proposed locations, type rules, loops, schema fit (also against existing code's real signatures), spec rules, required details per type, dependency gates and touch points; runs logic-step checks.",
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
            "review_comments",
            "The user's review points on a plan: comments and change requests on components, spec sections, relations or the whole plan, with their threads, plus what changed since the user last reviewed it. Answer every open point (answer_comment); approval waits until the user accepts each answer.",
            json!({ "type": "object", "properties": { "plan": plan, "all": { "type": "boolean", "description": "Include resolved points too" } }, "required": ["plan"] }),
        ),
        tool(
            "answer_comment",
            "Answer one review point. For a change request, first change the plan (plan_edit), then link the components you changed; the answer carries those changes so the user can check them. Questions can be answered without changes.",
            json!({ "type": "object", "properties": { "plan": plan, "comment": { "type": "string", "description": "Comment id, e.g. c3" }, "answer": { "type": "string" }, "changes": { "type": "array", "items": id, "description": "Components you changed for it (since the last review)" } }, "required": ["plan", "comment", "answer"] }),
        ),
        tool(
            "reply_comment",
            "Reply in a review point's thread without answering it (e.g. to ask the user a clarifying question).",
            json!({ "type": "object", "properties": { "plan": plan, "comment": { "type": "string" }, "text": { "type": "string" } }, "required": ["plan", "comment", "text"] }),
        ),
        tool(
            "review",
            "Run the reviewers that apply to one planned component (faithfulness to the plan, anti-patterns, logic...). Blocking points must be fixed before it can be verified as implemented.",
            json!({ "type": "object", "properties": { "plan": plan, "id": id }, "required": ["plan", "id"] }),
        ),
    ]
}
