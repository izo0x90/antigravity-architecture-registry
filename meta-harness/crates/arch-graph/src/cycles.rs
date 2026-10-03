//! Finding loops in directed graphs (containment, implements, schema refs).

use std::collections::{BTreeMap, BTreeSet};

/// Every simple loop, each once, as a path that starts and ends at its
/// smallest member. `edges` maps a node to the nodes it points at.
pub fn loops<N: Ord + Clone>(edges: &BTreeMap<N, Vec<N>>) -> Vec<Vec<N>> {
    let mut found: BTreeSet<Vec<N>> = BTreeSet::new();
    for start in edges.keys() {
        // Paths back to `start` through larger nodes only, so each loop is
        // found once, from its smallest member.
        let mut stack: Vec<Vec<N>> = vec![vec![start.clone()]];
        while let Some(path) = stack.pop() {
            let Some(node) = path.last() else { continue };
            for next in edges.get(node).into_iter().flatten() {
                if next == start {
                    let mut cycle = path.clone();
                    cycle.push(start.clone());
                    found.insert(cycle);
                } else if next > start && !path.contains(next) && edges.contains_key(next) {
                    let mut longer = path.clone();
                    longer.push(next.clone());
                    stack.push(longer);
                }
            }
        }
    }
    found.into_iter().collect()
}
