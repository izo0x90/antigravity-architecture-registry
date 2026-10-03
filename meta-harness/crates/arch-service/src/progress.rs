//! Moving a plan along: which components can be worked on next (in
//! dependency order), and how a change ripples back through what depends on
//! it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use arch_graph::{ComponentId, Graph, Plan, Stage, TypeSystem};
use serde::{Deserialize, Serialize};

use crate::checks::{is_data_type, stage_gate};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Detailed design: architecture approved, ready for the implementation plan.
    Plan,
    /// Code: plan approved (or skeleton laid), ready to implement.
    Build,
}

/// Plan components, dependencies first (ties and loops by id).
pub fn topological(plan: &Plan, graph: &Graph, types: &TypeSystem) -> Vec<ComponentId> {
    let ids: BTreeSet<&ComponentId> = plan.components.iter().map(|c| &c.id).collect();
    let deps: BTreeMap<&ComponentId, BTreeSet<ComponentId>> =
        ids.iter().map(|id| (*id, graph.dependencies(id, is_data_type(types)).into_iter().filter(|d| ids.contains(d)).collect())).collect();
    let mut order: Vec<ComponentId> = Vec::new();
    let mut done: BTreeSet<ComponentId> = BTreeSet::new();
    while done.len() < ids.len() {
        let ready: Vec<&ComponentId> = ids.iter().copied().filter(|id| !done.contains(*id) && deps[id].iter().all(|d| done.contains(d))).collect();
        // A loop: take its smallest member and carry on.
        let next = if ready.is_empty() { ids.iter().copied().filter(|id| !done.contains(*id)).take(1).collect() } else { ready };
        for id in next {
            done.insert(id.clone());
            order.push(id.clone());
        }
    }
    order
}

/// Components ready for `action`, in dependency order.
pub fn next_actionable(plan: &Plan, graph: &Graph, types: &TypeSystem, action: Action) -> Vec<ComponentId> {
    topological(plan, graph, types)
        .into_iter()
        .filter(|id| {
            let Some(c) = plan.component(id) else { return false };
            let (ready, goal) = match action {
                Action::Plan => (c.stage == Stage::ArchApproved, Stage::PlanApproved),
                Action::Build => (matches!(c.stage, Stage::PlanApproved | Stage::Skeleton), Stage::Implemented),
            };
            ready && stage_gate(plan, graph, types, id, goal).is_empty()
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Downgrade {
    pub id: ComponentId,
    pub from: Stage,
    pub to: Stage,
    /// 0 for the changed component, 1 for what depends on it directly, ...
    pub wave: u32,
}

/// Where a dependent falls back to when something it builds on changes.
fn fallback(stage: Stage) -> Option<Stage> {
    match stage {
        Stage::Implemented | Stage::Skeleton => Some(Stage::PlanApproved),
        Stage::PlanApproved => Some(Stage::ArchApproved),
        Stage::ArchApproved | Stage::Declared => None,
    }
}

/// Applies a change to `changed`: if the plan approved it, it goes back to
/// Declared for re-approval; everything that depends on it (transitively, in
/// waves) falls back a step. `graph` is the code merged with `plan`.
pub fn cascade(plan: &mut Plan, graph: &Graph, types: &TypeSystem, changed: &ComponentId) -> Vec<Downgrade> {
    let ids: Vec<ComponentId> = plan.components.iter().map(|c| c.id.clone()).collect();
    let mut dependents: BTreeMap<ComponentId, Vec<ComponentId>> = BTreeMap::new();
    for id in &ids {
        for dep in graph.dependencies(id, is_data_type(types)) {
            dependents.entry(dep).or_default().push(id.clone());
        }
    }
    let mut out = Vec::new();
    if let Some(c) = plan.component_mut(changed)
        && c.stage != Stage::Declared
    {
        out.push(Downgrade { id: changed.clone(), from: c.stage, to: Stage::Declared, wave: 0 });
        c.stage = Stage::Declared;
    }
    let mut seen: BTreeSet<ComponentId> = BTreeSet::from([changed.clone()]);
    let mut queue: VecDeque<(ComponentId, u32)> = VecDeque::from([(changed.clone(), 0)]);
    while let Some((current, wave)) = queue.pop_front() {
        for dependent in dependents.get(&current).into_iter().flatten() {
            if !seen.insert(dependent.clone()) {
                continue;
            }
            if let Some(c) = plan.component_mut(dependent)
                && let Some(to) = fallback(c.stage)
            {
                out.push(Downgrade { id: dependent.clone(), from: c.stage, to, wave: wave + 1 });
                c.stage = to;
            }
            queue.push_back((dependent.clone(), wave + 1));
        }
    }
    out
}
