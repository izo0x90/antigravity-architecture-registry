//! Loops: containment, implements and schema references, each reported once
//! with its path when the plan takes part in it; missing schema references.

use std::collections::BTreeSet;

use arch_graph::{ComponentId, Finding, FindingCode, Graph, Plan, RelationKind, Report, SchemaSet, Target};

use super::component;

pub(super) fn loops(plan: &Plan, graph: &Graph, set: &SchemaSet, report: &mut Report) {
    let planned: BTreeSet<&ComponentId> = plan.components.iter().map(|c| &c.id).collect();
    for kind in [RelationKind::Contains, RelationKind::Implements] {
        for cycle in graph.cycles(kind) {
            if cycle.iter().any(|id| planned.contains(id)) {
                let path: Vec<&str> = cycle.iter().map(ComponentId::as_str).collect();
                report.push(Finding::error(
                    FindingCode::Cycle,
                    component(&cycle[0]),
                    format!("{} loop: {}", kind.name(), path.join(" -> ")),
                ));
            }
        }
    }
    let planned_names: BTreeSet<&str> = plan.components.iter().map(|c| c.name.as_str()).collect();
    for cycle in set.cycles() {
        if cycle.iter().any(|t| planned_names.contains(t.as_str())) {
            report.push(Finding::error(
                FindingCode::Cycle,
                Target::Plan { plan: plan.id.clone() },
                format!("schema reference loop: {}", cycle.join(" -> ")),
            ));
        }
    }
    for planned in &plan.components {
        let Some(c) = graph.component(&planned.id) else { continue };
        for (field, schema) in [("properties", &c.schemas.properties), ("inputs", &c.schemas.inputs), ("outputs", &c.schemas.outputs)] {
            for missing in schema.as_ref().map(|s| set.missing(s)).unwrap_or_default() {
                report.push(
                    Finding::error(
                        FindingCode::MissingSchemaRef,
                        component(&c.id),
                        format!("its {field} reference {missing}, which isn't a data type here"),
                    )
                    .suggest("add the data type to the plan, or reference an existing one by name"),
                );
            }
        }
    }
}

/// References to a name more than one data type has.
pub(super) fn ambiguous_refs(
    plan: &Plan,
    graph: &Graph,
    ambiguous: &std::collections::BTreeMap<String, Vec<ComponentId>>,
    report: &mut Report,
) {
    if ambiguous.is_empty() {
        return;
    }
    for planned in &plan.components {
        let Some(c) = graph.component(&planned.id) else { continue };
        let mut named: BTreeSet<String> = BTreeSet::new();
        for schema in [&c.schemas.properties, &c.schemas.inputs, &c.schemas.outputs].into_iter().flatten() {
            named.extend(arch_graph::schema::refs(schema));
        }
        for title in named {
            if let Some(ids) = ambiguous.get(&title) {
                let list: Vec<&str> = ids.iter().map(ComponentId::as_str).collect();
                report.push(
                    Finding::error(
                        FindingCode::AmbiguousSchemaRef,
                        component(&c.id),
                        format!("{title} could be any of {}", list.join(", ")),
                    )
                    .suggest("rename one of the data types so the reference is unique"),
                );
            }
        }
    }
}
