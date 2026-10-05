//! Implementation-spec rules: logic steps numbered 1..n, invariants that
//! keep the kind of the one they redefine, validation tools that exist, and
//! side effects the interface (or the caller) declares.

use std::collections::{BTreeMap, BTreeSet};

use arch_graph::{Finding, FindingCode, Graph, Plan, RelationKind, Report, Target};

use super::component;
use crate::ToolCatalog;

pub(super) fn rules(plan: &Plan, graph: &Graph, tools: &ToolCatalog, report: &mut Report) {
    for planned in &plan.components {
        let Some(c) = graph.component(&planned.id) else { continue };
        if let Some(spec) = &c.spec {
            let mut seen: BTreeMap<u32, usize> = BTreeMap::new();
            for step in &spec.logic_steps {
                *seen.entry(step.sequence).or_default() += 1;
            }
            for (seq, n) in seen.iter().filter(|(_, n)| **n > 1) {
                report.push(Finding::error(FindingCode::LogicStepOrder, component(&c.id), format!("logic step {seq} appears {n} times")));
            }
            let expected: Vec<u32> = (1..=seen.len() as u32).collect();
            let actual: Vec<u32> = seen.keys().copied().collect();
            if actual != expected {
                report.push(Finding::error(
                    FindingCode::LogicStepOrder,
                    component(&c.id),
                    format!("logic steps must be numbered 1..{} without gaps; found {actual:?}", seen.len()),
                ));
            }
            // What the interfaces declare, nearest first (before any override).
            let inherited: Vec<_> = graph
                .interface_chain(&c.id)
                .into_iter()
                .filter_map(|i| graph.component(&i).and_then(|ic| ic.spec.as_ref()).map(|s| (i, s.invariants.clone())))
                .flat_map(|(i, invs)| invs.into_iter().map(move |inv| (i.clone(), inv)))
                .collect();
            for own in &spec.invariants {
                if let Some((from, base)) = inherited.iter().find(|(_, i)| i.name == own.name && i.kind != own.kind) {
                    report.push(Finding::error(
                        FindingCode::InvariantMismatch,
                        component(&c.id),
                        format!("invariant '{}' is a {} here but a {} in {from}", own.name, own.kind, base.kind),
                    ));
                }
            }
            for tool in spec.validation.iter().filter(|t| tools.get(&t.tool).is_none()) {
                report.push(
                    Finding::error(FindingCode::UnknownTool, component(&c.id), format!("validation tool '{}' isn't defined", tool.tool))
                        .suggest("define it in .nah/tools.toml or remove the reference"),
                );
            }
        }
        // Side effects stay within what the interface declares.
        let declared: BTreeSet<String> =
            graph.interface_chain(&c.id).iter().filter_map(|i| graph.component(i)).flat_map(|i| i.schemas.side_effects.iter().map(|e| e.target.clone())).collect();
        let has_interface = graph.outgoing(&c.id, Some(RelationKind::Implements)).next().is_some();
        if has_interface {
            for effect in c.schemas.side_effects.iter().filter(|e| !declared.contains(&e.target)) {
                report.push(Finding::error(
                    FindingCode::UndeclaredSideEffect,
                    component(&c.id),
                    format!("changes '{}', which the interface it implements doesn't declare", effect.target),
                ));
            }
        }
    }
    // A caller that states what a call may change must expect everything the
    // callee changes.
    for r in plan.relations.iter().filter(|r| r.kind == RelationKind::Calls) {
        let (Some(expects), Some(callee)) = (&r.expects, graph.component(&r.target)) else { continue };
        let expected: BTreeSet<&str> = expects.side_effects.iter().map(|e| e.target.as_str()).collect();
        for effect in callee.schemas.side_effects.iter().filter(|e| !expected.contains(e.target.as_str())) {
            report.push(Finding::error(
                FindingCode::UndeclaredSideEffect,
                Target::Relation { id: r.id() },
                format!("{} also changes '{}', which {} doesn't expect", r.target, effect.target, r.source),
            ));
        }
    }
}
