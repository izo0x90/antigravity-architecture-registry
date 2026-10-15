//! Building: verifying a component against its plan, and the project's tools.

use std::fmt::Write;
use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_graph::{ComponentId, Stage};
use arch_service::{ArchitectureService, ServiceError};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ToolSet;
use super::schema::{Fragments, tool};
use crate::arch::blocking;
use crate::mcp::protocol::McpTool;
use crate::mcp::text::*;

pub(crate) struct Build {
    pub(crate) arch: Arc<ArchitectureService>,
}

#[derive(Deserialize)]
struct ToolsArgs {
    plan: String,
    #[serde(default)]
    id: Option<String>,
}

#[derive(Deserialize)]
struct VerifyArgs {
    plan: String,
    id: String,
    goal: Stage,
}

impl Build {
    /// Its tools, as agents see them (in the list's order).
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<McpTool> {
        let Fragments { id, plan, .. } = Fragments::new(trees);
        vec![
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
        ]
    }

    /// The next steps for `plan` (or one component of it), from its current state.
    async fn steps(&self, plan: arch_graph::PlanId, only: Option<arch_graph::ComponentId>) -> Result<String, ServiceError> {
        steps(&self.arch, plan, only).await
    }
}

#[async_trait::async_trait]
impl ToolSet for Build {
    fn tools(&self, trees: &TreeConfig) -> Vec<McpTool> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value) -> Result<String, ServiceError> {
        match name {
        "verify" => {
            let a: VerifyArgs = parse(args)?;
            let plan = plan_id(&a.plan)?;
            let cid = ComponentId::new(a.id);
            let target = cid.clone();
            let p = plan.clone();
            let v = blocking(&self.arch, move |s| s.verify(&p, &target, a.goal)).await?;
            let mut out = match v.advanced_to {
                Some(stage) => format!("Verified: {cid} is now {stage:?}.\n"),
                None => format!("Not yet: {cid} doesn't match the plan.\n"),
            };
            for f in &v.report.findings {
                let at = f.at.as_ref().map(|c| format!(" ({}:{})", c.file, c.span.start_line)).unwrap_or_default();
                let _ = writeln!(out, "- {}{at}{}", f.message, f.suggestion.as_ref().map(|s| format!(" — {s}")).unwrap_or_default());
            }
            out.push_str(&self.steps(plan, Some(cid)).await?);
            Ok(out)
        }
        "run_tools" => {
            let a: ToolsArgs = parse(args)?;
            let plan = plan_id(&a.plan)?;
            let only = a.id.map(ComponentId::new);
            let (p, o) = (plan.clone(), only.clone());
            let report = blocking(&self.arch, move |s| s.run_tools(&p, o.as_ref())).await?;
            let mut out = String::new();
            for run in &report.runs {
                let _ = writeln!(out, "{} {} on {}: `{} {}`", if run.passed { "PASS" } else { "FAIL" }, run.invocation.tool, run.invocation.component, run.invocation.program, run.invocation.args.join(" "));
            }
            if report.runs.is_empty() {
                out.push_str("No tools apply (define them in .nah/tools.toml, bind them in a component's spec `validation`).\n");
            }
            out.push_str(&findings_text(&report.report));
            if report.report.has_errors() {
                out.push_str("\nFix what failed and run_tools again before verifying.");
            }
            out.push_str(&self.steps(plan, only).await?);
            Ok(out)
        }
            other => Err(ServiceError::NotFound(format!("tool '{other}'"))),
        }
    }
}
