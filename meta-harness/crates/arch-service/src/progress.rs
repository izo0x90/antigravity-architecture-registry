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

/// Plan components, dependencies first. Components that depend on each
/// other (a loop) come together, after everything the loop depends on and
/// before what depends on it; ties and loop members go by id.
pub fn topological(plan: &Plan, graph: &Graph, types: &TypeSystem) -> Vec<ComponentId> {
    let ids: Vec<&ComponentId> = plan.components.iter().map(|c| &c.id).collect::<BTreeSet<_>>().into_iter().collect();
    let index: BTreeMap<&ComponentId, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let deps: Vec<Vec<usize>> =
        ids.iter().map(|id| graph.dependencies(id, is_data_type(types)).iter().filter_map(|d| index.get(d).copied()).collect()).collect();
    let groups = strongly_connected(&deps);
    let group_of: Vec<usize> = {
        let mut g = vec![0; ids.len()];
        for (gi, members) in groups.iter().enumerate() {
            for &m in members {
                g[m] = gi;
            }
        }
        g
    };
    // Kahn's algorithm over the groups (a loop is one group).
    let group_deps: Vec<BTreeSet<usize>> = groups
        .iter()
        .enumerate()
        .map(|(gi, members)| members.iter().flat_map(|&m| deps[m].iter().map(|&d| group_of[d])).filter(|&g| g != gi).collect())
        .collect();
    let mut done = vec![false; groups.len()];
    let mut order = Vec::with_capacity(ids.len());
    loop {
        let mut ready: Vec<usize> = (0..groups.len()).filter(|&g| !done[g] && group_deps[g].iter().all(|&d| done[d])).collect();
        if ready.is_empty() {
            break;
        }
        ready.sort_by_key(|&g| groups[g].iter().copied().min());
        for g in ready {
            done[g] = true;
            let mut members = groups[g].clone();
            members.sort();
            order.extend(members.into_iter().map(|m| ids[m].clone()));
        }
    }
    order
}

/// Strongly connected components of a graph given as adjacency lists
/// (Tarjan's algorithm, iterative).
fn strongly_connected(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0;
    let mut groups = Vec::new();
    for start in 0..n {
        if index[start] != usize::MAX {
            continue;
        }
        // (node, next edge to look at)
        let mut work: Vec<(usize, usize)> = vec![(start, 0)];
        index[start] = next;
        low[start] = next;
        next += 1;
        stack.push(start);
        on_stack[start] = true;
        while let Some(&mut (v, ref mut i)) = work.last_mut() {
            if *i < edges[v].len() {
                let w = edges[v][*i];
                *i += 1;
                if index[w] == usize::MAX {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    let mut group = Vec::new();
                    while let Some(w) = stack.pop() {
                        on_stack[w] = false;
                        group.push(w);
                        if w == v {
                            break;
                        }
                    }
                    groups.push(group);
                }
            }
        }
    }
    groups
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
