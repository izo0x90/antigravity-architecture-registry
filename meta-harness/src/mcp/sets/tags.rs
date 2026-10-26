//! Tags, for agents: which there are, what one is on (and what each thing
//! says now), and (for a role granted `tags.edit`) tagging, taking a tag
//! off, renaming, recolouring, noting and deleting one. Things are named
//! as their owners name them (`tag_read` shows each one's name).

use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_service::ServiceError;
use harness_contracts::{Held, TAG_COLOURS};
use nah_access::Capability;
use serde_json::{Value, json};

use super::reach::{NOT_YOURS, Reach};
use super::schema::tool;
use super::{ToolDef, ToolSet};

pub(crate) struct Tags {
    pub(crate) tags: Arc<nah_tags::Tags>,
    /// Whose a chat is, and what a caller may read of the slots' chats (as
    /// `slot_read` lets it: its own; a sub-slot's with `slots.observe`).
    pub(crate) slots: Arc<harness_core::SlotManager>,
    pub(crate) access: Arc<nah_access::Access>,
    /// A discussion is read as what it's on is.
    pub(crate) discussions: Arc<nah_discussions::Discussions>,
}

impl Tags {
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<ToolDef> {
        let _ = trees;
        let on = json!({ "type": "object", "properties": {
            "tag": { "type": "string", "description": "Its name (a new tag when there's none of that name: letters, digits, - and _)" },
            "subject": { "type": "string", "description": format!("What to tag, named as its owner names it (`tag_read` shows each thing's name): {}", super::names_of_things()) }
        }, "required": ["tag", "subject"] });
        let edit = json!({ "type": "object", "properties": {
            "tag": { "type": "string" },
            "name": { "type": "string", "description": "A new name (letters, digits, - and _)" },
            "colour": { "type": "integer", "minimum": 0, "maximum": TAG_COLOURS - 1 },
            "note": { "type": "string" },
            "delete": { "type": "boolean", "description": "Delete the tag (from everything it's on)" }
        }, "required": ["tag"] });
        vec![
            tool(
                Capability::ViewRead,
                "tags",
                "The project's tags (names given to things to find them again): each one's name, how many things it's on, and its note.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool(
                Capability::ViewRead,
                "tag_read",
                "What a tag is on: each thing's name, who tagged it, and what it says (from where it is, or as it was when tagged once it's gone from there): the start of each, or one thing (`thing`, its number) whole or by `lines`.",
                json!({ "type": "object", "properties": {
                    "tag": { "type": "string" },
                    "thing": { "type": "integer", "minimum": 1, "description": "One of its things (as numbered), whole" },
                    "lines": { "type": "string", "description": "With `thing`: which of its lines, `a-b`" },
                    "full": { "type": "boolean", "description": "Everything: a chat turn's tool calls with what each gave (one line each, otherwise)" }
                }, "required": ["tag"] }),
            ),
            tool(
                Capability::ViewRead,
                "tag_find",
                "Search what tagged things say (all of it, kept or where it is): each line found, with its tag, its thing (numbered as `tag_read` numbers them) and which of its lines.",
                json!({ "type": "object", "properties": {
                    "query": { "type": "string", "description": "Words a line has in it, in any order (capitals heeded only when it has some)" },
                    "tag": { "type": "string", "description": "Only this tag's things" }
                }, "required": ["query"] }),
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

/// How many of a thing's lines `tag_read` shows of each, and of one
/// asked for at most.
const SHOWN_LINES: usize = 20;
pub(super) const READ_MOST: usize = 2000;
/// How many lines `tag_find` gives at most.
const FOUND_MOST: usize = 50;

/// What one thing read by its lines shows (`read`: the lines from `from`,
/// and how many it has), and what's said after them when there are
/// more: the same for every read of one thing (`tag_read`, `thing_read`).
pub(super) fn some_lines(what: &str, read: Option<(Vec<String>, u64)>, from: u64) -> Result<(String, String), ServiceError> {
    let (shown, all) = read.ok_or_else(|| ServiceError::NotFound(format!("{what} is gone, and nothing was kept of it")))?;
    if from > all.max(1) {
        return Err(ServiceError::InvalidOp(format!("{what} has {all} lines")));
    }
    let to = (from + shown.len() as u64).saturating_sub(1).min(all);
    let more = if to < all { format!("\n(lines {from}-{to} of {all}: `lines` gives others)") } else { String::new() };
    Ok((shown.join("\n"), more))
}

/// `a-b` (1-based, both in), or `a`.
pub(super) fn line_range(words: &str) -> Result<(usize, usize), ServiceError> {
    let number = |n: &str| n.trim().parse::<usize>().map_err(|_| ServiceError::InvalidOp(format!("{n:?} isn't a line number")));
    let (from, to) = match words.split_once('-') {
        Some((a, b)) => (number(a)?, number(b)?),
        None => (number(words)?, number(words)?),
    };
    let from = from.max(1);
    Ok((from, to.max(from).min(from.saturating_add(READ_MOST - 1))))
}

/// A tag in a line.
fn said(tag: &harness_contracts::Tag) -> String {
    let count = harness_contracts::on_things(tag.on.len());
    match (tag.note.as_str(), tag.noted_by.as_str()) {
        ("", _) => format!("{} ({count})", tag.name),
        (note, "") => format!("{} ({count}): {note}", tag.name),
        (note, by) => format!("{} ({count}): {note} (noted by {})", tag.name, harness_contracts::by_to_agent(by)),
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
        let reach = Reach::of(&self.slots, &self.access, &self.discussions, caller);
        // Owners say what subjects say (a terminal's lines), and tags are
        // kept in the state database: off the async threads.
        let blocking = |f: Box<dyn FnOnce(&nah_tags::Tags) -> Result<String, ServiceError> + Send>| async move {
            tokio::task::spawn_blocking(move || f(&tags)).await.map_err(|e| ServiceError::InvalidOp(e.to_string()))?
        };
        match name {
            "tags" => {
                let list = self.tags.list();
                Ok(if list.is_empty() {
                    "No tags yet (`tag` makes one).".into()
                } else {
                    list.iter().map(said).collect::<Vec<_>>().join("\n")
                })
            }
            "tag_read" => {
                let tag = tag?;
                let thing = args["thing"].as_u64();
                let lines = text("lines");
                let full = args["full"].as_bool().unwrap_or(false);
                blocking(Box::new(move |t| match thing {
                    // One thing: its lines asked for (as numbered, as tag_find
                    // numbers them), else its start, a turn's tool output folded
                    // unless in full.
                    Some(n) => {
                        let (from, to) = match lines.as_deref().map(line_range).transpose()? {
                            Some(range) => range,
                            None => (1, READ_MOST),
                        };
                        let (tagged, held, read) =
                            t.thing_lines(&tag, n.saturating_sub(1) as usize, from as u64, to as u64).map_err(refused)?;
                        if !reach.reaches(&tagged.subject) {
                            return Err(ServiceError::Forbidden(format!("{} is {NOT_YOURS}", tagged.what)));
                        }
                        let (shown, more) = some_lines(&tagged.what, read, from as u64)?;
                        let shown = if full || lines.is_some() { shown } else { t.fold(&tagged.subject, &shown) };
                        Ok(format!("{} ({}):\n{shown}{more}", tagged.what, held.words()))
                    }
                    None => {
                        let read = t.read(&tag).map_err(refused)?;
                        let mut out = vec![said(&read.tag)];
                        for (n, on) in read.on.iter().enumerate() {
                            let when = format!("tagged by {}", harness_contracts::by_to_agent(&on.tagged.by));
                            let what = &on.tagged.what;
                            out.push(match &on.text {
                                _ if !reach.reaches(&on.tagged.subject) => format!("{}. {NOT_YOURS} ({when})", n + 1),
                                // Its owner isn't here (no shells run here), and nothing was kept.
                                _ if what.is_empty() && on.held != Held::Kept => {
                                    format!("{}. something nothing here can show ({when})", n + 1)
                                }
                                None => format!("{}. {what} ({when}): gone, nothing kept of it", n + 1),
                                Some(text) => {
                                    let text = if full { text.clone() } else { t.fold(&on.tagged.subject, text) };
                                    let head: Vec<&str> = text.lines().take(SHOWN_LINES).collect();
                                    let more = if on.lines > head.len() as u64 {
                                        format!("\n  ({} lines in all: tag_read with `thing` {} for all of it)", on.lines, n + 1)
                                    } else {
                                        String::new()
                                    };
                                    format!("{}. {what} ({when}; {}):\n{}{more}", n + 1, on.held.words(), head.join("\n"))
                                }
                            });
                        }
                        Ok(out.join("\n"))
                    }
                }))
                .await
            }
            "tag_find" => {
                let query = text("query")
                    .filter(|q| !q.trim().is_empty())
                    .ok_or_else(|| ServiceError::InvalidOp("`query` (what to find) is needed".into()))?;
                let only = text("tag");
                blocking(Box::new(move |t| {
                    let (found, partial) = t.find_text(&query, only.as_deref(), FOUND_MOST, &|s| reach.reaches(s)).map_err(refused)?;
                    let cut = if partial { "\n(the search ran out of time: some may be missing)" } else { "" };
                    if found.is_empty() {
                        return Ok(format!("Nothing tagged says {query:?}.{cut}"));
                    }
                    let more = if found.len() == FOUND_MOST {
                        format!("\n(the first {FOUND_MOST}: `tag` narrows it){cut}")
                    } else {
                        cut.to_owned()
                    };
                    Ok(found
                        .iter()
                        .map(|f| format!("{} · {}. {} · line {}: {}", f.tag, f.thing, f.what, f.line, f.text))
                        .collect::<Vec<_>>()
                        .join("\n")
                        + &more)
                }))
                .await
            }
            "tag" | "untag" => {
                let (tag, words) = (
                    tag?,
                    text("subject").ok_or_else(|| ServiceError::InvalidOp("`subject` (what to tag, by its name) is needed".into()))?,
                );
                let tagging = name == "tag";
                blocking(Box::new(move |t| {
                    let subject = t.named(&words).map_err(refused)?;
                    if !reach.reaches(&subject) {
                        return Err(ServiceError::Forbidden(format!("that's {NOT_YOURS}")));
                    }
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
