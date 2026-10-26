//! Driving the user's cockpit, while the user lets agents.

use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_service::ServiceError;
use serde_json::{Value, json};

use super::ToolSet;
use super::schema::tool;
use nah_access::Capability;

use super::ToolDef;

pub(crate) struct Cockpit {
    pub(crate) remote: Arc<crate::remote::Remote>,
}

impl Cockpit {
    /// Its tools, as agents see them (in the list's order).
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<ToolDef> {
        let _ = trees;
        vec![
            tool(
                Capability::ViewRead,
                "cockpit_read",
                "What the user's cockpit shows now: the input mode, each panel's buffer (which has focus, and where its cursor is), the note line, and every piece of text drawn, row by row with its x. Works while the user lets agents drive the cockpit. Read again after acting: the screen updates as results arrive.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool(
                Capability::ViewRead,
                "cockpit_actions",
                "Everything the cockpit can run now (what the Space menu, keys and palette offer): id, label, keys, what it needs (a choice, text, on/off) with its choices, and whether it applies now.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool(
                Capability::ViewDrive,
                "cockpit_run",
                "Run a cockpit action by id, as its key would (see cockpit_actions). A choice is given by its value or label; on/off as on or off.",
                json!({ "type": "object", "properties": { "id": { "type": "string" }, "value": { "type": "string" } }, "required": ["id"] }),
            ),
            tool(
                Capability::ViewDrive,
                "cockpit_click",
                "Click in the cockpit: on drawn text (the topmost match, exact before containing), or at x, y (screen points, as cockpit_read gives them).",
                json!({ "type": "object", "properties": { "text": { "type": "string" }, "x": { "type": "number" }, "y": { "type": "number" } } }),
            ),
            tool(
                Capability::ViewDrive,
                "cockpit_type",
                "Type text into whatever has the keyboard in the cockpit (a text box in insert mode, the palette, a search).",
                json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }),
            ),
            tool(
                Capability::ViewDrive,
                "cockpit_keys",
                "Press keys in the cockpit, in order, space separated: names as in its key sheet, with modifiers joined by + (\"space s n\", \"i\", \"ctrl+enter\", \"escape\", \"G\").",
                json!({ "type": "object", "properties": { "keys": { "type": "string" } }, "required": ["keys"] }),
            ),
        ]
    }

    async fn drive(&self, op: &str, args: Value) -> Result<String, ServiceError> {
        let mut call = match args {
            Value::Object(fields) => fields,
            _ => Default::default(),
        };
        call.insert("op".into(), op.into());
        let answer = self.remote.call(Value::Object(call)).await.map_err(ServiceError::InvalidOp)?;
        Ok(match op {
            "read" => crate::remote::screen_text(&answer),
            "actions" => crate::remote::actions_text(&answer),
            "run" => match answer["note"].as_str() {
                Some(note) => format!("Ran it. The cockpit says: {note}"),
                None => "Ran it. cockpit_read shows what changed.".into(),
            },
            "click" => format!(
                "Clicked {}. cockpit_read shows what changed.",
                answer["clicked"].as_str().map_or_else(|| format!("at {}, {}", answer["x"], answer["y"]), |t| format!("{t:?}"))
            ),
            _ => "Done. cockpit_read shows what changed.".into(),
        })
    }
}

#[async_trait::async_trait]
impl ToolSet for Cockpit {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value, _caller: &crate::mcp::McpCaller) -> Result<String, ServiceError> {
        let op = name.strip_prefix("cockpit_").ok_or_else(|| ServiceError::NotFound(format!("tool '{name}'")))?;
        self.drive(op, args).await
    }
}
