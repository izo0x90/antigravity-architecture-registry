//! MCP server over the architecture service. Every tool result ends with next
//! steps derived from the state, so the agent knows what the workflow expects
//! without relying on instructions it may drift from.

use std::fmt::Write;
use std::sync::Arc;

use arch_graph::{Comment, CommentId, CommentKind, CommentStatus, ComponentId, Plan, PlanId, Requester, Severity, Stage, ViewRequest};
use arch_service::{Actor, ArchitectureService, PlanOp, ServiceError};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::arch::blocking;
use crate::mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolResult};
use crate::mcp::tools::list_tools;

#[derive(Clone)]
pub struct McpService {
    arch: Arc<ArchitectureService>,
}

fn parse<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, ServiceError> {
    serde_json::from_value(args).map_err(|e| ServiceError::InvalidOp(format!("bad arguments: {e}")))
}

fn pretty(value: &impl serde::Serialize) -> Result<String, ServiceError> {
    serde_json::to_string_pretty(value).map_err(|e| ServiceError::Corrupt(format!("unserializable result: {e}")))
}

/// What the workflow expects next for each component in a plan.
pub fn next_steps(plan: &Plan, only: Option<&ComponentId>) -> String {
    let mut out = String::from("\n\nNext:");
    let mut any = false;
    for c in plan.components.iter().filter(|c| only.is_none_or(|id| &c.id == id)) {
        let file = c.proposed.as_ref().map(|p| p.file.to_string()).unwrap_or_else(|| "its file".into());
        let step = match c.stage {
            Stage::Declared => "write its spec (plan_edit), relate it to the code it touches, plan_validate; then stop and let the user approve the architecture in the cockpit".to_owned(),
            Stage::ArchApproved => format!("either lay the skeleton in {file} (structure with placeholder bodies) and verify goal=skeleton, or detail the spec for the user's plan approval"),
            Stage::PlanApproved => format!("implement it in {file} (see contract), then verify goal=implemented (or skeleton first)"),
            Stage::Skeleton => format!("replace the placeholder bodies in {file}, then verify goal=implemented"),
            Stage::Implemented => continue,
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

#[derive(Deserialize)]
struct ScopeArgs {
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct IdArgs {
    id: String,
    #[serde(default)]
    plan: Option<String>,
}

#[derive(Deserialize)]
struct ViewArgs {
    name: String,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    depth: Option<u32>,
}

#[derive(Deserialize)]
struct NavigateArgs {
    #[serde(default)]
    view: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    focus: Option<String>,
}

#[derive(Deserialize)]
struct CreateArgs {
    id: String,
    title: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct ImportArgs {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Deserialize)]
struct EditArgs {
    plan: String,
    ops: Vec<PlanOp>,
}

#[derive(Deserialize)]
struct PlanArgs {
    plan: String,
}

#[derive(Deserialize)]
struct ComponentArgs {
    plan: String,
    id: String,
}

#[derive(Deserialize)]
struct ToolsArgs {
    plan: String,
    #[serde(default)]
    id: Option<String>,
}

fn findings_text(report: &arch_graph::Report) -> String {
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

/// The agent behind the MCP endpoint (one per server; sessions aren't told apart here).
fn agent() -> Actor {
    Actor::Agent { session: String::new() }
}

#[derive(Deserialize)]
struct VerifyArgs {
    plan: String,
    id: String,
    goal: Stage,
}

fn plan_id(raw: &str) -> Result<PlanId, ServiceError> {
    PlanId::new(raw).map_err(|e| ServiceError::InvalidOp(e.to_string()))
}

fn opt_plan(raw: &Option<String>) -> Result<Option<PlanId>, ServiceError> {
    raw.as_deref().filter(|p| !p.is_empty()).map(plan_id).transpose()
}

impl McpService {
    pub fn new(arch: Arc<ArchitectureService>) -> Self {
        Self { arch }
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let req_id = req.id.clone();
        match req.method.as_str() {
            "initialize" => JsonRpcResponse::success(
                req_id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "nah-architecture", "version": env!("CARGO_PKG_VERSION") }
                }),
            ),
            "notifications/initialized" | "initialized" | "ping" => JsonRpcResponse::success(req_id, json!({})),
            "tools/list" => JsonRpcResponse::success(req_id, json!({ "tools": list_tools(self.arch.views()) })),
            "tools/call" => {
                let params = req.params.unwrap_or(Value::Null);
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let result = match self.call(&name, args).await {
                    Ok(text) => McpToolResult::text(text),
                    Err(err) => McpToolResult::error(err.to_string()),
                };
                match serde_json::to_value(&result) {
                    Ok(val) => JsonRpcResponse::success(req_id, val),
                    Err(e) => JsonRpcResponse::error(req_id, -32603, format!("Internal error serializing tool response: {e}"), None),
                }
            }
            other => JsonRpcResponse::error(req_id, -32601, format!("Method not found: '{other}'"), None),
        }
    }

    /// Runs a tool; the text is what the agent sees.
    pub async fn call(&self, name: &str, args: Value) -> Result<String, ServiceError> {
        match name {
            "ingest" => {
                let a: ScopeArgs = parse(args)?;
                let report = blocking(&self.arch, move |s| {
                    let path = match &a.scope {
                        Some(p) => s.root().join(arch_graph::RepoPath::new(p).map_err(|e| ServiceError::InvalidOp(e.to_string()))?.as_str()),
                        None => s.root().to_path_buf(),
                    };
                    s.ingest(&path)
                })
                .await?;
                Ok(format!(
                    "Ingested {} files ({} parsed, {} cached, {} removed) in {} ms: {} components, {} relations; {} ambiguous calls, {} external calls.\n\nNext: search for the code your change touches, then view it (calls/structure) before planning.",
                    report.files, report.parsed, report.cached, report.removed, report.millis, report.components, report.relations, report.ambiguous, report.external
                ))
            }
            "list_types" => pretty(self.arch.types()),
            "search" => {
                let a: SearchArgs = parse(args)?;
                let plan = opt_plan(&a.plan)?;
                let hits = blocking(&self.arch, move |s| s.search(&a.query, plan.as_ref(), a.limit.unwrap_or(20))).await?;
                let mut out = String::new();
                for c in &hits {
                    let at = c.anchor.as_ref().map(|x| format!("{}:{}", x.file, x.span.start_line)).or(c.proposed.as_ref().map(|p| format!("-> {}", p.file))).unwrap_or_default();
                    let _ = writeln!(out, "{} [{} {:?}/{:?}] {at}", c.id, c.type_name, c.status, c.stage);
                }
                if hits.is_empty() {
                    out.push_str("No matches. Try a shorter query, or ingest first.\n");
                }
                out.push_str("\nNext: inspect a match, or view it (e.g. view name=calls roots=[id]) to see what it touches.");
                Ok(out)
            }
            "inspect" => {
                let a: IdArgs = parse(args)?;
                let plan = opt_plan(&a.plan)?;
                let detail = blocking(&self.arch, move |s| s.component(&ComponentId::new(a.id), plan.as_ref())).await?;
                pretty(&detail)
            }
            "view" => {
                let a: ViewArgs = parse(args)?;
                let plan = opt_plan(&a.plan)?;
                let result = blocking(&self.arch, move |s| s.named_view(&a.name, plan.as_ref(), a.roots.into_iter().map(ComponentId::new).collect(), a.depth)).await?;
                Ok(result.text)
            }
            "navigate" => {
                let a: NavigateArgs = parse(args)?;
                let request = ViewRequest {
                    view: a.view,
                    spec: None,
                    plan: opt_plan(&a.plan)?,
                    roots: a.roots.into_iter().map(ComponentId::new).collect(),
                    depth: a.depth,
                    focus: a.focus.map(ComponentId::new),
                };
                blocking(&self.arch, move |s| s.request_view(request, Requester::Agent)).await?;
                Ok("The user's graph now shows it (they can go back).".into())
            }
            "plan_list" => {
                let plans = blocking(&self.arch, |s| s.plans()).await?;
                let mut out = String::new();
                for p in &plans {
                    let _ = writeln!(out, "{} — {}{}", p.id, p.title, if p.greenfield { " (greenfield)" } else { "" });
                    for c in &p.components {
                        let _ = writeln!(out, "  {} [{} {:?}/{:?}]", c.id, c.type_name, c.status, c.stage);
                    }
                }
                if plans.is_empty() {
                    out.push_str("No plans yet. plan_create starts one.");
                }
                Ok(out)
            }
            "plan_create" => {
                let a: CreateArgs = parse(args)?;
                let plan = blocking(&self.arch, move |s| s.create_plan(&a.id, &a.title, &a.description)).await?;
                Ok(format!(
                    "Created plan {}.\n\nNext: reference the existing components it builds on, add new components under real parents, relate them to where they integrate, then plan_validate.",
                    plan.id
                ))
            }
            "plan_import" => {
                let a: ImportArgs = parse(args)?;
                let (plan, report) = blocking(&self.arch, move |s| {
                    let file = a.file.clone().unwrap_or_else(|| "system_architecture.json".to_owned());
                    let title = match &a.title {
                        Some(t) => t.clone(),
                        None => format!("Imported from {file}"),
                    };
                    s.import_registry_file(&a.id, &title, &file, a.language.as_deref())
                })
                .await?;
                let matched = report.components.iter().filter(|c| c.matched).count();
                let mut text = format!(
                    "Imported plan {}: {} components ({matched} matched to code), {} relations, {} workflows.",
                    plan.id,
                    report.components.len(),
                    plan.relations.len(),
                    plan.workflows.len()
                );
                if !report.unmapped.is_empty() {
                    text.push_str("\n\nNot carried over as is:");
                    for u in &report.unmapped {
                        text.push_str(&format!("\n- {}: {}", u.item, u.reason));
                    }
                }
                if !report.tools.tools.is_empty() {
                    text.push_str(&format!("\n\nOld validation tools (add to .nah/tools.toml):\n{}", pretty(&report.tools)?));
                }
                text.push_str(&next_steps(&plan, None));
                Ok(text)
            }
            "plan_edit" => {
                let a: EditArgs = parse(args)?;
                let id = plan_id(&a.plan)?;
                let plan = blocking(&self.arch, move |s| {
                    let mut current = s.plan(&id)?;
                    for (i, op) in a.ops.into_iter().enumerate() {
                        current = s.apply(&id, op).map_err(|e| ServiceError::InvalidOp(format!("op {} failed (earlier ops were applied): {e}", i + 1)))?;
                    }
                    Ok(current)
                })
                .await?;
                Ok(format!("Plan {} now has {} components, {} relations.{}", plan.id, plan.components.len(), plan.relations.len(), next_steps(&plan, None)))
            }
            "plan_validate" => {
                let a: PlanArgs = parse(args)?;
                let id = plan_id(&a.plan)?;
                let (report, plan) = blocking(&self.arch, move |s| Ok((s.validate(&id)?, s.plan(&id)?))).await?;
                let mut out = String::new();
                if report.findings.is_empty() {
                    out.push_str("The plan checks out.");
                }
                for f in &report.findings {
                    let sev = match f.severity {
                        Severity::Error => "ERROR",
                        Severity::Warning => "warning",
                        Severity::Info => "info",
                    };
                    let _ = writeln!(out, "{sev} [{}] {}{}", serde_json::to_value(f.code).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(), f.message, f.suggestion.as_ref().map(|s| format!(" — {s}")).unwrap_or_default());
                }
                out.push_str(&next_steps(&plan, None));
                Ok(out)
            }
            "contract" => {
                let a: ComponentArgs = parse(args)?;
                let plan = plan_id(&a.plan)?;
                let slice = blocking(&self.arch, move |s| s.contract(&plan, &ComponentId::new(a.id))).await?;
                Ok(slice.to_markdown())
            }
            "verify" => {
                let a: VerifyArgs = parse(args)?;
                let plan = plan_id(&a.plan)?;
                let cid = ComponentId::new(a.id);
                let target = cid.clone();
                let (v, current) = blocking(&self.arch, move |s| Ok((s.verify(&plan, &target, a.goal)?, s.plan(&plan)?))).await?;
                let mut out = match v.advanced_to {
                    Some(stage) => format!("Verified: {cid} is now {stage:?}.\n"),
                    None => format!("Not yet: {cid} doesn't match the plan.\n"),
                };
                for f in &v.report.findings {
                    let at = f.at.as_ref().map(|c| format!(" ({}:{})", c.file, c.span.start_line)).unwrap_or_default();
                    let _ = writeln!(out, "- {}{at}{}", f.message, f.suggestion.as_ref().map(|s| format!(" — {s}")).unwrap_or_default());
                }
                out.push_str(&next_steps(&current, Some(&cid)));
                Ok(out)
            }
            "run_tools" => {
                let a: ToolsArgs = parse(args)?;
                let plan = plan_id(&a.plan)?;
                let only = a.id.map(ComponentId::new);
                let report = blocking(&self.arch, move |s| s.run_tools(&plan, only.as_ref())).await?;
                let mut out = String::new();
                for run in &report.runs {
                    let _ = writeln!(out, "{} {} on {}: `{} {}`", if run.passed { "PASS" } else { "FAIL" }, run.invocation.tool, run.invocation.component, run.invocation.program, run.invocation.args.join(" "));
                }
                if report.runs.is_empty() {
                    out.push_str("No tools apply (define them in .nah/tools.toml, bind them in a component's spec `validation`).\n");
                }
                out.push_str(&findings_text(&report.report));
                out.push_str(if report.report.has_errors() { "\nNext: fix what failed, run_tools again, then verify." } else { "\nNext: verify goal=implemented." });
                Ok(out)
            }
            "review" => {
                let a: ComponentArgs = parse(args)?;
                let plan = plan_id(&a.plan)?;
                let cid = ComponentId::new(a.id);
                let reviews = blocking(&self.arch, move |s| s.review(&plan, &cid)).await?;
                let mut out = String::new();
                for run in &reviews.runs {
                    let _ = writeln!(out, "{} ({}): {}", run.reviewer, run.category, if run.ran { format!("{} points", run.findings) } else { "didn't run".to_owned() });
                }
                if reviews.runs.is_empty() {
                    out.push_str("No reviewers apply to this component.\n");
                }
                out.push_str(&findings_text(&reviews.report));
                out.push_str(if reviews.report.has_errors() { "\nNext: address the blocking points (ERROR), then review again." } else { "\nNext: verify goal=implemented." });
                Ok(out)
            }
            "review_comments" => {
                let a: CommentsArgs = parse(args)?;
                let plan = plan_id(&a.plan)?;
                let (comments, diff, current) = blocking(&self.arch, move |s| Ok((s.comments(&plan, a.all)?, s.review_diff(&plan)?, s.plan(&plan)?))).await?;
                let mut out = comments_text(&comments, &diff);
                out.push_str(&next_steps(&current, None));
                Ok(out)
            }
            "answer_comment" => {
                let a: AnswerArgs = parse(args)?;
                let plan = plan_id(&a.plan)?;
                let changes: Vec<ComponentId> = a.changes.into_iter().map(ComponentId::new).collect();
                let (comment, current) = blocking(&self.arch, move |s| {
                    let c = s.answer_comment(&plan, &CommentId(a.comment), &a.answer, &changes, &agent())?;
                    Ok((c, s.plan(&plan)?))
                })
                .await?;
                let mut out = format!("Answered {}; the user will accept it or reopen it.\n", comment.id);
                for ch in comment.answer.iter().flat_map(|a| &a.changes) {
                    let _ = writeln!(out, "- linked: {}", ch.describe());
                }
                out.push_str(&next_steps(&current, None));
                Ok(out)
            }
            "reply_comment" => {
                let a: ReplyArgs = parse(args)?;
                let plan = plan_id(&a.plan)?;
                let comment = blocking(&self.arch, move |s| s.reply(&plan, &CommentId(a.comment), &a.text, &agent())).await?;
                Ok(format!("Replied in {}'s thread ({} replies). It stays {}; wait for the user, then answer it.", comment.id, comment.replies.len(), if comment.status == CommentStatus::Open { "open" } else { "as it was" }))
            }
            other => Err(ServiceError::NotFound(format!("tool '{other}'"))),
        }
    }
}
