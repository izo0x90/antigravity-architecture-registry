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
use arch_index::PackSet;

use crate::ToolCatalog;

pub use gates::{missing_details, stage_gate};

/// What the checks need besides the plan and the graph.
pub struct CheckCx<'a> {
    pub root: &'a Path,
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
    basic::components(plan, graph, cx.root, cx.packs, cx.types, &mut report);
    basic::touch_points(plan, graph, &mut report);
    types::rules(plan, graph, cx.types, &mut report);
    let set = schema_set(graph, cx.types);
    cycles::loops(plan, graph, &set, &mut report);
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
