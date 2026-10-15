//! Plans: listing, creating, importing, editing, checking, and a component's contract.

use std::fmt::Write;
use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_graph::{ComponentId, Severity};
use arch_service::{ArchitectureService, PlanOp, ServiceError};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ToolSet;
use super::schema::{Fragments, tool};
use crate::arch::blocking;
use nah_access::Capability;

use super::ToolDef;
use crate::mcp::text::*;

pub(crate) struct Plans {
    pub(crate) arch: Arc<ArchitectureService>,
}

#[derive(Deserialize)]
struct CreateArgs {
    id: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    kind: arch_graph::PlanKind,
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
#[serde(deny_unknown_fields)]
struct PlanArgs {
    plan: String,
}

impl Plans {
    /// Its tools, as agents see them (in the list's order).
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<ToolDef> {
        let Fragments { id, plan, op, .. } = Fragments::new(trees);
        vec![
            tool(Capability::ViewRead, "plan_list", "All plans, with their components and stages.", json!({ "type": "object", "properties": {} })),
            tool(
                Capability::PlanEdit,
                "plan_create",
                "Start a plan. kind \"change\" (the default) plans one change (a feature, a fix): it holds only what's new or changing, and existing code is referenced, never re-described. kind \"walkthrough\" explains existing code to the user: modify brings in each component the behavior goes through as it is, with a description and spec written from the code; nothing is added, related, approved or built, and navigate with source plan:<id> shows the user only those components.",
                json!({ "type": "object", "properties": { "id": { "type": "string", "description": "letters, digits, - and _" }, "title": { "type": "string" }, "description": { "type": "string" }, "kind": { "type": "string", "enum": ["change", "walkthrough"], "description": "change (the default) or walkthrough" } }, "required": ["id", "title"] }),
            ),
            tool(
                Capability::PlanEdit,
                "plan_import",
                "Create a plan from the old registry document (system_architecture.json): components are matched to the code where possible, everything else is reported. Approvals aren't carried over.",
                json!({ "type": "object", "properties": {
                    "id": { "type": "string", "description": "new plan id" },
                    "title": { "type": "string" },
                    "file": { "type": "string", "description": "repo-relative path (default system_architecture.json)" },
                    "language": { "type": "string", "description": "language for components with no location and no code to match" }
                }, "required": ["id"] }),
            ),
            tool(
                Capability::PlanEdit,
                "plan_edit",
                "Apply operations to a plan, in order (stops at the first that fails). Add new components under a real parent (they get a proposed location), modify planned or existing ones, reference existing code you build on, and relate components (calls with expectations, data flow, implements). New work must connect to existing code. Changing an approved component's design sends it, and what depends on it, back for approval.",
                json!({ "type": "object", "properties": { "plan": plan, "ops": { "type": "array", "items": op } }, "required": ["plan", "ops"] }),
            ),
            tool(
                Capability::CodeCheck,
                "plan_validate",
                "Check a plan before any code is written: references, proposed locations, type rules, loops, schema fit (also against existing code's real signatures), spec rules, required details per type, dependency gates and touch points; runs logic-step checks.",
                json!({ "type": "object", "properties": { "plan": plan }, "required": ["plan"] }),
            ),
            tool(
                Capability::ViewRead,
                "contract",
                "Everything needed to implement one component: where it goes, its signature and spec, what it must call (with the callees' real signatures and locations) and who calls it.",
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
impl ToolSet for Plans {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value, _caller: &crate::mcp::McpCaller) -> Result<String, ServiceError> {
        match name {
        "plan_list" => {
            let plans = blocking(&self.arch, |s| s.plans()).await?;
            let mut out = String::new();
            for p in &plans {
                let _ = writeln!(out, "{} — {} ({}){}", p.id, p.title, p.kind.name(), if p.greenfield { " (greenfield)" } else { "" });
                for c in &p.components {
                    let _ = writeln!(out, "  {} [{} {:?}/{:?}]", c.id, c.type_name, c.status, c.stage);
                }
            }
            if plans.is_empty() {
                out.push_str("No plans yet.\n\nNext: plan_create starts one (or plan_import brings in system_architecture.json).");
            }
            for p in &plans {
                let _ = write!(out, "\n[{}]", p.id);
                out.push_str(&self.steps(p.id.clone(), None).await?);
            }
            Ok(out)
        }
        "plan_create" => {
            let a: CreateArgs = parse(args)?;
            let plan = blocking(&self.arch, move |s| s.create_plan(&a.id, &a.title, &a.description, a.kind)).await?;
            Ok(match plan.kind {
                arch_graph::PlanKind::Change => format!(
                    "Created plan {}.\n\nNext: reference the existing components it builds on, add new components under real parents, relate them to where they integrate, then plan_validate.",
                    plan.id
                ),
                arch_graph::PlanKind::Walkthrough => format!(
                    "Created walkthrough {}.\n\nNext: plan_edit modify each component the behavior goes through (functions, and the data types it passes), writing its description and spec (logic steps, invariants) from what the code does; then navigate with source plan:{} to show the user only those components, from the entry point.",
                    plan.id, plan.id
                ),
            })
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
            text.push_str(&self.steps(plan.id.clone(), None).await?);
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
            let steps = self.steps(plan.id.clone(), None).await?;
            Ok(format!("Plan {} now has {} components, {} relations.{steps}", plan.id, plan.components.len(), plan.relations.len()))
        }
        "plan_validate" => {
            let a: PlanArgs = parse(args)?;
            let id = plan_id(&a.plan)?;
            let plan = id.clone();
            let report = blocking(&self.arch, move |s| s.validate(&id)).await?;
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
            out.push_str(&self.steps(plan, None).await?);
            Ok(out)
        }
        "contract" => {
            let a: ComponentArgs = parse(args)?;
            let plan = plan_id(&a.plan)?;
            let cid = ComponentId::new(a.id);
            let (p, c) = (plan.clone(), cid.clone());
            let slice = blocking(&self.arch, move |s| s.contract(&p, &c)).await?;
            Ok(format!("{}{}", slice.to_markdown(), self.steps(plan, Some(cid)).await?))
        }
            other => Err(ServiceError::NotFound(format!("tool '{other}'"))),
        }
    }
}
