//! Reading the code's structure, and pointing the user's graph at it.

use std::fmt::Write;
use std::sync::Arc;

use arch_graph::tree::{TreeConfig, ViewKind};
use arch_graph::{Requester, Source, ViewRequest};
use arch_service::{ArchitectureService, ServiceError};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ToolSet;
use super::schema::{Fragments, tool};
use crate::arch::blocking;
use nah_access::Capability;

use super::ToolDef;
use crate::mcp::text::*;

pub(crate) struct Explore {
    pub(crate) arch: Arc<ArchitectureService>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeArgs {
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    kind: arch_service::SearchKind,
    #[serde(default)]
    source: Source,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    id: String,
    #[serde(default)]
    source: Source,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewArgs {
    view: String,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    way: Option<arch_graph::tree::Way>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    source: Source,
    #[serde(default)]
    full_code: bool,
    #[serde(default)]
    open: Vec<String>,
    #[serde(default)]
    expand: Option<String>,
    #[serde(default)]
    levels: Option<u32>,
    #[serde(default)]
    context: Option<String>,
}

// An argument it doesn't know is refused, never ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NavigateArgs {
    #[serde(default)]
    view: Option<String>,
    /// "code", or "plan:<id>" (absent: leave it as it is).
    #[serde(default)]
    source: Option<Source>,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    way: Option<arch_graph::tree::Way>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    select: Option<String>,
    #[serde(default)]
    column: Option<u32>,
}

impl Explore {
    /// Its tools, as agents see them (in the list's order).
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<ToolDef> {
        let Fragments { tree_help, card, source, .. } = Fragments::new(trees);
        vec![
            tool(
                Capability::ViewRead,
                "ingest",
                "Read the code into the architecture graph (incremental: only changed files are parsed). Run it after editing code, or with a path to refresh just that part.",
                json!({ "type": "object", "properties": { "scope": { "type": "string", "description": "Repo-relative path; the whole repo when absent" } } }),
            ),
            tool(Capability::ViewRead, "list_types", "The component types available (generic defaults and any specializations), with their rules.", json!({ "type": "object", "properties": {} })),
            tool(
                Capability::ViewRead,
                "search",
                "Find what your work will touch: symbols by name (fuzzy, a typo forgiven; `a::b` matches whole symbols), files by path, or lines by text (as written; `*.rs` or `src/` narrows the files). Each hit gives the card to inspect or view. Query words all must match; 'word matches as written, ^word starts, word$ ends, !word excludes.",
                json!({ "type": "object", "properties": {
                    "query": { "type": "string" },
                    "kind": { "type": "string", "enum": ["symbols", "files", "text"], "description": "symbols (the default), files or text" },
                    "source": source,
                    "limit": { "type": "integer" }
                }, "required": ["query"] }),
            ),
            tool(
                Capability::ViewRead,
                "inspect",
                "One card's component: type, stage, location (file:symbol), signature, relations in and out (each link found in the code says whether the language's analyzer confirmed it or it was matched by name), and its source code. Any card a view shows: a component, a data type (type:<Name>), a flow's start or end (start:<id>, end:<id>).",
                json!({ "type": "object", "properties": { "id": card, "source": source }, "required": ["id"] }),
            ),
            tool(
                Capability::ViewRead,
                "view",
                &format!("One of the three views as a text tree, the same one the user sees: each card with where it is, what it hides (+N more), what sits on its line, and lines back to cards shown elsewhere. With a plan: its own items first, marked NEW / CHANGED / DONE / DOESN'T MATCH. Give `expand` to get only what opening one card adds, or `context` to see what holds a card and what's beside it.\n{tree_help}"),
                json!({ "type": "object", "properties": {
                    "view": { "type": "string", "enum": ViewKind::ALL.iter().map(|v| v.name()).collect::<Vec<_>>() },
                    "root": card,
                    "way": { "type": "string", "enum": ["down", "up"], "description": "down: what it calls, holds, turns into; up: what calls it, holds it, it came from" },
                    "depth": { "type": "integer" },
                    "source": source,
                    "full_code": { "type": "boolean", "description": "With a plan: show it in the whole codebase, not only its items and what they plug into" },
                    "open": { "type": "array", "items": card, "description": "Cards to open one level further" },
                    "expand": card,
                    "levels": { "type": "integer", "description": "With expand: how many levels (default 1)" },
                    "context": card
                }, "required": ["view"] }),
            ),
            tool(
                Capability::ViewDrive,
                "navigate",
                &format!("Show something in the user's graph (moves their view; they can go back). Use it to point at what you're discussing. Leave out what should stay as the user has it. The user may have several graphs, one per column: `column` picks one (counted from 1 on the left; a column that isn't a graph becomes one); without it, the graph they used last moves.\n{tree_help}"),
                json!({ "type": "object", "properties": {
                    "view": { "type": "string", "enum": ViewKind::ALL.iter().map(|v| v.name()).collect::<Vec<_>>() },
                    "source": source,
                    "root": card,
                    "way": { "type": "string", "enum": ["down", "up"] },
                    "depth": { "type": "integer" },
                    "select": card,
                    "column": { "type": "integer", "description": "The user's column to show it in, from 1 on the left (default: the graph they used last)" }
                } }),
            ),
        ]
    }

    /// The next steps for `plan` (or one component of it), from its current state.
    async fn steps(&self, plan: arch_graph::PlanId, only: Option<arch_graph::ComponentId>) -> Result<String, ServiceError> {
        steps(&self.arch, plan, only).await
    }
}

#[async_trait::async_trait]
impl ToolSet for Explore {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value) -> Result<String, ServiceError> {
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
        "list_types" => Ok(format!(
            "{}\n\nNext: use these type names in plan_edit add_component; a type's required_fields must be filled before the user can approve.",
            pretty(self.arch.types())?
        )),
        "search" => {
            let a: SearchArgs = parse(args)?;
            let source = a.source.clone();
            let (found, states) = blocking(&self.arch, move |s| {
                let found = s.search(&a.query, a.kind, &source, a.limit)?;
                // Each card's status and stage, as the agent works by them.
                let graph = s.graph(&source)?;
                let states: Vec<String> = found
                    .hits
                    .iter()
                    .map(|h| h.key.as_ref().and_then(|k| graph.component(&arch_graph::ComponentId::new(k.as_str()))).map(|c| format!(" {:?}/{:?}", c.status, c.stage)).unwrap_or_default())
                    .collect();
                Ok((found, states))
            })
            .await?;
            let mut out = String::new();
            for (h, state) in found.hits.iter().zip(&states) {
                let card = h.key.as_deref().unwrap_or("(no card)");
                let _ = match found.kind {
                    arch_service::SearchKind::Symbols | arch_service::SearchKind::Files => writeln!(out, "{card} [{}{state}] {}", h.detail, h.label),
                    arch_service::SearchKind::Text => writeln!(out, "{}: {}  (card {card}{state})", h.detail, h.label),
                };
            }
            if found.hits.is_empty() {
                out.push_str("No matches. Try fewer letters, another kind (symbols, files, text), or ingest first.\n");
            } else if found.total > found.hits.len() || found.partial {
                let _ = writeln!(out, "({} of {}{} shown; give a bigger limit or a narrower query.)", found.hits.len(), found.total, if found.partial { "+" } else { "" });
            }
            out.push_str("\nNext: inspect a card, or view it (e.g. view view=calls root=<card>) to see what it touches.");
            Ok(out)
        }
        "inspect" => {
            let a: IdArgs = parse(args)?;
            let (source, key) = (a.source.clone(), a.id);
            let detail = blocking(&self.arch, move |s| s.card(&key, &source)).await?;
            let plan = a.source.plan().cloned();
            let cid = detail.component.id.clone();
            let mut out = pretty(&marked(&detail)?)?;
            match plan.filter(|_| detail.component.status != arch_graph::Status::Existing || detail.alignment != arch_graph::Alignment::Observed) {
                Some(plan) => out.push_str(&self.steps(plan, Some(cid)).await?),
                None => out.push_str("\n\nNext: view its neighborhood (view view=calls root=<id>), or reference it in a plan you're writing (plan_edit op=reference)."),
            }
            Ok(out)
        }
        "view" => {
            let a: ViewArgs = parse(args)?;
            let plan = a.source.plan().cloned();
            let view = arch_graph::tree::ViewKind::from_name(&a.view).ok_or_else(|| ServiceError::InvalidOp(format!("view '{}': use contains, calls or data_flow", a.view)))?;
            let request = arch_graph::tree::TreeRequest {
                view,
                source: a.source,
                root: a.root,
                way: a.way.unwrap_or_default(),
                depth: a.depth,
                opened: a.open.into_iter().map(|card| arch_graph::tree::Opened { card, levels: 1 }).collect(),
                closed: Vec::new(),
                full_code: a.full_code,
            };
            let (expand, levels, context) = (a.expand, a.levels.unwrap_or(1), a.context);
            let mut out = blocking(&self.arch, move |s| {
                let tree = match (expand, context) {
                    (Some(card), _) => s.expand(&request, &card, levels)?.added(),
                    (None, Some(card)) => s.context(&request, &card)?,
                    (None, None) => s.tree(&request)?,
                };
                Ok(arch_graph::tree::render(&tree, s.trees()))
            })
            .await?;
            match plan {
                Some(plan) => out.push_str(&self.steps(plan, None).await?),
                None => out.push_str("\nNext: inspect a card, go into one (root=<key>), flip the way, or navigate to show the user the same view."),
            }
            Ok(out)
        }
        "navigate" => {
            let a: NavigateArgs = parse(args)?;
            let view = a.view.as_deref().map(|v| arch_graph::tree::ViewKind::from_name(v).ok_or_else(|| ServiceError::InvalidOp(format!("view '{v}': use contains, calls or data_flow")))).transpose()?;
            let request = ViewRequest { view, source: a.source, root: a.root, way: a.way, depth: a.depth, select: a.select, column: a.column };
            blocking(&self.arch, move |s| s.request_view(request, Requester::Agent)).await?;
            Ok("The user's graph now shows it (they can go back).\n\nNext: explain what they're looking at, or continue with the plan.".into())
        }
            other => Err(ServiceError::NotFound(format!("tool '{other}'"))),
        }
    }
}
