//! The graph aggregate: components and relations with lookup indices, and the
//! merge of what the code shows (observed) with what plans say (planned).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Component, ComponentId, Invariant, Plan, Provenance, Relation, RelationId, RelationKind};

/// How plan and code line up for one element (derived, never stored).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    /// In the code, not part of any plan.
    Observed,
    /// In a plan, and the code isn't expected to have it yet.
    Planned,
    /// In a plan that says the code should have it, but the code doesn't.
    Missing,
    /// In both plan and code.
    Present,
}

/// One call in a workflow's chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainStep {
    pub caller: ComponentId,
    pub callee: ComponentId,
    /// Position among the caller's calls, when known.
    pub order: Option<u32>,
    /// 1 for calls made by the entry, 2 for theirs, ...
    pub depth: u32,
}

/// A call chain walked from an entry in call order. Recursion is not
/// followed; each loop is reported once as a path that returns to its start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallChain {
    pub entry: ComponentId,
    pub steps: Vec<ChainStep>,
    pub cycles: Vec<Vec<ComponentId>>,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    components: BTreeMap<ComponentId, Component>,
    provenance: BTreeMap<ComponentId, Provenance>,
    relations: BTreeMap<RelationId, Relation>,
    by_file: BTreeMap<String, BTreeSet<ComponentId>>,
    by_name: BTreeMap<String, BTreeSet<ComponentId>>,
    outgoing: BTreeMap<ComponentId, BTreeSet<RelationId>>,
    incoming: BTreeMap<ComponentId, BTreeSet<RelationId>>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a component, or merges it into the one with the same id: plan data
    /// wins for intent (type, description, stage, spec, signature), code data
    /// wins for facts (anchor, placeholders).
    pub fn upsert_component(&mut self, component: Component, provenance: Provenance) {
        let id = component.id.clone();
        let merged = match self.components.remove(&id) {
            Some(existing) => {
                let existing_prov = self.provenance.get(&id).copied().unwrap_or_default();
                let (plan, code) = if provenance.planned && !existing_prov.planned { (component, existing) } else { (existing, component) };
                let mut merged = plan;
                if code.anchor.is_some() {
                    merged.anchor = code.anchor;
                    merged.placeholders = code.placeholders;
                }
                if merged.signature.is_none() {
                    merged.signature = code.signature;
                }
                merged
            }
            None => component,
        };
        self.unindex_component(&id);
        if let Some(file) = merged.file() {
            self.by_file.entry(file.as_str().to_owned()).or_default().insert(id.clone());
        }
        self.by_name.entry(merged.name.clone()).or_default().insert(id.clone());
        let prov = self.provenance.get(&id).copied().unwrap_or_default().merge(provenance);
        self.provenance.insert(id.clone(), prov);
        self.components.insert(id, merged);
    }

    fn unindex_component(&mut self, id: &ComponentId) {
        for set in self.by_file.values_mut().chain(self.by_name.values_mut()) {
            set.remove(id);
        }
    }

    /// Adds a relation, or folds it into the existing one with the same id.
    pub fn upsert_relation(&mut self, relation: Relation) {
        let id = relation.id();
        match self.relations.get_mut(&id) {
            Some(existing) => existing.absorb(&relation),
            None => {
                self.outgoing.entry(relation.source.clone()).or_default().insert(id.clone());
                self.incoming.entry(relation.target.clone()).or_default().insert(id.clone());
                self.relations.insert(id, relation);
            }
        }
    }

    pub fn component(&self, id: &ComponentId) -> Option<&Component> {
        self.components.get(id)
    }

    pub fn components(&self) -> impl Iterator<Item = &Component> {
        self.components.values()
    }

    pub fn relations(&self) -> impl Iterator<Item = &Relation> {
        self.relations.values()
    }

    pub fn relation(&self, id: &RelationId) -> Option<&Relation> {
        self.relations.get(id)
    }

    pub fn len(&self) -> (usize, usize) {
        (self.components.len(), self.relations.len())
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    pub fn provenance(&self, id: &ComponentId) -> Provenance {
        self.provenance.get(id).copied().unwrap_or_default()
    }

    pub fn in_file(&self, file: &str) -> impl Iterator<Item = &Component> {
        self.by_file.get(file).into_iter().flatten().filter_map(|id| self.components.get(id))
    }

    pub fn named(&self, name: &str) -> impl Iterator<Item = &Component> {
        self.by_name.get(name).into_iter().flatten().filter_map(|id| self.components.get(id))
    }

    pub fn outgoing(&self, id: &ComponentId, kind: Option<RelationKind>) -> impl Iterator<Item = &Relation> {
        self.outgoing.get(id).into_iter().flatten().filter_map(|r| self.relations.get(r)).filter(move |r| kind.is_none_or(|k| r.kind == k))
    }

    pub fn incoming(&self, id: &ComponentId, kind: Option<RelationKind>) -> impl Iterator<Item = &Relation> {
        self.incoming.get(id).into_iter().flatten().filter_map(|r| self.relations.get(r)).filter(move |r| kind.is_none_or(|k| r.kind == k))
    }

    /// The component's container (via Contains), if any.
    pub fn parent(&self, id: &ComponentId) -> Option<&ComponentId> {
        self.incoming(id, Some(RelationKind::Contains)).map(|r| &r.source).next()
    }

    /// What `id` implements, transitively, nearest first (loops are cut).
    pub fn interface_chain(&self, id: &ComponentId) -> Vec<ComponentId> {
        let mut chain: Vec<ComponentId> = Vec::new();
        let mut frontier = vec![id.clone()];
        while let Some(current) = frontier.pop() {
            let mut next: Vec<ComponentId> =
                self.outgoing(&current, Some(RelationKind::Implements)).map(|r| r.target.clone()).filter(|t| t != id && !chain.contains(t)).collect();
            next.sort();
            chain.extend(next.iter().cloned());
            frontier.extend(next.into_iter().rev());
        }
        chain
    }

    /// The invariants that hold for `id`: its own, then those it inherits
    /// through what it implements (nearest first; a nearer one with the same
    /// name overrides). Each comes with the component that declares it.
    pub fn invariants(&self, id: &ComponentId) -> Vec<(ComponentId, Invariant)> {
        let mut out: Vec<(ComponentId, Invariant)> = Vec::new();
        for source in std::iter::once(id.clone()).chain(self.interface_chain(id)) {
            let declared = self.components.get(&source).and_then(|c| c.spec.as_ref()).map(|s| s.invariants.clone()).unwrap_or_default();
            for inv in declared {
                if !out.iter().any(|(_, o)| o.name == inv.name) {
                    out.push((source.clone(), inv));
                }
            }
        }
        out
    }

    /// The calls made from `entry`, depth first in call order.
    pub fn call_chain(&self, entry: &ComponentId) -> CallChain {
        let mut chain = CallChain { entry: entry.clone(), steps: Vec::new(), cycles: Vec::new() };
        let mut found: BTreeSet<Vec<ComponentId>> = BTreeSet::new();
        let mut expanded: BTreeSet<ComponentId> = BTreeSet::new();
        self.walk_calls(entry, &mut vec![entry.clone()], &mut expanded, &mut chain.steps, &mut found);
        chain.cycles = found.into_iter().collect();
        chain
    }

    fn walk_calls(&self, caller: &ComponentId, path: &mut Vec<ComponentId>, expanded: &mut BTreeSet<ComponentId>, steps: &mut Vec<ChainStep>, cycles: &mut BTreeSet<Vec<ComponentId>>) {
        let mut calls: Vec<&Relation> = self.outgoing(caller, Some(RelationKind::Calls)).collect();
        calls.sort_by(|a, b| a.order.unwrap_or(u32::MAX).cmp(&b.order.unwrap_or(u32::MAX)).then_with(|| a.target.cmp(&b.target)));
        for call in calls {
            steps.push(ChainStep { caller: caller.clone(), callee: call.target.clone(), order: call.order, depth: path.len() as u32 });
            if let Some(at) = path.iter().position(|p| p == &call.target) {
                let mut cycle = path[at..].to_vec();
                cycle.push(call.target.clone());
                cycles.insert(cycle);
            } else if expanded.insert(call.target.clone()) {
                path.push(call.target.clone());
                self.walk_calls(&call.target, path, expanded, steps, cycles);
                path.pop();
            }
        }
    }

    /// Loops along one relation kind (e.g. containment or implements), each
    /// once, as a path back to its smallest member.
    pub fn cycles(&self, kind: RelationKind) -> Vec<Vec<ComponentId>> {
        let mut edges: BTreeMap<ComponentId, Vec<ComponentId>> = BTreeMap::new();
        for r in self.relations.values().filter(|r| r.kind == kind) {
            edges.entry(r.source.clone()).or_default().push(r.target.clone());
            edges.entry(r.target.clone()).or_default();
        }
        crate::cycles::loops(&edges)
    }

    /// The innermost component whose code span contains `line` in `file`.
    pub fn enclosing(&self, file: &str, line: u32) -> Option<&Component> {
        self.in_file(file)
            .filter(|c| c.anchor.as_ref().is_some_and(|a| a.span.contains_line(line)))
            .min_by_key(|c| c.anchor.as_ref().map(|a| a.span.end_line - a.span.start_line).unwrap_or(u32::MAX))
    }

    pub fn alignment(&self, id: &ComponentId) -> Option<Alignment> {
        let component = self.components.get(id)?;
        let prov = self.provenance(id);
        Some(match (prov.planned, prov.observed) {
            (false, _) => Alignment::Observed,
            (true, true) => Alignment::Present,
            (true, false) if component.stage.expects_code() => Alignment::Missing,
            (true, false) => Alignment::Planned,
        })
    }

    pub fn relation_alignment(&self, relation: &Relation) -> Alignment {
        match (relation.provenance.planned, relation.provenance.observed) {
            (false, _) => Alignment::Observed,
            (true, true) => Alignment::Present,
            (true, false) => {
                let source_in_code = self.components.get(&relation.source).is_some_and(|c| c.stage.expects_code() && c.anchor.is_some());
                if source_in_code { Alignment::Missing } else { Alignment::Planned }
            }
        }
    }

    /// The code (observed graph) with every plan laid over it.
    pub fn merge(observed: Graph, plans: &[Plan]) -> Graph {
        let mut graph = observed;
        for plan in plans {
            for component in &plan.components {
                graph.upsert_component(component.clone(), Provenance::PLANNED);
            }
            for relation in &plan.relations {
                let mut relation = relation.clone();
                relation.provenance = Provenance::PLANNED;
                graph.upsert_relation(relation);
            }
        }
        graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::tests::{anchor_for, planned};
    use crate::{Plan, Stage, Status};

    fn observed(symbol: &str) -> Component {
        let mut c = planned(&format!("rust:{symbol}"), Stage::Implemented);
        c.status = Status::Existing;
        c.anchor = Some(anchor_for(symbol));
        c
    }

    fn code() -> Graph {
        let mut g = Graph::new();
        for s in ["app", "app::run", "app::step"] {
            g.upsert_component(observed(s), Provenance::OBSERVED);
        }
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        g.upsert_relation(Relation::new(RelationKind::Contains, id("app"), id("app::run"), Provenance::OBSERVED));
        g.upsert_relation(Relation::new(RelationKind::Calls, id("app::run"), id("app::step"), Provenance::OBSERVED));
        g
    }

    // req: P2.9
    #[test]
    fn merge_derives_alignment_for_every_case() {
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let mut plan = Plan::new("p1", "Add validation");
        // Modifying existing code: plan + code.
        let mut run = observed("app::run");
        run.status = Status::Modifying;
        run.anchor = None;
        run.description = "planned intent".into();
        plan.components.push(run);
        // New, not expected in code yet.
        plan.components.push(planned("rust:app::validate", Stage::ArchApproved));
        // New, expected in code (skeleton) but missing.
        let mut ghost = planned("rust:app::ghost", Stage::Skeleton);
        ghost.proposed = Some(crate::ProposedLocation {
            language: "rust".into(),
            file: crate::RepoPath::new("src/lib.rs").unwrap(),
            parent: Some(id("app")),
            symbol: "app::ghost".into(),
        });
        plan.components.push(ghost);
        plan.relations.push(Relation::new(RelationKind::Calls, id("app::run"), id("app::validate"), Provenance::PLANNED));
        plan.relations.push(Relation::new(RelationKind::Calls, id("app::run"), id("app::step"), Provenance::PLANNED));

        let g = Graph::merge(code(), &[plan]);
        assert_eq!(g.alignment(&id("app")), Some(Alignment::Observed));
        assert_eq!(g.alignment(&id("app::run")), Some(Alignment::Present));
        assert_eq!(g.alignment(&id("app::validate")), Some(Alignment::Planned));
        assert_eq!(g.alignment(&id("app::ghost")), Some(Alignment::Missing));
        let run = g.component(&id("app::run")).unwrap();
        assert_eq!(run.description, "planned intent", "plan wins for intent");
        assert!(run.anchor.is_some(), "code wins for facts");
        assert_eq!(run.status, Status::Modifying);

        let calls: Vec<_> = g.outgoing(&id("app::run"), Some(RelationKind::Calls)).collect();
        assert_eq!(calls.len(), 2);
        let to_step = calls.iter().find(|r| r.target == id("app::step")).unwrap();
        assert_eq!(g.relation_alignment(to_step), Alignment::Present);
        let to_validate = calls.iter().find(|r| r.target == id("app::validate")).unwrap();
        assert_eq!(g.relation_alignment(to_validate), Alignment::Missing, "caller is in code but doesn't call it");
    }

    // req: P2.9
    #[test]
    fn indices_answer_lookups_deterministically() {
        let g = code();
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        assert_eq!(g.parent(&id("app::run")), Some(&id("app")));
        assert_eq!(g.in_file("src/lib.rs").count(), 3);
        assert_eq!(g.named("run").count(), 1);
        assert_eq!(g.incoming(&id("app::step"), Some(RelationKind::Calls)).count(), 1);
        assert_eq!(g.incoming(&id("app::step"), Some(RelationKind::Contains)).count(), 0);
        let order: Vec<_> = g.components().map(|c| c.id.0.clone()).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted);
        // enclosing() picks the innermost span containing the line.
        let mut g2 = Graph::new();
        let mut outer = observed("m");
        outer.anchor.as_mut().unwrap().span = crate::Span { start_line: 1, end_line: 100, start_byte: 0, end_byte: 1000 };
        let mut inner = observed("m::f");
        inner.anchor.as_mut().unwrap().span = crate::Span { start_line: 10, end_line: 20, start_byte: 100, end_byte: 200 };
        g2.upsert_component(outer, Provenance::OBSERVED);
        g2.upsert_component(inner, Provenance::OBSERVED);
        assert_eq!(g2.enclosing("src/lib.rs", 15).unwrap().id, id("m::f"));
        assert_eq!(g2.enclosing("src/lib.rs", 50).unwrap().id, id("m"));
    }

    // req: P2.7
    #[test]
    fn specs_round_trip_and_invariants_resolve_through_implements() {
        use crate::{ImplementationSpec, InvariantKind, LogicStep, Task, ToolRef};
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let inv = |name: &str, kind: InvariantKind| Invariant { name: name.into(), kind, description: format!("{name} holds") };
        let with_spec = |symbol: &str, invariants: Vec<Invariant>| {
            let mut c = planned(&format!("rust:{symbol}"), Stage::Declared);
            c.spec = Some(ImplementationSpec { invariants, ..ImplementationSpec::default() });
            c
        };
        let mut g = Graph::new();
        g.upsert_component(with_spec("Store", vec![inv("durable", InvariantKind::PostCondition), inv("bounded", InvariantKind::Complexity)]), Provenance::PLANNED);
        g.upsert_component(with_spec("CachedStore", vec![inv("bounded", InvariantKind::StateInvariant), inv("fresh", InvariantKind::StateInvariant)]), Provenance::PLANNED);
        let mut file = with_spec("FileStore", vec![inv("atomic", InvariantKind::PostCondition)]);
        let spec = file.spec.as_mut().unwrap();
        spec.pattern = Some("write-then-rename".into());
        spec.logic_steps = vec![
            LogicStep { sequence: 1, name: "write".into(), description: "Write to a temp file.".into(), algorithm: None, complexity: Some("O(n)".into()), check: Some("test -d .".into()) },
            LogicStep { sequence: 2, name: "rename".into(), description: "Rename over the target.".into(), algorithm: Some("atomic rename".into()), complexity: None, check: None },
        ];
        spec.tasks = vec![Task { text: "fsync the directory".into(), done: false }];
        spec.validation = vec![ToolRef { tool: "clippy".into(), targets: vec!["src/store.rs".into()], args: None }];
        g.upsert_component(file.clone(), Provenance::PLANNED);
        g.upsert_relation(Relation::new(RelationKind::Implements, id("FileStore"), id("CachedStore"), Provenance::PLANNED));
        g.upsert_relation(Relation::new(RelationKind::Implements, id("CachedStore"), id("Store"), Provenance::PLANNED));
        // A loop in implements doesn't hang resolution.
        g.upsert_relation(Relation::new(RelationKind::Implements, id("Store"), id("FileStore"), Provenance::PLANNED));

        let json = serde_json::to_string(&file).unwrap();
        assert_eq!(serde_json::from_str::<Component>(&json).unwrap(), file, "specs round-trip");
        assert!(json.contains("\"kind\":\"post_condition\""));

        assert_eq!(g.interface_chain(&id("FileStore")), vec![id("CachedStore"), id("Store")]);
        let resolved: Vec<(String, String, InvariantKind)> = g.invariants(&id("FileStore")).into_iter().map(|(from, i)| (from.0, i.name, i.kind)).collect();
        assert_eq!(
            resolved,
            vec![
                ("rust:FileStore".into(), "atomic".into(), InvariantKind::PostCondition),
                ("rust:CachedStore".into(), "bounded".into(), InvariantKind::StateInvariant),
                ("rust:CachedStore".into(), "fresh".into(), InvariantKind::StateInvariant),
                ("rust:Store".into(), "durable".into(), InvariantKind::PostCondition),
            ],
            "own first, then nearest interface; nearer overrides by name"
        );
    }

    // req: P2.8
    #[test]
    fn workflow_chains_follow_ordered_calls_and_report_cycles() {
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let mut g = Graph::new();
        for s in ["main", "load", "parse", "save", "retry"] {
            g.upsert_component(observed(s), Provenance::OBSERVED);
        }
        let call = |a: &str, b: &str, order: Option<u32>, prov: Provenance| {
            let mut r = Relation::new(RelationKind::Calls, id(a), id(b), prov);
            r.order = order;
            r
        };
        // Planned and observed calls both count; order decides, not insertion.
        g.upsert_relation(call("main", "save", Some(1), Provenance::PLANNED));
        g.upsert_relation(call("main", "load", Some(0), Provenance::OBSERVED));
        g.upsert_relation(call("load", "parse", Some(0), Provenance::OBSERVED));
        g.upsert_relation(call("save", "retry", Some(0), Provenance::PLANNED));
        g.upsert_relation(call("retry", "save", Some(0), Provenance::PLANNED));
        let chain = g.call_chain(&id("main"));
        let steps: Vec<(String, String, u32)> = chain.steps.iter().map(|s| (s.caller.0.clone(), s.callee.0.clone(), s.depth)).collect();
        assert_eq!(
            steps,
            vec![
                ("rust:main".into(), "rust:load".into(), 1),
                ("rust:load".into(), "rust:parse".into(), 2),
                ("rust:main".into(), "rust:save".into(), 1),
                ("rust:save".into(), "rust:retry".into(), 2),
                ("rust:retry".into(), "rust:save".into(), 3),
            ]
        );
        assert_eq!(chain.cycles, vec![vec![id("save"), id("retry"), id("save")]]);
        let mut plan = Plan::new("p", "P");
        plan.workflows.push(crate::Workflow { name: "checkout".into(), entry: id("main"), description: String::new() });
        assert_eq!(Plan::from_json(&plan.to_json().unwrap()).unwrap(), plan, "workflows persist by entry only");
    }

    #[test]
    fn cycles_along_one_kind_come_with_their_loop() {
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let mut g = code();
        g.upsert_relation(Relation::new(RelationKind::Contains, id("app::run"), id("app"), Provenance::PLANNED));
        assert_eq!(g.cycles(RelationKind::Contains), vec![vec![id("app"), id("app::run"), id("app")]]);
        assert!(g.cycles(RelationKind::Implements).is_empty());
    }
}
