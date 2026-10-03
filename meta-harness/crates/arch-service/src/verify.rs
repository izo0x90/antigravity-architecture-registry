//! Implementation verification: does the code now contain what the plan says,
//! where it says? Finds the symbol (or reports it moved/missing), confirms the
//! planned calls happen inside its body, compares its signature with the
//! plan, and tracks placeholder bodies for the skeleton stage.

use arch_graph::{Component, ComponentId, Finding, FindingCode, Graph, Plan, Relation, RelationKind, Report, Signature, Stage, Target};

fn target(id: &ComponentId) -> Target {
    Target::Component { id: id.clone() }
}

fn normalize(ty: &str) -> String {
    ty.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Differences between the planned and the observed signature (only what the
/// plan states is compared: a planned param without a type matches any type).
pub fn signature_diff(planned: &Signature, observed: &Signature) -> Vec<String> {
    let mut diff = Vec::new();
    let names = |s: &Signature| s.params.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    if planned.params.len() != observed.params.len() {
        diff.push(format!("parameters: planned ({}) vs code ({})", names(planned).join(", "), names(observed).join(", ")));
    } else {
        for (p, o) in planned.params.iter().zip(&observed.params) {
            if p.name != o.name {
                diff.push(format!("parameter '{}' is named '{}' in the code", p.name, o.name));
            }
            if let Some(pt) = &p.ty
                && o.ty.as_deref().map(normalize) != Some(normalize(pt))
            {
                diff.push(format!("parameter '{}': planned type {pt}, code has {}", p.name, o.ty.as_deref().unwrap_or("none")));
            }
        }
    }
    if let Some(r) = &planned.returns
        && observed.returns.as_deref().map(normalize) != Some(normalize(r))
    {
        diff.push(format!("returns: planned {r}, code has {}", observed.returns.as_deref().unwrap_or("nothing")));
    }
    diff
}

/// Does the code make this planned call? None when it does; otherwise a
/// MissingCallSite, or WrongCallTarget when it calls something of the same
/// name instead.
pub(crate) fn check_planned_call(relation: &Relation, observed: &Graph) -> Option<Finding> {
    let observed_calls: Vec<_> = observed.outgoing(&relation.source, Some(RelationKind::Calls)).collect();
    if observed_calls.iter().any(|r| r.target == relation.target) {
        return None;
    }
    let last = |id: &ComponentId| id.as_str().rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_owned();
    let target_name = last(&relation.target);
    let similar: Vec<String> = observed_calls.iter().filter(|r| last(&r.target) == target_name).map(|r| r.target.to_string()).collect();
    let (source, target) = (&relation.source, &relation.target);
    Some(if similar.is_empty() {
        Finding::error(FindingCode::MissingCallSite, Target::Relation { id: relation.id() }, format!("{source} doesn't call {target}"))
    } else {
        Finding::error(FindingCode::WrongCallTarget, Target::Relation { id: relation.id() }, format!("{source} calls {} instead of {target}", similar.join(", ")))
    })
}

/// Verifies one planned component against the observed code, for reaching
/// `goal` (Skeleton tolerates placeholder bodies; Implemented doesn't).
pub fn verify_component(plan: &Plan, planned: &Component, observed: &Graph, goal: Stage) -> Report {
    let mut report = Report::default();
    let id = &planned.id;
    let Some(code) = observed.component(id) else {
        // Not where planned: same name and type elsewhere means it moved.
        let elsewhere: Vec<&Component> = observed.named(&planned.name).filter(|c| c.type_name == planned.type_name).collect();
        let at = planned.proposed.as_ref().map(|p| format!("{} in {}", p.symbol, p.file)).unwrap_or_else(|| id.to_string());
        if elsewhere.is_empty() {
            report.push(
                Finding::error(FindingCode::MissingSymbol, target(id), format!("{at} isn't in the code"))
                    .suggest("implement it at its proposed location, then verify again"),
            );
        } else {
            let found: Vec<String> = elsewhere.iter().map(|c| c.anchor.as_ref().map(|a| format!("{} ({})", a.symbol, a.file)).unwrap_or_default()).collect();
            let mut finding = Finding::error(FindingCode::Moved, target(id), format!("expected {at}, found {}", found.join(", ")));
            if let Some(anchor) = elsewhere[0].anchor.as_ref() {
                finding = finding.at(anchor.file.clone(), anchor.span);
            }
            report.push(finding.suggest("move it to the proposed location, or update the plan to where it is"));
        }
        return report;
    };
    let anchor = code.anchor.clone();
    let at = |f: Finding| match &anchor {
        Some(a) => f.at(a.file.clone(), a.span),
        None => f,
    };

    // Planned calls from this component must happen inside its body.
    for relation in plan.relations.iter().filter(|r| r.kind == RelationKind::Calls && &r.source == id) {
        if let Some(finding) = check_planned_call(relation, observed) {
            report.push(at(finding));
        }
    }

    if let (Some(planned_sig), Some(code_sig)) = (&planned.signature, &code.signature) {
        let diff = signature_diff(planned_sig, code_sig);
        if !diff.is_empty() {
            report.push(at(Finding::error(FindingCode::SignatureMismatch, target(id), diff.join("; "))));
        }
    } else if planned.signature.is_some() {
        report.push(at(Finding::error(FindingCode::SignatureMismatch, target(id), "planned with a signature, but the code has none")));
    }

    if !code.placeholders.is_empty() {
        let message = format!("{} placeholder bodies remain", code.placeholders.len());
        let finding = match goal {
            Stage::Implemented => Finding::error(FindingCode::PlaceholderRemains, target(id), message).suggest("implement the bodies, then verify again"),
            _ => Finding { severity: arch_graph::Severity::Info, ..Finding::error(FindingCode::PlaceholderRemains, target(id), message) },
        };
        let finding = match (&anchor, code.placeholders.first()) {
            (Some(a), Some(span)) => finding.at(a.file.clone(), *span),
            _ => finding,
        };
        report.push(finding);
    }
    report.sorted()
}

/// Each planned data flow into or out of `id` ends up confirmed (the code
/// hands the source's output to the target's input: no finding), missing
/// (both ends are in the code but it doesn't), or unverifiable (the hand-off
/// is there but types can't confirm it). Flows with an end not in the code
/// yet wait for that end's own verification.
pub fn planned_flows(plan: &Plan, id: &ComponentId, observed: &Graph, unverified: &[arch_index::UnverifiedFlow]) -> Vec<Finding> {
    let mut out = Vec::new();
    for flow in plan.relations.iter().filter(|r| r.kind == RelationKind::DataFlow && (&r.source == id || &r.target == id)) {
        let in_code = |c: &ComponentId| observed.component(c).is_some_and(|c| c.anchor.is_some());
        if !in_code(&flow.source) || !in_code(&flow.target) {
            continue;
        }
        let wanted_port = flow.ports.as_ref().map(|(_, to)| to.as_str());
        let confirmed = observed
            .outgoing(&flow.source, Some(RelationKind::DataFlow))
            .any(|r| r.target == flow.target && (wanted_port.is_none() || r.ports.as_ref().map(|(_, to)| to.as_str()) == wanted_port));
        if confirmed {
            continue;
        }
        let ports = flow.ports.as_ref().map(|(from, to)| format!(" ({from} -> {to})")).unwrap_or_default();
        let target = Target::Relation { id: flow.id() };
        match unverified.iter().find(|u| u.source == flow.source && u.target == flow.target) {
            Some(u) => out.push(
                Finding::warning(FindingCode::DataFlowUnverifiable, target, format!("{} -> {}{ports} is handed over but can't be confirmed: {}", flow.source, flow.target, u.reason))
                    .at(u.file.clone(), u.span)
                    .suggest("declare the types involved so the flow can be checked"),
            ),
            None => out.push(
                Finding::error(FindingCode::DataFlowMissing, target, format!("the code doesn't pass {}'s output into {}{ports}", flow.source, flow.target))
                    .suggest("call the target with the source's result, or update the plan"),
            ),
        }
    }
    out
}
