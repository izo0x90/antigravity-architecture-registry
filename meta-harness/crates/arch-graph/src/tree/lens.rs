//! What each view means: a card's children (down or up), how a card reads,
//! where a view starts without a root, and with a plan in focus, what's
//! part of the plan and what's only the code it plugs into.

use std::collections::{BTreeMap, BTreeSet};

use super::{Crumb, GraphRead, Input, Mark, Pill, PillRole, TreeRequest, ViewKind, Way};
use crate::{Alignment, Component, ComponentId, RelationKind, Stage, Status};

const TYPE: &str = "type:";
const START: &str = "start:";
const END: &str = "end:";

/// A card's child, and the line to it.
pub(super) struct Child {
    pub key: String,
    pub relation: RelationKind,
    pub order: Option<u32>,
    pub mark: Mark,
    pub pills: Vec<Pill>,
    pub evidence: Option<super::Evidence>,
}

/// How a card reads.
pub(super) struct Face {
    pub label: String,
    pub kind: String,
    pub detail: String,
    pub component: Option<ComponentId>,
    pub mark: Mark,
    pub other_plans: Vec<crate::PlanId>,
    pub callable: bool,
}

/// The data-flow view's index: every type's producers and consumers, and
/// data types by name. Built once per graph (a snapshot, or a plan laid
/// over it) and shared by every request on it.
#[derive(Default)]
pub struct FlowIndex {
    flows: Flows,
    /// Data types by name (to link a data card to its component).
    data_named: BTreeMap<String, ComponentId>,
    /// Every data type's name (some shared by two types).
    data_names: BTreeSet<String>,
}

/// Data flow seen by type: which functions take and produce each type.
#[derive(Default)]
struct Flows {
    /// Type -> the functions it flows into.
    consumers: BTreeMap<String, BTreeSet<ComponentId>>,
    /// Type -> the functions it flows out of.
    producers: BTreeMap<String, BTreeSet<ComponentId>>,
    takes: BTreeMap<ComponentId, BTreeSet<String>>,
    produces: BTreeMap<ComponentId, BTreeSet<String>>,
    /// The worst mark among the flows of a type through a function.
    marks: BTreeMap<(ComponentId, String), Mark>,
    /// How the flows of a type through a function are known (the weakest).
    evidence: BTreeMap<(ComponentId, String), super::Evidence>,
    /// Types only plans carry (no flow of them is in the code).
    planned_only: BTreeSet<String>,
}

/// What's in focus: a plan's own components, or only those asked for
/// (`TreeRequest::only`), or both.
struct Focus {
    /// The components in focus.
    items: BTreeSet<ComponentId>,
    /// Contains: the items and every box that holds one.
    held: BTreeSet<ComponentId>,
    /// Data flow: the types the items' flows carry (a plan's), and the
    /// data items themselves.
    types: BTreeSet<String>,
    /// A box in focus shows all it holds (a plan's: what it adds goes in
    /// whole); otherwise only what's in focus (only those asked for).
    whole: bool,
}

pub(super) struct Lens<'a, G: GraphRead> {
    input: &'a Input<'a, G>,
    pub view: ViewKind,
    pub way: Way,
    index: std::sync::Arc<FlowIndex>,
    focus: Option<Focus>,
}

/// Plan marks for a component.
pub(super) fn component_mark(alignment: Alignment, c: &Component) -> Mark {
    match alignment {
        Alignment::Observed => Mark::Code,
        Alignment::Missing => Mark::Mismatch,
        Alignment::Planned => match c.status {
            Status::Modifying | Status::Deprecated => Mark::Changed,
            _ => Mark::New,
        },
        Alignment::Present => match (c.status, c.stage) {
            (Status::Existing, _) => Mark::Code,
            (_, Stage::Implemented) => Mark::Done,
            (Status::Modifying | Status::Deprecated, _) => Mark::Changed,
            (Status::New, _) => Mark::New,
        },
    }
}

pub(super) fn relation_mark(alignment: Alignment) -> Mark {
    match alignment {
        Alignment::Observed => Mark::Code,
        Alignment::Planned => Mark::New,
        Alignment::Missing => Mark::Mismatch,
        Alignment::Present => Mark::Done,
    }
}

/// The worse of two marks (what a merged line shows).
fn worse(a: Mark, b: Mark) -> Mark {
    let rank = |m: Mark| match m {
        Mark::Code => 0,
        Mark::Done => 1,
        Mark::Changed => 2,
        Mark::New => 3,
        Mark::Mismatch => 4,
    };
    if rank(b) > rank(a) { b } else { a }
}

impl<'a, G: GraphRead> Lens<'a, G> {
    pub fn new(input: &'a Input<'a, G>, request: &TreeRequest) -> Self {
        let mut lens = Lens { input, view: request.view, way: request.way, index: Default::default(), focus: None };
        if request.view == ViewKind::DataFlow {
            lens.index = match &input.flows {
                Some(index) => index.clone(),
                None => std::sync::Arc::new(lens.flow_index()),
            };
        }
        // In focus: the plan's items (unless shown in the whole code), and
        // of them (or of everything) only those asked for.
        let plan = input.plan.filter(|_| !request.full_code);
        let only: Option<BTreeSet<ComponentId>> = request.only.as_ref().map(|ids| ids.iter().map(ComponentId::new).collect());
        let items: Option<BTreeSet<ComponentId>> = match (plan, only) {
            (Some(plan), only) => {
                Some(plan.components.iter().map(|c| c.id.clone()).filter(|id| only.as_ref().is_none_or(|o| o.contains(id))).collect())
            }
            (None, only) => only,
        };
        if let Some(items) = items {
            let mut held = items.clone();
            for id in &items {
                held.extend(lens.ancestors(id));
            }
            let mut types: BTreeSet<String> = plan
                .into_iter()
                .flat_map(|p| &p.relations)
                .filter(|r| r.kind == RelationKind::DataFlow && (items.contains(&r.source) || items.contains(&r.target)))
                .filter_map(|r| lens.flow_type(r))
                .collect();
            let data = |id: &ComponentId| {
                plan.and_then(|p| p.component(id))
                    .or_else(|| lens.graph().component(id))
                    .filter(|c| lens.is_data(c))
                    .map(|c| format!("{TYPE}{}", c.name))
            };
            types.extend(items.iter().filter_map(data));
            lens.focus = Some(Focus { items, held, types, whole: request.only.is_none() });
        }
        lens
    }

    fn graph(&self) -> &'a G {
        self.input.graph
    }

    /// A data type: it has fields and no signature (by its type's rules).
    fn is_data(&self, c: &Component) -> bool {
        self.input.types.resolve(&c.type_name).is_ok_and(|t| t.allows_properties && !t.allows_signature)
    }

    /// Something that can be called: its type allows a signature.
    fn is_callable(&self, c: &Component) -> bool {
        self.input.types.resolve(&c.type_name).is_ok_and(|t| t.allows_signature)
    }

    /// `text` as a data card's key, or none if it isn't data worth a card.
    fn type_key(&self, text: &str) -> Option<String> {
        let data = &self.input.config.data;
        let mut t = text.trim();
        loop {
            let before = t;
            for prefix in &data.strip {
                t = t.strip_prefix(prefix.as_str()).unwrap_or(t).trim_start();
            }
            if t == before {
                break;
            }
        }
        (!t.is_empty() && !data.ignore.iter().any(|i| i == t)).then(|| format!("{TYPE}{t}"))
    }

    /// What a flow carries, as a data card's key: what it says it carries,
    /// else what its ports name (the source's return type, or the type of
    /// the target's parameter it goes into).
    fn flow_type(&self, r: &crate::Relation) -> Option<String> {
        if let Some(data) = &r.data {
            return self.type_key(data);
        }
        let (from, to) = r.ports.as_ref()?;
        let written = if from == "return" { self.graph().component(&r.source)?.signature.as_ref()?.returns.clone() } else { None }
            .or_else(|| self.graph().component(&r.target)?.signature.as_ref()?.params.iter().find(|p| &p.name == to)?.ty.clone())?;
        self.type_key(&written)
    }

    /// The data-flow index of this lens's graph (see `FlowIndex`).
    pub(super) fn flow_index(&self) -> FlowIndex {
        let mut data_named: BTreeMap<String, ComponentId> = BTreeMap::new();
        for c in self.input.graph.components().filter(|c| self.is_data(c)) {
            // A name two data types share links to neither.
            match data_named.get(&c.name) {
                Some(other) if other != &c.id => {
                    data_named.insert(c.name.clone(), ComponentId::new(""));
                }
                _ => {
                    data_named.insert(c.name.clone(), c.id.clone());
                }
            }
        }
        let data_names = data_named.keys().cloned().collect();
        data_named.retain(|_, id| !id.as_str().is_empty());
        FlowIndex { flows: self.index_flows(), data_named, data_names }
    }

    fn index_flows(&self) -> Flows {
        let mut flows = Flows::default();
        let mut observed_types: BTreeSet<String> = BTreeSet::new();
        let mut planned_types: BTreeSet<String> = BTreeSet::new();
        // A data type at either end is the type itself (made, or taken
        // apart), not a step: its card stands for it.
        let step = |id: &ComponentId| self.graph().component(id).is_none_or(|c| !self.is_data(c));
        for r in self.graph().relations().filter(|r| r.kind == RelationKind::DataFlow) {
            let Some(key) = self.flow_type(r) else { continue };
            let mark = relation_mark(self.graph().relation_alignment(r));
            let evidence = super::Evidence::of(r);
            let mut known = |function: &ComponentId| {
                let m = flows.marks.entry((function.clone(), key.clone())).or_insert(mark);
                *m = worse(*m, mark);
                if let Some(e) = evidence.clone() {
                    let slot = (function.clone(), key.clone());
                    let combined = flows.evidence.remove(&slot).map_or(e.clone(), |had| had.weaker(e));
                    flows.evidence.insert(slot, combined);
                }
            };
            if step(&r.target) {
                known(&r.target);
                flows.consumers.entry(key.clone()).or_default().insert(r.target.clone());
                flows.takes.entry(r.target.clone()).or_default().insert(key.clone());
            }
            if step(&r.source) {
                known(&r.source);
                flows.producers.entry(key.clone()).or_default().insert(r.source.clone());
                flows.produces.entry(r.source.clone()).or_default().insert(key.clone());
            }
            if r.provenance.observed {
                observed_types.insert(key);
            } else {
                planned_types.insert(key);
            }
        }
        flows.planned_only = planned_types.difference(&observed_types).cloned().collect();
        flows
    }

    /// What holds `id`, innermost first (a containment loop stops).
    pub fn ancestors(&self, id: &ComponentId) -> Vec<ComponentId> {
        let mut out: Vec<ComponentId> = Vec::new();
        let mut current = id;
        while let Some(parent) = self.graph().parent(current) {
            if parent == id || out.contains(parent) {
                break;
            }
            out.push(parent.clone());
            current = parent;
        }
        out
    }

    fn function_of<'k>(&self, key: &'k str) -> Option<&'k str> {
        key.strip_prefix(START).or_else(|| key.strip_prefix(END))
    }

    /// Whether `key` names something the view can show.
    pub fn exists(&self, key: &str) -> bool {
        match self.view {
            ViewKind::DataFlow => {
                if key.starts_with(TYPE) {
                    return self.index.flows.consumers.contains_key(key)
                        || self.index.flows.producers.contains_key(key)
                        || self.focus.as_ref().is_some_and(|f| f.types.contains(key));
                }
                self.function_of(key).is_some_and(|f| self.graph().component(&ComponentId::new(f)).is_some())
            }
            _ => self.graph().component(&ComponentId::new(key)).is_some(),
        }
    }

    /// The view's starting points: with a plan in focus, the plan's own
    /// top-level items; otherwise the natural ones.
    pub fn starts(&self) -> Vec<String> {
        if let Some(focus) = &self.focus {
            return self.plan_starts(focus);
        }
        match self.view {
            ViewKind::Contains => {
                let mut tops: Vec<&Component> = self.graph().components().filter(|c| self.graph().parent(&c.id).is_none()).collect();
                tops.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
                tops.into_iter().map(|c| c.id.0.clone()).collect()
            }
            // The entry points, to pick one from.
            ViewKind::Calls => self.entries().map(|e| e.id.0.clone()).collect(),
            // The data the entry points take in: what their parameters are,
            // and what flows into them.
            ViewKind::DataFlow => {
                let mut types: Vec<String> = Vec::new();
                for entry in self.entries() {
                    let params = self
                        .graph()
                        .component(&entry.id)
                        .and_then(|c| c.signature.as_ref())
                        .into_iter()
                        .flat_map(|s| &s.params)
                        .filter_map(|p| p.ty.as_deref())
                        .filter_map(|t| self.type_key(t));
                    // The data types the code defines in them, wrapped or
                    // not (`Json<ImportArgs>` takes `ImportArgs`): not a
                    // primitive (`str`), not a framework's wrapper.
                    for written in params {
                        let Some(written) = written.strip_prefix(TYPE) else { continue };
                        // A generic's head is the wrapper: what's in it is taken.
                        let inside = written.split_once('<').map_or(written, |(_, rest)| rest);
                        for word in
                            inside.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| self.index.data_names.contains(*w))
                        {
                            let t = format!("{TYPE}{word}");
                            if !types.contains(&t) {
                                types.push(t);
                            }
                        }
                    }
                }
                // Those data flows through first: a start nothing makes or
                // uses shows nothing to follow.
                let flows = |t: &String| self.index.flows.consumers.contains_key(t) || self.index.flows.producers.contains_key(t);
                types.sort_by_key(|t| !flows(t));
                types
            }
        }
    }

    /// Whether some step (a function `through` accepts) makes `t` from other
    /// data (looking down), or turns `t` into other data (looking up).
    fn joined(&self, t: &str, through: impl Fn(&ComponentId) -> bool) -> bool {
        let (makers, inputs) = match self.way {
            Way::Down => (&self.index.flows.producers, &self.index.flows.takes),
            Way::Up => (&self.index.flows.consumers, &self.index.flows.produces),
        };
        makers.get(t).into_iter().flatten().filter(|f| through(f)).any(|f| inputs.get(f).is_some_and(|i| i.iter().any(|other| other != t)))
    }

    /// The entry points in this graph, in the order given.
    fn entries(&self) -> impl Iterator<Item = &super::EntryPoint> {
        self.input.entries.iter().filter(|e| self.graph().component(&e.id).is_some())
    }

    fn plan_starts(&self, focus: &Focus) -> Vec<String> {
        let mut starts: Vec<String> = match self.view {
            ViewKind::Contains => focus
                .held
                .iter()
                .filter(|id| self.graph().parent(id).is_none_or(|p| !focus.held.contains(p)))
                .map(|id| id.0.clone())
                .collect(),
            ViewKind::Calls => {
                let linked = |id: &ComponentId| {
                    let mut others: Box<dyn Iterator<Item = &ComponentId>> = match self.way {
                        Way::Down => Box::new(self.graph().incoming(id, RelationKind::Calls).map(|r| &r.source)),
                        Way::Up => Box::new(self.graph().outgoing(id, RelationKind::Calls).map(|r| &r.target)),
                    };
                    others.any(|o| o != id && focus.items.contains(o))
                };
                let callable = |id: &ComponentId| self.graph().component(id).is_some_and(|c| self.is_callable(c));
                let top: Vec<String> = focus.items.iter().filter(|id| callable(id) && !linked(id)).map(|id| id.0.clone()).collect();
                // A plan whose functions all call each other in a loop: all of them.
                if top.is_empty() { focus.items.iter().filter(|id| callable(id)).map(|id| id.0.clone()).collect() } else { top }
            }
            // The plan's data that no step of the plan makes from other data.
            ViewKind::DataFlow => focus.types.iter().filter(|t| !self.joined(t, |f| focus.items.contains(f))).cloned().collect(),
        };
        starts.sort();
        starts
    }

    /// The view's default depth (with a plan's boxes in focus, deep enough
    /// to reach every plan item).
    pub fn default_depth(&self) -> u32 {
        let configured = self.input.config.view(self.view).depth;
        match (&self.focus, self.view) {
            (Some(focus), ViewKind::Contains) => {
                let deepest =
                    focus.items.iter().map(|id| self.ancestors(id).iter().filter(|a| focus.held.contains(*a)).count()).max().unwrap_or(0)
                        as u32;
                configured.max(deepest + 1)
            }
            _ => configured,
        }
    }

    /// Whether a card's children are walked (with a plan in focus, the code
    /// it plugs into isn't, unless opened by hand).
    pub fn walks(&self, key: &str) -> bool {
        let Some(focus) = &self.focus else { return true };
        match self.view {
            ViewKind::Contains => focus.held.contains(&ComponentId::new(key)),
            ViewKind::Calls => focus.items.contains(&ComponentId::new(key)),
            ViewKind::DataFlow => focus.types.contains(key),
        }
    }

    /// With a plan in focus: a card that's only code the plan plugs into.
    pub fn outside_plan(&self, key: &str) -> bool {
        let Some(focus) = &self.focus else { return false };
        match self.view {
            ViewKind::DataFlow => {
                if key.starts_with(TYPE) {
                    !focus.types.contains(key)
                } else {
                    self.function_of(key).is_none_or(|f| !focus.items.contains(&ComponentId::new(f)))
                }
            }
            _ => !focus.items.contains(&ComponentId::new(key)),
        }
    }

    /// A card's children, in order, each with the line to it.
    pub fn children(&self, key: &str) -> Vec<Child> {
        let mut kids = match self.view {
            ViewKind::Contains => self.contained(key),
            ViewKind::Calls => self.calls(key),
            ViewKind::DataFlow => self.flow(key),
        };
        // With a plan in focus, a box shows what holds or is part of the plan.
        if let (Some(focus), ViewKind::Contains, Way::Down) = (&self.focus, self.view, self.way)
            && (!focus.whole || !focus.items.contains(&ComponentId::new(key)))
        {
            kids.retain(|k| focus.held.contains(&ComponentId::new(&k.key)));
        }
        kids
    }

    fn contained(&self, key: &str) -> Vec<Child> {
        let id = ComponentId::new(key);
        let child = |k: &ComponentId, mark| Child {
            key: k.0.clone(),
            relation: RelationKind::Contains,
            order: None,
            mark,
            pills: Vec::new(),
            evidence: None,
        };
        match self.way {
            Way::Down => {
                let mut kids: Vec<(&str, &ComponentId, Mark)> = self
                    .graph()
                    .outgoing(&id, RelationKind::Contains)
                    .map(|r| {
                        (
                            self.graph().component(&r.target).map_or("", |c| c.name.as_str()),
                            &r.target,
                            relation_mark(self.graph().relation_alignment(r)),
                        )
                    })
                    .collect();
                kids.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.cmp(b.1)));
                kids.into_iter().map(|(_, k, m)| child(k, m)).collect()
            }
            Way::Up => self.graph().parent(&id).map(|p| child(p, Mark::Code)).into_iter().collect(),
        }
    }

    /// What goes into a call and comes back: the callee's parameters and
    /// what it returns (from its signature, or the plan's expectation).
    fn call_pills(&self, callee: &ComponentId, relation: &crate::Relation) -> Vec<Pill> {
        let mut pills = Vec::new();
        let schema_names = |v: &Option<serde_json::Value>| -> Vec<String> {
            v.as_ref()
                .and_then(|s| s.get("properties"))
                .and_then(|p| p.as_object())
                .map(|p| p.keys().cloned().collect())
                .unwrap_or_default()
        };
        match self.graph().component(callee).and_then(|c| c.signature.as_ref()) {
            Some(sig) => {
                for p in &sig.params {
                    pills.push(Pill {
                        text: p.ty.as_ref().map_or_else(|| p.name.clone(), |t| format!("{}: {t}", p.name)),
                        role: PillRole::In,
                        component: None,
                        rect: None,
                    });
                }
                if let Some(r) = &sig.returns {
                    pills.push(Pill { text: r.clone(), role: PillRole::Out, component: None, rect: None });
                }
            }
            None => {
                if let Some(expects) = &relation.expects {
                    pills.extend(schema_names(&expects.inputs).into_iter().map(|text| Pill {
                        text,
                        role: PillRole::In,
                        component: None,
                        rect: None,
                    }));
                    pills.extend(schema_names(&expects.outputs).into_iter().map(|text| Pill {
                        text,
                        role: PillRole::Out,
                        component: None,
                        rect: None,
                    }));
                }
            }
        }
        pills
    }

    fn calls(&self, key: &str) -> Vec<Child> {
        let id = ComponentId::new(key);
        match self.way {
            Way::Down => {
                let mut calls: Vec<&crate::Relation> = self.graph().outgoing(&id, RelationKind::Calls).collect();
                calls.sort_by(|a, b| a.order.unwrap_or(u32::MAX).cmp(&b.order.unwrap_or(u32::MAX)).then_with(|| a.target.cmp(&b.target)));
                calls
                    .into_iter()
                    .map(|r| Child {
                        key: r.target.0.clone(),
                        relation: RelationKind::Calls,
                        order: r.order,
                        mark: relation_mark(self.graph().relation_alignment(r)),
                        pills: self.call_pills(&r.target, r),
                        evidence: super::Evidence::of(r),
                    })
                    .collect()
            }
            Way::Up => {
                let mut callers: Vec<(&str, &crate::Relation)> = self
                    .graph()
                    .incoming(&id, RelationKind::Calls)
                    .map(|r| (self.graph().component(&r.source).map_or("", |c| c.name.as_str()), r))
                    .collect();
                callers.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.source.cmp(&b.1.source)));
                callers
                    .into_iter()
                    .map(|(_, r)| Child {
                        key: r.source.0.clone(),
                        relation: RelationKind::Calls,
                        order: r.order,
                        mark: relation_mark(self.graph().relation_alignment(r)),
                        pills: self.call_pills(&id, r),
                        evidence: super::Evidence::of(r),
                    })
                    .collect()
            }
        }
    }

    fn step(&self, function: &ComponentId) -> Pill {
        let text = self.graph().component(function).map_or_else(|| function.0.clone(), |c| c.name.clone());
        Pill { text, role: PillRole::Step, component: Some(function.clone()), rect: None }
    }

    fn flow(&self, key: &str) -> Vec<Child> {
        if !key.starts_with(TYPE) {
            return Vec::new();
        }
        let (via, onward, end) = match self.way {
            Way::Down => (&self.index.flows.consumers, &self.index.flows.produces, END),
            Way::Up => (&self.index.flows.producers, &self.index.flows.takes, START),
        };
        // One line per pair of types, with every function between them on it.
        let mut kids: Vec<Child> = Vec::new();
        for function in via.get(key).into_iter().flatten() {
            let mark = self.index.flows.marks.get(&(function.clone(), key.to_owned())).copied().unwrap_or_default();
            let evidence = self.index.flows.evidence.get(&(function.clone(), key.to_owned())).cloned();
            let next: Vec<String> = onward.get(function).map(|n| n.iter().cloned().collect()).unwrap_or_default();
            if next.is_empty() {
                kids.push(Child {
                    key: format!("{end}{function}"),
                    relation: RelationKind::DataFlow,
                    order: None,
                    mark,
                    pills: Vec::new(),
                    evidence,
                });
                continue;
            }
            for t in next {
                let pill = self.step(function);
                // Going on from the function: how that flow is known too.
                let onward_evidence = self.index.flows.evidence.get(&(function.clone(), t.clone())).cloned();
                let both = match (evidence.clone(), onward_evidence) {
                    (Some(a), Some(b)) => Some(a.weaker(b)),
                    (a, b) => a.or(b),
                };
                match kids.iter_mut().find(|k| k.key == t) {
                    Some(k) => {
                        k.mark = worse(k.mark, mark);
                        k.pills.push(pill);
                        k.evidence = match (k.evidence.take(), both) {
                            (Some(a), Some(b)) => Some(a.weaker(b)),
                            (a, b) => a.or(b),
                        };
                    }
                    None => {
                        kids.push(Child { key: t, relation: RelationKind::DataFlow, order: None, mark, pills: vec![pill], evidence: both })
                    }
                }
            }
        }
        kids
    }

    /// How a card reads.
    pub fn face(&self, key: &str) -> Face {
        let others = |id: &ComponentId| self.input.other_plans.get(id).cloned().unwrap_or_default();
        let of_component = |id: ComponentId, kind: Option<&str>| -> Face {
            match self.graph().component(&id) {
                Some(c) => {
                    let detail = match (&c.anchor, &c.proposed) {
                        // A folder has no line.
                        (Some(a), _) if a.span.start_line == 0 => a.file.to_string(),
                        (Some(a), _) => format!("{}:{}", a.file, a.span.start_line),
                        (None, Some(p)) => format!("→ {}", p.file),
                        (None, None) => String::new(),
                    };
                    let mark = if self.input.plan.is_some() { component_mark(self.graph().alignment(&id), c) } else { Mark::Code };
                    Face {
                        label: c.name.clone(),
                        kind: kind.unwrap_or(&c.type_name).to_owned(),
                        detail,
                        mark,
                        other_plans: others(&id),
                        callable: self.is_callable(c),
                        component: Some(id),
                    }
                }
                None => Face {
                    label: id.0.clone(),
                    kind: kind.unwrap_or("unknown").to_owned(),
                    detail: String::new(),
                    mark: Mark::Code,
                    other_plans: Vec::new(),
                    callable: false,
                    component: None,
                },
            }
        };
        if let Some(t) = key.strip_prefix(TYPE) {
            let component = self.index.data_named.get(t).cloned();
            let users = self.index.flows.consumers.get(key).map_or(0, BTreeSet::len);
            let makers = self.index.flows.producers.get(key).map_or(0, BTreeSet::len);
            let mark = match &component {
                Some(id) if self.input.plan.is_some() => {
                    self.graph().component(id).map_or(Mark::Code, |c| component_mark(self.graph().alignment(id), c))
                }
                _ if self.input.plan.is_some() && self.index.flows.planned_only.contains(key) => Mark::New,
                _ => Mark::Code,
            };
            let other_plans = component.as_ref().map(others).unwrap_or_default();
            return Face {
                label: t.to_owned(),
                kind: "data".into(),
                detail: format!("{makers} make it · {users} use it"),
                component,
                mark,
                other_plans,
                callable: false,
            };
        }
        if let Some(f) = key.strip_prefix(START) {
            return of_component(ComponentId::new(f), Some("start"));
        }
        if let Some(f) = key.strip_prefix(END) {
            return of_component(ComponentId::new(f), Some("end"));
        }
        of_component(ComponentId::new(key), None)
    }

    /// Where `key` sits: what holds it, outermost first.
    pub fn path(&self, key: &str) -> Vec<Crumb> {
        let id = match key.strip_prefix(TYPE) {
            Some(t) => match self.index.data_named.get(t) {
                Some(id) => id.clone(),
                None => return Vec::new(),
            },
            None => ComponentId::new(self.function_of(key).unwrap_or(key)),
        };
        self.ancestors(&id)
            .into_iter()
            .rev()
            .map(|a| Crumb { label: self.graph().component(&a).map_or_else(|| a.0.clone(), |c| c.name.clone()), key: a.0 })
            .collect()
    }
}
