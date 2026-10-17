//! Tags, for agents: which there are, what one is on (and what each thing
//! says now), and (for a role granted `tags.edit`) tagging, taking a tag
//! off, renaming, recolouring, noting and deleting one. Things are named
//! as their owners name them (`tag_read` shows each one's name).

use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_service::ServiceError;
use harness_contracts::TAG_COLOURS;
use nah_access::Capability;
use serde_json::{Value, json};

use super::schema::tool;
use super::{ToolDef, ToolSet};

pub(crate) struct Tags {
    pub(crate) tags: Arc<nah_tags::Tags>,
}

impl Tags {
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<ToolDef> {
        let _ = trees;
        let on = json!({ "type": "object", "properties": {
            "tag": { "type": "string", "description": "Its name (a new tag when there's none of that name: letters, digits, - and _)" },
            "subject": { "type": "string", "description": "What to tag, named as its owner names it: `tag_read` shows each thing's name; a terminal's are `terminal t1 command 3` and `terminal t1 lines 120-124` (`terminal_read` gives the numbers)" }
        }, "required": ["tag", "subject"] });
        let edit = json!({ "type": "object", "properties": {
            "tag": { "type": "string" },
            "name": { "type": "string", "description": "A new name (letters, digits, - and _)" },
            "colour": { "type": "integer", "minimum": 0, "maximum": TAG_COLOURS - 1 },
            "note": { "type": "string" },
            "delete": { "type": "boolean", "description": "Delete the tag (from everything it's on)" }
        }, "required": ["tag"] });
        vec![
            tool(Capability::ViewRead, "tags", "The project's tags (names given to things to find them again): each one's name, how many things it's on, and its note.", json!({ "type": "object", "properties": {} })),
            tool(
                Capability::ViewRead,
                "tag_read",
                "What a tag is on: each thing's name, what it says now (or that it's gone) and what it said when tagged, and who tagged it.",
                json!({ "type": "object", "properties": { "tag": { "type": "string" } }, "required": ["tag"] }),
            ),
            tool(Capability::TagsEdit, "tag", "Tag something.", on.clone()),
            tool(Capability::TagsEdit, "untag", "Take a tag off something it's on.", on),
            tool(Capability::TagsEdit, "tag_edit", "Rename a tag, give it another colour or a note, or delete it.", edit),
        ]
    }
}

fn refused(e: nah_tags::TagError) -> ServiceError {
    match e {
        nah_tags::TagError::NotFound(_) => ServiceError::NotFound(e.to_string()),
        _ => ServiceError::InvalidOp(e.to_string()),
    }
}

/// Who changed something, to an agent: the user, or an agent's slot.
fn who(by: &str) -> &str {
    match by {
        "user" => "the user",
        "" => "?",
        slot => slot,
    }
}

/// A tag in a line.
fn said(tag: &harness_contracts::Tag) -> String {
    let count = harness_contracts::on_things(tag.on.len());
    match (tag.note.as_str(), tag.noted_by.as_str()) {
        ("", _) => format!("{} ({count})", tag.name),
        (note, "") => format!("{} ({count}): {note}", tag.name),
        (note, by) => format!("{} ({count}): {note} (noted by {})", tag.name, who(by)),
    }
}

#[async_trait::async_trait]
impl ToolSet for Tags {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value, caller: &crate::mcp::McpCaller) -> Result<String, ServiceError> {
        let text = |key: &str| args[key].as_str().map(str::to_owned);
        let tag = text("tag").ok_or_else(|| ServiceError::InvalidOp("`tag` (its name) is needed".into()));
        let tags = self.tags.clone();
        // Who an agent's change is by: its slot's name.
        let by = caller.label.clone();
        // Owners say what subjects say (a terminal's lines), and tags are
        // kept in the state database: off the async threads.
        let blocking = |f: Box<dyn FnOnce(&nah_tags::Tags) -> Result<String, ServiceError> + Send>| async move { tokio::task::spawn_blocking(move || f(&tags)).await.map_err(|e| ServiceError::InvalidOp(e.to_string()))? };
        match name {
            "tags" => {
                let list = self.tags.list();
                Ok(if list.is_empty() { "No tags yet (`tag` makes one).".into() } else { list.iter().map(said).collect::<Vec<_>>().join("\n") })
            }
            "tag_read" => {
                let tag = tag?;
                blocking(Box::new(move |t| {
                    let read = t.read(&tag).map_err(refused)?;
                    let mut out = vec![said(&read.tag)];
                    for on in read.on {
                        let when = format!("tagged by {}", who(&on.tagged.by));
                        let what = &on.tagged.what;
                        out.push(match on.now {
                            // Its owner isn't here (no shells run here): what it said isn't given.
                            _ if what.is_empty() => format!("- something nothing here can show ({when})"),
                            Some(text) if text == on.tagged.said => format!("- {what} ({when}):\n{text}"),
                            Some(text) => format!("- {what} ({when}), now:\n{text}\n  when tagged:\n{}", on.tagged.said),
                            None => format!("- {what} ({when}): gone; when tagged:\n{}", on.tagged.said),
                        });
                    }
                    Ok(out.join("\n"))
                }))
                .await
            }
            "tag" | "untag" => {
                let (tag, words) = (tag?, text("subject").ok_or_else(|| ServiceError::InvalidOp("`subject` (what to tag, by its name) is needed".into()))?);
                let tagging = name == "tag";
                blocking(Box::new(move |t| {
                    let subject = t.named(&words).map_err(refused)?;
                    let done = if tagging { t.tag(&tag, subject, &by) } else { t.untag(&tag, &subject, &by) }.map_err(refused)?;
                    Ok(format!("{}. tag_read shows what it's on.", said(&done)))
                }))
                .await
            }
            "tag_edit" => {
                let tag = tag?;
                let colour = match args["colour"].as_u64() {
                    Some(c) => Some(u8::try_from(c).map_err(|_| refused(nah_tags::TagError::BadColour))?),
                    None => None,
                };
                let edit = nah_tags::Edit { name: text("name"), colour, note: text("note") };
                let delete = args["delete"].as_bool().unwrap_or(false);
                blocking(Box::new(move |t| {
                    if delete {
                        t.delete(&tag, &by).map_err(refused)?;
                        return Ok(format!("Deleted {tag}."));
                    }
                    Ok(said(&t.edit(&tag, edit, &by).map_err(refused)?))
                }))
                .await
            }
            _ => Err(ServiceError::NotFound(format!("tool '{name}'"))),
        }
    }
}
