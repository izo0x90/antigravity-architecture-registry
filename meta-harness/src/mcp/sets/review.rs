//! Review: the user's points on a plan, answering them, and the reviewers.

use std::fmt::Write;
use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_graph::{CommentId, CommentStatus, ComponentId};
use arch_service::{ArchitectureService, ServiceError};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ToolSet;
use super::schema::{Fragments, tool};
use crate::arch::blocking;
use nah_access::Capability;

use super::ToolDef;
use crate::mcp::text::*;

pub(crate) struct Review {
    pub(crate) arch: Arc<ArchitectureService>,
}

#[derive(Deserialize)]
struct CommentsArgs {
    plan: String,
    #[serde(default)]
    all: bool,
}

#[derive(Deserialize)]
struct AnswerArgs {
    plan: String,
    comment: String,
    answer: String,
    #[serde(default)]
    changes: Vec<String>,
}

#[derive(Deserialize)]
struct ReplyArgs {
    plan: String,
    comment: String,
    text: String,
}

impl Review {
    /// Its tools, as agents see them (in the list's order).
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<ToolDef> {
        let Fragments { id, plan, .. } = Fragments::new(trees);
        vec![
            tool(
                Capability::ViewRead,
                "review_comments",
                "The user's review points on a plan: comments and change requests on components, spec sections, relations or the whole plan, with their threads, plus what changed since the user last reviewed it. Answer every open point (answer_comment); approval waits until the user accepts each answer.",
                json!({ "type": "object", "properties": { "plan": plan, "all": { "type": "boolean", "description": "Include resolved points too" } }, "required": ["plan"] }),
            ),
            tool(
                Capability::PlanEdit,
                "answer_comment",
                "Answer one review point. For a change request, first change the plan (plan_edit), then link the components you changed; the answer carries those changes so the user can check them. Questions can be answered without changes.",
                json!({ "type": "object", "properties": { "plan": plan, "comment": { "type": "string", "description": "Comment id, e.g. c3" }, "answer": { "type": "string" }, "changes": { "type": "array", "items": id, "description": "Components you changed for it (since the last review)" } }, "required": ["plan", "comment", "answer"] }),
            ),
            tool(
                Capability::PlanEdit,
                "reply_comment",
                "Reply in a review point's thread without answering it (e.g. to ask the user a clarifying question).",
                json!({ "type": "object", "properties": { "plan": plan, "comment": { "type": "string" }, "text": { "type": "string" } }, "required": ["plan", "comment", "text"] }),
            ),
            tool(
                Capability::CodeCheck,
                "review",
                "Run the reviewers that apply to one planned component (faithfulness to the plan, anti-patterns, logic...). Blocking points must be fixed before it can be verified as implemented.",
                json!({ "type": "object", "properties": { "plan": plan, "id": id }, "required": ["plan", "id"] }),
            ),
        ]
    }

    /// The next steps for `plan` (or one component of it), from its current state.
    async fn steps(&self, plan: arch_graph::PlanId, only: Option<arch_graph::ComponentId>) -> Result<String, ServiceError> {
        steps(&self.arch, plan, only).await
    }
}

#[async_trait::async_trait]
impl ToolSet for Review {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value, caller: &crate::mcp::McpCaller) -> Result<String, ServiceError> {
        match name {
        "review_comments" => {
            let a: CommentsArgs = parse(args)?;
            let plan = plan_id(&a.plan)?;
            let p = plan.clone();
            let (comments, diff) = blocking(&self.arch, move |s| Ok((s.comments(&p, a.all)?, s.review_diff(&p)?))).await?;
            let mut out = comments_text(&comments, &diff);
            out.push_str(&self.steps(plan, None).await?);
            Ok(out)
        }
        "answer_comment" => {
            let a: AnswerArgs = parse(args)?;
            let plan = plan_id(&a.plan)?;
            let changes: Vec<ComponentId> = a.changes.into_iter().map(ComponentId::new).collect();
            let p = plan.clone();
            let actor = agent(caller);
            let comment = blocking(&self.arch, move |s| s.answer_comment(&p, &CommentId(a.comment), &a.answer, &changes, &actor)).await?;
            let mut out = format!("Answered {}; the user will accept it or reopen it.\n", comment.id);
            for ch in comment.answer.iter().flat_map(|a| &a.changes) {
                let _ = writeln!(out, "- linked: {}", ch.describe());
            }
            out.push_str(&self.steps(plan, None).await?);
            Ok(out)
        }
        "reply_comment" => {
            let a: ReplyArgs = parse(args)?;
            let plan = plan_id(&a.plan)?;
            let actor = agent(caller);
            let comment = blocking(&self.arch, move |s| s.reply(&plan, &CommentId(a.comment), &a.text, &actor)).await?;
            Ok(format!("Replied in {}'s thread ({} replies). It stays {}; wait for the user, then answer it.", comment.id, comment.replies.len(), if comment.status == CommentStatus::Open { "open" } else { "as it was" }))
        }
        "review" => {
            let a: ComponentArgs = parse(args)?;
            let plan = plan_id(&a.plan)?;
            let cid = ComponentId::new(a.id);
            let (p, c) = (plan.clone(), cid.clone());
            let reviews = blocking(&self.arch, move |s| s.review(&p, &c)).await?;
            let mut out = String::new();
            for run in &reviews.runs {
                let _ = writeln!(out, "{} ({}): {}", run.reviewer, run.category, if run.ran { format!("{} points", run.findings) } else { "didn't run".to_owned() });
            }
            if reviews.runs.is_empty() {
                out.push_str("No reviewers apply to this component.\n");
            }
            out.push_str(&findings_text(&reviews.report));
            if reviews.report.has_errors() {
                out.push_str("\nAddress the blocking points (ERROR) and review again before verifying.");
            }
            out.push_str(&self.steps(plan, Some(cid)).await?);
            Ok(out)
        }
            other => Err(ServiceError::NotFound(format!("tool '{other}'"))),
        }
    }
}
