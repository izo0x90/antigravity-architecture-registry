//! The real rust-analyzer, through its pack, over a copy of the Rust
//! fixture: the calls it resolves are the fixture's expected calls, proven.
//! It needs rust-analyzer (meta-harness pack install rust-analyzer).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use arch_graph::{ComponentId, RelationKind, TypeSystem};
use code_analyzer::{CodeRules, Indexers, PackSet, analyze};

fn shipped() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")
}

// req: AP.9
#[test]
fn rust_analyzer_confirms_the_fixtures_calls() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust/sample");
    for file in ["Cargo.toml", "src/lib.rs", "src/store.rs", "src/util.rs"] {
        std::fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
        std::fs::copy(fixture.join(file), root.join(file)).unwrap();
    }
    let places = nah_packs::Places::isolated(&dir.path().join("user"));
    let catalog = nah_packs::Catalog::load(&nah_packs::Distribution::from_dir(&shipped()).unwrap(), &places).unwrap();
    let lock = nah_packs::Lock::default();
    let state = nah_packs::state(&catalog, &lock, "rust-analyzer").unwrap();
    assert!(state.is_ready(), "this test needs rust-analyzer: meta-harness pack install rust-analyzer ({state:?})");

    let packs = PackSet::from_catalog(&catalog).unwrap();
    let types = TypeSystem::defaults().unwrap();
    let analysis = analyze(&root, Path::new("."), &packs, &CodeRules::default(), &types, Some(Indexers { catalog: &catalog, lock: &lock })).unwrap();
    assert!(analysis.skipped.is_empty(), "{:?}", analysis.skipped);
    assert_eq!(analysis.indexed.iter().map(|f| f.path.as_str()).collect::<BTreeSet<_>>(), BTreeSet::from(["src/lib.rs", "src/store.rs", "src/util.rs"]));
    assert!(root.join(".nah/cache/rust-analyzer/target").is_dir() && !root.join("target").exists(), "its build stays in the project's cache");

    #[derive(serde::Deserialize)]
    struct Expected {
        relations: Vec<(String, String, String)>,
    }
    let expected: Expected = toml::from_str(&std::fs::read_to_string(fixture.join("../expected.toml")).unwrap()).unwrap();
    let id = |s: &str| ComponentId::for_symbol("rust", s);
    let graph = &analysis.resolved.graph;
    let mut unconfirmed = Vec::new();
    for (kind, from, to) in expected.relations.iter().filter(|(k, ..)| k == "calls") {
        let r = graph.relations().find(|r| r.kind == RelationKind::Calls && r.source == id(from) && r.target == id(to));
        match r {
            None => panic!("{kind} {from} -> {to}: missing"),
            Some(r) if r.proven_by.as_deref().is_some_and(|p| p.starts_with("rust-analyzer ")) => {}
            Some(_) => unconfirmed.push(format!("{from} -> {to}")),
        }
    }
    assert!(unconfirmed.is_empty(), "rust-analyzer didn't confirm: {unconfirmed:#?}");
}
