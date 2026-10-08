//! Clean cuts stay clean: the superseded domain types, the old registry layer
//! and the old MCP surface are gone from the code, the docs and the agent
//! files. The old Python engine (skills/, cli.py, tests/ at the repo root) is
//! the legacy product and isn't scanned; neither are the spec and audit files
//! that record what was removed.

use std::path::{Path, PathBuf};

fn meta_harness() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Files under `dir` with one of `extensions`, skipping build output, served
/// assets, the spec/audit records and this file.
fn files(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    const SKIP: [&str; 7] = ["target", "static", "spec", "reference", "dist", ".nah", "node_modules"];
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !SKIP.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push(path);
                }
            } else if extensions.iter().any(|e| name.ends_with(e)) && path != Path::new(file!()) && !path.ends_with("tests/clean_cut.rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Identifier-like words in `text`.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-')).filter(|w| !w.is_empty())
}

/// `file:line: word` for every whole-word use of `names`, and every
/// substring use of `phrases`.
fn hits(files: &[PathBuf], names: &[&str], phrases: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        for (n, line) in text.lines().enumerate() {
            for word in words(line).filter(|w| names.contains(w)) {
                out.push(format!("{}:{}: {word}", file.display(), n + 1));
            }
            for phrase in phrases.iter().filter(|p| line.contains(*p)) {
                out.push(format!("{}:{}: {phrase}", file.display(), n + 1));
            }
        }
    }
    out
}

fn rust_sources() -> Vec<PathBuf> {
    let root = meta_harness();
    let mut all = files(&root.join("src"), &[".rs"]);
    all.extend(files(&root.join("tests"), &[".rs"]));
    all.extend(files(&root.join("crates"), &[".rs", ".toml"]));
    all.push(root.join("Cargo.toml"));
    all
}

// req: P2.11
#[test]
fn superseded_domain_types_are_gone() {
    // The architecture-model crate's types, replaced by arch-graph.
    let old = [
        "SystemArchitecture",
        "ComponentSpec",
        "TaskSpec",
        "SideEffectSpec",
        "UsageNode",
        "CodeLocation",
        "AlignmentStatus",
        "GraphNode",
        "NodeKind",
        "GraphEdge",
        "EdgeKind",
        "GraphPerspective",
        "UnifiedArchitectureGraph",
        "ComponentFilter",
        "ComponentSummary",
        "ArchitectureSummary",
        "SubtreeQuery",
        "architecture_model",
        "architecture-model",
    ];
    let found = hits(&rust_sources(), &old, &[]);
    assert!(found.is_empty(), "old domain types are still referenced:\n{}", found.join("\n"));
    assert!(!meta_harness().join("crates/architecture-model").exists(), "the old crate is deleted");
}

// req: P3.7
#[test]
fn the_old_registry_layer_is_gone() {
    let old = ["ArchitectureRepository", "FsJsonRepository", "RegistryService", "RegistryError"];
    let mut found = hits(&rust_sources(), &old, &["meta_harness::registry"]);
    // The server crate's own registry module (harness-core's provider registry is unrelated).
    found.extend(hits(&files(&meta_harness().join("src"), &[".rs"]), &[], &["mod registry", "crate::registry"]));
    assert!(found.is_empty(), "the old registry layer is still referenced:\n{}", found.join("\n"));
    assert!(!meta_harness().join("src/registry").exists());
    // The server runs on the architecture service.
    let server = std::fs::read_to_string(meta_harness().join("src/server.rs")).unwrap();
    assert!(server.contains("ArchitectureService"), "AppState holds the architecture service");
}

// req: P8.5
#[test]
fn the_old_mcp_surface_is_gone_from_code_docs_and_agent_files() {
    let repo = meta_harness().join("..");
    // The old Rust tools, and the Python engine's tools the agent files used to call.
    let old_tools = [
        "search_components",
        "inspect_component",
        "get_architecture_summary",
        "focus_architecture",
        "approve_component",
        "plan_component",
        "implement_component",
        "approve_arch",
        "compile_component_contract",
        "get_next_actionable_components",
        "update_component",
        "check_compatibility",
        "visualize_architecture",
    ];
    let mut scanned = rust_sources();
    scanned.extend(files(&meta_harness(), &[".md"]));
    for dir in ["agents", "plugin/agents"] {
        scanned.extend(files(&repo.join(dir), &[".md", ".json"]));
    }
    let found = hits(&scanned, &old_tools, &["/api/registry/architecture", "ARCHITECTURE_REGISTRY_PATH"]);
    assert!(found.is_empty(), "the old MCP surface is still referenced:\n{}", found.join("\n"));
    // The agent files point at the current tools and leave approval to the user.
    for agent in ["agents/ComponentPlanner.md", "agents/ComponentImplementer.md", "plugin/agents/ComponentPlanner.md", "plugin/agents/ComponentImplementer.md"] {
        let text = std::fs::read_to_string(repo.join(agent)).unwrap();
        assert!(text.contains("`contract`") && text.contains("cockpit"), "{agent} uses the NAH tools and leaves approval to the user");
    }
    let tools = meta_harness::mcp::tools::list_tools(&arch_graph::tree::TreeConfig::defaults().unwrap());
    let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
    for old in old_tools {
        assert!(!names.iter().any(|n| n == old), "{old} is not served");
    }
    assert!(!names.iter().any(|n| n.contains("approve")), "agents have no approval tools");
}
