//! Type rules for planned components: fields the type allows, where it may
//! sit, and whether it can call anything.

use arch_graph::{Finding, FindingCode, Graph, Plan, RelationKind, Report, Target, TypeSystem};

use super::{a, component};

pub(super) fn rules(plan: &Plan, graph: &Graph, types: &TypeSystem, report: &mut Report) {
    for planned in &plan.components {
        let Some(c) = graph.component(&planned.id) else { continue };
        let Ok(ty) = types.resolve(&c.type_name) else { continue }; // reported as unknown
        let mut disallowed = Vec::new();
        if c.schemas.properties.is_some() && !ty.allows_properties {
            disallowed.push("properties");
        }
        if !ty.allows_signature {
            if planned.signature.is_some() {
                disallowed.push("signature");
            }
            if c.schemas.inputs.is_some() {
                disallowed.push("inputs");
            }
            if c.schemas.outputs.is_some() {
                disallowed.push("outputs");
            }
        }
        if c.spec.is_some() && !ty.allows_implementation_spec {
            disallowed.push("implementation spec");
        }
        for field in disallowed {
            report.push(Finding::error(FindingCode::DisallowedField, component(&c.id), format!("{} can't have {field}", a(&c.type_name))));
        }
        if let Some(parent) = graph.parent(&c.id).and_then(|p| graph.component(p)) {
            let fits = types.resolve(&parent.type_name).is_ok_and(|p| ty.allowed_parents.iter().any(|allowed| p.is_a(allowed)));
            if !fits {
                let allowed = if ty.allowed_parents.is_empty() { "nothing (top level only)".to_owned() } else { ty.allowed_parents.join(", ") };
                report.push(Finding::error(
                    FindingCode::DisallowedParent,
                    component(&c.id),
                    format!("{} can't sit in {} ({}); it may sit in: {allowed}", a(&c.type_name), parent.id, a(&parent.type_name)),
                ));
            }
        }
    }
    for r in plan.relations.iter().filter(|r| r.kind == RelationKind::Calls) {
        let Some(caller) = graph.component(&r.source) else { continue };
        if types.resolve(&caller.type_name).is_ok_and(|t| !t.executable) {
            report.push(Finding::error(
                FindingCode::NotExecutable,
                Target::Relation { id: r.id() },
                format!("{} is {}, which can't call {}", caller.id, a(&caller.type_name), r.target),
            ));
        }
    }
}
