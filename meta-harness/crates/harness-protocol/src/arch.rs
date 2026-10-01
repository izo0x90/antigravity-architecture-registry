use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// -----------------------------------------------------------------------------
// 1. Architecture Registry Ground Truth (system_architecture.json)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SystemArchitecture {
    #[serde(default)]
    pub components: BTreeMap<String, ComponentSpec>,
    #[serde(default)]
    pub usage_trees: BTreeMap<String, UsageNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComponentSpec {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub comp_type: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default = "default_stage")]
    pub stage: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub implements_id: Option<String>,
    #[serde(default)]
    pub location: Option<CodeLocation>,
    #[serde(default)]
    pub inputs: Option<serde_json::Value>,
    #[serde(default)]
    pub outputs: Option<serde_json::Value>,
    #[serde(default)]
    pub properties: Option<serde_json::Value>,
    #[serde(default)]
    pub side_effects: Vec<SideEffectSpec>,
    #[serde(default)]
    pub implementation_spec: Option<serde_json::Value>,
    #[serde(default)]
    pub modification_tasks: Vec<TaskSpec>,
}

fn default_status() -> String {
    "new".to_string()
}

fn default_stage() -> String {
    "draft".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskSpec {
    #[serde(default)]
    pub id: Option<String>,
    pub task: String,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub subtasks: Vec<TaskSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SideEffectSpec {
    pub target: String,
    pub description: String,
}

// -----------------------------------------------------------------------------
// 2. Usage Trees (Call Graphs & Runtime Dependencies)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageNode {
    pub node_id: String,
    pub caller_id: String,
    pub component_id: String,
    pub description: String,
    #[serde(default)]
    pub expected_inputs: Option<serde_json::Value>,
    #[serde(default)]
    pub expected_outputs: Option<serde_json::Value>,
    #[serde(default)]
    pub expected_side_effects: Vec<SideEffectSpec>,
    #[serde(default)]
    pub dependencies: Vec<UsageNode>,
}

// -----------------------------------------------------------------------------
// 3. Code Binding & Alignment (AST-Grep Grounding)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeLocation {
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    #[serde(default)]
    pub symbol_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AlignmentStatus {
    PlannedOnly,
    VerifiedInCode {
        location: CodeLocation,
    },
    MissingCallSite {
        expected_in: String,
    },
    SignatureMismatch {
        location: CodeLocation,
        diff: String,
    },
}

impl Default for AlignmentStatus {
    fn default() -> Self {
        Self::PlannedOnly
    }
}

// -----------------------------------------------------------------------------
// 4. Unified Graph IR (Consumed by Layout Engines & Renderers)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub kind: NodeKind,
    #[serde(default)]
    pub parent_group: Option<String>,
    #[serde(default)]
    pub alignment: AlignmentStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum NodeKind {
    Component(ComponentSpec),
    UsageCall(UsageNode),
    PlanStep(TaskSpec),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEdge {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EdgeKind {
    CallSite {
        #[serde(default)]
        call_site: Option<CodeLocation>,
    },
    DataFlow {
        from_port: String,
        to_port: String,
    },
    Decomposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphPerspective {
    #[default]
    CallFlow,
    ComponentHierarchy,
    DataFlow,
    Unified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UnifiedArchitectureGraph {
    pub nodes: BTreeMap<String, GraphNode>,
    pub edges: Vec<GraphEdge>,
}

impl SystemArchitecture {
    /// Builds the Unified Architecture Graph by synthesizing components,
    /// usage call trees, and parent-child decomposition hierarchies.
    pub fn to_unified_graph(&self) -> UnifiedArchitectureGraph {
        let mut graph = UnifiedArchitectureGraph::default();

        // 1. Add all components as primary nodes
        for (id, comp) in &self.components {
            let alignment = if let Some(loc) = &comp.location {
                AlignmentStatus::VerifiedInCode {
                    location: loc.clone(),
                }
            } else {
                AlignmentStatus::PlannedOnly
            };

            graph.nodes.insert(
                id.clone(),
                GraphNode {
                    id: id.clone(),
                    label: comp.name.clone(),
                    kind: NodeKind::Component(comp.clone()),
                    parent_group: comp.parent_id.clone(),
                    alignment,
                },
            );

            // Add decomposition edge if parent_id exists
            if let Some(parent) = &comp.parent_id {
                graph.edges.push(GraphEdge {
                    id: format!("{}_child_of_{}", id, parent),
                    source_id: parent.clone(),
                    target_id: id.clone(),
                    kind: EdgeKind::Decomposition,
                });
            }
        }

        // 2. Add usage trees (caller -> callee call sites)
        for (tree_id, root_usage) in &self.usage_trees {
            Self::flatten_usage_node(root_usage, tree_id, &mut graph);
        }

        graph
    }

    fn flatten_usage_node(
        usage: &UsageNode,
        tree_prefix: &str,
        graph: &mut UnifiedArchitectureGraph,
    ) {
        let node_id = format!("{}:{}", tree_prefix, usage.node_id);

        graph.nodes.insert(
            node_id.clone(),
            GraphNode {
                id: node_id.clone(),
                label: usage.description.clone(),
                kind: NodeKind::UsageCall(usage.clone()),
                parent_group: Some(usage.caller_id.clone()),
                alignment: AlignmentStatus::PlannedOnly,
            },
        );

        // Caller -> Usage Node
        graph.edges.push(GraphEdge {
            id: format!("{}_calls_{}", usage.caller_id, node_id),
            source_id: usage.caller_id.clone(),
            target_id: node_id.clone(),
            kind: EdgeKind::CallSite { call_site: None },
        });

        // Usage Node -> Callee Component
        graph.edges.push(GraphEdge {
            id: format!("{}_invokes_{}", node_id, usage.component_id),
            source_id: node_id.clone(),
            target_id: usage.component_id.clone(),
            kind: EdgeKind::CallSite { call_site: None },
        });

        // Recurse child dependencies
        for (idx, dep) in usage.dependencies.iter().enumerate() {
            let child_prefix = format!("{}:{}", tree_prefix, idx);
            Self::flatten_usage_node(dep, &child_prefix, graph);
        }
    }

    /// Builds a graph tailored to a specific perspective (Call Flow, Component Hierarchy, Data Flow, or Unified).
    pub fn to_graph(&self, perspective: GraphPerspective) -> UnifiedArchitectureGraph {
        match perspective {
            GraphPerspective::Unified => self.to_unified_graph(),
            GraphPerspective::ComponentHierarchy => self.to_component_hierarchy_graph(),
            GraphPerspective::CallFlow => self.to_call_flow_graph(),
            GraphPerspective::DataFlow => self.to_data_flow_graph(),
        }
    }

    /// Pure structural component hierarchy: Modules -> Classes -> Functions / Data Objects
    pub fn to_component_hierarchy_graph(&self) -> UnifiedArchitectureGraph {
        let mut graph = UnifiedArchitectureGraph::default();

        for (id, comp) in &self.components {
            let alignment = if let Some(loc) = &comp.location {
                AlignmentStatus::VerifiedInCode {
                    location: loc.clone(),
                }
            } else {
                AlignmentStatus::PlannedOnly
            };

            graph.nodes.insert(
                id.clone(),
                GraphNode {
                    id: id.clone(),
                    label: comp.name.clone(),
                    kind: NodeKind::Component(comp.clone()),
                    parent_group: comp.parent_id.clone(),
                    alignment,
                },
            );

            if let Some(parent) = &comp.parent_id {
                graph.edges.push(GraphEdge {
                    id: format!("{}_decomposes_{}", parent, id),
                    source_id: parent.clone(),
                    target_id: id.clone(),
                    kind: EdgeKind::Decomposition,
                });
            }
        }

        graph
    }

    /// Pure runtime execution call flow: Callers -> Callees with call site metadata
    pub fn to_call_flow_graph(&self) -> UnifiedArchitectureGraph {
        let mut graph = UnifiedArchitectureGraph::default();

        for root_usage in self.usage_trees.values() {
            self.collect_call_flow(root_usage, &mut graph);
        }

        graph
    }

    fn collect_call_flow(&self, usage: &UsageNode, graph: &mut UnifiedArchitectureGraph) {
        // Ensure caller component is in graph
        if !graph.nodes.contains_key(&usage.caller_id)
            && let Some(comp) = self.components.get(&usage.caller_id)
        {
            let label = if let Some(parent) = &comp.parent_id {
                if let Some(parent_comp) = self.components.get(parent) {
                    format!("{}::{}", parent_comp.name, comp.name)
                } else {
                    comp.name.clone()
                }
            } else {
                comp.name.clone()
            };

            let alignment = if let Some(loc) = &comp.location {
                AlignmentStatus::VerifiedInCode {
                    location: loc.clone(),
                }
            } else {
                AlignmentStatus::PlannedOnly
            };

            graph.nodes.insert(
                usage.caller_id.clone(),
                GraphNode {
                    id: usage.caller_id.clone(),
                    label,
                    kind: NodeKind::Component(comp.clone()),
                    parent_group: comp.parent_id.clone(),
                    alignment,
                },
            );
        }

        // Ensure callee component is in graph
        if !graph.nodes.contains_key(&usage.component_id)
            && let Some(comp) = self.components.get(&usage.component_id)
        {
            let label = if let Some(parent) = &comp.parent_id {
                if let Some(parent_comp) = self.components.get(parent) {
                    format!("{}::{}", parent_comp.name, comp.name)
                } else {
                    comp.name.clone()
                }
            } else {
                comp.name.clone()
            };

            let alignment = if let Some(loc) = &comp.location {
                AlignmentStatus::VerifiedInCode {
                    location: loc.clone(),
                }
            } else {
                AlignmentStatus::PlannedOnly
            };

            graph.nodes.insert(
                usage.component_id.clone(),
                GraphNode {
                    id: usage.component_id.clone(),
                    label,
                    kind: NodeKind::Component(comp.clone()),
                    parent_group: comp.parent_id.clone(),
                    alignment,
                },
            );
        }

        // Add direct execution call edge from caller to callee
        let edge_id = format!("{}_calls_{}", usage.caller_id, usage.component_id);
        if !graph.edges.iter().any(|e| e.id == edge_id) {
            graph.edges.push(GraphEdge {
                id: edge_id,
                source_id: usage.caller_id.clone(),
                target_id: usage.component_id.clone(),
                kind: EdgeKind::CallSite { call_site: None },
            });
        }

        // Recurse dependencies
        for dep in &usage.dependencies {
            self.collect_call_flow(dep, graph);
        }
    }

    /// Data flow view: Data objects and the components producing / consuming them
    pub fn to_data_flow_graph(&self) -> UnifiedArchitectureGraph {
        let mut graph = UnifiedArchitectureGraph::default();

        // 1. Add data objects and enums
        for (id, comp) in &self.components {
            if comp.comp_type == "data_object" || comp.comp_type == "enum" {
                graph.nodes.insert(
                    id.clone(),
                    GraphNode {
                        id: id.clone(),
                        label: comp.name.clone(),
                        kind: NodeKind::Component(comp.clone()),
                        parent_group: comp.parent_id.clone(),
                        alignment: comp.location.as_ref().map_or(
                            AlignmentStatus::PlannedOnly,
                            |l| AlignmentStatus::VerifiedInCode { location: l.clone() },
                        ),
                    },
                );
            }
        }

        // 2. Add producers and consumers connected by dataflow edges
        for root_usage in self.usage_trees.values() {
            self.collect_data_flow(root_usage, &mut graph);
        }

        graph
    }

    fn collect_data_flow(&self, usage: &UsageNode, graph: &mut UnifiedArchitectureGraph) {
        // Check expected inputs (consumed)
        if let Some(inputs) = &usage.expected_inputs
            && let Some(props) = inputs.get("properties").and_then(|p| p.as_object())
        {
            for (_prop_name, prop_val) in props {
                if let Some(title) = prop_val.get("title").and_then(|t| t.as_str())
                    && graph.nodes.contains_key(title)
                {
                    // Ensure callee component exists as consumer
                    self.ensure_component_node(usage.component_id.as_str(), graph);
                    let edge_id = format!("{}_feeds_{}", title, usage.component_id);
                    if !graph.edges.iter().any(|e| e.id == edge_id) {
                        graph.edges.push(GraphEdge {
                            id: edge_id,
                            source_id: title.to_string(),
                            target_id: usage.component_id.clone(),
                            kind: EdgeKind::DataFlow {
                                from_port: "out".to_string(),
                                to_port: "in".to_string(),
                            },
                        });
                    }
                }
            }
        }

        // Check expected outputs (produced)
        if let Some(outputs) = &usage.expected_outputs
            && let Some(props) = outputs.get("properties").and_then(|p| p.as_object())
        {
            for (_prop_name, prop_val) in props {
                if let Some(title) = prop_val.get("title").and_then(|t| t.as_str())
                    && graph.nodes.contains_key(title)
                {
                    // Ensure callee component exists as producer
                    self.ensure_component_node(usage.component_id.as_str(), graph);
                    let edge_id = format!("{}_produces_{}", usage.component_id, title);
                    if !graph.edges.iter().any(|e| e.id == edge_id) {
                        graph.edges.push(GraphEdge {
                            id: edge_id,
                            source_id: usage.component_id.clone(),
                            target_id: title.to_string(),
                            kind: EdgeKind::DataFlow {
                                from_port: "out".to_string(),
                                to_port: "in".to_string(),
                            },
                        });
                    }
                }
            }
        }

        for dep in &usage.dependencies {
            self.collect_data_flow(dep, graph);
        }
    }

    fn ensure_component_node(&self, id: &str, graph: &mut UnifiedArchitectureGraph) {
        if !graph.nodes.contains_key(id)
            && let Some(comp) = self.components.get(id)
        {
            graph.nodes.insert(
                id.to_string(),
                GraphNode {
                    id: id.to_string(),
                    label: comp.name.clone(),
                    kind: NodeKind::Component(comp.clone()),
                    parent_group: comp.parent_id.clone(),
                    alignment: comp.location.as_ref().map_or(
                        AlignmentStatus::PlannedOnly,
                        |l| AlignmentStatus::VerifiedInCode { location: l.clone() },
                    ),
                },
            );
        }
    }

    /// Generates high-level summary statistics of the entire architecture.
    pub fn summarize(&self) -> ArchitectureSummary {
        let mut stages_count = BTreeMap::new();
        let mut types_count = BTreeMap::new();
        let mut root_modules = Vec::new();

        for comp in self.components.values() {
            *stages_count.entry(comp.stage.clone()).or_insert(0) += 1;
            *types_count.entry(comp.comp_type.clone()).or_insert(0) += 1;
            if comp.comp_type == "module" && comp.parent_id.is_none() {
                root_modules.push(comp.id.clone());
            }
        }

        ArchitectureSummary {
            total_components: self.components.len(),
            stages_count,
            types_count,
            root_modules,
            workflow_trees: self.usage_trees.keys().cloned().collect(),
        }
    }

    /// Generates a human- and LLM-readable textual tree plan for a given perspective and root node.
    pub fn to_textual_tree(&self, root_id: &str, perspective: GraphPerspective, max_depth: usize) -> String {
        let graph = self.to_graph(perspective);
        let sub = graph.extract_subtree(root_id, max_depth);

        if sub.nodes.is_empty() {
            return format!("(Subtree for '{}' not found in {:?})", root_id, perspective);
        }

        let mut out = String::new();
        out.push_str(&format!(
            "Subtree: '{}' [perspective={:?}, depth={}]\n",
            root_id, perspective, max_depth
        ));

        let mut visited = std::collections::HashSet::new();
        Self::render_textual_subtree_level(&sub, root_id, 0, &mut visited, &mut out);
        out
    }

    fn render_textual_subtree_level(
        sub: &UnifiedArchitectureGraph,
        current_id: &str,
        depth: usize,
        visited: &mut std::collections::HashSet<String>,
        out: &mut String,
    ) {
        if !visited.insert(current_id.to_string()) {
            return;
        }

        let indent = "  ".repeat(depth);
        let prefix = if depth == 0 { "● " } else { "├── " };

        if let Some(node) = sub.nodes.get(current_id) {
            match &node.kind {
                NodeKind::Component(comp) => {
                    out.push_str(&format!(
                        "{}{}{} [{}] ({})\n",
                        indent, prefix, comp.id, comp.comp_type, comp.stage
                    ));
                    if !comp.description.is_empty() {
                        out.push_str(&format!("{}    desc: {}\n", indent, comp.description));
                    }
                }
                NodeKind::UsageCall(usage) => {
                    out.push_str(&format!(
                        "{}{}Call: {} -> {} ({})\n",
                        indent, prefix, usage.caller_id, usage.component_id, usage.description
                    ));
                }
                NodeKind::PlanStep(task) => {
                    out.push_str(&format!(
                        "{}{}Task: {} [completed={}]\n",
                        indent, prefix, task.task, task.completed
                    ));
                }
            }
        }

        // Find direct children in subgraph edges
        for edge in &sub.edges {
            if edge.source_id == current_id && !visited.contains(&edge.target_id) {
                Self::render_textual_subtree_level(sub, &edge.target_id, depth + 1, visited, out);
            }
        }
    }
}

impl UnifiedArchitectureGraph {
    /// Extracts a directed neighborhood subgraph rooted at `root_id` up to `max_depth` hops.
    pub fn extract_subtree(&self, root_id: &str, max_depth: usize) -> UnifiedArchitectureGraph {
        use std::collections::{HashSet, VecDeque};

        if !self.nodes.contains_key(root_id) {
            return UnifiedArchitectureGraph::default();
        }

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        visited.insert(root_id.to_string());
        queue.push_back((root_id.to_string(), 0usize));

        while let Some((curr, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            for edge in &self.edges {
                if edge.source_id == curr && !visited.contains(&edge.target_id) {
                    visited.insert(edge.target_id.clone());
                    queue.push_back((edge.target_id.clone(), depth + 1));
                }
                if edge.target_id == curr && matches!(edge.kind, EdgeKind::Decomposition) && !visited.contains(&edge.source_id) {
                    visited.insert(edge.source_id.clone());
                    queue.push_back((edge.source_id.clone(), depth + 1));
                }
            }
        }

        let mut sub_nodes = BTreeMap::new();
        for id in &visited {
            if let Some(node) = self.nodes.get(id) {
                sub_nodes.insert(id.clone(), node.clone());
            }
        }

        let sub_edges = self
            .edges
            .iter()
            .filter(|e| visited.contains(&e.source_id) && visited.contains(&e.target_id))
            .cloned()
            .collect();

        UnifiedArchitectureGraph {
            nodes: sub_nodes,
            edges: sub_edges,
        }
    }
}

// -----------------------------------------------------------------------------
// 5. Query, Slicing & Summary Types
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentFilter {
    pub query: Option<String>,
    pub stage: Option<String>,
    pub comp_type: Option<String>,
    pub status: Option<String>,
    pub parent_id: Option<String>,
    pub tag: Option<String>,
}

impl ComponentFilter {
    pub fn matches(&self, comp: &ComponentSpec) -> bool {
        if let Some(ref stage) = self.stage
            && &comp.stage != stage
        {
            return false;
        }
        if let Some(ref comp_type) = self.comp_type
            && &comp.comp_type != comp_type
        {
            return false;
        }
        if let Some(ref status) = self.status
            && &comp.status != status
        {
            return false;
        }
        if let Some(ref parent_id) = self.parent_id
            && comp.parent_id.as_deref() != Some(parent_id.as_str())
        {
            return false;
        }
        if let Some(ref tag) = self.tag
            && !comp.side_effects.iter().any(|se| se.target.eq_ignore_ascii_case(tag))
        {
            return false;
        }
        if let Some(ref q) = self.query {
            let q_lower = q.to_lowercase();
            let matches_id = comp.id.to_lowercase().contains(&q_lower);
            let matches_name = comp.name.to_lowercase().contains(&q_lower);
            let matches_desc = comp.description.to_lowercase().contains(&q_lower);
            if !matches_id && !matches_name && !matches_desc {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentSummary {
    pub id: String,
    pub name: String,
    pub comp_type: String,
    pub stage: String,
    pub status: String,
    pub description: String,
    pub parent_id: Option<String>,
}

impl ComponentSummary {
    pub fn from_spec(comp: &ComponentSpec) -> Self {
        Self {
            id: comp.id.clone(),
            name: comp.name.clone(),
            comp_type: comp.comp_type.clone(),
            stage: comp.stage.clone(),
            status: comp.status.clone(),
            description: comp.description.clone(),
            parent_id: comp.parent_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ArchitectureSummary {
    pub total_components: usize,
    pub stages_count: BTreeMap<String, usize>,
    pub types_count: BTreeMap<String, usize>,
    pub root_modules: Vec<String>,
    pub workflow_trees: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtreeQuery {
    pub root_id: String,
    pub perspective: GraphPerspective,
    #[serde(default = "default_subtree_depth")]
    pub depth: usize,
}

fn default_subtree_depth() -> usize {
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_system_architecture() {
        let json_raw = include_str!("../../../../system_architecture.json");
        let arch: SystemArchitecture = serde_json::from_str(json_raw)
            .expect("system_architecture.json must parse into SystemArchitecture");

        assert!(!arch.components.is_empty(), "Must contain components");
        assert!(!arch.usage_trees.is_empty(), "Must contain usage_trees");

        // Verify known component
        let op_type = arch.components.get("operation_type").expect("operation_type component exists");
        assert_eq!(op_type.name, "Operation Type Enum");
        assert_eq!(op_type.comp_type, "enum");
        assert_eq!(op_type.stage, "implemented");
        assert!(!op_type.modification_tasks.is_empty());

        // Verify known usage tree
        let calc_flow = arch.usage_trees.get("calculation_flow").expect("calculation_flow usage tree exists");
        assert_eq!(calc_flow.caller_id, "calculator_client_run");
        assert_eq!(calc_flow.component_id, "calculator_service_calculate");
        assert!(!calc_flow.dependencies.is_empty());

        // Synthesize Unified Architecture Graph
        let graph = arch.to_unified_graph();
        assert!(!graph.nodes.is_empty(), "Graph must contain nodes");
        assert!(!graph.edges.is_empty(), "Graph must contain edges");

        // Check that component node exists in graph
        let comp_node = graph.nodes.get("operation_type").expect("operation_type in graph");
        assert_eq!(comp_node.label, "Operation Type Enum");

        // Check that usage node exists in graph
        let usage_node = graph.nodes.get("calculation_flow:user_calls_calculate").expect("usage node in graph");
        assert!(matches!(usage_node.kind, NodeKind::UsageCall(_)));

        // Check that call edges were generated
        let has_call_edge = graph.edges.iter().any(|e| matches!(e.kind, EdgeKind::CallSite { .. }));
        assert!(has_call_edge, "Graph must have call edges generated from usage tree");

        // Test Call Flow perspective
        let call_graph = arch.to_graph(GraphPerspective::CallFlow);
        assert_eq!(call_graph.nodes.len(), 4, "Call graph should contain exactly the 4 execution components");
        assert!(call_graph.nodes.contains_key("calculator_client_run"));
        assert!(call_graph.nodes.contains_key("calculator_service_calculate"));
        assert!(call_graph.nodes.contains_key("math_validator_validate"));
        assert!(call_graph.nodes.contains_key("audit_logger_log"));
        assert_eq!(call_graph.edges.len(), 3, "Call graph should have 3 direct call edges");

        // Test Component Hierarchy perspective
        let comp_graph = arch.to_graph(GraphPerspective::ComponentHierarchy);
        assert_eq!(comp_graph.nodes.len(), arch.components.len());
        let decomp_count = comp_graph.edges.iter().filter(|e| matches!(e.kind, EdgeKind::Decomposition)).count();
        assert_eq!(decomp_count, 4, "Should have 4 class->method decomposition edges");
    }

    #[test]
    fn test_summarize_architecture() {
        let json_raw = include_str!("../../../../system_architecture.json");
        let arch: SystemArchitecture = serde_json::from_str(json_raw).unwrap();
        let summary = arch.summarize();

        assert_eq!(summary.total_components, arch.components.len());
        assert!(!summary.stages_count.is_empty());
        assert!(summary.stages_count.contains_key("implemented"));
        assert!(summary.workflow_trees.contains(&"calculation_flow".to_string()));
    }

    #[test]
    fn test_component_filter() {
        let json_raw = include_str!("../../../../system_architecture.json");
        let arch: SystemArchitecture = serde_json::from_str(json_raw).unwrap();

        let filter_stage = ComponentFilter {
            stage: Some("implemented".to_string()),
            ..Default::default()
        };
        let matches_stage: Vec<_> = arch.components.values().filter(|c| filter_stage.matches(c)).collect();
        assert!(!matches_stage.is_empty());
        assert!(matches_stage.iter().all(|c| c.stage == "implemented"));

        let filter_query = ComponentFilter {
            query: Some("calculator".to_string()),
            ..Default::default()
        };
        let matches_query: Vec<_> = arch.components.values().filter(|c| filter_query.matches(c)).collect();
        assert!(!matches_query.is_empty());
        assert!(matches_query.iter().all(|c| c.id.contains("calculator") || c.name.to_lowercase().contains("calculator")));
    }

    #[test]
    fn test_subtree_extraction_and_textual_tree() {
        let json_raw = include_str!("../../../../system_architecture.json");
        let arch: SystemArchitecture = serde_json::from_str(json_raw).unwrap();

        // Subtree from call graph
        let call_graph = arch.to_graph(GraphPerspective::CallFlow);
        let sub = call_graph.extract_subtree("calculator_client_run", 1);
        assert!(sub.nodes.contains_key("calculator_client_run"));
        assert!(sub.nodes.contains_key("calculator_service_calculate"));
        // At depth 1, validator and logger should NOT be in subgraph
        assert_eq!(sub.nodes.len(), 2);

        // Subtree at depth 2 includes second-hop callees
        let sub2 = call_graph.extract_subtree("calculator_client_run", 2);
        assert_eq!(sub2.nodes.len(), 4);

        // Textual tree rendering
        let text_tree = arch.to_textual_tree("calculator_client_run", GraphPerspective::CallFlow, 2);
        assert!(text_tree.contains("calculator_client_run"));
        assert!(text_tree.contains("calculator_service_calculate"));
        assert!(text_tree.contains("math_validator_validate"));
        assert!(text_tree.contains("audit_logger_log"));
    }
}

