use harness_protocol::arch::{
    AlignmentStatus, CodeLocation, ComponentSpec, EdgeKind, GraphEdge, GraphNode, NodeKind,
    SideEffectSpec, TaskSpec, UnifiedArchitectureGraph, UsageNode,
};
use std::collections::BTreeMap;

/// Sample 1: Calculation Call Graph (Caller -> Service -> Dependencies)
pub fn sample_calculation_call_graph() -> UnifiedArchitectureGraph {
    let mut nodes = BTreeMap::new();
    let mut edges = Vec::new();

    // 1. Client Node
    nodes.insert(
        "calculator_client".to_string(),
        GraphNode {
            id: "calculator_client".to_string(),
            label: "Calculator CLI Client".to_string(),
            kind: NodeKind::Component(ComponentSpec {
                id: "calculator_client".to_string(),
                name: "Calculator CLI Client".to_string(),
                comp_type: "module".to_string(),
                description: "Parses CLI arguments and invokes service".to_string(),
                status: "active".to_string(),
                stage: "implemented".to_string(),
                parent_id: None,
                implements_id: None,
                location: Some(CodeLocation {
                    file_path: "calculator/client.py".to_string(),
                    start_line: 1,
                    end_line: 45,
                    symbol_name: Some("CalculatorClient".to_string()),
                }),
                inputs: None,
                outputs: None,
                properties: None,
                side_effects: vec![],
                implementation_spec: None,
                modification_tasks: vec![],
            }),
            parent_group: None,
            alignment: AlignmentStatus::VerifiedInCode {
                location: CodeLocation {
                    file_path: "calculator/client.py".to_string(),
                    start_line: 1,
                    end_line: 45,
                    symbol_name: Some("CalculatorClient".to_string()),
                },
            },
        },
    );

    // 2. Calculator Service Node
    nodes.insert(
        "calculator_service".to_string(),
        GraphNode {
            id: "calculator_service".to_string(),
            label: "Calculator Service".to_string(),
            kind: NodeKind::Component(ComponentSpec {
                id: "calculator_service".to_string(),
                name: "Calculator Service".to_string(),
                comp_type: "service".to_string(),
                description: "Coordinates arithmetic logic and validation".to_string(),
                status: "active".to_string(),
                stage: "implemented".to_string(),
                parent_id: None,
                implements_id: None,
                location: Some(CodeLocation {
                    file_path: "calculator/service.py".to_string(),
                    start_line: 10,
                    end_line: 80,
                    symbol_name: Some("CalculatorService".to_string()),
                }),
                inputs: None,
                outputs: None,
                properties: None,
                side_effects: vec![],
                implementation_spec: None,
                modification_tasks: vec![],
            }),
            parent_group: None,
            alignment: AlignmentStatus::VerifiedInCode {
                location: CodeLocation {
                    file_path: "calculator/service.py".to_string(),
                    start_line: 10,
                    end_line: 80,
                    symbol_name: Some("CalculatorService".to_string()),
                },
            },
        },
    );

    // 3. Math Validator Node
    nodes.insert(
        "math_validator".to_string(),
        GraphNode {
            id: "math_validator".to_string(),
            label: "Math Validator".to_string(),
            kind: NodeKind::Component(ComponentSpec {
                id: "math_validator".to_string(),
                name: "Math Validator".to_string(),
                comp_type: "validator".to_string(),
                description: "Guards against division by zero and NaN".to_string(),
                status: "active".to_string(),
                stage: "implemented".to_string(),
                parent_id: None,
                implements_id: None,
                location: Some(CodeLocation {
                    file_path: "calculator/validator.py".to_string(),
                    start_line: 5,
                    end_line: 35,
                    symbol_name: Some("validate_operands".to_string()),
                }),
                inputs: None,
                outputs: None,
                properties: None,
                side_effects: vec![],
                implementation_spec: None,
                modification_tasks: vec![],
            }),
            parent_group: None,
            alignment: AlignmentStatus::VerifiedInCode {
                location: CodeLocation {
                    file_path: "calculator/validator.py".to_string(),
                    start_line: 5,
                    end_line: 35,
                    symbol_name: Some("validate_operands".to_string()),
                },
            },
        },
    );

    // 4. Audit Logger Node
    nodes.insert(
        "audit_logger".to_string(),
        GraphNode {
            id: "audit_logger".to_string(),
            label: "Audit Logger".to_string(),
            kind: NodeKind::Component(ComponentSpec {
                id: "audit_logger".to_string(),
                name: "Audit Logger".to_string(),
                comp_type: "logger".to_string(),
                description: "Logs calculation history to disk audit log".to_string(),
                status: "planned".to_string(),
                stage: "draft".to_string(),
                parent_id: None,
                implements_id: None,
                location: None,
                inputs: None,
                outputs: None,
                properties: None,
                side_effects: vec![SideEffectSpec {
                    target: "fs".to_string(),
                    description: "Appends to /var/log/audit.json".to_string(),
                }],
                implementation_spec: None,
                modification_tasks: vec![],
            }),
            parent_group: None,
            alignment: AlignmentStatus::PlannedOnly,
        },
    );

    // 5. Usage Call Node: Client invokes Service
    nodes.insert(
        "call_calculate".to_string(),
        GraphNode {
            id: "call_calculate".to_string(),
            label: "calculate(op, a, b)".to_string(),
            kind: NodeKind::UsageCall(UsageNode {
                node_id: "call_calculate".to_string(),
                caller_id: "calculator_client".to_string(),
                component_id: "calculator_service".to_string(),
                description: "Dispatch arithmetic request from CLI arguments".to_string(),
                expected_inputs: None,
                expected_outputs: None,
                expected_side_effects: vec![],
                dependencies: vec![],
            }),
            parent_group: Some("calculator_client".to_string()),
            alignment: AlignmentStatus::VerifiedInCode {
                location: CodeLocation {
                    file_path: "calculator/client.py".to_string(),
                    start_line: 32,
                    end_line: 32,
                    symbol_name: Some("service.calculate".to_string()),
                },
            },
        },
    );

    // Edges
    edges.push(GraphEdge {
        id: "edge_client_to_call".to_string(),
        source_id: "calculator_client".to_string(),
        target_id: "call_calculate".to_string(),
        kind: EdgeKind::CallSite { call_site: None },
    });
    edges.push(GraphEdge {
        id: "edge_call_to_service".to_string(),
        source_id: "call_calculate".to_string(),
        target_id: "calculator_service".to_string(),
        kind: EdgeKind::CallSite { call_site: None },
    });
    edges.push(GraphEdge {
        id: "edge_service_to_validator".to_string(),
        source_id: "calculator_service".to_string(),
        target_id: "math_validator".to_string(),
        kind: EdgeKind::CallSite { call_site: None },
    });
    edges.push(GraphEdge {
        id: "edge_service_to_logger".to_string(),
        source_id: "calculator_service".to_string(),
        target_id: "audit_logger".to_string(),
        kind: EdgeKind::CallSite { call_site: None },
    });

    UnifiedArchitectureGraph { nodes, edges }
}

/// Sample 2: Data Flow Pipeline (Multi-stage data transforms)
pub fn sample_dataflow_pipeline() -> UnifiedArchitectureGraph {
    let mut nodes = BTreeMap::new();
    let mut edges = Vec::new();

    let stages = [
        ("raw_source", "Raw Input Stream", "Ingests raw event payload", "source"),
        ("token_lexer", "Lexical Tokenizer", "Tokenizes text into semantic lexemes", "transform"),
        ("ast_parser", "Syntax Parser", "Constructs concrete syntax tree", "transform"),
        ("type_checker", "Type Inference & Checker", "Enforces static contract guarantees", "validator"),
        ("code_emitter", "Target Bytecode Emitter", "Generates optimized target representation", "sink"),
    ];

    for (id, label, desc, ctype) in stages {
        nodes.insert(
            id.to_string(),
            GraphNode {
                id: id.to_string(),
                label: label.to_string(),
                kind: NodeKind::Component(ComponentSpec {
                    id: id.to_string(),
                    name: label.to_string(),
                    comp_type: ctype.to_string(),
                    description: desc.to_string(),
                    status: "active".to_string(),
                    stage: "implemented".to_string(),
                    parent_id: None,
                    implements_id: None,
                    location: None,
                    inputs: None,
                    outputs: None,
                    properties: None,
                    side_effects: vec![],
                    implementation_spec: None,
                    modification_tasks: vec![],
                }),
                parent_group: None,
                alignment: AlignmentStatus::VerifiedInCode {
                    location: CodeLocation {
                        file_path: format!("pipeline/{}.rs", id),
                        start_line: 1,
                        end_line: 100,
                        symbol_name: Some(label.to_string()),
                    },
                },
            },
        );
    }

    // Data flow edges connecting stages sequentially
    for i in 0..(stages.len() - 1) {
        let from = stages[i].0;
        let to = stages[i + 1].0;
        edges.push(GraphEdge {
            id: format!("{}_to_{}", from, to),
            source_id: from.to_string(),
            target_id: to.to_string(),
            kind: EdgeKind::DataFlow {
                from_port: "out".to_string(),
                to_port: "in".to_string(),
            },
        });
    }

    UnifiedArchitectureGraph { nodes, edges }
}

/// Sample 3: Hierarchical Task Plan (Decomposition)
pub fn sample_task_hierarchy() -> UnifiedArchitectureGraph {
    let mut nodes = BTreeMap::new();
    let mut edges = Vec::new();

    // Root plan
    nodes.insert(
        "system_refactor".to_string(),
        GraphNode {
            id: "system_refactor".to_string(),
            label: "Meta-Harness Architecture System Refactor".to_string(),
            kind: NodeKind::PlanStep(TaskSpec {
                id: Some("system_refactor".to_string()),
                task: "Core Protocol & UI Synchronization".to_string(),
                completed: false,
                subtasks: vec![],
            }),
            parent_group: None,
            alignment: AlignmentStatus::PlannedOnly,
        },
    );

    let tasks = [
        ("task_protocol", "1. Extract Shared harness-protocol Crate", true),
        ("task_graph_ir", "2. Unify Graph & Component Data Models", true),
        ("task_layout", "3. Implement Directed Hierarchical Layout", true),
        ("task_ast_grep", "4. Integrate AST-Grep Call-Site Verifier", false),
        ("task_feedback", "5. Wire Bidirectional Change Invariants", false),
    ];

    for (id, label, done) in tasks {
        nodes.insert(
            id.to_string(),
            GraphNode {
                id: id.to_string(),
                label: label.to_string(),
                kind: NodeKind::PlanStep(TaskSpec {
                    id: Some(id.to_string()),
                    task: label.to_string(),
                    completed: done,
                    subtasks: vec![],
                }),
                parent_group: Some("system_refactor".to_string()),
                alignment: if done {
                    AlignmentStatus::VerifiedInCode {
                        location: CodeLocation {
                            file_path: "src/lib.rs".to_string(),
                            start_line: 1,
                            end_line: 50,
                            symbol_name: None,
                        },
                    }
                } else {
                    AlignmentStatus::PlannedOnly
                },
            },
        );

        edges.push(GraphEdge {
            id: format!("subtask_{}", id),
            source_id: "system_refactor".to_string(),
            target_id: id.to_string(),
            kind: EdgeKind::Decomposition,
        });
    }

    UnifiedArchitectureGraph { nodes, edges }
}
