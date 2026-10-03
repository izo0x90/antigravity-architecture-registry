//! Completeness and lifecycle gates: the details a type requires before
//! approval, and how far along a component's dependencies must be for each
//! stage.

use arch_graph::{Component, ComponentId, Finding, FindingCode, Graph, Plan, Report, RequiredField, ResolvedType, Stage, TypeSystem};

use super::{a, component, is_data_type};

/// The details `c`'s type requires that it doesn't have yet.
pub fn missing_details(c: &Component, ty: &ResolvedType) -> Vec<RequiredField> {
    let spec = c.spec.as_ref();
    ty.required_fields
        .iter()
        .copied()
        .filter(|field| {
            let present = match field {
                RequiredField::Description => !c.description.trim().is_empty(),
                RequiredField::Signature => c.signature.is_some() || c.schemas.inputs.is_some() || c.schemas.outputs.is_some(),
                RequiredField::Inputs => c.schemas.inputs.is_some(),
                RequiredField::Outputs => c.schemas.outputs.is_some(),
                RequiredField::Properties => c.schemas.properties.is_some(),
                RequiredField::SideEffects => !c.schemas.side_effects.is_empty(),
                RequiredField::Spec => spec.is_some(),
                RequiredField::Pattern => spec.is_some_and(|s| s.pattern.is_some()),
                RequiredField::LogicSteps => spec.is_some_and(|s| !s.logic_steps.is_empty()),
                RequiredField::Invariants => spec.is_some_and(|s| !s.invariants.is_empty()),
                RequiredField::Validation => spec.is_some_and(|s| !s.validation.is_empty()),
            };
            !present
        })
        .collect()
}

/// The least stage a planned dependency must have for `stage`.
fn needed(stage: Stage) -> Option<Stage> {
    match stage {
        Stage::Declared | Stage::ArchApproved => None,
        // Designing or laying out structure builds on approved designs.
        Stage::PlanApproved | Stage::Skeleton => Some(Stage::ArchApproved),
        // Implementing needs the structure it calls into to exist.
        Stage::Implemented => Some(Stage::Skeleton),
    }
}

/// Why `id` can't be at `stage` yet because of what it depends on. Code that
/// isn't part of the plan already exists and never holds anything up.
pub fn stage_gate(plan: &Plan, graph: &Graph, types: &TypeSystem, id: &ComponentId, stage: Stage) -> Vec<Finding> {
    let Some(min) = needed(stage) else { return Vec::new() };
    graph
        .dependencies(id, is_data_type(types))
        .into_iter()
        .filter_map(|dep| plan.component(&dep).map(|d| (dep, d.stage)))
        .filter(|(_, at)| *at < min)
        .map(|(dep, at)| {
            Finding::error(FindingCode::DependencyNotReady, component(id), format!("{stage:?} needs {dep} at {min:?} or later; it is {at:?}"))
                .suggest(format!("move {dep} along first"))
        })
        .collect()
}

pub(super) fn check(plan: &Plan, graph: &Graph, types: &TypeSystem, report: &mut Report) {
    for planned in &plan.components {
        let Some(c) = graph.component(&planned.id) else { continue };
        if let Ok(ty) = types.resolve(&c.type_name) {
            let missing = missing_details(c, &ty);
            if !missing.is_empty() {
                let names: Vec<&str> = missing.iter().map(|f| f.name()).collect();
                let msg = format!("{} needs {} before approval", a(&c.type_name), names.join(", "));
                report.push(if c.stage == Stage::Declared {
                    Finding::warning(FindingCode::SpecIncomplete, component(&c.id), msg)
                } else {
                    Finding::error(FindingCode::SpecIncomplete, component(&c.id), msg)
                });
            }
        }
        for f in stage_gate(plan, graph, types, &c.id, c.stage) {
            report.push(f);
        }
    }
}
