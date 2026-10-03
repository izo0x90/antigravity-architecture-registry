//! Ingestion against fixture codebases with hand-written expected graphs.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use arch_graph::{ComponentId, RelationKind, TypeSystem};
use arch_index::{FileExtract, PackSet, Resolved, ingest};
use serde::Deserialize;

#[derive(Deserialize)]
struct Expected {
    components: Vec<(String, String)>,
    relations: Vec<(String, String, String)>,
    ambiguous: Vec<Vec<String>>,
    placeholders: Vec<String>,
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Built-ins plus the packs that exist only as test data (tests/packs).
fn packs_with_test_data() -> PackSet {
    let mut packs = PackSet::built_in().unwrap();
    packs.load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/packs")).unwrap();
    packs
}

fn run(root: &Path, packs: &PackSet) -> (Vec<FileExtract>, Resolved) {
    ingest(root, Path::new("."), packs, &TypeSystem::defaults().unwrap()).unwrap()
}

/// Compares the ingested graph with an expected file, exactly.
fn assert_matches(lang: &str, resolved: &Resolved, expected_file: &Path) {
    let expected: Expected = toml::from_str(&std::fs::read_to_string(expected_file).unwrap()).unwrap();
    let id = |s: &str| ComponentId::for_symbol(lang, s);
    let got: BTreeSet<(String, String)> = resolved.graph.components().map(|c| (c.id.to_string(), c.type_name.clone())).collect();
    let want: BTreeSet<(String, String)> = expected.components.iter().map(|(s, t)| (id(s).to_string(), t.clone())).collect();
    assert_eq!(got, want, "components");
    let got: BTreeSet<(String, String, String)> =
        resolved.graph.relations().map(|r| (r.kind.name().to_owned(), r.source.to_string(), r.target.to_string())).collect();
    let want: BTreeSet<(String, String, String)> =
        expected.relations.iter().map(|(k, a, b)| (k.clone(), id(a).to_string(), id(b).to_string())).collect();
    assert_eq!(got.difference(&want).collect::<Vec<_>>(), Vec::<&(String, String, String)>::new(), "unexpected relations");
    assert_eq!(want.difference(&got).collect::<Vec<_>>(), Vec::<&(String, String, String)>::new(), "missing relations");
    let got: Vec<Vec<String>> = resolved
        .ambiguous
        .iter()
        .map(|a| {
            let mut row = vec![a.caller.to_string(), a.callee.clone()];
            row.extend(a.candidates.iter().map(ToString::to_string));
            row
        })
        .collect();
    let want: Vec<Vec<String>> = expected
        .ambiguous
        .iter()
        .map(|row| row.iter().enumerate().map(|(i, s)| if i == 1 { s.clone() } else { id(s).to_string() }).collect())
        .collect();
    assert_eq!(got, want, "ambiguous calls");
    let got: BTreeSet<String> = resolved.graph.components().filter(|c| !c.placeholders.is_empty()).map(|c| c.id.to_string()).collect();
    let want: BTreeSet<String> = expected.placeholders.iter().map(|s| id(s).to_string()).collect();
    assert_eq!(got, want, "placeholders");
    for c in resolved.graph.components() {
        c.check_location().unwrap();
        let expect = if c.placeholders.is_empty() { arch_graph::Stage::Implemented } else { arch_graph::Stage::Skeleton };
        assert_eq!(c.stage, expect, "{}", c.id);
    }
}

// req: P4.3, P4.9
#[test]
fn rust_fixture_ingests_to_exactly_the_expected_graph() {
    let packs = PackSet::built_in().unwrap();
    let (extracts, resolved) = run(&fixtures().join("rust/sample"), &packs);
    assert_eq!(extracts.len(), 3);
    assert_matches("rust", &resolved, &fixtures().join("rust/expected.toml"));
    let save = resolved.graph.component(&ComponentId::for_symbol("rust", "sample::store::Store::save")).unwrap();
    let sig = save.signature.as_ref().unwrap();
    assert_eq!(sig.params.iter().map(|p| (p.name.as_str(), p.ty.as_deref())).collect::<Vec<_>>(), [("&self", None), ("item", Some("&str"))]);
    let shout = resolved.graph.component(&ComponentId::for_symbol("rust", "sample::util::shout")).unwrap();
    assert_eq!(shout.signature.as_ref().unwrap().returns.as_deref(), Some("String"));
    assert_eq!(shout.anchor.as_ref().unwrap().file.as_str(), "src/util.rs");
    assert_eq!(shout.anchor.as_ref().unwrap().span.start_line, 1);
}

// req: P4.6
#[test]
fn calls_resolve_by_evidence_with_order_and_sites() {
    let packs = PackSet::built_in().unwrap();
    let (_, resolved) = run(&fixtures().join("rust/sample"), &packs);
    let id = |s: &str| ComponentId::for_symbol("rust", s);
    let run_calls: Vec<_> = resolved.graph.outgoing(&id("sample::run"), Some(RelationKind::Calls)).collect();
    let order = |target: &str| run_calls.iter().find(|r| r.target == id(target)).and_then(|r| r.order).unwrap();
    assert!(order("sample::store::Store::new") < order("sample::Upper::apply"));
    assert!(order("sample::Upper::apply") < order("sample::store::Store::save"));
    assert!(order("sample::store::Store::save") < order("sample::log"));
    assert!(run_calls.iter().all(|r| r.sites.len() == 1 && r.sites[0].start_line > 0));
    // Vec::new and to_uppercase aren't in the repo: external, never matched by name.
    let external: BTreeSet<&str> = resolved.external.iter().map(|e| e.callee.as_str()).collect();
    assert!(external.contains("Vec::new") && external.contains("s.to_uppercase"), "{external:?}");
    assert!(!resolved.graph.relations().any(|r| r.kind == RelationKind::Calls && r.source == id("sample::store::Store::new")));
    // Two free functions named helper, neither imported: recorded with both candidates.
    assert_eq!(resolved.ambiguous.len(), 1);
    assert_eq!(resolved.ambiguous[0].candidates.len(), 2);
}

// req: P4.4
#[test]
fn python_fixture_ingests_to_exactly_the_expected_graph() {
    let packs = PackSet::built_in().unwrap();
    let (_, resolved) = run(&fixtures().join("python/app"), &packs);
    assert_matches("python", &resolved, &fixtures().join("python/expected.toml"));
    let format = resolved.graph.component(&ComponentId::for_symbol("python", "app.Greeter.format")).unwrap();
    let params: Vec<_> = format.signature.as_ref().unwrap().params.iter().map(|p| (p.name.clone(), p.ty.clone())).collect();
    assert_eq!(params, [("self".into(), None), ("text".into(), Some("str".into())), ("loud".into(), None)]);
}

// req: P4.5
#[test]
fn typescript_fixture_ingests_to_exactly_the_expected_graph() {
    let packs = PackSet::built_in().unwrap();
    let (extracts, resolved) = run(&fixtures().join("typescript/app"), &packs);
    assert_eq!(extracts.len(), 2);
    assert_matches("typescript", &resolved, &fixtures().join("typescript/expected.toml"));
    let greet = resolved.graph.component(&ComponentId::for_symbol("typescript", "app.Greeter.greet")).unwrap();
    let sig = greet.signature.as_ref().unwrap();
    assert_eq!(sig.params.iter().map(|p| (p.name.as_str(), p.ty.as_deref())).collect::<Vec<_>>(), [("name", Some("string"))], "annotations unwrap to the type");
    assert_eq!(sig.returns.as_deref(), Some("string"));
    let external: BTreeSet<&str> = resolved.external.iter().map(|e| e.callee.as_str()).collect();
    assert!(external.contains("console.log") && external.contains("Error"), "{external:?}");
}

// req: P4.2, P4.13
#[test]
fn a_pack_defined_only_as_data_ingests_its_language() {
    // Go has no built-in pack and no engine code: only tests/packs/go.toml.
    assert!(PackSet::built_in().unwrap().by_name("go").is_none());
    let packs = packs_with_test_data();
    let (extracts, resolved) = run(&fixtures().join("go/app"), &packs);
    assert!(extracts.iter().all(|e| e.language == "go"));
    assert_matches("go", &resolved, &fixtures().join("go/expected.toml"));
    let save = resolved.graph.component(&ComponentId::for_symbol("go", "app.store.store.Store.Save")).unwrap();
    let sig = save.signature.as_ref().unwrap();
    assert_eq!(sig.params.iter().map(|p| (p.name.as_str(), p.ty.as_deref())).collect::<Vec<_>>(), [("item", Some("string"))]);
    assert_eq!(sig.returns.as_deref(), Some("error"));
}

// req: P4.8
#[test]
fn typed_values_between_calls_become_data_flow_and_untyped_ones_are_reported() {
    let flows = |resolved: &Resolved| -> BTreeSet<(String, String, String, String)> {
        resolved
            .graph
            .relations()
            .filter(|r| r.kind == RelationKind::DataFlow)
            .map(|r| {
                let (from, to) = r.ports.clone().unwrap();
                assert!(r.provenance.observed && !r.provenance.planned && r.sites.len() == 1);
                (r.source.to_string(), r.target.to_string(), from, to)
            })
            .collect()
    };
    let row = |lang: &str, a: &str, b: &str, to: &str| (ComponentId::for_symbol(lang, a).to_string(), ComponentId::for_symbol(lang, b).to_string(), "return".to_owned(), to.to_owned());
    // Rust: through a local binding and a borrow, String into &str.
    let (_, rust) = run(&fixtures().join("rust/sample"), &PackSet::built_in().unwrap());
    assert_eq!(flows(&rust), BTreeSet::from([row("rust", "sample::Upper::apply", "sample::store::Store::save", "item"), row("rust", "sample::Upper::apply", "sample::log", "msg")]));
    assert!(rust.unverified_flows.is_empty(), "{:?}", rust.unverified_flows);
    // Python and TypeScript: a nested call is typed; load() has no return type.
    for (lang, fixture, untyped_targets) in [("python", "python/app", vec!["app.Greeter.greet"]), ("typescript", "typescript/app", vec!["app.Greeter.greet", "app.show"])] {
        let (_, resolved) = run(&fixtures().join(fixture), &PackSet::built_in().unwrap());
        assert_eq!(flows(&resolved), BTreeSet::from([row(lang, "helpers.clean", "app.Greeter.format", "text")]), "{lang}");
        let reported: Vec<(String, String)> = resolved.unverified_flows.iter().map(|u| (u.source.to_string(), u.target.to_string())).collect();
        let expected: Vec<(String, String)> =
            untyped_targets.iter().map(|t| (ComponentId::for_symbol(lang, "helpers.load").to_string(), ComponentId::for_symbol(lang, t).to_string())).collect();
        assert_eq!(reported, expected, "{lang}");
        assert!(resolved.unverified_flows.iter().all(|u| u.reason.contains("no return type") && u.position == 0 && u.span.start_line > 0));
    }
    // Go: a qualified type (*store.Store) matches the producer's Store.
    let (_, go) = run(&fixtures().join("go/app"), &packs_with_test_data());
    assert!(flows(&go).contains(&row("go", "app.store.store.New", "app.main.keep", "st")));
    // A mismatch is reported, not invented: a number passed where a string is expected.
    let dir = std::env::temp_dir().join(format!("nah-flow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/app.ts"), "function make(): number { return 1; }\nfunction take(s: string): void {}\nfunction run(): void { take(make()); }\n").unwrap();
    let (_, mismatch) = run(&dir, &PackSet::built_in().unwrap());
    assert!(flows(&mismatch).is_empty());
    assert_eq!(mismatch.unverified_flows.len(), 1);
    assert!(mismatch.unverified_flows[0].reason.contains("number doesn't match parameter 's': string"), "{:?}", mismatch.unverified_flows);
    std::fs::remove_dir_all(&dir).unwrap();
}

// req: P4.2
#[test]
fn malformed_packs_are_rejected_with_a_reason() {
    let base = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("packs/python.toml")).unwrap();
    let cases = [
        (base.replace("ast_grep = \"python\"", "ast_grep = \"klingon\""), "unknown ast-grep language"),
        (base.replace("rule = { kind = \"call\" }", "rule = { kind = \"call\", nonsense = 1 }"), "calls[0]"),
        ("[language]\nname = \"x\"\n".to_owned(), "missing field"),
    ];
    for (text, needle) in cases {
        let err = arch_index::Pack::from_toml(&text).unwrap_err().to_string();
        assert!(err.contains(needle), "{err} should mention {needle}");
    }
}

// req: P2.10
#[test]
fn index_entries_round_trip_and_reject_other_versions() {
    let packs = PackSet::built_in().unwrap();
    let (extracts, _) = run(&fixtures().join("rust/sample"), &packs);
    let util = extracts.iter().find(|e| e.file.as_str() == "src/util.rs").unwrap();
    let golden_path = fixtures().join("rust/golden_util_extract.json");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden_path, util.to_json().unwrap()).unwrap();
    }
    let golden = std::fs::read_to_string(golden_path).unwrap();
    assert_eq!(FileExtract::from_json(&golden).unwrap(), *util, "the util.rs extract matches the golden file");
    assert_eq!(FileExtract::from_json(&util.to_json().unwrap()).unwrap(), *util);
    let future = golden.replacen("\"schema_version\": 1", "\"schema_version\": 7", 1);
    assert!(FileExtract::from_json(&future).unwrap_err().to_string().contains("schema version 7"));
}
