//! Contract slices: exactly what one session needs to work on one component,
//! with real context from the code.

use std::fmt::Write;

use arch_graph::{CodeAnchor, Component, ComponentId, Graph, ImplementationSpec, Plan, ProposedLocation, RelationKind, Signature, Span, Stage, Status};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Neighbor {
    pub id: ComponentId,
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
    /// Where it is in the code (None if it's planned too).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<CodeAnchor>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContractSlice {
    pub plan: arch_graph::PlanId,
    pub id: ComponentId,
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub description: String,
    pub status: Status,
    pub stage: Stage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<ProposedLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<CodeAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<ImplementationSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<Neighbor>,
    /// What it must call, in order (real signatures for existing ones).
    pub calls: Vec<Neighbor>,
    /// Who calls it.
    pub called_by: Vec<Neighbor>,
    pub implements: Vec<Neighbor>,
    /// Placeholder bodies still in its code.
    pub open_placeholders: Vec<Span>,
}

fn neighbor(graph: &Graph, id: &ComponentId, note: &str) -> Option<Neighbor> {
    let c = graph.component(id)?;
    Some(Neighbor {
        id: c.id.clone(),
        name: c.name.clone(),
        type_name: c.type_name.clone(),
        status: c.status,
        signature: c.signature.clone(),
        anchor: c.anchor.clone(),
        note: note.to_owned(),
    })
}

/// The slice for `planned` in `plan`; `graph` is the code merged with the plan.
pub fn slice(plan: &Plan, planned: &Component, graph: &Graph) -> ContractSlice {
    let id = &planned.id;
    let merged = graph.component(id).unwrap_or(planned);
    let mut calls: Vec<(u32, Neighbor)> = graph
        .outgoing(id, Some(RelationKind::Calls))
        .filter_map(|r| neighbor(graph, &r.target, &r.note).map(|n| (r.order.unwrap_or(u32::MAX), n)))
        .collect();
    calls.sort_by_key(|(order, n)| (*order, n.id.clone()));
    ContractSlice {
        plan: plan.id.clone(),
        id: id.clone(),
        name: merged.name.clone(),
        type_name: merged.type_name.clone(),
        description: merged.description.clone(),
        status: merged.status,
        stage: merged.stage,
        proposed: merged.proposed.clone(),
        anchor: merged.anchor.clone(),
        signature: merged.signature.clone(),
        spec: merged.spec.clone(),
        container: graph.parent(id).and_then(|p| neighbor(graph, p, "")),
        calls: calls.into_iter().map(|(_, n)| n).collect(),
        called_by: graph.incoming(id, Some(RelationKind::Calls)).filter_map(|r| neighbor(graph, &r.source, &r.note)).collect(),
        implements: graph.outgoing(id, Some(RelationKind::Implements)).filter_map(|r| neighbor(graph, &r.target, "")).collect(),
        open_placeholders: merged.placeholders.clone(),
    }
}

fn sig(s: &Option<Signature>) -> String {
    match s {
        Some(s) => {
            let params: Vec<String> = s.params.iter().map(|p| match &p.ty {
                Some(t) => format!("{}: {t}", p.name),
                None => p.name.clone(),
            }).collect();
            match &s.returns {
                Some(r) => format!("({}) -> {r}", params.join(", ")),
                None => format!("({})", params.join(", ")),
            }
        }
        None => "(no signature)".to_owned(),
    }
}

fn where_(n: &Neighbor) -> String {
    match &n.anchor {
        Some(a) => format!("{}:{}", a.file, a.span.start_line),
        None => "planned".to_owned(),
    }
}

impl ContractSlice {
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# {} `{}` ({}, {:?}/{:?})", self.type_name, self.name, self.id, self.status, self.stage);
        if !self.description.is_empty() {
            let _ = writeln!(out, "\n{}", self.description);
        }
        out.push_str("\n## Location\n");
        match (&self.anchor, &self.proposed) {
            (Some(a), _) => {
                let _ = writeln!(out, "In the code: `{}` in {} (lines {}-{}).", a.symbol, a.file, a.span.start_line, a.span.end_line);
            }
            (None, Some(p)) => {
                let _ = writeln!(out, "Put it in {} as `{}`{}.", p.file, p.symbol, p.parent.as_ref().map(|x| format!(", inside {x}")).unwrap_or_default());
            }
            (None, None) => out.push_str("Not placed yet.\n"),
        }
        let _ = writeln!(out, "\n## Signature\n`{}`", sig(&self.signature));
        if let Some(spec) = &self.spec {
            if let Some(p) = &spec.pattern {
                let _ = writeln!(out, "\n## Pattern\n{p}");
            }
            if !spec.logic_steps.is_empty() {
                out.push_str("\n## Logic steps\n");
                for s in &spec.logic_steps {
                    let _ = writeln!(out, "{}. **{}**: {}", s.sequence, s.name, s.description);
                }
            }
            if !spec.invariants.is_empty() {
                out.push_str("\n## Invariants\n");
                for i in &spec.invariants {
                    let _ = writeln!(out, "- **{}** ({}): {}", i.name, i.kind, i.description);
                }
            }
        }
        let section = |out: &mut String, title: &str, list: &[Neighbor]| {
            if list.is_empty() {
                return;
            }
            let _ = writeln!(out, "\n## {title}");
            for n in list {
                let _ = writeln!(out, "- `{}` {} {} — {}{}", n.name, n.type_name, sig(&n.signature), where_(n), if n.note.is_empty() { String::new() } else { format!(" ({})", n.note) });
            }
        };
        section(&mut out, "Must call (in order)", &self.calls);
        section(&mut out, "Called by", &self.called_by);
        section(&mut out, "Implements", &self.implements);
        if let Some(c) = &self.container {
            let _ = writeln!(out, "\n## Inside\n`{}` ({}) — {}", c.name, c.type_name, where_(c));
        }
        if !self.open_placeholders.is_empty() {
            let lines: Vec<String> = self.open_placeholders.iter().map(|s| s.start_line.to_string()).collect();
            let _ = writeln!(out, "\n## Open placeholders\nLines {} still have placeholder bodies.", lines.join(", "));
        }
        out
    }
}
