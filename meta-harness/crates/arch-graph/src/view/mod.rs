//! Views: named lenses on the graph (from config), a query engine that cuts
//! the subgraph a view shows, layouts, and a text form for agents.

pub mod layout;
mod text;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

pub use layout::{Layout, Rect, layout};
pub use text::render_text;

use crate::{Alignment, ComponentId, Graph, ModelError, RelationId, RelationKind, Stage, Status};

const DEFAULT_VIEWS: &str = include_str!("../../config/views.toml");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Out,
    In,
    Both,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    /// Containers drawn as boxes with their children inside.
    Nested,
    /// Left to right along the view's edges.
    #[default]
    Layered,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Filters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<Stage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statuses: Vec<Status>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alignments: Vec<Alignment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_prefix: Option<String>,
}

impl Filters {
    /// The type filter widened to every type that specializes a listed one
    /// (filtering on `function` keeps an `endpoint` that extends it).
    pub fn with_specializations(&self, types: &crate::TypeSystem) -> Filters {
        let mut out = self.clone();
        if out.types.is_empty() {
            return out;
        }
        for name in types.types.keys() {
            let specializes = types.resolve(name).is_ok_and(|t| self.types.iter().any(|wanted| t.is_a(wanted)));
            if specializes && !out.types.contains(name) {
                out.types.push(name.clone());
            }
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub kinds: Vec<RelationKind>,
    #[serde(default)]
    pub direction: Direction,
    pub depth: u32,
    #[serde(default)]
    pub layout: LayoutKind,
    #[serde(default)]
    pub overlays: Vec<RelationKind>,
    #[serde(default)]
    pub include_ancestors: bool,
    /// Start points; empty means the view's natural entry points.
    #[serde(default)]
    pub roots: Vec<ComponentId>,
    #[serde(default)]
    pub filters: Filters,
}

/// The named views, from the built-in config plus any overrides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewCatalog {
    pub views: BTreeMap<String, ViewSpec>,
}

impl ViewCatalog {
    pub fn defaults() -> Result<Self, ModelError> {
        Self::from_toml(DEFAULT_VIEWS)
    }

    pub fn from_toml(text: &str) -> Result<Self, ModelError> {
        let mut catalog: ViewCatalog = toml::from_str(text).map_err(|e| ModelError::Config(e.to_string()))?;
        for (name, spec) in &mut catalog.views {
            spec.name = name.clone();
        }
        Ok(catalog)
    }

    /// Adds or replaces whole views by name.
    pub fn extend_with(&mut self, other: ViewCatalog) {
        self.views.extend(other.views);
    }

    /// Applies a project's view config: a view that already exists takes
    /// only the fields given (`[views.calls] depth = 9`); a new one must be
    /// complete.
    pub fn extend_from_toml(&mut self, text: &str) -> Result<(), ModelError> {
        let config = |e: &dyn std::fmt::Display| ModelError::Config(e.to_string());
        let mut doc: toml::Table = toml::from_str(text).map_err(|e| config(&e))?;
        let Some(views) = doc.remove("views") else { return Ok(()) };
        let toml::Value::Table(views) = views else { return Err(ModelError::Config("`views` must be a table of views".into())) };
        for (name, fields) in views {
            let toml::Value::Table(fields) = fields else { return Err(ModelError::Config(format!("view '{name}' must be a table"))) };
            let merged = match self.views.get(&name) {
                Some(base) => {
                    let toml::Value::Table(mut table) = toml::Value::try_from(base).map_err(|e| config(&e))? else {
                        return Err(ModelError::Config(format!("view '{name}' didn't serialize to a table")));
                    };
                    for (key, value) in fields {
                        match (table.get_mut(&key), value) {
                            // Filters merge one level down, like the view itself.
                            (Some(toml::Value::Table(existing)), toml::Value::Table(update)) if key == "filters" => existing.extend(update),
                            (_, value) => {
                                table.insert(key, value);
                            }
                        }
                    }
                    table
                }
                None => fields,
            };
            let mut spec: ViewSpec = toml::Value::Table(merged).try_into().map_err(|e| ModelError::Config(format!("view '{name}': {e}")))?;
            spec.name = name.clone();
            self.views.insert(name, spec);
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ViewSpec> {
        self.views.get(name)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewNode {
    pub id: ComponentId,
    /// Hops from the nearest root (0 for roots; ancestors added for context
    /// have no depth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// It has more relations of the view's kinds beyond the depth limit.
    pub truncated: bool,
    pub alignment: Alignment,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewEdge {
    pub id: RelationId,
    pub kind: RelationKind,
    pub source: ComponentId,
    pub target: ComponentId,
    pub alignment: Alignment,
    /// Drawn on top of the view's own kinds.
    pub overlay: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub spec: ViewSpec,
    pub roots: Vec<ComponentId>,
    pub nodes: Vec<ViewNode>,
    pub edges: Vec<ViewEdge>,
}

impl View {
    pub fn node(&self, id: &ComponentId) -> Option<&ViewNode> {
        self.nodes.iter().find(|n| &n.id == id)
    }
}

fn neighbors<'g>(graph: &'g Graph, id: &ComponentId, kinds: &[RelationKind], direction: Direction) -> Vec<(&'g crate::Relation, &'g ComponentId)> {
    let mut out = Vec::new();
    for kind in kinds {
        if matches!(direction, Direction::Out | Direction::Both) {
            out.extend(graph.outgoing(id, Some(*kind)).map(|r| (r, &r.target)));
        }
        if matches!(direction, Direction::In | Direction::Both) {
            out.extend(graph.incoming(id, Some(*kind)).map(|r| (r, &r.source)));
        }
    }
    out
}

fn passes(graph: &Graph, id: &ComponentId, filters: &Filters) -> bool {
    let Some(c) = graph.component(id) else { return false };
    let alignment = graph.alignment(id).unwrap_or(Alignment::Observed);
    (filters.stages.is_empty() || filters.stages.contains(&c.stage))
        && (filters.statuses.is_empty() || filters.statuses.contains(&c.status))
        && (filters.alignments.is_empty() || filters.alignments.contains(&alignment))
        && (filters.types.is_empty() || filters.types.contains(&c.type_name))
        && filters.file_prefix.as_ref().is_none_or(|p| c.file().is_some_and(|f| f.as_str().starts_with(p.as_str())))
}

/// The view's natural entry points: components with relations of its kinds
/// going out and none coming in (for structure, the top-level containers).
/// Walking both ways there is no "in": one entry per connected group, its
/// first source if it has one, else its first member.
fn natural_roots(graph: &Graph, spec: &ViewSpec) -> Vec<ComponentId> {
    let shown = |c: &&crate::Component| passes(graph, &c.id, &spec.filters);
    let has = |id: &ComponentId, direction| neighbors(graph, id, &spec.kinds, direction).iter().any(|(_, n)| passes(graph, n, &spec.filters));
    match spec.direction {
        Direction::Out | Direction::In => {
            let reverse = if spec.direction == Direction::Out { Direction::In } else { Direction::Out };
            graph.components().filter(shown).filter(|c| has(&c.id, spec.direction) && !has(&c.id, reverse)).map(|c| c.id.clone()).collect()
        }
        Direction::Both => {
            let mut grouped: BTreeSet<ComponentId> = BTreeSet::new();
            let mut roots = Vec::new();
            // Sources first, so a group is entered where its flow starts.
            let candidates: Vec<&crate::Component> = graph.components().filter(shown).filter(|c| has(&c.id, Direction::Both)).collect();
            let is_source = |c: &&&crate::Component| has(&c.id, Direction::Out) && !has(&c.id, Direction::In);
            let sources = candidates.iter().filter(is_source);
            let rest = candidates.iter().filter(|c| !is_source(c));
            for c in sources.chain(rest) {
                if grouped.contains(&c.id) {
                    continue;
                }
                roots.push(c.id.clone());
                let mut queue = VecDeque::from([c.id.clone()]);
                grouped.insert(c.id.clone());
                while let Some(id) = queue.pop_front() {
                    for (_, next) in neighbors(graph, &id, &spec.kinds, Direction::Both) {
                        if passes(graph, next, &spec.filters) && grouped.insert(next.clone()) {
                            queue.push_back(next.clone());
                        }
                    }
                }
            }
            roots.sort();
            roots
        }
    }
}

/// Cuts the subgraph a view shows.
pub fn query(graph: &Graph, spec: &ViewSpec) -> View {
    let roots: Vec<ComponentId> = if spec.roots.is_empty() {
        natural_roots(graph, spec)
    } else {
        spec.roots.iter().filter(|r| graph.component(r).is_some()).cloned().collect()
    };

    let mut depth_of: BTreeMap<ComponentId, u32> = BTreeMap::new();
    let mut edges: BTreeMap<RelationId, ViewEdge> = BTreeMap::new();
    let mut queue: VecDeque<ComponentId> = VecDeque::new();
    for root in &roots {
        depth_of.insert(root.clone(), 0);
        queue.push_back(root.clone());
    }
    while let Some(id) = queue.pop_front() {
        let depth = depth_of[&id];
        if depth >= spec.depth {
            continue;
        }
        for (_, next) in neighbors(graph, &id, &spec.kinds, spec.direction) {
            if passes(graph, next, &spec.filters) && !depth_of.contains_key(next) {
                depth_of.insert(next.clone(), depth + 1);
                queue.push_back(next.clone());
            }
        }
    }
    // Every relation of the view's kinds between shown nodes is drawn, also
    // at the depth limit; a node with more beyond the limit is truncated.
    let mut truncated: BTreeSet<ComponentId> = BTreeSet::new();
    for id in depth_of.keys() {
        for (relation, next) in neighbors(graph, id, &spec.kinds, spec.direction) {
            if !passes(graph, next, &spec.filters) {
                continue;
            }
            if depth_of.contains_key(next) {
                edges.insert(relation.id(), edge(graph, relation, false));
            } else {
                truncated.insert(id.clone());
            }
        }
    }

    let mut nodes: BTreeMap<ComponentId, Option<u32>> = depth_of.iter().map(|(id, d)| (id.clone(), Some(*d))).collect();
    if spec.include_ancestors {
        let shown: Vec<ComponentId> = nodes.keys().cloned().collect();
        for id in shown {
            // Containment can loop (a plan may say so): stop at the first repeat.
            let mut seen: BTreeSet<ComponentId> = BTreeSet::from([id.clone()]);
            let mut current = id;
            while let Some(parent) = graph.parent(&current) {
                if !seen.insert(parent.clone()) {
                    break;
                }
                nodes.entry(parent.clone()).or_insert(None);
                current = parent.clone();
            }
        }
    }
    // Overlays (and containment links to added ancestors) between shown nodes.
    let mut overlay_kinds = spec.overlays.clone();
    if spec.include_ancestors && !overlay_kinds.contains(&RelationKind::Contains) {
        overlay_kinds.push(RelationKind::Contains);
    }
    for id in nodes.keys() {
        for kind in &overlay_kinds {
            for relation in graph.outgoing(id, Some(*kind)) {
                if nodes.contains_key(&relation.target) && !edges.contains_key(&relation.id()) {
                    edges.insert(relation.id(), edge(graph, relation, !spec.kinds.contains(kind)));
                }
            }
        }
    }

    View {
        spec: spec.clone(),
        roots,
        nodes: nodes
            .into_iter()
            .map(|(id, depth)| ViewNode {
                truncated: truncated.contains(&id),
                alignment: graph.alignment(&id).unwrap_or(Alignment::Observed),
                id,
                depth,
            })
            .collect(),
        edges: edges.into_values().collect(),
    }
}

fn edge(graph: &Graph, relation: &crate::Relation, overlay: bool) -> ViewEdge {
    ViewEdge {
        id: relation.id(),
        kind: relation.kind,
        source: relation.source.clone(),
        target: relation.target.clone(),
        alignment: graph.relation_alignment(relation),
        overlay,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::component::tests::{anchor_for, planned};
    use crate::{Provenance, Relation};

    pub fn id(s: &str) -> ComponentId {
        ComponentId::new(format!("rust:{s}"))
    }

    /// app { run -> step -> save; run -> log }, lib { util }, step implements Step.
    pub fn sample() -> Graph {
        let mut g = Graph::new();
        for (s, ty) in [("app", "module"), ("app::run", "function"), ("app::step", "function"), ("app::save", "function"), ("app::log", "function"), ("lib", "module"), ("lib::util", "function"), ("lib::Step", "interface")] {
            let mut c = planned(&format!("rust:{s}"), crate::Stage::Implemented);
            c.status = crate::Status::Existing;
            c.type_name = ty.into();
            c.anchor = Some(anchor_for(s));
            g.upsert_component(c, Provenance::OBSERVED);
        }
        let rel = |k, a: &str, b: &str| Relation::new(k, id(a), id(b), Provenance::OBSERVED);
        for (a, b) in [("app", "app::run"), ("app", "app::step"), ("app", "app::save"), ("app", "app::log"), ("lib", "lib::util"), ("lib", "lib::Step")] {
            g.upsert_relation(rel(RelationKind::Contains, a, b));
        }
        for (a, b) in [("app::run", "app::step"), ("app::step", "app::save"), ("app::run", "app::log")] {
            g.upsert_relation(rel(RelationKind::Calls, a, b));
        }
        g.upsert_relation(rel(RelationKind::Implements, "app::step", "lib::Step"));
        g
    }

    // req: P7.1
    #[test]
    fn named_views_load_and_overrides_merge() {
        let mut catalog = ViewCatalog::defaults().unwrap();
        for name in ["structure", "calls", "data_flow", "combined"] {
            let view = catalog.get(name).unwrap();
            assert_eq!(view.name, name);
            assert!(!view.description.is_empty());
        }
        let before = catalog.get("calls").unwrap().clone();
        // A partial override changes only what it names; new views are complete.
        catalog.extend_from_toml("[views.calls]\ndepth = 9\n[views.calls.filters]\ntypes = [\"function\"]\n[views.mine]\nkinds = [\"implements\"]\ndepth = 1\nlayout = \"nested\"\n").unwrap();
        let calls = catalog.get("calls").unwrap();
        assert_eq!(calls.depth, 9);
        assert_eq!(calls.filters.types, vec!["function"]);
        assert_eq!((&calls.description, &calls.kinds, calls.layout), (&before.description, &before.kinds, before.layout), "the rest is kept");
        assert_eq!(catalog.get("mine").unwrap().layout, LayoutKind::Nested);
        assert!(catalog.clone().extend_from_toml("[views.half]\ndepth = 2\n").is_err(), "a new view needs its kinds");
        assert!(catalog.clone().extend_from_toml("[views.calls]\nkinds = [\"teleports\"]\n").is_err());
        // Whole catalogs still replace by name.
        catalog.extend_with(ViewCatalog::from_toml("[views.calls]\nkinds = [\"calls\"]\ndepth = 2\n").unwrap());
        assert_eq!(catalog.get("calls").unwrap().depth, 2);
        assert!(ViewCatalog::from_toml("[views.bad]\nkinds = [\"teleports\"]\ndepth = 1\n").is_err());
    }

    // req: P7.2
    #[test]
    fn query_walks_kinds_to_depth_with_truncation_and_filters() {
        let g = sample();
        let catalog = ViewCatalog::defaults().unwrap();
        let mut calls = catalog.get("calls").unwrap().clone();
        let v = query(&g, &calls);
        assert_eq!(v.roots, vec![id("app::run")], "natural entry point");
        let ids: Vec<_> = v.nodes.iter().map(|n| n.id.clone()).collect();
        assert_eq!(ids, vec![id("app::log"), id("app::run"), id("app::save"), id("app::step")]);
        assert!(v.edges.iter().all(|e| e.kind == RelationKind::Calls && !e.overlay));

        calls.depth = 1;
        let v = query(&g, &calls);
        assert!(v.node(&id("app::save")).is_none());
        assert!(v.node(&id("app::step")).unwrap().truncated, "step calls on beyond the cut");
        assert!(!v.node(&id("app::log")).unwrap().truncated);

        calls.direction = Direction::In;
        calls.roots = vec![id("app::save")];
        calls.depth = 5;
        let v = query(&g, &calls);
        assert_eq!(v.nodes.len(), 3, "save <- step <- run");

        let mut structure = catalog.get("structure").unwrap().clone();
        structure.filters.types = vec!["module".into(), "interface".into()];
        let v = query(&g, &structure);
        assert!(v.nodes.iter().all(|n| g.component(&n.id).unwrap().type_name != "function"));
        // Deterministic.
        assert_eq!(query(&g, &structure), query(&g, &structure));
    }


    // req: P7.2
    #[test]
    fn ancestors_stop_at_a_containment_loop() {
        let mut g = sample();
        // A plan can say what the code says contains it, contains it back.
        g.upsert_relation(crate::Relation::new(RelationKind::Contains, id("app::run"), id("app"), crate::Provenance::PLANNED));
        let combined = ViewCatalog::defaults().unwrap().get("combined").unwrap().clone();
        let v = query(&g, &combined);
        assert!(v.node(&id("app")).is_some() && v.node(&id("app::run")).is_some(), "the query returns, with both ends");
    }
    // req: P7.2
    #[test]
    fn combined_view_overlays_structure_and_keeps_ancestors() {
        let g = sample();
        let combined = ViewCatalog::defaults().unwrap().get("combined").unwrap().clone();
        let v = query(&g, &combined);
        assert!(v.node(&id("app")).is_some(), "container kept for context");
        assert!(v.node(&id("app")).unwrap().depth.is_none());
        let overlay_kinds: BTreeSet<_> = v.edges.iter().filter(|e| e.overlay).map(|e| e.kind).collect();
        assert!(overlay_kinds.contains(&RelationKind::Contains));
        assert!(v.edges.iter().any(|e| e.kind == RelationKind::Calls && !e.overlay));
    }

    // req: P7.2
    #[test]
    fn edges_between_shown_nodes_survive_the_depth_limit_and_both_ways_has_one_entry() {
        let mut g = sample();
        g.upsert_relation(crate::Relation::new(RelationKind::Calls, id("app::step"), id("app::log"), crate::Provenance::OBSERVED));
        let mut calls = ViewCatalog::defaults().unwrap().get("calls").unwrap().clone();
        calls.depth = 1;
        let v = query(&g, &calls);
        let edges: Vec<(String, String)> = v.edges.iter().map(|e| (e.source.0.clone(), e.target.0.clone())).collect();
        assert!(edges.contains(&("rust:app::step".into(), "rust:app::log".into())), "both ends shown: drawn even at the limit: {edges:?}");
        assert_eq!(v.edges.len(), 3);
        assert!(v.node(&id("app::step")).unwrap().truncated, "save is still beyond the cut");
        assert!(!v.node(&id("app::log")).unwrap().truncated);
        // Walking both ways: one entry per connected group, at its source.
        calls.direction = Direction::Both;
        calls.depth = 5;
        let v = query(&g, &calls);
        assert_eq!(v.roots, vec![id("app::run")]);
        assert_eq!(v.nodes.len(), 4);
    }

    // req: P7.2
    #[test]
    fn type_filters_include_specialized_types() {
        let mut types = crate::TypeSystem::defaults().unwrap();
        types.extend_from_toml("[types.endpoint]\nextends = \"function\"").unwrap();
        let mut g = sample();
        let mut hook = planned("rust:app::hook", crate::Stage::Implemented);
        hook.status = crate::Status::Existing;
        hook.type_name = "endpoint".into();
        hook.anchor = Some(anchor_for("app::hook"));
        g.upsert_component(hook, crate::Provenance::OBSERVED);
        g.upsert_relation(crate::Relation::new(RelationKind::Calls, id("app::run"), id("app::hook"), crate::Provenance::OBSERVED));
        let mut calls = ViewCatalog::defaults().unwrap().get("calls").unwrap().clone();
        calls.filters.types = vec!["function".into()];
        assert!(query(&g, &calls).node(&id("app::hook")).is_none(), "exact names only");
        calls.filters = calls.filters.with_specializations(&types);
        assert!(calls.filters.types.contains(&"endpoint".to_owned()));
        assert!(query(&g, &calls).node(&id("app::hook")).is_some(), "an endpoint is a function");
    }
}
