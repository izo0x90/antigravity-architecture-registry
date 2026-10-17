//! Tags, for agents: which there are, what one is on (and what each thing
//! says now), and (for a role granted `tags.edit`) tagging, taking a tag
//! off, renaming, recolouring, noting and deleting one. A terminal's lines
//! and commands are named by the terminal and their numbers.

use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_service::ServiceError;
use harness_contracts::{Subject, TerminalId};
use nah_access::Capability;
use serde_json::{Value, json};

use super::schema::tool;
use super::{ToolDef, ToolSet};

pub(crate) struct Tags {
    pub(crate) tags: Arc<nah_tags::Tags>,
    /// Where a terminal's subjects are numbered (none: no shells here).
    pub(crate) terminals: Option<Arc<crate::terminals::Terminals>>,
}

/// What a tool names a thing to tag by.
fn subject_schema() -> Value {
    json!({
        "terminal": { "type": "string", "description": "A terminal's id (from `terminals`)" },
        "command": { "type": "integer", "minimum": 1, "description": "One of its commands, by its number (from `terminal_read`)" },
        "lines": { "type": "string", "description": "Some of its lines, by their numbers: `120` or `120-124` (`terminal_read` with `numbered` shows them)" }
    })
}

impl Tags {
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<ToolDef> {
        let _ = trees;
        let mut on = subject_schema();
        on["tag"] = json!({ "type": "string" });
        let edit = json!({ "type": "object", "properties": {
            "tag": { "type": "string" },
            "name": { "type": "string", "description": "A new name (letters, digits, - _ . /; no spaces)" },
            "colour": { "type": "integer", "minimum": 0, "maximum": 7 },
            "note": { "type": "string" },
            "delete": { "type": "boolean", "description": "Delete the tag (from everything it's on)" }
        }, "required": ["tag"] });
        vec![
            tool(Capability::ViewRead, "tags", "The project's tags (names given to things to find them again): each one's name, how many things it's on, and its note.", json!({ "type": "object", "properties": {} })),
            tool(
                Capability::ViewRead,
                "tag_read",
                "What a tag (by its name) is on: each thing, where it is, what it says now (or that it's gone) and what it said when tagged.",
                json!({ "type": "object", "properties": { "tag": { "type": "string" } }, "required": ["tag"] }),
            ),
            tool(Capability::TagsEdit, "tag", "Tag something with `tag` (a new tag when the name is new): a terminal's command (`terminal` and `command`) or lines (`terminal` and `lines`).", json!({ "type": "object", "properties": on.clone(), "required": ["tag", "terminal"] })),
            tool(Capability::TagsEdit, "untag", "Take `tag` off something it's on (named as `tag` names it).", json!({ "type": "object", "properties": on, "required": ["tag", "terminal"] })),
            tool(Capability::TagsEdit, "tag_edit", "Rename a tag, give it another colour or a note, or delete it.", edit),
        ]
    }

    /// The thing `args` names, in its terminal's current start.
    fn subject(&self, args: &Value) -> Result<Subject, ServiceError> {
        let invalid = |m: &str| ServiceError::InvalidOp(m.to_owned());
        let terminals = self.terminals.as_ref().ok_or_else(|| invalid("this server runs no shells (it was started with --no-shells)"))?;
        let id = TerminalId::new(args["terminal"].as_str().ok_or_else(|| invalid("`terminal` (its id, from `terminals`) is needed"))?);
        let shell = terminals.get(&id).map_err(|e| ServiceError::InvalidOp(e.to_string()))?.started();
        if let Some(command) = args["command"].as_u64() {
            return Ok(Subject::TerminalCommand { terminal: id, shell, command });
        }
        let lines = args["lines"].as_str().ok_or_else(|| invalid("`command` or `lines` is needed"))?;
        let number = |s: &str| s.trim().parse::<u64>().map_err(|_| ServiceError::InvalidOp(format!("{lines:?}: lines are `120` or `120-124`")));
        let (first, last) = match lines.split_once('-') {
            Some((a, b)) => (number(a)?, number(b)?),
            None => (number(lines)?, number(lines)?),
        };
        Ok(Subject::TerminalLines { terminal: id, shell, first: first.min(last), last: first.max(last) })
    }
}

fn refused(e: nah_tags::TagError) -> ServiceError {
    match e {
        nah_tags::TagError::NotFound(_) => ServiceError::NotFound(e.to_string()),
        _ => ServiceError::InvalidOp(e.to_string()),
    }
}

/// A tag in a line.
fn said(tag: &harness_contracts::Tag) -> String {
    let count = match tag.on.len() {
        1 => "on 1 thing".to_owned(),
        n => format!("on {n} things"),
    };
    if tag.note.is_empty() { format!("{} ({count})", tag.name) } else { format!("{} ({count}): {}", tag.name, tag.note) }
}

#[async_trait::async_trait]
impl ToolSet for Tags {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value, _caller: &crate::mcp::McpCaller) -> Result<String, ServiceError> {
        let tag = || args["tag"].as_str().map(str::to_owned).ok_or_else(|| ServiceError::InvalidOp("`tag` (its name) is needed".into()));
        let tags = self.tags.clone();
        // Owners say what subjects say (a terminal's lines), and tags are
        // kept in the state database: off the async threads.
        let blocking = |f: Box<dyn FnOnce(&nah_tags::Tags) -> Result<String, ServiceError> + Send>| async move { tokio::task::spawn_blocking(move || f(&tags)).await.map_err(|e| ServiceError::InvalidOp(e.to_string()))? };
        match name {
            "tags" => {
                let list = self.tags.list();
                Ok(if list.is_empty() { "No tags yet (`tag` makes one).".into() } else { list.iter().map(said).collect::<Vec<_>>().join("\n") })
            }
            "tag_read" => {
                let tag = tag()?;
                blocking(Box::new(move |t| {
                    let read = t.read(&tag).map_err(refused)?;
                    let mut out = vec![said(&read.tag)];
                    for (tagged, now) in read.tag.on.iter().zip(read.now) {
                        let (what, now) = match now {
                            Some(now) => (now.what, now.now),
                            None => ("(nothing here can say what it is)".into(), None),
                        };
                        out.push(match now {
                            Some(now) if now == tagged.said => format!("- {what}:\n{now}"),
                            Some(now) => format!("- {what}, now:\n{now}\n  when tagged:\n{}", tagged.said),
                            None => format!("- {what} (gone); when tagged:\n{}", tagged.said),
                        });
                    }
                    Ok(out.join("\n"))
                }))
                .await
            }
            "tag" | "untag" => {
                let (tag, subject) = (tag()?, self.subject(&args)?);
                let tagging = name == "tag";
                blocking(Box::new(move |t| {
                    let done = if tagging { t.tag(&tag, subject) } else { t.untag(&tag, &subject) }.map_err(refused)?;
                    Ok(format!("{}. tag_read shows what it's on.", said(&done)))
                }))
                .await
            }
            "tag_edit" => {
                let tag = tag()?;
                let text = |key: &str| args[key].as_str().map(str::to_owned);
                let edit = nah_tags::Edit { name: text("name"), colour: args["colour"].as_u64().map(|c| c.min(255) as u8), note: text("note") };
                let delete = args["delete"].as_bool().unwrap_or(false);
                blocking(Box::new(move |t| {
                    if delete {
                        t.delete(&tag).map_err(refused)?;
                        return Ok(format!("Deleted {tag}."));
                    }
                    Ok(said(&t.edit(&tag, edit).map_err(refused)?))
                }))
                .await
            }
            _ => Err(ServiceError::NotFound(format!("tool '{name}'"))),
        }
    }
}
