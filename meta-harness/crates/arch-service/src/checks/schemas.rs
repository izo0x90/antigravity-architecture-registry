//! Schema fit: implementations against their interfaces, planned calls
//! against what the callee takes and returns (its planned schemas and, for
//! code that exists, its real signature), and data flows port to port.

use arch_graph::schema::compatibility;
use arch_graph::{Component, ComponentId, Finding, FindingCode, Graph, Plan, Relation, RelationKind, Report, SchemaSet, Target};
use code_analyzer::PackSet;
use code_analyzer::typing::carries;
use serde_json::Value;

use super::component;

pub(super) fn fit(plan: &Plan, graph: &Graph, set: &SchemaSet, packs: &PackSet, report: &mut Report) {
    let resolve = |s: &Option<Value>| s.as_ref().and_then(|v| set.resolve(v).ok());
    // Implementations accept what their interface promises to take and
    // return what it promises to give.
    for planned in &plan.components {
        let Some(c) = graph.component(&planned.id) else { continue };
        // Every interface it implements (directly or through others) that
        // states a shape must be satisfied, not just the nearest.
        let interfaces = graph.interface_chain(&c.id).into_iter().filter_map(|i| graph.component(&i)).filter(|i| i.schemas.inputs.is_some() || i.schemas.outputs.is_some());
        for interface in interfaces {
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
    }
    for r in &plan.relations {
        match r.kind {
            RelationKind::Calls => call(r, graph, &resolve, packs, report),
            RelationKind::DataFlow => flow(r, graph, &resolve, report),
            _ => {}
        }
    }
}

/// A shape and the component that states it.
pub(crate) type Stated = Option<(Value, ComponentId)>;

/// A component's inputs and outputs, falling back to the nearest interface
/// it implements that states them, with who states each.
pub(crate) fn effective_with_source(graph: &Graph, c: &Component) -> (Stated, Stated) {
    let chain: Vec<&Component> = graph.interface_chain(&c.id).iter().filter_map(|i| graph.component(i)).collect();
    let pick = |own: &Option<Value>, of: fn(&Component) -> &Option<Value>| -> Stated {
        own.clone().map(|v| (v, c.id.clone())).or_else(|| chain.iter().find_map(|i| of(i).clone().map(|v| (v, i.id.clone()))))
    };
    (pick(&c.schemas.inputs, |x| &x.schemas.inputs), pick(&c.schemas.outputs, |x| &x.schemas.outputs))
}

fn effective(graph: &Graph, c: &Component) -> (Option<Value>, Option<Value>) {
    let (inputs, outputs) = effective_with_source(graph, c);
    (inputs.map(|(v, _)| v), outputs.map(|(v, _)| v))
}

fn requires_inputs(schema: &Value) -> bool {
    schema.get("required").and_then(Value::as_array).is_some_and(|r| !r.is_empty())
}

fn call(r: &Relation, graph: &Graph, resolve: &dyn Fn(&Option<Value>) -> Option<Value>, packs: &PackSet, report: &mut Report) {
    let (Some(expects), Some(callee)) = (&r.expects, graph.component(&r.target)) else { return };
    let target = || Target::Relation { id: r.id() };
    let mut mismatch = |code: FindingCode, msg: String| report.push(Finding::error(code, target(), format!("{} -> {}: {msg}", r.source, r.target)));
    let (callee_inputs, callee_outputs) = effective(graph, callee);
    let callee_inputs = resolve(&callee_inputs);
    match (resolve(&expects.inputs), &callee_inputs) {
        (Some(passed), Some(accepted)) => {
            for p in compatibility(&passed, accepted) {
                mismatch(FindingCode::SchemaMismatch, format!("inputs: {p}"));
            }
        }
        // States what it expects, but passes nothing to something that needs input.
        (None, Some(accepted)) if expects.inputs.is_none() && requires_inputs(accepted) => {
            mismatch(FindingCode::SchemaMismatch, format!("passes no inputs, but {} requires some", callee.name));
        }
        _ => {}
    }
    if let (Some(given), Some(wanted)) = (resolve(&callee_outputs), resolve(&expects.outputs)) {
        for p in compatibility(&given, &wanted) {
            mismatch(FindingCode::SchemaMismatch, format!("outputs: {p}"));
        }
    }
    // Stating expectations without inputs means passing nothing: check that
    // against the real signature too.
    let passed = match &expects.inputs {
        Some(_) => resolve(&expects.inputs),
        None => Some(serde_json::json!({"type": "object", "properties": {}})),
    };
    for p in against_signature(callee, passed.as_ref(), resolve(&expects.outputs).as_ref(), packs) {
        mismatch(FindingCode::CallMismatch, p);
    }
}

/// Words of a parameter as written (`&mut self` -> ["mut", "self"]).
fn words(name: &str) -> impl Iterator<Item = &str> {
    name.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty())
}

/// The call's expected inputs/outputs against the callee's real signature
/// (only for code that exists: a planned signature is checked by P6.3).
/// Receivers (self, this...) aren't arguments; optional parameters may be
/// left out; a variadic parameter (`*args`) takes any number, none included.
fn against_signature(callee: &Component, inputs: Option<&Value>, outputs: Option<&Value>, packs: &PackSet) -> Vec<String> {
    let (Some(anchor), Some(sig)) = (&callee.anchor, &callee.signature) else { return Vec::new() };
    let Some(lang) = packs.by_name(&anchor.language).map(|p| &p.spec.language) else { return Vec::new() };
    let mut out = Vec::new();
    let params: Vec<_> = sig.params.iter().filter(|p| !words(&p.name).any(|w| lang.self_names.iter().any(|s| s == w))).collect();
    if let Some(inputs) = inputs {
        let empty = serde_json::Map::new();
        let props = inputs.get("properties").and_then(Value::as_object).unwrap_or(&empty);
        let required: Vec<&str> = inputs.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        let needed = params.iter().filter(|p| !p.optional && !p.variadic).count();
        let variadic = params.iter().any(|p| p.variadic);
        if (!variadic && (props.len() > params.len() || required.len() > params.len())) || needed > props.len() {
            let optional = params.len() - needed;
            let takes = if optional > 0 { format!("{needed} argument(s) (+{optional} optional)") } else { format!("{} argument(s)", params.len()) };
            out.push(format!("{} takes {takes}; the call passes {} ({} required)", callee.name, props.len(), required.len()));
        }
        for p in &params {
            match props.get(&p.name) {
                None if p.optional || p.variadic => {}
                None => out.push(format!("parameter '{}' isn't among the planned inputs", p.name)),
                Some(schema) => {
                    if let Some(Err(why)) = p.ty.as_deref().and_then(|ty| carries(lang, ty, schema)) {
                        out.push(format!("parameter '{}': {why}", p.name));
                    }
                }
            }
        }
        for name in props.keys().filter(|k| !variadic && !params.iter().any(|p| &p.name == *k)) {
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

/// The part of `schema` a port names: a property of an object schema, or the
/// whole value when the port is the source's return (`return`) or the schema
/// has no fields. Err when the port names a field that isn't there.
fn port(schema: Value, name: &str, whole_ok: bool) -> Result<Value, Vec<String>> {
    match schema.get("properties").and_then(Value::as_object) {
        Some(props) => match props.get(name) {
            Some(field) => Ok(field.clone()),
            None if whole_ok => Ok(schema.clone()),
            None => Err(props.keys().cloned().collect()),
        },
        None => Ok(schema),
    }
}

fn flow(r: &Relation, graph: &Graph, resolve: &dyn Fn(&Option<Value>) -> Option<Value>, report: &mut Report) {
    let (Some((out_port, in_port)), Some(source), Some(target)) = (&r.ports, graph.component(&r.source), graph.component(&r.target)) else { return };
    let (Some(outputs), Some(inputs)) = (resolve(&effective(graph, source).1), resolve(&effective(graph, target).0)) else { return };
    let rel = || Target::Relation { id: r.id() };
    let mut missing = |side: &str, who: &arch_graph::ComponentId, name: &str, fields: Vec<String>| {
        report.push(Finding::error(
            FindingCode::SchemaMismatch,
            rel(),
            format!("port '{name}' isn't a field of {who}'s {side} (fields: {})", fields.join(", ")),
        ));
    };
    let provided = match port(outputs, out_port, out_port == "return") {
        Ok(v) => Some(v),
        Err(fields) => {
            missing("outputs", &r.source, out_port, fields);
            None
        }
    };
    let accepted = match port(inputs, in_port, false) {
        Ok(v) => Some(v),
        Err(fields) => {
            missing("inputs", &r.target, in_port, fields);
            None
        }
    };
    let (Some(provided), Some(accepted)) = (provided, accepted) else { return };
    for p in compatibility(&provided, &accepted) {
        report.push(Finding::error(FindingCode::SchemaMismatch, rel(), format!("{}.{out_port} -> {}.{in_port}: {p}", r.source, r.target)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arch_graph::{CodeAnchor, ComponentId, Param, RepoPath, Schemas, Signature, Span, Stage, Status};
    use serde_json::json;

    fn method() -> Component {
        let param = |name: &str, ty: &str, optional: bool| Param { name: name.into(), ty: Some(ty.into()), optional, variadic: false };
        Component {
            id: ComponentId::new("rust:app::Store::save"),
            name: "save".into(),
            type_name: "function".into(),
            description: String::new(),
            status: Status::Existing,
            stage: Stage::Implemented,
            anchor: Some(CodeAnchor {
                language: "rust".into(),
                file: RepoPath::new("src/store.rs").unwrap(),
                symbol: "app::Store::save".into(),
                syntax_kind: "function_item".into(),
                span: Span { start_line: 1, end_line: 3, start_byte: 0, end_byte: 10 },
                content_hash: "h".into(),
            }),
            proposed: None,
            signature: Some(Signature { params: vec![param("&mut self", "", false), param("item", "&str", false), param("flush", "bool", true)], returns: None, qualifiers: vec![] }),
            schemas: Schemas::default(),
            spec: None,
            placeholders: vec![],
        }
    }

    fn inputs(v: serde_json::Value) -> Value {
        arch_graph::Dsl::defaults().unwrap().compile(&v).unwrap()
    }

    // req: P5.6
    #[test]
    fn receivers_aren_t_arguments_and_optional_parameters_may_be_left_out() {
        let packs = code_analyzer::PackSet::from_shipped_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")).unwrap();
        let callee = method();
        assert!(against_signature(&callee, Some(&inputs(json!({"item": "str"}))), None, &packs).is_empty(), "&mut self is the receiver; flush is optional");
        assert!(against_signature(&callee, Some(&inputs(json!({"item": "str", "flush": "bool"}))), None, &packs).is_empty());
        assert_eq!(
            against_signature(&callee, Some(&inputs(json!({"flush": "bool"}))), None, &packs),
            vec!["parameter 'item' isn't among the planned inputs"]
        );
        assert_eq!(
            against_signature(&callee, Some(&inputs(json!({"item": "str", "flush": "bool", "extra": "int"}))), None, &packs),
            vec!["save takes 1 argument(s) (+1 optional); the call passes 3 (3 required)", "planned input 'extra' isn't a parameter of save"]
        );
    }
}
