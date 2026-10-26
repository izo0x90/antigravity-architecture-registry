//! The graph aggregate: components and relations with lookup indices, and the
//! merge of what the code shows (observed) with what plans say (planned).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use imbl::{HashMap as PMap, OrdMap, OrdSet};

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

/// Whether the plan says this component is in the code: it exists already
/// (or is being changed or retired), or its stage means it was built.
fn expects_code(c: &Component) -> bool {
    matches!(c.status, crate::Status::Existing | crate::Status::Modifying | crate::Status::Deprecated) || c.stage.expects_code()
}

/// A graph can't be copied: this stops compiling the day `Graph` gets a
/// `Clone` (the call names two impls then, and can't pick).
const _: fn() = || {
    trait AmbiguousIfClone<A> {
        fn which() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    impl<T: Clone> AmbiguousIfClone<u8> for T {}
    let _ = <Graph as AmbiguousIfClone<_>>::which;
};

/// Components put into any graph in this process. The gate tests count
/// them too: a whole rebuild puts every component in again.
#[cfg(any(test, feature = "testing"))]
static INSERTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many components have been put into graphs so far.
#[cfg(any(test, feature = "testing"))]
pub fn inserts() -> usize {
    INSERTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Every component id seen, interned: the
/// graph's adjacency is keyed by these small numbers. Lookups take no lock,
/// so readers never wait on the writer interning new ids.
static IDS: std::sync::LazyLock<lasso::ThreadedRodeo> = std::sync::LazyLock::new(lasso::ThreadedRodeo::default);

fn intern(id: &ComponentId) -> lasso::Spur {
    IDS.get_or_intern(id.as_str())
}

fn interned(id: &ComponentId) -> Option<lasso::Spur> {
    IDS.get(id.as_str())
}

/// One component's relations of each kind (in `RelationKind::ALL`'s order).
#[derive(Clone, Debug, Default)]
struct Adjacent([OrdSet<RelationId>; 4]);

impl Adjacent {
    fn kind(kind: RelationKind) -> usize {
        RelationKind::ALL.iter().position(|k| *k == kind).unwrap_or(0)
    }

    /// The relations of `kind`, or of every kind in id order.
    fn of(&self, kind: Option<RelationKind>) -> Vec<&RelationId> {
        match kind {
            Some(k) => self.0[Self::kind(k)].iter().collect(),
            None => {
                let mut all: Vec<&RelationId> = self.0.iter().flatten().collect();
                all.sort();
                all
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.0.iter().all(OrdSet::is_empty)
    }
}

/// Components and relations with lookups. Every part is a persistent map:
/// a change makes a new version sharing everything it didn't touch, so a
/// snapshot (`share`) copies nothing.
#[derive(Debug, Default)]
pub struct Graph {
    components: OrdMap<ComponentId, Arc<Component>>,
    provenance: OrdMap<ComponentId, Provenance>,
    relations: OrdMap<RelationId, Arc<Relation>>,
    by_file: PMap<String, OrdSet<ComponentId>>,
    by_name: PMap<String, OrdSet<ComponentId>>,
    outgoing: PMap<lasso::Spur, Adjacent>,
    incoming: PMap<lasso::Spur, Adjacent>,
}

impl Graph {
    /// This graph, shared: a new version to change without touching this
    /// one (an overlay's base, a snapshot). Nothing is copied.
    pub fn share(&self) -> Graph {
        Graph {
            components: self.components.clone(),
            provenance: self.provenance.clone(),
            relations: self.relations.clone(),
            by_file: self.by_file.clone(),
            by_name: self.by_name.clone(),
            outgoing: self.outgoing.clone(),
            incoming: self.incoming.clone(),
        }
    }

    /// Adds a component, or merges it into the one with the same id: plan data
    /// wins for intent (type, description, stage, spec, signature), code data
    /// wins for facts (anchor, placeholders).
    pub fn upsert_component(&mut self, component: Component, provenance: Provenance) {
        let id = component.id.clone();
        let merged = match self.components.get(&id) {
            Some(existing) => {
                let existing_prov = self.provenance.get(&id).copied().unwrap_or_default();
                let existing = (**existing).clone();
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
        let prov = self.provenance.get(&id).copied().unwrap_or_default().merge(provenance);
        self.put_component(merged, prov);
    }

    /// Puts `component` in as it is (replacing any with its id, not merged).
    pub fn put_component(&mut self, component: Component, provenance: Provenance) {
        #[cfg(any(test, feature = "testing"))]
        INSERTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = component.id.clone();
        if let Some(old) = self.components.get(&id).cloned() {
            self.unindex_component(&old);
        }
        if let Some(file) = component.file() {
            self.by_file.entry(file.as_str().to_owned()).or_default().insert(id.clone());
        }
        self.by_name.entry(component.name.clone()).or_default().insert(id.clone());
        intern(&id);
        self.provenance.insert(id.clone(), provenance);
        self.components.insert(id, Arc::new(component));
    }

    /// Takes a component out (its relations stay until taken out too).
    pub fn remove_component(&mut self, id: &ComponentId) {
        if let Some(old) = self.components.remove(id) {
            self.unindex_component(&old);
        }
        self.provenance.remove(id);
    }

    /// Drops `old` from the file and name lookups (only the two sets it's in).
    fn unindex_component(&mut self, old: &Component) {
        if let Some(file) = old.file()
            && let Some(set) = self.by_file.get_mut(file.as_str())
        {
            set.remove(&old.id);
            if set.is_empty() {
                self.by_file.remove(file.as_str());
            }
        }
        if let Some(set) = self.by_name.get_mut(&old.name) {
            set.remove(&old.id);
            if set.is_empty() {
                self.by_name.remove(&old.name);
            }
        }
    }

    /// Adds a relation, or folds it into the existing one with the same id.
    pub fn upsert_relation(&mut self, relation: Relation) {
        let id = relation.id();
        match self.relations.get(&id) {
            Some(existing) => {
                let mut folded = (**existing).clone();
                folded.absorb(&relation);
                self.relations.insert(id, Arc::new(folded));
            }
            None => self.put_relation(relation),
        }
    }

    /// Puts `relation` in as it is (replacing any with its id, not merged).
    pub fn put_relation(&mut self, relation: Relation) {
        let id = relation.id();
        let kind = Adjacent::kind(relation.kind);
        self.outgoing.entry(intern(&relation.source)).or_default().0[kind].insert(id.clone());
        self.incoming.entry(intern(&relation.target)).or_default().0[kind].insert(id.clone());
        self.relations.insert(id, Arc::new(relation));
    }

    /// Takes a relation out.
    pub fn remove_relation(&mut self, id: &RelationId) {
        let Some(old) = self.relations.remove(id) else { return };
        let kind = Adjacent::kind(old.kind);
        for (map, end) in [(&mut self.outgoing, &old.source), (&mut self.incoming, &old.target)] {
            if let Some(key) = interned(end)
                && let Some(adjacent) = map.get_mut(&key)
            {
                adjacent.0[kind].remove(id);
                if adjacent.is_empty() {
                    map.remove(&key);
                }
            }
        }
    }

    pub fn component(&self, id: &ComponentId) -> Option<&Component> {
        self.components.get(id).map(|c| &**c)
    }

    pub fn components(&self) -> impl Iterator<Item = &Component> {
        self.components.values().map(|c| &**c)
    }

    pub fn relations(&self) -> impl Iterator<Item = &Relation> {
        self.relations.values().map(|r| &**r)
    }

    pub fn relation(&self, id: &RelationId) -> Option<&Relation> {
        self.relations.get(id).map(|r| &**r)
    }

    pub fn len(&self) -> (usize, usize) {
        (self.components.len(), self.relations.len())
    }

    pub fn provenance(&self, id: &ComponentId) -> Provenance {
        self.provenance.get(id).copied().unwrap_or_default()
    }

    pub fn in_file(&self, file: &str) -> impl Iterator<Item = &Component> {
        self.by_file.get(file).into_iter().flatten().filter_map(|id| self.component(id))
    }

    pub fn named(&self, name: &str) -> impl Iterator<Item = &Component> {
        self.by_name.get(name).into_iter().flatten().filter_map(|id| self.component(id))
    }

    pub fn outgoing<'a>(&'a self, id: &ComponentId, kind: Option<RelationKind>) -> impl Iterator<Item = &'a Relation> + use<'a> {
        let ids = interned(id).and_then(|key| self.outgoing.get(&key)).map(|a| a.of(kind)).unwrap_or_default();
        ids.into_iter().filter_map(|r| self.relation(r))
    }

    pub fn incoming<'a>(&'a self, id: &ComponentId, kind: Option<RelationKind>) -> impl Iterator<Item = &'a Relation> + use<'a> {
        let ids = interned(id).and_then(|key| self.incoming.get(&key)).map(|a| a.of(kind)).unwrap_or_default();
        ids.into_iter().filter_map(|r| self.relation(r))
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
            let mut next: Vec<ComponentId> = self
                .outgoing(&current, Some(RelationKind::Implements))
                .map(|r| r.target.clone())
                .filter(|t| t != id && !chain.contains(t))
                .collect();
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

    /// What `id` needs to exist before it can be built: what it calls and
    /// implements, its container, the sources of data flowing into it, and the
    /// data types its schemas reference (`is_data_type` picks which components
    /// can be referenced by title).
    pub fn dependencies(&self, id: &ComponentId, is_data_type: impl Fn(&Component) -> bool) -> BTreeSet<ComponentId> {
        let mut deps: BTreeSet<ComponentId> = BTreeSet::new();
        deps.extend(self.outgoing(id, Some(RelationKind::Calls)).map(|r| r.target.clone()));
        deps.extend(self.outgoing(id, Some(RelationKind::Implements)).map(|r| r.target.clone()));
        deps.extend(self.incoming(id, Some(RelationKind::DataFlow)).map(|r| r.source.clone()));
        deps.extend(self.parent(id).cloned());
        if let Some(c) = self.components.get(id) {
            let schemas = [&c.schemas.properties, &c.schemas.inputs, &c.schemas.outputs];
            for title in schemas.into_iter().flatten().flat_map(crate::schema::refs) {
                deps.extend(self.named(&title).filter(|d| is_data_type(d)).map(|d| d.id.clone()));
            }
        }
        deps.remove(id);
        deps
    }

    /// What `from` reaches along `kinds` (outgoing) within `depth` steps,
    /// itself included.
    pub fn reach(&self, from: &ComponentId, kinds: &[RelationKind], depth: u32) -> BTreeSet<ComponentId> {
        let mut seen: BTreeSet<ComponentId> = BTreeSet::from([from.clone()]);
        let mut frontier = vec![from.clone()];
        for _ in 0..depth {
            let mut next = Vec::new();
            for id in &frontier {
                for kind in kinds {
                    for r in self.outgoing(id, Some(*kind)) {
                        if seen.insert(r.target.clone()) {
                            next.push(r.target.clone());
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        seen
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
            (true, false) if expects_code(component) => Alignment::Missing,
            (true, false) => Alignment::Planned,
        })
    }

    pub fn relation_alignment(&self, relation: &Relation) -> Alignment {
        match (relation.provenance.planned, relation.provenance.observed) {
            (false, _) => Alignment::Observed,
            (true, true) => Alignment::Present,
            (true, false) => {
                let source_in_code = self.components.get(&relation.source).is_some_and(|c| expects_code(c) && c.anchor.is_some());
                if source_in_code { Alignment::Missing } else { Alignment::Planned }
            }
        }
    }

    /// The code (observed graph) with every plan laid over it.
    pub fn merge(observed: Graph, plans: &[Plan]) -> Graph {
        let mut graph = observed;
        for plan in plans {
            for component in &plan.components {
                // Where something is in the code comes from the code only.
                let mut component = component.clone();
                component.anchor = None;
                component.placeholders.clear();
                graph.upsert_component(component, Provenance::PLANNED);
            }
            for relation in &plan.relations {
                let mut relation = relation.clone();
                relation.provenance = Provenance::PLANNED;
                graph.upsert_relation(relation);
            }
        }
        graph
    }

    /// The code (`self`) with `plan` laid over it, as `merge` makes it. When
    /// `from` is the code with an earlier version of the same plan laid over
    /// it, only what changed between the two versions is put in (a thin
    /// layer: everything else is shared).
    pub fn laid_over(&self, plan: &Plan, from: Option<(&Plan, &Graph)>) -> Graph {
        let Some((earlier, layered)) = from else { return Graph::merge(self.share(), std::slice::from_ref(plan)) };
        let mut out = layered.share();
        let now: BTreeMap<&ComponentId, &Component> = plan.components.iter().map(|c| (&c.id, c)).collect();
        let before: BTreeMap<&ComponentId, &Component> = earlier.components.iter().map(|c| (&c.id, c)).collect();
        for id in now.keys().chain(before.keys()).copied().collect::<BTreeSet<_>>() {
            let same = match (now.get(id), before.get(id)) {
                (Some(a), Some(b)) => a.identical(b),
                (a, b) => a.is_none() && b.is_none(),
            };
            if same {
                continue;
            }
            // The code's, with the plan's laid over it.
            out.remove_component(id);
            if let Some(code) = self.components.get(id) {
                out.put_component((**code).clone(), self.provenance(id));
            }
            if let Some(planned) = now.get(id) {
                let mut planned = (*planned).clone();
                planned.anchor = None;
                planned.placeholders.clear();
                out.upsert_component(planned, Provenance::PLANNED);
            }
        }
        let now: BTreeMap<RelationId, &Relation> = plan.relations.iter().map(|r| (r.id(), r)).collect();
        let before: BTreeMap<RelationId, &Relation> = earlier.relations.iter().map(|r| (r.id(), r)).collect();
        for id in now.keys().chain(before.keys()).cloned().collect::<BTreeSet<_>>() {
            if now.get(&id) == before.get(&id) {
                continue;
            }
            out.remove_relation(&id);
            if let Some(code) = self.relations.get(&id) {
                out.put_relation((**code).clone());
            }
            if let Some(planned) = now.get(&id) {
                let mut planned = (*planned).clone();
                planned.provenance = Provenance::PLANNED;
                out.upsert_relation(planned);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::tests::{anchor_for, planned};
    use crate::{Plan, Stage, Status};

    /// A change made to a plan.
    type Edit = Box<dyn Fn(&mut Plan)>;

    fn observed(symbol: &str) -> Component {
        let mut c = planned(&format!("rust:{symbol}"), Stage::Implemented);
        c.status = Status::Existing;
        c.anchor = Some(anchor_for(symbol));
        c
    }

    fn code() -> Graph {
        let mut g = Graph::default();
        for s in ["app", "app::run", "app::step"] {
            g.upsert_component(observed(s), Provenance::OBSERVED);
        }
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        g.upsert_relation(Relation::new(RelationKind::Contains, id("app"), id("app::run"), Provenance::OBSERVED));
        g.upsert_relation(Relation::new(RelationKind::Calls, id("app::run"), id("app::step"), Provenance::OBSERVED));
        g
    }

    /// A graph as comparable text.
    fn text(g: &Graph) -> Vec<String> {
        let mut out: Vec<String> =
            g.components().map(|c| format!("{} {:?}", serde_json::to_string(c).unwrap(), g.provenance(&c.id))).collect();
        out.extend(g.relations().map(|r| serde_json::to_string(r).unwrap()));
        for c in g.components() {
            out.extend(g.outgoing(&c.id, None).map(|r| format!("out {} {}", c.id, r.id())));
            out.extend(g.incoming(&c.id, None).map(|r| format!("in {} {}", c.id, r.id())));
        }
        out
    }

    #[test]
    fn adjacency_is_kept_per_kind_and_a_change_takes_out_and_puts_in_only_its_own() {
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let mut g = code();
        g.put_relation(Relation::new(RelationKind::Implements, id("app::run"), id("app"), Provenance::OBSERVED));
        let kinds = |g: &Graph| g.outgoing(&id("app::run"), None).map(|r| r.kind).collect::<Vec<_>>();
        assert_eq!(kinds(&g), [RelationKind::Calls, RelationKind::Implements], "every kind, in id order");
        assert_eq!(g.outgoing(&id("app::run"), Some(RelationKind::Calls)).count(), 1, "one kind's own list");
        // A snapshot taken before the change keeps what it had.
        let before = g.share();
        let calls = Relation::new(RelationKind::Calls, id("app::run"), id("app::step"), Provenance::OBSERVED).id();
        g.remove_relation(&calls);
        g.remove_component(&id("app::step"));
        assert_eq!(kinds(&g), [RelationKind::Implements]);
        assert!(g.incoming(&id("app::step"), None).next().is_none() && g.component(&id("app::step")).is_none());
        assert_eq!(kinds(&before), [RelationKind::Calls, RelationKind::Implements], "shared, not changed");
        assert!(before.component(&id("app::step")).is_some());
        assert!(g.in_file(before.component(&id("app::step")).unwrap().file().unwrap().as_str()).all(|c| c.id != id("app::step")));
    }

    #[test]
    fn a_plan_laid_over_by_its_changes_is_the_plan_laid_over_whole_and_shares_the_rest() {
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let code = code();
        let mut plan = Plan::new("p", "P");
        let mut run = observed("app::run");
        run.status = Status::Modifying;
        run.anchor = None;
        plan.components.push(run);
        plan.components.push(planned("rust:app::check", Stage::ArchApproved));
        plan.relations.push(Relation::new(RelationKind::Calls, id("app::run"), id("app::check"), Provenance::PLANNED));
        let mut layered = code.laid_over(&plan, None);
        let mut earlier = plan.clone();
        // Edits: a stage, a new component, a relation dropped, a component dropped.
        let edits: Vec<Edit> = vec![
            Box::new(|p: &mut Plan| p.components[1].stage = Stage::PlanApproved),
            Box::new(|p: &mut Plan| p.components.push(planned("rust:app::log", Stage::ArchApproved))),
            Box::new(|p: &mut Plan| p.relations.clear()),
            Box::new(|p: &mut Plan| p.components.retain(|c| c.id != ComponentId::new("rust:app::run"))),
        ];
        for edit in edits {
            let mut next = earlier.clone();
            edit(&mut next);
            let thin = code.laid_over(&next, Some((&earlier, &layered)));
            assert_eq!(text(&thin), text(&Graph::merge(code.share(), std::slice::from_ref(&next))));
            layered = thin;
            earlier = next;
        }
    }

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
        let mut g2 = Graph::default();
        let mut outer = observed("m");
        outer.anchor.as_mut().unwrap().span = crate::Span { start_line: 1, end_line: 100, start_byte: 0, end_byte: 1000 };
        let mut inner = observed("m::f");
        inner.anchor.as_mut().unwrap().span = crate::Span { start_line: 10, end_line: 20, start_byte: 100, end_byte: 200 };
        g2.upsert_component(outer, Provenance::OBSERVED);
        g2.upsert_component(inner, Provenance::OBSERVED);
        assert_eq!(g2.enclosing("src/lib.rs", 15).unwrap().id, id("m::f"));
        assert_eq!(g2.enclosing("src/lib.rs", 50).unwrap().id, id("m"));
    }

    #[test]
    fn adding_components_keeps_lookups_right_and_takes_linear_time() {
        let mut g = Graph::default();
        let mut moved = observed("m::f");
        g.upsert_component(moved.clone(), Provenance::OBSERVED);
        // The same component found again in another file.
        moved.anchor.as_mut().unwrap().file = crate::RepoPath::new("src/other.rs").unwrap();
        g.upsert_component(moved, Provenance::OBSERVED);
        assert_eq!(g.in_file("src/lib.rs").count(), 0, "gone from its old file");
        assert_eq!(g.in_file("src/other.rs").count(), 1);
        assert_eq!(g.named("f").count(), 1, "found once by name");
        // Many files and names: each add touches only its own two sets.
        let started = std::time::Instant::now();
        let mut big = Graph::default();
        for i in 0..40_000 {
            let mut c = observed(&format!("m{}::f{i}", i % 400));
            c.anchor.as_mut().unwrap().file = crate::RepoPath::new(&format!("src/m{}.rs", i % 4000)).unwrap();
            big.upsert_component(c, Provenance::OBSERVED);
        }
        assert_eq!(big.len().0, 40_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn reach_follows_kinds_to_a_depth() {
        let g = code();
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        assert_eq!(g.reach(&id("app"), &[RelationKind::Contains], 1), BTreeSet::from([id("app"), id("app::run")]));
        assert_eq!(g.reach(&id("app"), &[RelationKind::Contains, RelationKind::Calls], 2).len(), 3);
        assert_eq!(g.reach(&id("app"), &[RelationKind::Calls], 5), BTreeSet::from([id("app")]));
    }

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
        let mut g = Graph::default();
        g.upsert_component(
            with_spec("Store", vec![inv("durable", InvariantKind::PostCondition), inv("bounded", InvariantKind::Complexity)]),
            Provenance::PLANNED,
        );
        g.upsert_component(
            with_spec("CachedStore", vec![inv("bounded", InvariantKind::StateInvariant), inv("fresh", InvariantKind::StateInvariant)]),
            Provenance::PLANNED,
        );
        let mut file = with_spec("FileStore", vec![inv("atomic", InvariantKind::PostCondition)]);
        let spec = file.spec.as_mut().unwrap();
        spec.pattern = Some("write-then-rename".into());
        spec.logic_steps = vec![
            LogicStep {
                sequence: 1,
                name: "write".into(),
                description: "Write to a temp file.".into(),
                algorithm: None,
                complexity: Some("O(n)".into()),
                check: Some("test -d .".into()),
            },
            LogicStep {
                sequence: 2,
                name: "rename".into(),
                description: "Rename over the target.".into(),
                algorithm: Some("atomic rename".into()),
                complexity: None,
                check: None,
            },
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
        let resolved: Vec<(String, String, InvariantKind)> =
            g.invariants(&id("FileStore")).into_iter().map(|(from, i)| (from.0, i.name, i.kind)).collect();
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

    #[test]
    fn workflows_persist_by_entry_only() {
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let mut plan = Plan::new("p", "P");
        plan.workflows.push(crate::Workflow { name: "checkout".into(), entry: id("main"), description: String::new() });
        assert_eq!(Plan::from_json(&plan.to_json().unwrap()).unwrap(), plan);
    }

    #[test]
    fn cycles_along_one_kind_come_with_their_loop() {
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let mut g = code();
        g.upsert_relation(Relation::new(RelationKind::Contains, id("app::run"), id("app"), Provenance::PLANNED));
        assert_eq!(g.cycles(RelationKind::Contains), vec![vec![id("app"), id("app::run"), id("app")]]);
        assert!(g.cycles(RelationKind::Implements).is_empty());
    }

    #[test]
    fn alignment_follows_status_and_covers_every_relation_case() {
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let mut plan = Plan::new("p", "P");
        // Planned changes to code that has since gone: the plan expects code.
        let mut gone = observed("app::gone");
        gone.status = Status::Modifying;
        gone.stage = Stage::Declared;
        gone.anchor = None;
        plan.components.push(gone);
        let mut retired = observed("app::retired");
        retired.status = Status::Deprecated;
        retired.stage = Stage::Declared;
        plan.components.push(retired);
        plan.components.push(planned("rust:app::new", Stage::Declared));
        plan.relations.push(Relation::new(RelationKind::Calls, id("app::new"), id("app::step"), Provenance::PLANNED));
        let g = Graph::merge(code(), &[plan]);
        assert_eq!(g.alignment(&id("app::gone")), Some(Alignment::Missing));
        assert_eq!(g.alignment(&id("app::retired")), Some(Alignment::Missing));
        assert_eq!(g.alignment(&id("app::new")), Some(Alignment::Planned));
        let observed_call = g.outgoing(&id("app::run"), Some(RelationKind::Calls)).next().unwrap();
        assert_eq!(g.relation_alignment(observed_call), Alignment::Observed);
        let planned_call = g.outgoing(&id("app::new"), Some(RelationKind::Calls)).next().unwrap();
        assert_eq!(g.relation_alignment(planned_call), Alignment::Planned, "its caller isn't in code yet");
    }

    #[test]
    fn anchors_only_come_from_the_code() {
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let mut plan = Plan::new("p", "P");
        // A plan that claims an anchor (hand-edited) for code that isn't there.
        let mut forged = planned("rust:app::forged", Stage::Implemented);
        forged.anchor = Some(anchor_for("app::forged"));
        forged.placeholders.push(crate::Span { start_line: 1, end_line: 1, start_byte: 0, end_byte: 1 });
        plan.components.push(forged);
        // A planned change to real code keeps the code's anchor.
        let mut run = observed("app::run");
        run.status = Status::Modifying;
        run.anchor = Some(anchor_for("elsewhere::run"));
        plan.components.push(run);
        let g = Graph::merge(code(), &[plan]);
        let forged = g.component(&id("app::forged")).unwrap();
        assert!(forged.anchor.is_none() && forged.placeholders.is_empty());
        assert!(forged.check_location().is_err(), "implemented without code fails the location rule");
        assert_eq!(g.component(&id("app::run")).unwrap().anchor.as_ref().unwrap().symbol, "app::run", "the code's anchor, not the plan's");
    }
}
