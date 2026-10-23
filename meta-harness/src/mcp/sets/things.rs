//! Things, for agents: one read by its name (as its owner names it), as
//! it is now: what a link sent as a pointer says to read the rest with.

use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_service::ServiceError;
use nah_access::Capability;
use serde_json::{Value, json};

use super::reach::{NOT_YOURS, Reach};
use super::schema::tool;
use super::{ToolDef, ToolSet};

/// How many lines a read gives at most.
const READ_MOST: u64 = 2000;

pub(crate) struct Things {
    pub(crate) things: Arc<nah_things::Things>,
    pub(crate) slots: Arc<harness_core::SlotManager>,
    pub(crate) access: Arc<nah_access::Access>,
    pub(crate) discussions: Arc<nah_discussions::Discussions>,
}

impl Things {
    pub(crate) fn schemas(_trees: &TreeConfig) -> Vec<ToolDef> {
        vec![tool(
            Capability::ViewRead,
            "thing_read",
            "Read a thing by its name, as it is now: what a link the user sent as a pointer names (its start was in the message), or any thing.",
            json!({ "type": "object", "properties": {
                "thing": { "type": "string", "description": format!("Its name: {}", super::names_of_things()) },
                "lines": { "type": "string", "description": "Which of its lines, `a-b` (from 1)" }
            }, "required": ["thing"] }),
        )]
    }
}

#[async_trait::async_trait]
impl ToolSet for Things {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value, caller: &crate::mcp::McpCaller) -> Result<String, ServiceError> {
        if name != "thing_read" {
            return Err(ServiceError::NotFound(format!("no tool {name}")));
        }
        let words = args["thing"].as_str().ok_or_else(|| ServiceError::InvalidOp("`thing` (its name) is needed".into()))?.to_owned();
        let (from, to) = match args["lines"].as_str() {
            Some(range) => super::tags::line_range(range).map(|(a, b)| (a as u64, b as u64))?,
            None => (1, READ_MOST),
        };
        let reach = Reach::of(&self.slots, &self.access, &self.discussions, caller);
        let things = self.things.clone();
        tokio::task::spawn_blocking(move || {
            let subject = things.named(&words).map_err(ServiceError::InvalidOp)?;
            let what = things.name(&subject).unwrap_or(words);
            if !reach.reaches(&subject) {
                return Err(ServiceError::Forbidden(format!("{what} is {NOT_YOURS}")));
            }
            let (_, read) = things.lines(&subject, None, from, to.min(from + READ_MOST - 1));
            let (shown, all) = read.ok_or_else(|| ServiceError::NotFound(format!("{what} isn't there now")))?;
            if from > all.max(1) {
                return Err(ServiceError::InvalidOp(format!("{what} has {all} lines")));
            }
            let to = (from + shown.len() as u64).saturating_sub(1);
            let more = if to < all { format!("\n(lines {from}-{to} of {all}: `lines` gives others)") } else { String::new() };
            Ok(format!("{what}:\n{}{more}", shown.join("\n")))
        })
        .await
        .map_err(|e| ServiceError::InvalidOp(e.to_string()))?
    }
}
