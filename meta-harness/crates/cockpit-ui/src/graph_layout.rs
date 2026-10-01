use egui::Pos2;
use harness_protocol::arch::UnifiedArchitectureGraph;
use std::collections::{BTreeMap, HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutDirection {
    #[default]
    LeftToRight,
    TopToBottom,
}

#[derive(Debug, Clone)]
pub struct LayoutSettings {
    pub card_width: f32,
    pub card_height: f32,
    pub col_gap: f32,
    pub row_gap: f32,
    pub origin_x: f32,
    pub origin_y: f32,
    pub direction: LayoutDirection,
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            card_width: 230.0,
            card_height: 105.0,
            col_gap: 75.0,
            row_gap: 30.0,
            origin_x: 60.0,
            origin_y: 60.0,
            direction: LayoutDirection::LeftToRight,
        }
    }
}

/// Computes directed hierarchical rank layout (Sugiyama-style column layering)
/// with longest-path DAG ranking and barycenter edge-crossing minimization.
pub fn compute_hierarchical_layout(
    graph: &UnifiedArchitectureGraph,
    settings: &LayoutSettings,
) -> BTreeMap<String, Pos2> {
    let mut positions = BTreeMap::new();
    if graph.nodes.is_empty() {
        return positions;
    }

    // 1. Build forward and reverse adjacency lists, and compute in-degrees
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    let mut rev_adj: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degrees: HashMap<String, usize> = HashMap::new();

    for id in graph.nodes.keys() {
        adj.insert(id.clone(), Vec::new());
        rev_adj.insert(id.clone(), Vec::new());
        in_degrees.insert(id.clone(), 0);
    }

    for edge in &graph.edges {
        if graph.nodes.contains_key(&edge.source_id) && graph.nodes.contains_key(&edge.target_id) {
            adj.entry(edge.source_id.clone()).or_default().push(edge.target_id.clone());
            rev_adj.entry(edge.target_id.clone()).or_default().push(edge.source_id.clone());
            *in_degrees.entry(edge.target_id.clone()).or_insert(0) += 1;
        }
    }

    // 2. Kahn's Algorithm with Longest-Path DAG Rank Propagation
    // Invariant: For every forward edge u -> v, rank(v) >= rank(u) + 1
    let mut ranks: HashMap<String, usize> = HashMap::new();
    let mut cur_in_degrees = in_degrees.clone();
    let mut queue = VecDeque::new();

    for (id, &deg) in &cur_in_degrees {
        if deg == 0 {
            ranks.insert(id.clone(), 0);
            queue.push_back(id.clone());
        }
    }

    let mut processed_nodes = 0;
    while processed_nodes < graph.nodes.len() {
        if queue.is_empty() {
            // Cycle fallback: Pick unranked node with minimal remaining in-degree
            let next_unranked = graph.nodes.keys()
                .filter(|id| !ranks.contains_key(*id))
                .min_by_key(|id| cur_in_degrees.get(*id).copied().unwrap_or(usize::MAX));

            if let Some(fallback_id) = next_unranked {
                let max_pred_rank = rev_adj.get(fallback_id)
                    .map(|preds| preds.iter().filter_map(|p| ranks.get(p)).max().copied().unwrap_or(0))
                    .unwrap_or(0);
                ranks.insert(fallback_id.clone(), max_pred_rank);
                queue.push_back(fallback_id.clone());
            } else {
                break;
            }
        }

        while let Some(curr) = queue.pop_front() {
            processed_nodes += 1;
            let curr_rank = ranks.get(&curr).copied().unwrap_or(0);

            if let Some(neighbors) = adj.get(&curr) {
                for neighbor in neighbors {
                    let next_rank = curr_rank + 1;
                    ranks.entry(neighbor.clone())
                        .and_modify(|r| *r = (*r).max(next_rank))
                        .or_insert(next_rank);

                    if let Some(deg) = cur_in_degrees.get_mut(neighbor) {
                        if *deg > 0 {
                            *deg -= 1;
                            if *deg == 0 {
                                queue.push_back(neighbor.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    // Ensure all disconnected nodes have a rank
    for id in graph.nodes.keys() {
        ranks.entry(id.clone()).or_insert(0);
    }

    // 3. Group nodes into discrete rank layers
    let mut layers: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut max_rank = 0;
    for (id, &rank) in &ranks {
        layers.entry(rank).or_default().push(id.clone());
        max_rank = max_rank.max(rank);
    }

    // Deterministic initial ordering
    for nodes_in_layer in layers.values_mut() {
        nodes_in_layer.sort();
    }

    // 4. Barycenter Crossing Minimization (2-pass smoothing)
    // Pass A: Forward pass (align each layer with average Y/X of incoming predecessors)
    for r in 1..=max_rank {
        if let Some(prev_layer) = layers.get(&(r - 1)).cloned() {
            let prev_positions: HashMap<String, usize> = prev_layer
                .iter()
                .enumerate()
                .map(|(idx, id)| (id.clone(), idx))
                .collect();

            if let Some(curr_layer) = layers.get_mut(&r) {
                curr_layer.sort_by(|a, b| {
                    let avg_a = compute_barycenter(a, &rev_adj, &prev_positions);
                    let avg_b = compute_barycenter(b, &rev_adj, &prev_positions);
                    avg_a.partial_cmp(&avg_b).unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }
    }

    // Pass B: Backward pass (align each layer with average Y/X of outgoing successors)
    for r in (0..max_rank).rev() {
        if let Some(next_layer) = layers.get(&(r + 1)).cloned() {
            let next_positions: HashMap<String, usize> = next_layer
                .iter()
                .enumerate()
                .map(|(idx, id)| (id.clone(), idx))
                .collect();

            if let Some(curr_layer) = layers.get_mut(&r) {
                curr_layer.sort_by(|a, b| {
                    let avg_a = compute_barycenter(a, &adj, &next_positions);
                    let avg_b = compute_barycenter(b, &adj, &next_positions);
                    avg_a.partial_cmp(&avg_b).unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }
    }

    // 5. Calculate 2D coordinates according to LayoutDirection
    match settings.direction {
        LayoutDirection::LeftToRight => {
            let max_col_nodes = layers.values().map(|v| v.len()).max().unwrap_or(1);
            let total_max_height = (max_col_nodes as f32) * (settings.card_height + settings.row_gap);

            for (&rank, nodes_in_col) in &layers {
                let col_x = settings.origin_x + (rank as f32) * (settings.card_width + settings.col_gap);
                let col_height = (nodes_in_col.len() as f32) * (settings.card_height + settings.row_gap);
                let col_start_y = settings.origin_y + (total_max_height - col_height) * 0.5;

                for (row_idx, node_id) in nodes_in_col.iter().enumerate() {
                    let node_y = col_start_y + (row_idx as f32) * (settings.card_height + settings.row_gap);
                    positions.insert(node_id.clone(), Pos2::new(col_x, node_y));
                }
            }
        }
        LayoutDirection::TopToBottom => {
            let max_row_nodes = layers.values().map(|v| v.len()).max().unwrap_or(1);
            let total_max_width = (max_row_nodes as f32) * (settings.card_width + settings.col_gap);

            for (&rank, nodes_in_row) in &layers {
                let row_y = settings.origin_y + (rank as f32) * (settings.card_height + settings.row_gap);
                let row_width = (nodes_in_row.len() as f32) * (settings.card_width + settings.col_gap);
                let row_start_x = settings.origin_x + (total_max_width - row_width) * 0.5;

                for (col_idx, node_id) in nodes_in_row.iter().enumerate() {
                    let node_x = row_start_x + (col_idx as f32) * (settings.card_width + settings.col_gap);
                    positions.insert(node_id.clone(), Pos2::new(node_x, row_y));
                }
            }
        }
    }

    positions
}

/// Computes the barycenter (mean index) of a node's connected neighbors in the adjacent layer.
fn compute_barycenter(
    node_id: &str,
    adj_map: &HashMap<String, Vec<String>>,
    neighbor_indices: &HashMap<String, usize>,
) -> f32 {
    if let Some(neighbors) = adj_map.get(node_id) {
        let indices: Vec<usize> = neighbors
            .iter()
            .filter_map(|n| neighbor_indices.get(n).copied())
            .collect();

        if !indices.is_empty() {
            let sum: usize = indices.iter().sum();
            return sum as f32 / indices.len() as f32;
        }
    }
    f32::MAX
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_protocol::arch::{GraphPerspective, SystemArchitecture};

    #[test]
    fn test_longest_path_layering_no_backward_edges() {
        let json_raw = include_str!("../../../../system_architecture.json");
        let arch: SystemArchitecture = serde_json::from_str(json_raw).unwrap();

        // Test Call Flow in LeftToRight
        let call_graph = arch.to_graph(GraphPerspective::CallFlow);
        let lr_settings = LayoutSettings {
            direction: LayoutDirection::LeftToRight,
            ..Default::default()
        };
        let lr_positions = compute_hierarchical_layout(&call_graph, &lr_settings);

        for edge in &call_graph.edges {
            let src_pos = lr_positions.get(&edge.source_id).expect("source pos exists");
            let tgt_pos = lr_positions.get(&edge.target_id).expect("target pos exists");
            assert!(
                tgt_pos.x > src_pos.x,
                "In LR, target X ({}) must be strictly greater than source X ({}) for edge {} -> {}",
                tgt_pos.x, src_pos.x, edge.source_id, edge.target_id
            );
        }

        // Test Call Flow in TopToBottom
        let tb_settings = LayoutSettings {
            direction: LayoutDirection::TopToBottom,
            ..Default::default()
        };
        let tb_positions = compute_hierarchical_layout(&call_graph, &tb_settings);

        for edge in &call_graph.edges {
            let src_pos = tb_positions.get(&edge.source_id).expect("source pos exists");
            let tgt_pos = tb_positions.get(&edge.target_id).expect("target pos exists");
            assert!(
                tgt_pos.y > src_pos.y,
                "In TB, target Y ({}) must be strictly greater than source Y ({}) for edge {} -> {}",
                tgt_pos.y, src_pos.y, edge.source_id, edge.target_id
            );
        }
    }

    #[test]
    fn test_component_hierarchy_layering() {
        let json_raw = include_str!("../../../../system_architecture.json");
        let arch: SystemArchitecture = serde_json::from_str(json_raw).unwrap();

        let comp_graph = arch.to_graph(GraphPerspective::ComponentHierarchy);
        let settings = LayoutSettings {
            direction: LayoutDirection::TopToBottom,
            ..Default::default()
        };
        let positions = compute_hierarchical_layout(&comp_graph, &settings);

        for edge in &comp_graph.edges {
            let src_pos = positions.get(&edge.source_id).expect("source pos exists");
            let tgt_pos = positions.get(&edge.target_id).expect("target pos exists");
            assert!(
                tgt_pos.y > src_pos.y,
                "In TB hierarchy, method Y ({}) must be below class Y ({}) for edge {} -> {}",
                tgt_pos.y, src_pos.y, edge.source_id, edge.target_id
            );
        }
    }
}
