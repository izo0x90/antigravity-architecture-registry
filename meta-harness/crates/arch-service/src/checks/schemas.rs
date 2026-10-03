//! Schema fit: implementations against their interfaces, planned calls
//! against what the callee takes and returns (its planned schemas and, for
//! code that exists, its real signature), and data flows port to port.

use arch_graph::schema::compatibility;
use arch_graph::{Component, Finding, FindingCode, Graph, Plan, Relation, RelationKind, Report, SchemaSet, Target};
use arch_index::PackSet;
use arch_index::typing::carries;
use serde_json::Value;

use super::component;

pub(super) fn fit(plan: &Plan, graph: &Graph, set: &SchemaSet, packs: &PackSet, report: &mut Report) {
    let resolve = |s: &Option<Value>| s.as_ref().and_then(|v| set.resolve(v).ok());
    // Implementations accept what their interface promises to take and
    // return what it promises to give.
    for planned in &plan.components {
        let Some(c) = graph.component(&planned.id) else { continue };
        let interface = graph.interface_chain(&c.id).into_iter().filter_map(|i| graph.component(&i)).find(|i| i.schemas.inputs.is_some() || i.schemas.outputs.is_some());
        let Some(interface) = interface else { continue };
        let mut problems = Vec::new();
        if let (Some(promised), Some(accepted)) = (resolve(&interface.schemas.inputs), resolve(&c.schemas.inputs)) {
            problems.extend(compatibility(&promised, &accepted).into_iter().map(|p| format!("inputs: {p}")));
        }
        if let (Some(given), Some(promised)) = (resolve(&c.schemas.outputs), resolve(&interface.schemas.outputs)) {
            problems.extend(compatibility(&given, &promised).into_iter().map(|p| format!("outputs: {p}")));
        }
        for p in problems {
            report.push(Finding::error(FindingCode::SchemaMismatch, component(&c.id), format!("doesn't fit {} it implements: {p}", interface.id)));
        }
    }
    for r in &plan.relations {
        match r.kind {
            RelationKind::Calls => call(r, graph, &resolve, packs, report),
            RelationKind::DataFlow => flow(r, graph, &resolve, report),
            _ => {}
        }
    }
}

fn call(r: &Relation, graph: &Graph, resolve: &dyn Fn(&Option<Value>) -> Option<Value>, packs: &PackSet, report: &mut Report) {
    let (Some(expects), Some(callee)) = (&r.expects, graph.component(&r.target)) else { return };
    let target = || Target::Relation { id: r.id() };
    let mut mismatch = |code: FindingCode, msg: String| report.push(Finding::error(code, target(), format!("{} -> {}: {msg}", r.source, r.target)));
    if let (Some(passed), Some(accepted)) = (resolve(&expects.inputs), resolve(&callee.schemas.inputs)) {
        for p in compatibility(&passed, &accepted) {
            mismatch(FindingCode::SchemaMismatch, format!("inputs: {p}"));
        }
    }
    if let (Some(given), Some(wanted)) = (resolve(&callee.schemas.outputs), resolve(&expects.outputs)) {
        for p in compatibility(&given, &wanted) {
            mismatch(FindingCode::SchemaMismatch, format!("outputs: {p}"));
        }
    }
    for p in against_signature(callee, resolve(&expects.inputs).as_ref(), resolve(&expects.outputs).as_ref(), packs) {
        mismatch(FindingCode::CallMismatch, p);
    }
}

/// The call's expected inputs/outputs against the callee's real signature
/// (only for code that exists: a planned signature is checked by P6.3).
fn against_signature(callee: &Component, inputs: Option<&Value>, outputs: Option<&Value>, packs: &PackSet) -> Vec<String> {
    let (Some(anchor), Some(sig)) = (&callee.anchor, &callee.signature) else { return Vec::new() };
    let Some(lang) = packs.by_name(&anchor.language).map(|p| &p.spec.language) else { return Vec::new() };
    let mut out = Vec::new();
    let params: Vec<_> = sig.params.iter().filter(|p| !lang.self_names.contains(&p.name)).collect();
    if let Some(inputs) = inputs {
        let empty = serde_json::Map::new();
        let props = inputs.get("properties").and_then(Value::as_object).unwrap_or(&empty);
        let required: Vec<&str> = inputs.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        if params.len() < required.len() || params.len() > props.len() {
            out.push(format!("{} takes {} argument(s); the call passes {} ({} required)", callee.name, params.len(), props.len(), required.len()));
        }
        for p in &params {
            match props.get(&p.name) {
                None => out.push(format!("parameter '{}' isn't among the planned inputs", p.name)),
                Some(schema) => {
                    if let Some(Err(why)) = p.ty.as_deref().and_then(|ty| carries(lang, ty, schema)) {
                        out.push(format!("parameter '{}': {why}", p.name));
                    }
                }
            }
        }
        for name in props.keys().filter(|k| !params.iter().any(|p| &p.name == *k)) {
            out.push(format!("planned input '{name}' isn't a parameter of {}", callee.name));
        }
    }
    if let (Some(outputs), Some(returns)) = (outputs, &sig.returns)
        && let Some(Err(why)) = carries(lang, returns, outputs)
    {
        out.push(format!("returns {returns}: {why}"));
    }
    out
}

fn flow(r: &Relation, graph: &Graph, resolve: &dyn Fn(&Option<Value>) -> Option<Value>, report: &mut Report) {
    let (Some((out_port, in_port)), Some(source), Some(target)) = (&r.ports, graph.component(&r.source), graph.component(&r.target)) else { return };
    let port = |schema: Option<Value>, name: &str| schema.map(|s| s.get("properties").and_then(|p| p.get(name)).cloned().unwrap_or(s));
    let (Some(provided), Some(accepted)) = (port(resolve(&source.schemas.outputs), out_port), port(resolve(&target.schemas.inputs), in_port)) else { return };
    for p in compatibility(&provided, &accepted) {
        report.push(Finding::error(
            FindingCode::SchemaMismatch,
            Target::Relation { id: r.id() },
            format!("{}.{out_port} -> {}.{in_port}: {p}", r.source, r.target),
        ));
    }
}
