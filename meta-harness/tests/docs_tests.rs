//! The README describes the system as it is: every crate,
//! every shipped pack and pack command, the analysis path and its endpoints,
//! and nothing that's been deleted.

use std::path::Path;

fn read(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

fn folders(path: &str) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

#[test]
fn the_readme_describes_the_system_as_it_is() {
    let readme = read("README.md");

    // Every crate has a row in the code map (`harness-*` covers its family).
    for krate in folders("crates") {
        let row = format!("`{krate}`");
        let family = krate.split_once('-').map(|(head, _)| format!("`{head}-*`"));
        assert!(readme.contains(&row) || family.is_some_and(|f| readme.contains(&f)), "the README's code map doesn't list {krate}");
    }
    // Every shipped pack, by kind, and every pack command.
    for kind in folders("packs") {
        assert!(readme.contains(&format!("`{kind}`")), "the README doesn't describe {kind} packs");
        for pack in folders(&format!("packs/{kind}")) {
            assert!(readme.contains(&format!("`{pack}`")), "the README doesn't list the {pack} pack");
        }
    }
    for command in ["list", "check", "install", "update", "which"] {
        assert!(readme.contains(&format!("meta-harness pack {command}")), "the README doesn't show `pack {command}`");
    }
    // The analysis path, its marks, and where to see it.
    for words in [
        "meta-harness analyze",
        "confirmed",
        "unconfirmed",
        "/api/code/analysis",
        "/api/perf",
        "highlights.scm",
        "contains",
        "calls",
        "data_flow",
    ] {
        assert!(readme.contains(words), "the README doesn't mention {words}");
    }
    // Nothing deleted is named.
    for gone in [
        "SCIP",
        "scip-",
        "indexer",
        "arch-index",
        "arch_index",
        "install-antigravity-acp.sh",
        "AGY_EXECUTABLE",
        "find_acp_server",
        "aren't built yet",
        "/explainer/",
        "UPDATE_EXPLAINER",
    ] {
        assert!(!readme.contains(gone), "the README still names {gone}");
    }
}
