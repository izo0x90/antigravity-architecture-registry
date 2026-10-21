//! Discussions, for agents: which there are (what's waiting on the caller
//! first; on a plan, or on a thing), one whole (with what it's on as it
//! is now and as it was when it started), and (with `discuss`) starting
//! one on a thing by its name, replying, answering (a change request's
//! answer names what changed). Settling and reopening are the user's. A
//! chat's things only as `slot_read` allows.

use std::fmt::Write;
use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_service::ServiceError;
use harness_contracts::{Discussion, DiscussionId, DiscussionKind, DiscussionStatus, Move, Subject, when};
use nah_access::Capability;
use nah_discussions::{DiscussionError, USER};
use serde_json::{Value, json};

use super::reach::{NOT_YOURS, Reach};
use super::schema::tool;
use super::{ToolDef, ToolSet};

pub(crate) struct Discussions {
    pub(crate) discussions: Arc<nah_discussions::Discussions>,
    /// What things say, now and as kept.
    pub(crate) things: Arc<nah_things::Things>,
    pub(crate) slots: Arc<harness_core::SlotManager>,
    pub(crate) access: Arc<nah_access::Access>,
}

fn refused(e: DiscussionError) -> ServiceError {
    match e {
        DiscussionError::NotFound(_) => ServiceError::NotFound(e.to_string()),
        DiscussionError::Forbidden(_) => ServiceError::Forbidden(e.to_string()),
        _ => ServiceError::InvalidOp(e.to_string()),
    }
}

/// How many of what a thing says `discussion_read` shows, now and as it
/// was, and how many discussions `discussions` lists at most.
const SHOWN_LINES: usize = 40;
const LISTED_MOST: usize = 100;

/// A discussion in a line.
fn line(d: &Discussion) -> String {
    let asked = if d.asked == USER { "asks the user".to_owned() } else { format!("asks {}", d.asked) };
    let asking: String = d.asking().lines().next().unwrap_or_default().chars().take(100).collect();
    format!("{} · {} on {} ({}, {asked}) by {}: {asking}", d.id, d.kind.words(), d.on.what, d.status.words(), harness_contracts::by_to_agent(&d.by))
}

/// The most of what a discussion is on that's read, to show its start.
const READ_MOST: usize = 16 * 1024;

/// The start of a text (`SHOWN_LINES` lines), saying how much more there
/// is (`lines`: how many it has in all).
fn some_of((text, lines): (String, u64)) -> String {
    let more = if lines > SHOWN_LINES as u64 { format!("\n  ({lines} lines in all)") } else { String::new() };
    text.lines().take(SHOWN_LINES).map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n") + &more
}

impl Discussions {
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<ToolDef> {
        let _ = trees;
        let thing = format!("A thing, named as its owner names it: {}", super::names_of_things());
        let id = json!({ "type": "string", "description": "The discussion, e.g. d3" });
        vec![
            tool(
                Capability::ViewRead,
                "discussions",
                "Discussions: a question or a change asked for, on a thing, and what's said in it. What's waiting on you comes first (open, asked of you), then the rest not settled; settled ones with `all`.",
                json!({ "type": "object", "properties": {
                    "plan": { "type": "string", "description": "Only those on this plan's parts" },
                    "on": { "type": "string", "description": format!("Only those on this thing (or in it, or it's in). {thing}") },
                    "all": { "type": "boolean", "description": "Settled ones too" }
                } }),
            ),
            tool(
                Capability::ViewRead,
                "discussion_read",
                "One discussion whole: what it's on as it is now (and as it was when it started, when that was kept), whose move it is, everything said in it.",
                json!({ "type": "object", "properties": { "id": id }, "required": ["id"] }),
            ),
            tool(
                Capability::Discuss,
                "discuss",
                "Start a discussion on a thing: a question, or a change asked for. It asks the lead unless you name another agent slot, or `user` to ask the user.",
                json!({ "type": "object", "properties": {
                    "on": { "type": "string", "description": thing },
                    "kind": { "type": "string", "enum": ["question", "change_request"], "description": "question (the default), or change_request (its answer names what changed)" },
                    "text": { "type": "string" },
                    "ask": { "type": "string", "description": "An agent slot (id or name), or `user`; none: the lead" }
                }, "required": ["on", "text"] }),
            ),
            tool(
                Capability::Discuss,
                "discussion_reply",
                "Say something in a discussion without answering it (to ask back, say where you are).",
                json!({ "type": "object", "properties": { "id": id, "text": { "type": "string" } }, "required": ["id", "text"] }),
            ),
            tool(
                Capability::Discuss,
                "discussion_answer",
                "Answer a discussion asked of you. A change request's answer names what you changed (by name: what it's on, or a part of it), changed first; the user then settles it or reopens it.",
                json!({ "type": "object", "properties": {
                    "id": id,
                    "text": { "type": "string" },
                    "changed": { "type": "array", "items": { "type": "string" }, "description": "What you changed, by name" }
                }, "required": ["id", "text"] }),
            ),
        ]
    }
}

#[async_trait::async_trait]
impl ToolSet for Discussions {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value, caller: &crate::mcp::McpCaller) -> Result<String, ServiceError> {
        let text = |key: &str| args[key].as_str().map(str::to_owned);
        let id = text("id").map(DiscussionId::new).ok_or_else(|| ServiceError::InvalidOp("`id` (the discussion, e.g. d3) is needed".into()));
        let (discussions, things, slots) = (self.discussions.clone(), self.things.clone(), self.slots.clone());
        let reach = Reach::of(&self.slots, &self.access, &self.discussions, caller);
        // Who an agent's writing is by: its slot's name; whose move: its slot.
        let (by, me) = (caller.label.clone(), caller.slot.as_ref().map(|s| s.to_string()));
        let mine = move |d: &Discussion| matches!(d.whose_move(), Move::Agent(slot) if Some(&slot) == me.as_ref());
        // Owners say where things are and what they say, and discussions
        // are kept in the state database: off the async threads.
        let blocking = |f: Box<dyn FnOnce() -> Result<String, ServiceError> + Send>| async move { tokio::task::spawn_blocking(f).await.map_err(|e| ServiceError::InvalidOp(e.to_string()))? };
        match name {
            "discussions" => {
                let (plan, on, all) = (text("plan"), text("on"), args["all"].as_bool().unwrap_or(false));
                blocking(Box::new(move || {
                    let mut list = match (plan, on) {
                        (Some(plan), _) => discussions.on(&Subject::Plan { plan: crate::mcp::text::plan_id(&plan)? }),
                        (None, Some(on)) => discussions.on(&things.named(&on).map_err(ServiceError::InvalidOp)?),
                        (None, None) => discussions.list(),
                    };
                    list.retain(|d| (all || d.status != DiscussionStatus::Settled) && reach.reaches(&d.on.subject));
                    if list.is_empty() {
                        return Ok(if all { "No discussions here." } else { "Nothing here waits on anyone (`all` lists settled ones too)." }.into());
                    }
                    let (yours, rest): (Vec<&Discussion>, Vec<&Discussion>) = list.iter().partition(|d| mine(d));
                    let mut out = String::new();
                    for (title, part) in [("Waiting on you", yours), ("The rest", rest)] {
                        if part.is_empty() {
                            continue;
                        }
                        let _ = writeln!(out, "{title}:");
                        for d in part.iter().take(LISTED_MOST) {
                            let _ = writeln!(out, "- {}", line(d));
                        }
                        if part.len() > LISTED_MOST {
                            let _ = writeln!(out, "  ({} more: `plan` or `on` narrows it)", part.len() - LISTED_MOST);
                        }
                    }
                    Ok(out.trim_end().to_owned())
                }))
                .await
            }
            "discussion_read" => {
                let id = id?;
                blocking(Box::new(move || {
                    let d = discussions.get(&id).map_err(refused)?;
                    if !reach.reaches(&d.on.subject) {
                        return Err(ServiceError::Forbidden(format!("{} is on {NOT_YOURS}", d.id)));
                    }
                    let whose = match d.whose_move() {
                        Move::Agent(slot) if mine(&d) => format!("yours ({slot} is asked)"),
                        Move::Agent(slot) => format!("{slot}'s (asked)"),
                        Move::User => "the user's".to_owned(),
                        Move::Nobody => "no one's (settled)".to_owned(),
                    };
                    let mut out = format!("{}\nWhose move: {whose}.\n", line(&d));
                    // Only the start of each is read (what's shown of it).
                    let (held, now) = things.glimpse(&d.on.subject, None, READ_MOST);
                    match now {
                        Some(now) => {
                            let _ = writeln!(out, "\n{} now ({}):\n{}", d.on.what, held.words(), some_of(now));
                        }
                        None => {
                            let _ = writeln!(out, "\n{} is gone from where it was.", d.on.what);
                        }
                    }
                    if let Some(kept) = &d.kept {
                        let (_, was) = things.glimpse(&d.on.subject, Some(kept), READ_MOST);
                        let _ = writeln!(out, "\nAs it was when the discussion started:\n{}", was.map_or_else(|| "  (what was kept couldn't be read)".to_owned(), some_of));
                    }
                    out.push_str("\nSaid:\n");
                    for said in &d.said {
                        let _ = writeln!(out, "- {} {} ({}): {}", harness_contracts::by_to_agent(&said.by), said.kind.words(), when::written(said.at_ms), said.text);
                        for changed in &said.changed {
                            let _ = writeln!(out, "    changed {}", changed.what);
                        }
                    }
                    Ok(out.trim_end().to_owned())
                }))
                .await
            }
            "discuss" => {
                let on = text("on").ok_or_else(|| ServiceError::InvalidOp("`on` (what it's on, by its name) is needed".into()))?;
                let body = text("text").unwrap_or_default();
                let kind = match text("kind").as_deref() {
                    None | Some("question") => DiscussionKind::Question,
                    Some("change_request") => DiscussionKind::ChangeRequest,
                    Some(other) => return Err(ServiceError::InvalidOp(format!("{other:?} isn't a kind: question or change_request"))),
                };
                let ask = text("ask");
                blocking(Box::new(move || {
                    let subject = things.named(&on).map_err(ServiceError::InvalidOp)?;
                    if !reach.reaches(&subject) {
                        return Err(ServiceError::Forbidden(format!("that's {NOT_YOURS}")));
                    }
                    let asked = crate::discussing::asked(&slots, ask.as_deref(), &by).map_err(ServiceError::InvalidOp)?;
                    // Asking another slot is messaging it: the lead, or the
                    // user, any agent may ask; any other slot, only one that
                    // may message slots.
                    let lead = slots.get(&slots.lead()).map(|s| s.name).unwrap_or_else(|| nah_discussions::LEAD.to_owned());
                    if asked != lead && asked != nah_discussions::USER && !reach.access.allows(&reach.principal, nah_access::Capability::SlotsMessage) {
                        return Err(ServiceError::Forbidden(format!("asking {asked} is messaging it, which this session can't (slots.message): ask the lead")));
                    }
                    let d = discussions.start(&subject, kind, &body, &by, Some(&asked)).map_err(refused)?;
                    Ok(format!("Started {}. discussion_read {} shows it; you're told when something's said in it.", line(&d), d.id))
                }))
                .await
            }
            "discussion_reply" | "discussion_answer" => {
                let (id, body) = (id?, text("text").unwrap_or_default());
                let changed: Vec<String> = args["changed"].as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                let answering = name == "discussion_answer";
                blocking(Box::new(move || {
                    let d = discussions.get(&id).map_err(refused)?;
                    if !reach.reaches(&d.on.subject) {
                        return Err(ServiceError::Forbidden(format!("{} is on {NOT_YOURS}", d.id)));
                    }
                    let d = if answering { discussions.answer(&id, &body, &changed, &by) } else { discussions.reply(&id, &body, &by) }.map_err(refused)?;
                    Ok(if answering { format!("Answered {}: the user settles it or reopens it.", d.id) } else { format!("Said in {} (it's {} still).", d.id, d.status.words()) })
                }))
                .await
            }
            _ => Err(ServiceError::NotFound(format!("tool '{name}'"))),
        }
    }
}
