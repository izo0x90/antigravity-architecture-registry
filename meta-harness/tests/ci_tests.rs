//! What CI runs (the repository's .github/workflows/check.yml).

use std::path::Path;

#[test]
fn ci_runs_the_layout_benchmark() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workflow = std::fs::read_to_string(root.join("../.github/workflows/check.yml")).unwrap();
    assert!(workflow.contains("cargo bench -p arch-service --bench hot -- --test"), "{workflow}");
    assert!(workflow.contains("make check"), "lints and tests too");
    let bench = std::fs::read_to_string(root.join("crates/arch-service/benches/hot.rs")).unwrap();
    assert!(
        bench.contains("arch_graph::tree::lay_out(") && bench.contains("\"layout {} depth {depth}\""),
        "a criterion benchmark of the layout alone"
    );
}

#[test]
fn make_check_lints_and_tests_every_crate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let make = std::fs::read_to_string(root.join("../Makefile")).unwrap();
    // The root crate is also the workspace: without --workspace cargo runs
    // only the root crate's tests and lints.
    assert!(make.contains("cargo clippy --workspace --all-targets -- -D warnings"), "{make}");
    assert!(make.contains("cargo test --workspace"), "{make}");
}
