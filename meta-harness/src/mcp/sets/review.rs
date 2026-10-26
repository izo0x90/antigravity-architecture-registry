//! Review: the reviewers that apply to a planned component.

use std::fmt::Write;
use std::sync::Arc;

use arch_graph::ComponentId;
use arch_graph::tree::TreeConfig;
use arch_service::{ArchitectureService, ServiceError};
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

impl Review {
    /// Its tools, as agents see them (in the list's order).
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<ToolDef> {
        let Fragments { id, plan, .. } = Fragments::new(trees);
        vec![tool(
            Capability::CodeCheck,
            "review",
            "Run the reviewers that apply to one planned component (faithfulness to the plan, anti-patterns, logic...). Blocking points must be fixed before it can be verified as implemented.",
            json!({ "type": "object", "properties": { "plan": plan, "id": id }, "required": ["plan", "id"] }),
        )]
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

    async fn call(&self, name: &str, args: Value, _caller: &crate::mcp::McpCaller) -> Result<String, ServiceError> {
        match name {
            "review" => {
                let a: ComponentArgs = parse(args)?;
                let plan = plan_id(&a.plan)?;
                let cid = ComponentId::new(a.id);
                let (p, c) = (plan.clone(), cid.clone());
                let reviews = blocking(&self.arch, move |s| s.review(&p, &c)).await?;
                let mut out = String::new();
                for run in &reviews.runs {
                    let _ = writeln!(
                        out,
                        "{} ({}): {}",
                        run.reviewer,
                        run.category,
                        if run.ran { format!("{} points", run.findings) } else { "didn't run".to_owned() }
                    );
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
