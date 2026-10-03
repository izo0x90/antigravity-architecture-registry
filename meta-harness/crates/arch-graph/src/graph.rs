//! The graph aggregate: components and relations with lookup indices, and the
//! merge of what the code shows (observed) with what plans say (planned).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Component, ComponentId, Plan, Provenance, Relation, RelationId, RelationKind};

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
}
