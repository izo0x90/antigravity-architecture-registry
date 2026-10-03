//! References resolve, locations are sound, the location rules hold, and
//! new work is tied into the existing code.

use std::path::Path;

use arch_graph::{Finding, FindingCode, Graph, Plan, RelationKind, Report, Status, Target, TypeSystem};
use arch_index::{PackSet, module_symbol};

use super::component;

/// Unknown types and the location rules, per planned component.
pub(super) fn components(plan: &Plan, graph: &Graph, root: &Path, packs: &PackSet, types: &TypeSystem, report: &mut Report) {
    for planned in &plan.components {
        let Some(merged) = graph.component(&planned.id) else { continue };
        if !types.contains(&merged.type_name) {
            report.push(Finding::error(FindingCode::UnknownType, component(&merged.id), format!("unknown type '{}'", merged.type_name)));
        }
        if let Err(err) = merged.check_location() {
            report.push(Finding::error(FindingCode::LocationRule, component(&merged.id), err.to_string()));
        }
        if planned.status == Status::New {
            location(planned, graph, root, packs, report);
        }
    }
}

pub(super) fn references(plan: &Plan, graph: &Graph, report: &mut Report) {
    for c in &plan.components {
        if let Some(parent) = c.proposed.as_ref().and_then(|p| p.parent.as_ref())
            && graph.component(parent).is_none()
        {
            report.push(Finding::error(FindingCode::DanglingReference, component(&c.id), format!("its parent {parent} doesn't exist")));
        }
    }
    for r in &plan.relations {
        for end in [&r.source, &r.target] {
            if graph.component(end).is_none() {
                report.push(Finding::error(
                    FindingCode::DanglingReference,
                    Target::Relation { id: r.id() },
                    format!("{} relation {} -> {} refers to {end}, which doesn't exist", r.kind.name(), r.source, r.target),
                ));
            }
        }
    }
    for reference in &plan.references {
        if graph.component(&reference.id).is_none_or(|c| c.anchor.is_none()) {
            report.push(Finding::error(
                FindingCode::DanglingReference,
                Target::Plan { plan: plan.id.clone() },
                format!("the plan builds on {}, which isn't in the code (anymore)", reference.id),
            ));
        }
    }
}

fn location(c: &arch_graph::Component, graph: &Graph, root: &Path, packs: &PackSet, report: &mut Report) {
    let Some(proposed) = &c.proposed else { return };
    let path = Path::new(proposed.file.as_str());
    let Some(pack) = packs.for_path(path) else {
        report.push(Finding::error(FindingCode::InvalidLocation, component(&c.id), format!("no language pack handles {}", proposed.file)));
        return;
    };
    if pack.name() != proposed.language {
        report.push(Finding::error(
            FindingCode::InvalidLocation,
            component(&c.id),
            format!("{} is {} but the component is planned as {}", proposed.file, pack.name(), proposed.language),
        ));
    }
    let absolute = root.join(path);
    if !absolute.exists() && absolute.parent().is_some_and(|dir| !dir.is_dir()) {
        report.push(
            Finding::error(FindingCode::InvalidLocation, component(&c.id), format!("{} would be a new file in a directory that doesn't exist", proposed.file))
                .suggest("place it in an existing directory, or plan the directory's module first"),
        );
    }
    // The proposed file must be the parent's own file, or be the parent module.
    if let Some(parent_id) = &proposed.parent
        && let Some(parent) = graph.component(parent_id)
    {
        let parent_symbol = parent.anchor.as_ref().map(|a| a.symbol.clone()).or(parent.proposed.as_ref().map(|p| p.symbol.clone())).unwrap_or_default();
        let same_file = parent.file() == Some(&proposed.file);
        let file_module = module_symbol(root, &proposed.file, pack);
        if !same_file && file_module != parent_symbol {
            report.push(Finding::error(
                FindingCode::InvalidLocation,
                component(&c.id),
                format!("its parent {} lives in {}, but {} is module {file_module}", parent.id, parent.file().map_or("?", |f| f.as_str()), proposed.file),
            ));
        }
    }
    // Before it's built, its symbol showing up in the code means a clash.
    if graph.provenance(&c.id).observed && !c.stage.expects_code() {
        report.push(
            Finding::warning(FindingCode::NameCollision, component(&c.id), format!("{} already exists in the code", proposed.symbol))
                .suggest("modify the existing component, or pick another name"),
        );
    }
}

/// New components must connect to existing code (calls, data flow or
/// implements, in either direction) unless the plan is greenfield; a plan
/// that modifies existing components is tied in by definition.
pub(super) fn touch_points(plan: &Plan, graph: &Graph, report: &mut Report) {
    let new: Vec<_> = plan.components.iter().filter(|c| c.status == Status::New).map(|c| &c.id).collect();
    if new.is_empty() || plan.greenfield || plan.components.iter().any(|c| c.status == Status::Modifying) {
        return;
    }
    let existing = |id: &arch_graph::ComponentId| graph.provenance(id).observed && !new.contains(&id);
    let tied = plan.relations.iter().any(|r| {
        r.kind != RelationKind::Contains && ((new.contains(&&r.source) && existing(&r.target)) || (new.contains(&&r.target) && existing(&r.source)))
    });
    if !tied {
        report.push(
            Finding::error(
                FindingCode::MissingTouchPoint,
                Target::Plan { plan: plan.id.clone() },
                "none of the new components connect to existing code (no calls, data flow or implements with an existing component)",
            )
            .suggest("relate the new work to where it integrates (e.g. which existing function calls it), or mark the plan greenfield"),
        );
    }
}
