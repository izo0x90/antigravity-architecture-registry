//! Plan checks that run before any code is written: references and
//! locations, type rules, loops, schema fit (including against the real
//! signatures of existing code), spec rules, completeness and lifecycle
//! gates. Pure: the same inputs always give the same report.

mod basic;
mod cycles;
mod gates;
mod schemas;
mod spec;
mod types;

use std::path::Path;

use arch_graph::{Component, Graph, Plan, Report, SchemaSet, Target, TypeSystem};
use code_analyzer::PackSet;

use crate::ToolCatalog;

pub use gates::{missing_details, stage_gate};
pub(crate) use schemas::effective_with_source;

/// What the checks need besides the plan and the graph.
pub struct CheckCx<'a> {
    pub root: &'a Path,
    /// What ingestion never reads (the one definition).
    pub ignores: &'a code_analyzer::Ignores,
    pub packs: &'a PackSet,
    pub types: &'a TypeSystem,
    pub tools: &'a ToolCatalog,
}

/// "a class", "an interface".
pub(crate) fn a(word: &str) -> String {
    let article = if word.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
    format!("{article} {word}")
}

pub(crate) fn component(id: &arch_graph::ComponentId) -> Target {
    Target::Component { id: id.clone() }
}

/// Checks `plan` against `graph` (the code merged with this plan).
pub fn check_plan(plan: &Plan, graph: &Graph, cx: &CheckCx) -> Report {
    let mut report = Report::default();
    basic::references(plan, graph, &mut report);
    basic::components(plan, graph, cx.root, cx.ignores, cx.packs, cx.types, &mut report);
    basic::touch_points(plan, graph, &mut report);
    types::rules(plan, graph, cx.types, &mut report);
    let set = schema_set(graph, cx.types);
    cycles::loops(plan, graph, &set, &mut report);
    cycles::ambiguous_refs(plan, graph, &ambiguous_names(graph, cx.types), &mut report);
    schemas::fit(plan, graph, &set, cx.packs, &mut report);
    spec::rules(plan, graph, cx.tools, &mut report);
    gates::check(plan, graph, cx.types, &mut report);
    report.sorted()
}

/// Components whose type holds data (properties, no signature): what schema
/// `$ref`s point at, by name.
pub fn is_data_type(types: &TypeSystem) -> impl Fn(&Component) -> bool + '_ {
    |c| types.resolve(&c.type_name).is_ok_and(|t| t.allows_properties && !t.allows_signature)
}

/// Data types sharing a name: a `$ref` to that name is ambiguous.
pub fn ambiguous_names(graph: &Graph, types: &TypeSystem) -> std::collections::BTreeMap<String, Vec<arch_graph::ComponentId>> {
    let data = is_data_type(types);
    let mut by_name: std::collections::BTreeMap<String, Vec<arch_graph::ComponentId>> = std::collections::BTreeMap::new();
    for c in graph.components().filter(|c| data(c)) {
        by_name.entry(c.name.clone()).or_default().push(c.id.clone());
    }
    by_name.retain(|_, ids| ids.len() > 1);
    by_name
}

/// The findings that keep `id` from being approved or advanced: errors on
/// it, on relations it's an end of, and on the plan as a whole.
pub fn blocking_for<'a>(report: &'a Report, plan: &Plan, id: &arch_graph::ComponentId) -> Vec<&'a arch_graph::Finding> {
    report
        .findings
        .iter()
        .filter(|f| f.severity == arch_graph::Severity::Error)
        .filter(|f| match &f.target {
            Target::Component { id: c } => c == id,
            Target::Plan { .. } => true,
            Target::Relation { id: r } => plan.relations.iter().any(|rel| &rel.id() == r && (&rel.source == id || &rel.target == id)),
        })
        .collect()
}

/// The named data types in `graph` (an unspecified shape when a type has no
/// properties schema, e.g. a struct found in code).
pub fn schema_set(graph: &Graph, types: &TypeSystem) -> SchemaSet {
    let data = is_data_type(types);
    let mut set = SchemaSet::new();
    for c in graph.components().filter(|c| data(c)) {
        set.insert(c.name.clone(), c.schemas.properties.clone().unwrap_or(serde_json::Value::Object(serde_json::Map::new())));
    }
    set
}
