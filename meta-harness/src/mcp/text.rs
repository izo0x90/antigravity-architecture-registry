//! What the tools share: argument parsing, the text they answer with, and
//! the next steps every result ends with.

use std::fmt::Write;
use std::sync::Arc;

use arch_graph::{Comment, CommentKind, CommentStatus, ComponentId, Graph, Plan, PlanId, Severity, Source, Stage, TypeSystem};
use arch_service::{Actor, ArchitectureService, ServiceError};
use serde::Deserialize;
use serde_json::Value;

use crate::arch::blocking;

pub(crate) fn parse<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, ServiceError> {
    serde_json::from_value(args).map_err(|e| ServiceError::InvalidOp(format!("bad arguments: {e}")))
}

pub(crate) fn pretty(value: &impl serde::Serialize) -> Result<String, ServiceError> {
    serde_json::to_string_pretty(value).map_err(|e| ServiceError::Corrupt(format!("unserializable result: {e}")))
}

/// A component's detail with each link found in the code saying how it's
/// known: read from the code's structure, confirmed by the language's
/// analyzer, or matched by name only.
pub(crate) fn marked(detail: &arch_graph::ComponentDetail) -> Result<Value, ServiceError> {
    let mut value = serde_json::to_value(detail).map_err(|e| ServiceError::Corrupt(format!("unserializable result: {e}")))?;
    for (way, relations) in [("outgoing", &detail.outgoing), ("incoming", &detail.incoming)] {
        let Some(Value::Array(out)) = value.get_mut(way) else { continue };
        for (json, relation) in out.iter_mut().zip(relations) {
            let Some(known) = arch_graph::tree::Evidence::of(relation).map(|e| e.describe()) else { continue };
            if let Value::Object(fields) = json {
                fields.insert("known".into(), Value::String(known));
            }
        }
    }
    Ok(value)
}

/// What the workflow expects next for each component in a plan.
pub fn next_steps(plan: &Plan, graph: &Graph, types: &TypeSystem, only: Option<&ComponentId>) -> String {
    if plan.kind == arch_graph::PlanKind::Walkthrough {
        return walkthrough_steps(plan, only);
    }
    let mut out = String::from("\n\nNext:");
    let mut any = false;
    // Dependency order: what others build on comes first.
    for id in arch_service::topological(plan, graph, types).iter().filter(|id| only.is_none_or(|o| o == *id)) {
        let Some(c) = plan.component(id) else { continue };
        let file = c.proposed.as_ref().map(|p| p.file.to_string()).unwrap_or_else(|| "its file".into());
        let goal = match c.stage {
            Stage::Declared => None,
            Stage::ArchApproved => Some(Stage::PlanApproved),
            Stage::PlanApproved | Stage::Skeleton => Some(Stage::Implemented),
            Stage::Implemented => continue,
        };
        let waiting: Vec<String> = goal
            .map(|g| arch_service::stage_gate(plan, graph, types, id, g).into_iter().map(|f| f.message).collect())
            .unwrap_or_default();
        let step = if !waiting.is_empty() {
            format!("waits: {}", waiting.join("; "))
        } else {
            match c.stage {
                Stage::Declared => "write its spec (plan_edit), relate it to the code it touches, plan_validate; then stop and let the user approve the architecture in the cockpit".to_owned(),
                Stage::ArchApproved => format!("either lay the skeleton in {file} (structure with placeholder bodies) and verify goal=skeleton, or detail the spec for the user's plan approval"),
                Stage::PlanApproved => format!("implement it in {file} (see contract), then verify goal=implemented (or skeleton first)"),
                Stage::Skeleton => format!("replace the placeholder bodies in {file}, then verify goal=implemented"),
                Stage::Implemented => continue,
            }
        };
        any = true;
        let _ = write!(out, "\n- {} ({:?}): {step}", c.id, c.stage);
    }
    if !any {
        out.push_str(" nothing pending in this plan.");
    }
    let open: Vec<&Comment> = plan.comments.iter().filter(|c| c.status == CommentStatus::Open && only.is_none_or(|id| c.anchor.touches(id))).collect();
    if !open.is_empty() {
        let ids: Vec<&str> = open.iter().map(|c| c.id.0.as_str()).collect();
        let _ = write!(out, "\n- review: {} open point(s) from the user ({}): read them (review_comments) and answer each (answer_comment); approval waits on them", open.len(), ids.join(", "));
    }
    out
}

/// A walkthrough's next steps: what's still to be explained, then showing it.
pub(crate) fn walkthrough_steps(plan: &Plan, only: Option<&ComponentId>) -> String {
    let mut out = String::from("\n\nNext:");
    let mut any = false;
    for c in plan.components.iter().filter(|c| only.is_none_or(|o| *o == c.id)) {
        let mut missing = Vec::new();
        if c.description.trim().is_empty() {
            missing.push("a description");
        }
        if c.spec.as_ref().is_none_or(|s| s.logic_steps.is_empty()) {
            missing.push("logic steps");
        }
        if !missing.is_empty() {
            any = true;
            let _ = write!(out, "\n- {}: write {} from its code (plan_edit modify)", c.id, missing.join(" and "));
        }
    }
    if !any {
        let _ = write!(out, " every component is explained; navigate with source plan:{} (and a root at the entry point) to show the user only these components.", plan.id);
    }
    out
}

/// Review points as the agent reads them: one block per point, with its thread.
pub fn comments_text(comments: &[Comment], diff: &arch_graph::PlanDiff) -> String {
    let mut out = String::new();
    if comments.is_empty() {
        out.push_str("No review points.\n");
    }
    for c in comments {
        let kind = match c.kind {
            CommentKind::Comment => "comment",
            CommentKind::ChangeRequest => "CHANGE REQUEST",
        };
        let status = match c.status {
            CommentStatus::Open => "open",
            CommentStatus::Answered => "answered, waiting for the user",
            CommentStatus::Resolved => "resolved",
        };
        let _ = writeln!(out, "[{}] {kind} on {} ({status}) by {}: {}", c.id, c.anchor.describe(), c.author, c.text);
        for r in &c.replies {
            let _ = writeln!(out, "    {}: {}", r.author, r.text);
        }
        if let Some(a) = &c.answer {
            let _ = writeln!(out, "    answer ({}): {}", a.author, a.text);
            for ch in &a.changes {
                let _ = writeln!(out, "      - {}", ch.describe());
            }
        }
    }
    let since = diff.since.as_deref().map_or_else(|| "since the plan was started".to_owned(), |at| format!("since the user's review at {at}"));
    if diff.is_empty() {
        let _ = writeln!(out, "\nNo plan changes {since}.");
    } else {
        let _ = writeln!(out, "\nPlan changes {since}:");
        for ch in &diff.changes {
            let _ = writeln!(out, "- {}", ch.describe());
        }
    }
    out
}

pub(crate) fn findings_text(report: &arch_graph::Report) -> String {
    let mut out = String::new();
    for f in &report.findings {
        let sev = match f.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        let at = f.at.as_ref().map(|c| format!(" ({}:{})", c.file, c.span.start_line)).unwrap_or_default();
        let _ = writeln!(out, "{sev}{at} {}{}", f.message, f.suggestion.as_ref().map(|s| format!(" — {s}")).unwrap_or_default());
    }
    out
}

/// The agent behind the MCP endpoint (one per server; sessions aren't told apart here).
pub(crate) fn agent() -> Actor {
    Actor::Agent { session: String::new() }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ComponentArgs {
    pub(crate) plan: String,
    pub(crate) id: String,
}

pub(crate) fn plan_id(raw: &str) -> Result<PlanId, ServiceError> {
    PlanId::new(raw).map_err(|e| ServiceError::InvalidOp(e.to_string()))
}

pub(crate) async fn steps(arch: &Arc<ArchitectureService>, plan: PlanId, only: Option<ComponentId>) -> Result<String, ServiceError> {
    blocking(arch, move |s| {
        let current = s.plan(&plan)?;
        let graph = s.graph(&Source::Plan(plan.clone()))?;
        Ok(next_steps(&current, &graph, s.types(), only.as_ref()))
    })
    .await
}
