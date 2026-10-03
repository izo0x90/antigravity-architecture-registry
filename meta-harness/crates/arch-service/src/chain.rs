//! Call chains: the planned calls a component takes part in, end to end.
//! Verifying one component re-checks every hop of its chain that should
//! already be in the code, so breaking any hop shows up as exactly that hop.

use std::collections::{BTreeSet, VecDeque};

use arch_graph::{ComponentId, Graph, Plan, Relation, RelationKind, Report};

use crate::verify::check_planned_call;

/// The planned call relations in the chain through `id`: every caller that
/// leads to it, and everything those callers (and it) go on to call.
pub fn chain<'a>(plan: &'a Plan, id: &ComponentId) -> Vec<&'a Relation> {
    let calls: Vec<&Relation> = plan.relations.iter().filter(|r| r.kind == RelationKind::Calls).collect();
    let walk = |starts: Vec<ComponentId>, forward: bool| {
        let mut seen: BTreeSet<ComponentId> = starts.iter().cloned().collect();
        let mut queue: VecDeque<ComponentId> = starts.into();
        while let Some(at) = queue.pop_front() {
            for r in &calls {
                let (from, to) = if forward { (&r.source, &r.target) } else { (&r.target, &r.source) };
                if from == &at && seen.insert(to.clone()) {
                    queue.push_back(to.clone());
                }
            }
        }
        seen
    };
    let upstream = walk(vec![id.clone()], false);
    let members = walk(upstream.into_iter().collect(), true);
    let mut hops: Vec<&Relation> = calls.into_iter().filter(|r| members.contains(&r.source)).collect();
    hops.sort_by(|a, b| a.source.cmp(&b.source).then(a.order.cmp(&b.order)).then_with(|| a.target.cmp(&b.target)));
    hops
}

/// Files whose code the chain's hops live in (the callers that should already
/// be in the code), so they can be re-read before checking.
pub fn caller_files(plan: &Plan, id: &ComponentId, graph: &Graph) -> BTreeSet<String> {
    chain(plan, id)
        .into_iter()
        .filter(|r| expects_code(plan, &r.source))
        .filter_map(|r| graph.component(&r.source).and_then(|c| c.file()).map(|f| f.as_str().to_owned()))
        .collect()
}

fn expects_code(plan: &Plan, id: &ComponentId) -> bool {
    plan.component(id).is_some_and(|c| c.stage.expects_code())
}

/// Re-checks every hop of the chain through `id` whose caller is a planned
/// component already in the code (Skeleton or Implemented). Hops leaving `id`
/// itself are left to its own verification.
pub fn recheck(plan: &Plan, id: &ComponentId, observed: &Graph) -> Report {
    let mut report = Report::default();
    for hop in chain(plan, id).into_iter().filter(|r| &r.source != id && expects_code(plan, &r.source)) {
        if let Some(finding) = check_planned_call(hop, observed) {
            let finding = match observed.component(&hop.source).and_then(|c| c.anchor.as_ref()) {
                Some(a) => finding.at(a.file.clone(), a.span),
                None => finding,
            };
            report.push(finding.suggest(format!("this hop of the call chain through {id} broke; restore the call in {}", hop.source)));
        }
    }
    report
}
