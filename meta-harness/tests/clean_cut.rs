//! Clean cuts stay clean: the superseded domain types, the old registry layer
//! and the old MCP surface are gone from the code, the docs and the agent
//! files. The old Python engine (skills/, cli.py, tests/ at the repo root) is
//! the legacy product and isn't scanned.

use std::path::{Path, PathBuf};

fn meta_harness() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Files under `dir` with one of `extensions`, skipping build output, served
/// assets and this file.
fn files(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    const SKIP: [&str; 6] = ["target", "static", "reference", "dist", ".nah", "node_modules"];
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

#[test]
fn one_pipeline_and_scip_is_gone() {
    // SCIP and the second orchestration.
    let old = ["IndexedFile", "IndexedSymbol", "from_shipped_dir", "scip", "protobuf", "Combined", "CombinedRows", "combine_key", "COMBINE_VERSION"];
    let found = hits(&rust_sources(), &old, &["code_analyzer::analyze(", "ROLE_"]);
    assert!(found.is_empty(), "SCIP and the second pipeline are still referenced:\n{}", found.join("\n"));
    for gone in ["crates/code-analyzer/src/index.rs", "crates/code-analyzer/src/resolve.rs", "packs/indexer"] {
        assert!(!meta_harness().join(gone).exists(), "{gone} is deleted");
    }
    // `meta-harness analyze` is the service's pipeline, waiting for the servers.
    let analyze = std::fs::read_to_string(meta_harness().join("src/project.rs")).unwrap();
    for step in ["ArchitectureService::open", ".with_servers(", ".ingest(", ".wait_for_servers(", ".file_rows()"] {
        assert!(analyze.contains(step), "analyze runs the service's pipeline: no {step}");
    }
}

#[test]
fn the_installer_knows_kinds_of_steps_not_tools() {
    // No tool, URL, platform-specific path or harness in the pack engine or
    // the pack commands: those are in pack and config files.
    let mut scanned = files(&meta_harness().join("crates/nah-packs/src"), &[".rs"]);
    scanned.push(meta_harness().join("src/packs.rs"));
    let found: Vec<String> = hits(&scanned, &[], &["antigravity", "Antigravity", "qemu", "QEMU", "debian.org", "ubuntu.com", "http://", "https://", "/opt/", "/etc/", "agy_acp", "localharness", "rust-analyzer", "groupadd", "ca-certificates"]);
    assert!(found.is_empty(), "tool names, URLs or paths in code:\n{}", found.join("\n"));
}

#[test]
fn dead_code_and_leftovers_are_gone_and_test_only_code_is_built_only_for_tests() {
    let root = meta_harness();
    // Deleted: dead, or a port nothing used.
    let sources = rust_sources();
    let deleted = ["fn unregister", "fn is_user", "fn resolve_antigravity_user_home", "fn flow_indexes_built", "fn settled_cursor", "fn settings(&self) -> &QueueSettings", "fn dsl(&self)", "fn config(&self) -> &QueueConfig", "fn from_plan("];
    assert_eq!(hits(&sources, &[], &deleted), Vec::<String>::new());
    // What only tests use is built only for them.
    let gated = ["pub fn inserts()", "pub fn problems(", "pub fn from_dir(", "pub fn isolated(", "pub fn symbol_lists_built(", "pub fn wait_ready(", "pub fn restarts(", "pub fn pid(", "pub fn overlaps(", "pub fn observed<", "pub use observed::observed;"];
    for file in &sources {
        let text = std::fs::read_to_string(file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (n, line) in lines.iter().enumerate().filter(|(_, l)| gated.iter().any(|g| l.contains(g))) {
            let above = lines[..n].iter().rev().take_while(|l| l.trim_start().starts_with("///") || l.trim_start().starts_with("#[")).any(|l| l.contains("cfg(test)") || l.contains("cfg(any(test, feature = \"testing\"))"));
            assert!(above, "{}:{}: {line} is built outside the tests", file.display(), n + 1);
        }
    }
    // The leftovers of the old indexers.
    assert!(!root.join("tsconfig.json").exists(), "scip-typescript's tsconfig.json");
    assert!(!root.join(".nah/cache/rust-analyzer").exists(), "rust-analyzer's build folder in the project");
}

#[test]
fn the_cockpit_lays_out_nothing_itself() {
    // Laying out runs on the server, with its time limit; the cockpit takes
    // the places it's given (its own tests stand in for the server).
    let cockpit: Vec<PathBuf> = files(&meta_harness().join("crates/cockpit-ui/src"), &[".rs"]).into_iter().filter(|f| !f.ends_with("graph/tests.rs")).collect();
    let found = hits(&cockpit, &[], &["lay_out(", "tree::build(", "tree::expand("]);
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn the_cockpits_texts_name_no_keys() {
    // Keys come from the bindings (they can be changed); a text that names
    // one goes stale when they are.
    let keys = ["(SPC ", "(Enter)", "(Esc)", "(Tab)"];
    let cockpit: Vec<PathBuf> = files(&meta_harness().join("crates/cockpit-ui/src"), &[".rs"]).into_iter().filter(|f| !f.ends_with("tests.rs")).collect();
    let mut found = Vec::new();
    for file in &cockpit {
        for (i, line) in std::fs::read_to_string(file).unwrap().lines().enumerate() {
            // A hint like "(g d)" or "(z o)": one letter, a space, one key.
            let short = line.as_bytes().windows(5).any(|w| w[0] == b'(' && w[1].is_ascii_lowercase() && w[2] == b' ' && w[3].is_ascii_graphic() && w[3] != b')' && w[4] == b')');
            if line.contains('"') && !line.trim_start().starts_with("//") && (short || keys.iter().any(|k| line.contains(k))) {
                found.push(format!("{}:{}: {}", file.display(), i + 1, line.trim()));
            }
        }
    }
    assert!(found.is_empty(), "{found:#?}");
}
