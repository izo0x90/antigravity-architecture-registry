//! A compact text form of any view, for agents: what the view means, then its
//! nodes and edges with anchors, stage, provenance and alignment.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use super::{LayoutKind, View};
use crate::{Alignment, ComponentId, Graph, RelationKind};

fn mark(a: Alignment) -> &'static str {
    match a {
        Alignment::Observed => "code",
        Alignment::Planned => "planned",
        Alignment::Missing => "MISSING",
        Alignment::Present => "planned+code",
    }
}

fn describe(graph: &Graph, view: &View, id: &ComponentId) -> String {
    let Some(c) = graph.component(id) else { return id.to_string() };
    let node = view.node(id);
    let at = match (&c.anchor, &c.proposed) {
        (Some(a), _) => format!("{}:{}", a.file, a.span.start_line),
        (None, Some(p)) => format!("-> {}", p.file),
        (None, None) => "unplaced".to_owned(),
    };
    let mut line = format!(
        "{} [{} {:?}/{:?} {}] {}",
        c.name,
        c.type_name,
        c.status,
        c.stage,
        node.map(|n| mark(n.alignment)).unwrap_or("code"),
        at
    );
    if !c.placeholders.is_empty() {
        let _ = write!(line, " ({} placeholders)", c.placeholders.len());
    }
    if node.is_some_and(|n| n.truncated) {
        line.push_str(" …more");
    }
    let _ = write!(line, "  id={id}");
    line
}

pub fn render_text(graph: &Graph, view: &View) -> String {
    let mut out = String::new();
    let kinds: Vec<&str> = view.spec.kinds.iter().map(|k| k.name()).collect();
    let _ = writeln!(out, "View '{}': {}", view.spec.name, view.spec.description);
    let _ = writeln!(out, "Walks {} {:?} to depth {}; {} nodes, {} edges.", kinds.join("+"), view.spec.direction, view.spec.depth, view.nodes.len(), view.edges.len());
    if view.nodes.is_empty() {
        out.push_str("(nothing to show: no components with these relations in scope)\n");
        return out;
    }
    match view.spec.layout {
        LayoutKind::Nested => {
            let mut children: BTreeMap<&ComponentId, Vec<&ComponentId>> = BTreeMap::new();
            let mut has_parent = BTreeSet::new();
            for e in view.edges.iter().filter(|e| e.kind == RelationKind::Contains) {
                if has_parent.insert(&e.target) {
                    children.entry(&e.source).or_default().push(&e.target);
                }
            }
            fn walk<'a>(graph: &Graph, view: &View, id: &'a ComponentId, depth: usize, children: &BTreeMap<&'a ComponentId, Vec<&'a ComponentId>>, seen: &mut BTreeSet<&'a ComponentId>, out: &mut String) {
                if !seen.insert(id) {
                    return;
                }
                let _ = writeln!(out, "{}{}", "  ".repeat(depth), describe(graph, view, id));
                for kid in children.get(id).into_iter().flatten() {
                    walk(graph, view, kid, depth + 1, children, seen, out);
                }
            }
            let mut seen = BTreeSet::new();
            for node in view.nodes.iter().filter(|n| !has_parent.contains(&n.id)) {
                walk(graph, view, &node.id, 0, &children, &mut seen, &mut out);
            }
            // Containment loops leave nodes no top-level walk reaches.
            for node in &view.nodes {
                walk(graph, view, &node.id, 0, &children, &mut seen, &mut out);
            }
            for e in view.edges.iter().filter(|e| e.kind != RelationKind::Contains) {
                let _ = writeln!(out, "  {} -{}-> {}{} ({})", short(&e.source), e.kind.name(), short(&e.target), ports(graph, e), mark(e.alignment));
            }
        }
        LayoutKind::Story => {
            // An outline: each step under what calls it, in call order.
            let story = super::story::tell(view, &BTreeSet::new());
            for row in &story.rows {
                let mut line = format!("{}{}", "  ".repeat(row.depth), describe(graph, view, &row.id));
                if let Some(n) = story.also.get(&row.id) {
                    let _ = write!(line, "  (also from {n} more)");
                }
                for back in story.loops.get(&row.id).into_iter().flatten() {
                    let _ = write!(line, "  (loops back to {})", short(back));
                }
                let _ = writeln!(out, "{line}");
            }
        }
        LayoutKind::Layered => {
            out.push_str("Nodes:\n");
            for node in &view.nodes {
                let _ = writeln!(out, "  {}", describe(graph, view, &node.id));
            }
            out.push_str("Edges:\n");
            for e in &view.edges {
                let overlay = if e.overlay { " (overlay)" } else { "" };
                let _ = writeln!(out, "  {} -{}-> {}{} ({}){overlay}", short(&e.source), e.kind.name(), short(&e.target), ports(graph, e), mark(e.alignment));
            }
        }
    }
    out
}

/// ` [out -> in]` for a data flow, so two flows between the same pair differ.
fn ports(graph: &Graph, e: &super::ViewEdge) -> String {
    match graph.relation(&e.id).and_then(|r| r.ports.as_ref()) {
        Some((from, to)) => format!(" [{from} -> {to}]"),
        None => String::new(),
    }
}

fn short(id: &ComponentId) -> &str {
    id.as_str().split_once(':').map_or(id.as_str(), |(_, symbol)| symbol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::tests::sample;
    use crate::view::{ViewCatalog, query};

    // req: P7.5
    #[test]
    fn text_views_explain_themselves_and_list_anchors_and_alignment() {
        let g = sample();
        let catalog = ViewCatalog::defaults().unwrap();
        let calls = render_text(&g, &query(&g, catalog.get("calls").unwrap()));
        assert!(calls.starts_with("View 'calls': Dynamic behavior"));
        assert!(calls.contains("run [function Existing/Implemented code] src/lib.rs:1"));
        assert!(calls.contains("app::run -calls-> app::step (code)"));
        let mut deep = catalog.get("structure").unwrap().clone();
        deep.depth = 3;
        let structure = render_text(&g, &query(&g, &deep));
        assert!(structure.contains("\napp [module"), "{structure}");
        assert!(structure.contains("\n  run [function"), "children indented under their container: {structure}");
        assert!(structure.contains("app::step -implements-> lib::Step"));
        let mut empty = catalog.get("data_flow").unwrap().clone();
        empty.roots.clear();
        assert!(render_text(&g, &query(&g, &empty)).contains("nothing to show"));
    }

    // req: P7.5
    #[test]
    fn text_shows_planned_missing_and_present_parts_proposed_places_overlays_and_ports() {
        use crate::component::tests::planned;
        use crate::{Plan, Provenance, ProposedLocation, Relation, RepoPath, Stage, Status};
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let place = |symbol: &str| Some(ProposedLocation { language: "rust".into(), file: RepoPath::new("src/new.rs").unwrap(), parent: Some(id("app")), symbol: symbol.into() });
        let mut plan = Plan::new("p", "P");
        let mut check = planned("rust:app::check", Stage::Declared);
        check.proposed = place("app::check");
        let mut ghost = planned("rust:app::ghost", Stage::Skeleton);
        ghost.proposed = place("app::ghost");
        let mut step = sample().component(&id("app::step")).unwrap().clone();
        step.status = Status::Modifying;
        step.stage = Stage::Declared;
        plan.components.extend([check, ghost, step]);
        for target in ["app::check", "app::ghost"] {
            plan.relations.push(Relation::new(RelationKind::Calls, id("app::run"), id(target), Provenance::PLANNED));
        }
        plan.relations.push(Relation::new(RelationKind::Calls, id("app::check"), id("app::log"), Provenance::PLANNED));
        for port in ["a", "b"] {
            let mut flow = Relation::new(RelationKind::DataFlow, id("app::step"), id("app::save"), Provenance::PLANNED);
            flow.ports = Some(("out".into(), port.into()));
            plan.relations.push(flow);
        }
        let g = Graph::merge(sample(), &[plan]);
        let combined = ViewCatalog::defaults().unwrap().get("combined").unwrap().clone();
        let text = render_text(&g, &query(&g, &combined));
        for needle in [
            "check [function New/Declared planned] -> src/new.rs",
            "ghost [function New/Skeleton MISSING] -> src/new.rs",
            "step [function Modifying/Declared planned+code] src/lib.rs:1",
            "run [function Existing/Implemented code] src/lib.rs:1",
            "app::run -calls-> app::check (MISSING)",
            "app::check -calls-> app::log (planned)",
            "app::step -data_flow-> app::save [out -> a] (MISSING) (overlay)",
            "app::step -data_flow-> app::save [out -> b] (MISSING) (overlay)",
            "app -contains-> app::run (code) (overlay)",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
        // A containment loop still lists every node.
        let mut looped = Graph::new();
        for s in ["x", "y"] {
            looped.upsert_component(planned(&format!("rust:{s}"), Stage::Declared), Provenance::PLANNED);
        }
        looped.upsert_relation(Relation::new(RelationKind::Contains, id("x"), id("y"), Provenance::PLANNED));
        looped.upsert_relation(Relation::new(RelationKind::Contains, id("y"), id("x"), Provenance::PLANNED));
        let mut structure = ViewCatalog::defaults().unwrap().get("structure").unwrap().clone();
        structure.roots = vec![id("x")];
        structure.depth = 3;
        let text = render_text(&looped, &query(&looped, &structure));
        assert!(text.contains("\nx [") && text.contains("\n  y ["), "{text}");
    }
}
