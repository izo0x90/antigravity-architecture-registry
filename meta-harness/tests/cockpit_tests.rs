use meta_harness::cockpit::chat::{ChatStreamState, TurnRole};
use meta_harness::cockpit::code::CodeViewerState;
use meta_harness::cockpit::graph::ArchGraphState;
use meta_harness::cockpit::tree::PlanTreeState;

#[test]
fn test_chat_stream_turn_lifecycle() {
    let mut chat = ChatStreamState::default();
    assert_eq!(chat.turns.len(), 1); // init turn

    // 1. User sends prompt
    chat.append_user_prompt("Can you inspect Cargo.toml?".to_string());
    assert_eq!(chat.turns.len(), 2);
    assert_eq!(chat.turns[1].role, TurnRole::User);
    assert_eq!(chat.turns[1].prompt, "Can you inspect Cargo.toml?");

    // 2. Assistant starts turn
    chat.start_assistant_turn("turn-test-1");
    assert_eq!(chat.turns.len(), 3);
    assert!(chat.is_streaming);

    // 3. Delta streaming (thought + text)
    chat.append_content_delta("Thinking about files...", true);
    chat.append_content_delta("Here is the content.", false);
    assert_eq!(chat.turns[2].thought, "Thinking about files...");
    assert_eq!(chat.turns[2].content, "Here is the content.");

    // 4. Tool update
    chat.update_task("task-1", "Reading Cargo.toml", "completed");
    assert_eq!(chat.turns[2].tool_calls.len(), 1);
    assert_eq!(chat.turns[2].tool_calls[0].status, "completed");
    assert_eq!(chat.turns[2].tool_calls[0].target_path, Some("Cargo.toml".to_string()));

    // 5. Turn completed
    chat.complete_turn("turn-test-1");
    assert!(!chat.is_streaming);
    assert!(!chat.turns[2].is_streaming);
}

#[test]
fn test_code_viewer_load_and_line_numbering() {
    let mut viewer = CodeViewerState::default();
    viewer.load_file("src/main.rs".to_string(), "fn main() {\n    println!(\"Hello\");\n}\n".to_string(), Some(2));

    assert_eq!(viewer.file_path, "src/main.rs");
    assert_eq!(viewer.lines.len(), 3);
    assert_eq!(viewer.highlight_line, Some(2));
    assert_eq!(viewer.lines[0], "fn main() {");
    assert_eq!(viewer.lines[1], "    println!(\"Hello\");");
}

#[test]
fn test_arch_graph_deterministic_nodes_and_edges() {
    let graph = ArchGraphState::default();
    assert_eq!(graph.nodes.len(), 5);
    assert_eq!(graph.edges.len(), 4);

    let adapter_node = graph.nodes.iter().find(|n| n.id == "adapter").unwrap();
    assert_eq!(adapter_node.title, "AntigravityAdapter");
    assert!(adapter_node.inputs.contains(&"turn_prompt"));
    assert!(adapter_node.outputs.contains(&"runtime_events"));

    // Verify all edges link to valid nodes
    for edge in &graph.edges {
        assert!(graph.nodes.iter().any(|n| n.id == edge.from));
        assert!(graph.nodes.iter().any(|n| n.id == edge.to));
    }
}

#[test]
fn test_plan_tree_hierarchy() {
    let tree = PlanTreeState::default();
    assert_eq!(tree.root.id, "root");
    assert!(!tree.root.children.is_empty());

    let cockpit_node = tree.root.children.iter().find(|n| n.id == "cockpit").unwrap();
    assert_eq!(cockpit_node.kind, "INVARIANT");
    assert_eq!(cockpit_node.children.len(), 3);
}

#[test]
fn test_available_models_catalog() {
    let models = ChatStreamState::available_models();
    assert!(models.iter().any(|(id, _)| *id == "gemini-3.7-flash-high"));
    assert!(models.iter().any(|(id, _)| *id == "gemini-pro-agent"));
    assert!(models.len() >= 10);
}
