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
    #[serde(default)]
    unproven: Vec<Vec<String>>,
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
    let id = |s: &str| if s.starts_with("folder:") { ComponentId::new(s) } else { ComponentId::for_symbol(lang, s) };
    let got: BTreeSet<(String, String)> = resolved.graph.components().map(|c| (c.id.to_string(), c.type_name.clone())).collect();
    let want: BTreeSet<(String, String)> = expected.components.iter().map(|(s, t)| (id(s).to_string(), t.clone())).collect();
    assert_eq!(got, want, "components");
    let got: BTreeSet<(String, String, String)> =
        resolved.graph.relations().map(|r| (r.kind.name().to_owned(), r.source.to_string(), r.target.to_string())).collect();
    let want: BTreeSet<(String, String, String)> =
        expected.relations.iter().map(|(k, a, b)| (k.clone(), id(a).to_string(), id(b).to_string())).collect();
    assert_eq!(got.difference(&want).collect::<Vec<_>>(), Vec::<&(String, String, String)>::new(), "unexpected relations");
    assert_eq!(want.difference(&got).collect::<Vec<_>>(), Vec::<&(String, String, String)>::new(), "missing relations");
    let rows = |list: &[arch_index::AmbiguousCall]| -> Vec<Vec<String>> {
        list.iter()
            .map(|a| {
                let mut row = vec![a.caller.to_string(), a.callee.clone()];
                row.extend(a.candidates.iter().map(ToString::to_string));
                row
            })
            .collect()
    };
    let want = |list: &[Vec<String>]| -> Vec<Vec<String>> {
        list.iter().map(|row| row.iter().enumerate().map(|(i, s)| if i == 1 { s.clone() } else { id(s).to_string() }).collect()).collect()
    };
    assert_eq!(rows(&resolved.ambiguous), want(&expected.ambiguous), "ambiguous calls");
    assert_eq!(rows(&resolved.unproven), want(&expected.unproven), "unproven calls");
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
    let fetch = resolved.graph.component(&ComponentId::for_symbol("rust", "sample::fetch")).unwrap();
    assert_eq!(fetch.signature.as_ref().unwrap().qualifiers, ["async"], "qualifiers from the pack");
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
    let optional: Vec<bool> = format.signature.as_ref().unwrap().params.iter().map(|p| p.optional).collect();
    assert_eq!(optional, [false, false, true], "a default value makes it optional");
    let fetch = resolved.graph.component(&ComponentId::for_symbol("python", "app.fetch")).unwrap();
    assert_eq!(fetch.signature.as_ref().unwrap().qualifiers, ["async"]);
}

// req: P4.5
#[test]
fn typescript_fixture_ingests_to_exactly_the_expected_graph() {
    let packs = PackSet::built_in().unwrap();
    let (extracts, resolved) = run(&fixtures().join("typescript/app"), &packs);
    assert_eq!(extracts.len(), 3, ".ts files and the .tsx one (its own grammar)");
    assert_matches("typescript", &resolved, &fixtures().join("typescript/expected.toml"));
    let greet = resolved.graph.component(&ComponentId::for_symbol("typescript", "app.Greeter.greet")).unwrap();
    let sig = greet.signature.as_ref().unwrap();
    assert_eq!(sig.params.iter().map(|p| (p.name.as_str(), p.ty.as_deref())).collect::<Vec<_>>(), [("name", Some("string"))], "annotations unwrap to the type");
    assert_eq!(sig.returns.as_deref(), Some("string"));
    let external: BTreeSet<&str> = resolved.external.iter().map(|e| e.callee.as_str()).collect();
    assert!(external.contains("console.log") && external.contains("Error"), "{external:?}");
    let fetch = resolved.graph.component(&ComponentId::for_symbol("typescript", "app.fetch")).unwrap();
    let sig = fetch.signature.as_ref().unwrap();
    assert_eq!(sig.qualifiers, ["async"]);
    assert_eq!(sig.params.iter().map(|p| (p.name.as_str(), p.optional)).collect::<Vec<_>>(), [("items", false), ("loud", true), ("times", true)]);
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
            // Values passed into calls (not kept, returned or made: see the flow tests).
            .filter(|r| r.kind == RelationKind::DataFlow && r.ports.as_ref().is_some_and(|(from, to)| from == "return" && to != "returns" && !to.starts_with("kept as ")))
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
    for (lang, fixture, untyped_targets) in [("python", "python/app", vec!["app.Greeter.greet"]), ("typescript", "typescript/app", vec!["app.Greeter.greet", "app.show", "helpers.shout"])] {
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
    assert!(flows(&go).contains(&row("go", "app.store.store.New", "app.main.keep", "s")));
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

// req: D.1
#[test]
fn data_flow_covers_results_kept_values_made_state_and_boundaries() {
    let dir = temp_repo(
        "flows",
        &[
            ("Cargo.toml", "[package]\nname = \"shop\"\n"),
            (
                "src/lib.rs",
                r#"pub enum Event { Saved { id: u32 }, Gone }
pub struct Store { count: u32 }
impl Store {
    pub fn new() -> Store { Store { count: 0 } }
    pub fn add(&mut self) { self.count += 1; }
    pub fn total(&self) -> u32 { self.count }
    pub fn save(&self) { self.emit(Event::Saved { id: 1 }); }
    fn emit(&self, _e: Event) {}
}
pub fn on_event(e: Event) { match e { Event::Saved { id } => {}, Event::Gone => {} } }
pub fn pair() -> (u32, u32) { (1, 2) }
pub fn split() -> u32 { let (a, b) = pair(); a + b }
pub fn routes(r: Router) -> Router { r.route("/api/items/{id}", get(item)).route("/api/other", get(other)) }
pub fn other() -> String { String::new() }
pub fn asks_other() -> String { fetch("/api/other") }
pub fn item() -> String { String::new() }
pub fn client(base: &str) -> String { fetch(&format!("{base}/api/items/{}", 3)) }
fn fetch(url: &str) -> String { url.to_owned() }
"#,
            ),
        ],
    );
    let (_, resolved) = run(&dir, &PackSet::built_in().unwrap());
    let id = |s: &str| ComponentId::for_symbol("rust", s).to_string();
    let flows: BTreeSet<(String, String, String, String, String)> = resolved
        .graph
        .relations()
        .filter(|r| r.kind == RelationKind::DataFlow)
        .map(|r| {
            let (from, to) = r.ports.clone().unwrap();
            (r.source.to_string(), r.target.to_string(), from, to, r.data.clone().unwrap_or_default())
        })
        .collect();
    let has = |a: &str, b: &str, from: &str, to: &str, data: &str| {
        let row = (id(a), id(b), from.to_owned(), to.to_owned(), data.to_owned());
        assert!(flows.contains(&row), "{row:?} in {flows:#?}");
    };
    has("shop::Store::new", "shop::Store", "makes", "value", "Store");
    has("shop::Store::add", "shop::Store", "writes", "count", "Store.count");
    has("shop::Store", "shop::Store::total", "count", "reads", "Store.count");
    has("shop::pair", "shop::split", "return", "kept as a, b", "(u32, u32)");
    has("shop::Store::save", "shop::on_event", "event", "Event::Saved", "Event::Saved");
    has("shop::client", "shop::item", "http", "/api/items/{}", "/api/items/{}");
    assert!(!flows.iter().any(|f| f.1 == id("shop::Store::emit") && f.2 == "count"), "a method call isn't a field read");
    assert!(!flows.iter().any(|f| f.0 == id("shop::routes") && f.2 == "http"), "registering a route isn't a request");
    has("shop::asks_other", "shop::other", "http", "/api/other", "/api/other");
    assert!(!flows.iter().any(|f| f.0 == id("shop::asks_other") && f.1 == id("shop::item")), "a request reaches only its own route's handler, however routes chain");
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
    // The pack fingerprint moves with every pack edit; everything else is fixed.
    let golden_extract = FileExtract::from_json(&golden).unwrap();
    assert!(!golden_extract.pack_fingerprint.is_empty() && !util.pack_fingerprint.is_empty());
    assert_eq!(FileExtract { pack_fingerprint: util.pack_fingerprint.clone(), ..golden_extract }, *util, "the util.rs extract matches the golden file");
    assert_eq!(FileExtract::from_json(&util.to_json().unwrap()).unwrap(), *util);
    let future = golden.replacen("\"schema_version\": 1", "\"schema_version\": 7", 1);
    assert!(FileExtract::from_json(&future).unwrap_err().to_string().contains("schema version 7"));
}

fn temp_repo(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nah-{name}-{}", std::process::id())).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    dir
}

// req: P4.6
#[test]
fn calls_stay_within_their_language_and_unknown_receivers_are_never_guessed() {
    // A Python run() and a TypeScript Store share names with the other language.
    let dir = temp_repo(
        "mixed",
        &[
            ("app.py", "class Store:\n    def save(self):\n        pass\n\ndef run():\n    helper()\n    thing.save()\n"),
            ("src/lib.ts", "export function helper(): void {}\nexport class Widget extends Store {}\nfunction go(): void { run(); }\n"),
        ],
    );
    let (_, resolved) = run(&dir, &PackSet::built_in().unwrap());
    let crossing: Vec<_> = resolved.graph.relations().filter(|r| r.source.as_str().split(':').next() != r.target.as_str().split(':').next()).collect();
    assert!(crossing.is_empty(), "no relation crosses languages: {crossing:?}");
    let external: BTreeSet<&str> = resolved.external.iter().map(|e| e.callee.as_str()).collect();
    assert!(external.contains("helper") && external.contains("run"), "{external:?}");
    // `thing` has no known type: recorded as unproven with the candidates, not linked.
    assert_eq!(resolved.unproven.len(), 1);
    assert_eq!(resolved.unproven[0].callee, "thing.save");
    assert_eq!(resolved.unproven[0].candidates, [ComponentId::for_symbol("python", "app.Store.save")]);
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

#[test]
fn observed_calls_carry_deferral_and_dead_code_is_not_observed() {
    let (_, resolved) = run(&fixtures().join("rust/sample"), &PackSet::built_in().unwrap());
    let id = |s: &str| ComponentId::for_symbol("rust", s);
    let later: Vec<_> = resolved.graph.outgoing(&id("sample::later_log"), Some(RelationKind::Calls)).collect();
    assert_eq!(later.len(), 1, "the dead branches add no calls: {later:?}");
    assert!(later[0].deferred, "the call sits in a closure");
    let run_calls: Vec<_> = resolved.graph.outgoing(&id("sample::run"), Some(RelationKind::Calls)).collect();
    assert!(run_calls.iter().all(|r| !r.deferred));
    for (lang, fixture, caller, callee) in [("python", "python/app", "app.fetch", "helpers.load"), ("typescript", "typescript/app", "app.fetch", "helpers.load")] {
        let (_, resolved) = run(&fixtures().join(fixture), &PackSet::built_in().unwrap());
        let call = resolved.graph.outgoing(&ComponentId::for_symbol(lang, caller), Some(RelationKind::Calls)).find(|r| r.target == ComponentId::for_symbol(lang, callee)).unwrap();
        assert!(call.deferred, "{lang}: inside a lambda/arrow");
        let external: BTreeSet<&str> = resolved.external.iter().map(|e| e.callee.as_str()).collect();
        assert!(!external.contains("tidy"), "{lang}: the dead branch's call isn't observed at all");
    }
}

#[test]
fn module_symbols_come_from_the_manifest_not_the_checkout_directory() {
    // The Rust fixture copied under another directory name keeps its crate name.
    let source = fixtures().join("rust/sample");
    let files: Vec<(String, String)> = ["Cargo.toml", "src/lib.rs", "src/store.rs", "src/util.rs"]
        .iter()
        .map(|f| (f.to_string(), std::fs::read_to_string(source.join(f)).unwrap()))
        .collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let dir = temp_repo("fx", &refs);
    let (extracts, _) = run(&dir, &PackSet::built_in().unwrap());
    let modules: BTreeSet<&str> = extracts.iter().map(|e| e.module.as_str()).collect();
    assert_eq!(modules, BTreeSet::from(["sample", "sample::store", "sample::util"]));
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    // A root given as "." names the package from its manifest, with no empty segments.
    let packs = PackSet::built_in().unwrap();
    let rust = packs.by_name("rust").unwrap();
    assert_eq!(arch_index::module_symbol(Path::new("."), &arch_graph::RepoPath::new("src/lib.rs").unwrap(), rust), "arch_index");
    assert_eq!(arch_index::module_symbol(Path::new("."), &arch_graph::RepoPath::new("src/walk.rs").unwrap(), rust), "arch_index::walk");
    // Go's line-based go.mod names the module through the pack's regex.
    let go = packs_with_test_data();
    let (extracts, _) = run(&fixtures().join("go/app"), &go);
    assert!(extracts.iter().all(|e| e.module.starts_with("app.")), "{:?}", extracts.iter().map(|e| &e.module).collect::<Vec<_>>());
}

// req: P4.6
#[test]
fn imports_match_whole_module_paths_and_inherited_members_resolve() {
    let packs = PackSet::built_in().unwrap();
    // Python: `from other.helpers import clean` is not this repo's helpers module,
    // even though the words overlap; `from helpers import clean` is.
    let root = temp_repo(
        "imports",
        &[
            ("helpers.py", "def clean(s):\n    return s\n"),
            ("other_use.py", "from other.helpers import clean\n\n\ndef a():\n    clean(\"x\")\n"),
            ("own_use.py", "from helpers import clean\n\n\ndef b():\n    clean(\"x\")\n"),
            ("src/pkg/tool.py", "def make(*parts, **options):\n    return parts\n"),
        ],
    );
    let (_, resolved) = run(&root, &packs);
    let py = |s: &str| ComponentId::for_symbol("python", s);
    let calls = |from: &str| resolved.graph.outgoing(&py(from), Some(RelationKind::Calls)).map(|r| r.target.to_string()).collect::<Vec<_>>();
    assert_eq!(calls("own_use.b"), vec![py("helpers.clean").to_string()]);
    assert!(calls("other_use.a").is_empty(), "another package's helpers");
    assert!(resolved.unproven.iter().any(|u| u.caller == py("other_use.a")), "recorded as unproven, never linked");
    // Source roots drop from module paths; *args/**kwargs are variadic.
    let make = resolved.graph.component(&py("pkg.tool.make")).expect("src/ is a source root");
    let params: Vec<(&str, bool)> = make.signature.as_ref().unwrap().params.iter().map(|p| (p.name.as_str(), p.variadic)).collect();
    assert_eq!(params, [("*parts", true), ("**options", true)]);
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
    // TypeScript: package.json names the package; .tsx files parse with their own grammar.
    let root = temp_repo(
        "tsxpkg",
        &[
            ("package.json", "{ \"name\": \"web\", \"version\": \"1.0.0\" }"),
            ("src/util.ts", "export const twice = (n: number): number => n * 2;\n"),
            ("src/view.tsx", "import { twice } from \"./util\";\n\nexport function View() {\n  return <b>{twice(2)}</b>;\n}\n"),
        ],
    );
    let (_, resolved) = run(&root, &packs);
    let ts = |s: &str| ComponentId::for_symbol("typescript", s);
    assert!(resolved.graph.component(&ts("web.util.twice")).is_some(), "a module-level arrow function, under the package name");
    assert!(resolved.graph.outgoing(&ts("web.view.View"), Some(RelationKind::Calls)).any(|r| r.target == ts("web.util.twice")));
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

// req: P4.2
#[test]
fn packs_are_strict_about_fields_and_the_types_they_map_onto() {
    let rust = include_str!("../packs/rust.toml");
    let typo = rust.replacen("separator = \"::\"", "seperator = \"::\"", 1);
    let err = arch_index::Pack::from_toml(&typo).unwrap_err().to_string();
    assert!(err.contains("seperator"), "unknown fields are rejected: {err}");
    let widget = rust.replacen("type = \"interface\"", "type = \"widget\"", 1);
    let mut packs = PackSet::default();
    packs.add(arch_index::Pack::from_toml(&widget).unwrap());
    let err = packs.check_types(&TypeSystem::defaults().unwrap()).unwrap_err().to_string();
    assert!(err.contains("'widget'"), "{err}");
    assert!(PackSet::built_in().unwrap().check_types(&TypeSystem::defaults().unwrap()).is_ok());
}

// req: P4.6
#[test]
fn types_resolve_only_through_scope_and_imports() {
    let packs = PackSet::built_in().unwrap();
    // Python: a parameter typed with a library's `Session` isn't the repo's
    // `client.Session`; one typed with a name that's neither local nor
    // imported proves nothing.
    let root = temp_repo(
        "typeimports",
        &[
            ("client.py", "class Session:\n    def get(self):\n        return 1\n"),
            ("lib_use.py", "from requests import Session\n\n\ndef f(s: Session):\n    s.get()\n"),
            ("own_use.py", "from client import Session\n\n\ndef g(s: Session):\n    s.get()\n"),
            ("loose.py", "def h(s: Session):\n    s.get()\n"),
        ],
    );
    let (_, resolved) = run(&root, &packs);
    let py = |s: &str| ComponentId::for_symbol("python", s);
    let calls = |from: &str| resolved.graph.outgoing(&py(from), Some(RelationKind::Calls)).map(|r| r.target.to_string()).collect::<Vec<_>>();
    assert_eq!(calls("own_use.g"), vec![py("client.Session.get").to_string()], "imported: proven");
    assert!(calls("lib_use.f").is_empty(), "imported from elsewhere: not the repo's");
    assert!(resolved.external.iter().any(|e| e.caller == py("lib_use.f")), "a library call");
    assert!(calls("loose.h").is_empty());
    let loose = resolved.unproven.iter().find(|u| u.caller == py("loose.h")).expect("recorded as unproven");
    assert_eq!(loose.candidates, vec![py("client.Session.get")], "with its candidates");
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
    // Rust: `Store::new()` proves nothing where `Store` isn't in scope; with
    // `use crate::store::Store` (or the full path) it does.
    let root = temp_repo(
        "rustpaths",
        &[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("src/lib.rs", "pub mod store;\npub mod a;\npub mod b;\n"),
            ("src/store.rs", "pub struct Store;\n\nimpl Store {\n    pub fn new() -> Self {\n        Store\n    }\n}\n"),
            ("src/a.rs", "use crate::store::Store;\n\npub fn with_import() {\n    Store::new();\n}\n\npub fn full_path() {\n    crate::store::Store::new();\n}\n"),
            ("src/b.rs", "pub fn without() {\n    Store::new();\n}\n"),
        ],
    );
    let (_, resolved) = run(&root, &packs);
    let rs = |s: &str| ComponentId::for_symbol("rust", s);
    let calls = |from: &str| resolved.graph.outgoing(&rs(from), Some(RelationKind::Calls)).map(|r| r.target.to_string()).collect::<Vec<_>>();
    assert_eq!(calls("app::a::with_import"), vec![rs("app::store::Store::new").to_string()]);
    assert_eq!(calls("app::a::full_path"), vec![rs("app::store::Store::new").to_string()]);
    assert!(calls("app::b::without").is_empty(), "not in scope: not linked");
    assert!(resolved.unproven.iter().any(|u| u.caller == rs("app::b::without") && u.candidates == vec![rs("app::store::Store::new")]));
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

// req: P4.7
#[test]
fn scoped_walks_honor_ignores_from_the_root() {
    let packs = PackSet::built_in().unwrap();
    let root = temp_repo(
        "scoped",
        &[
            (".gitignore", "gen/\n"),
            ("gen/out.py", "def generated():\n    pass\n"),
            ("src/keep.py", "def kept():\n    pass\n"),
            (".github/hook.py", "def hook():\n    pass\n"),
            ("github/real.py", "def real():\n    pass\n"),
        ],
    );
    let files = |scope: &str| arch_index::walk(&root, Path::new(scope), &packs).unwrap().into_iter().map(|f| f.path.to_string()).collect::<Vec<_>>();
    assert_eq!(files("."), vec!["github/real.py", "src/keep.py"]);
    assert!(files("gen").is_empty(), "a git-ignored scope yields nothing, as in a full walk");
    assert!(files(".github").is_empty(), "so does a hidden one");
    assert_eq!(files("github"), vec!["github/real.py"]);
    assert_eq!(files("src/keep.py"), vec!["src/keep.py"], "a single file");
    for (path, ignored) in [("gen/out.py", true), ("gen/new.py", true), (".github/hook.py", true), ("src/keep.py", false), ("src/new.py", false)] {
        assert_eq!(arch_index::ignored(&root, &root.join(path)), ignored, "{path}");
    }
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

// req: M.15
#[test]
fn locals_named_like_functions_and_same_named_nested_functions_dont_mislink() {
    let dir = temp_repo(
        "scopes",
        &[
            ("Cargo.toml", "[package]\nname = \"m\"\n"),
            (
                "src/lib.rs",
                "pub fn helper() {}\npub fn callback() {}\n\
                 pub fn run(callback: impl Fn()) {\n    let helper = || 1;\n    helper();\n    callback();\n}\n\
                 pub fn a() { fn inner() {} inner(); }\npub fn b() { fn inner() {} inner(); }\n\
                 pub fn outer() { fn x() { y(); } fn y() {} x(); }\n\
                 pub fn merge() { observed(); }\n\
                 #[cfg(test)]\nmod tests {\n    use super::*;\n    fn observed() {}\n    #[test]\n    fn t() { observed(); merge(); }\n}\n",
            ),
        ],
    );
    let (_, resolved) = run(&dir, &PackSet::built_in().unwrap());
    let id = |s: &str| ComponentId::for_symbol("rust", s);
    let calls: BTreeSet<(String, String)> = resolved.graph.relations().filter(|r| r.kind == RelationKind::Calls).map(|r| (r.source.to_string(), r.target.to_string())).collect();
    let call = |a: &str, b: &str| (id(a).to_string(), id(b).to_string());
    // A local closure and a parameter aren't the functions they're named like.
    assert!(!calls.contains(&call("m::run", "m::helper")) && !calls.contains(&call("m::run", "m::callback")), "{calls:?}");
    // Each nested function is its own parent's (and a sibling's, from inside).
    assert!(calls.contains(&call("m::a", "m::a::inner")) && calls.contains(&call("m::b", "m::b::inner")));
    assert!(calls.contains(&call("m::outer::x", "m::outer::y")));
    assert!(resolved.ambiguous.is_empty(), "{:?}", resolved.ambiguous);
    // What's in `mod tests` isn't seen outside it by name; from inside, the parent is (use super::*).
    assert!(!calls.contains(&call("m::merge", "m::tests::observed")));
    assert!(calls.contains(&call("m::tests::t", "m::tests::observed")) && calls.contains(&call("m::tests::t", "m::merge")));
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}
