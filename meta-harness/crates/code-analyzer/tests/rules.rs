//! Code rules: forbidden and required ast-grep patterns scoped to component
//! types, checked over the Rust fixture.

use std::path::Path;

use arch_graph::{ComponentId, FindingCode, Graph, Provenance, Severity, TypeSystem};
use code_analyzer::{CodeRules, FileExtract, PackSet, Observed, RuleSeverity};

mod common;

const PROJECT_RULES: &str = r#"
[[rule]]
id = "no-println"
language = "rust"
types = ["function"]
kind = "forbidden"
rule = { kind = "macro_invocation", has = { field = "macro", regex = "^println$" } }
message = "use the logger"

# Modules: only their own code counts, not the functions inside them.
[[rule]]
id = "module-no-println"
language = "rust"
types = ["module"]
kind = "forbidden"
rule = { kind = "macro_invocation", has = { field = "macro", regex = "^println$" } }
message = "no printing at module level"

[[rule]]
id = "handlers-check"
language = "rust"
types = ["handler"]
kind = "required"
rule = { pattern = "self.check()" }
message = "handlers validate before acting"

# Same id as a default: replaces it.
[[rule]]
id = "no-dbg"
language = "rust"
types = ["function"]
kind = "forbidden"
rule = { kind = "macro_invocation", has = { field = "macro", regex = "^dbg$" } }
message = "no dbg!"
severity = "error"
"#;

fn id(s: &str) -> ComponentId {
    ComponentId::for_symbol("rust", s)
}

#[test]
fn code_rules_are_scoped_to_types_and_report_forbidden_and_missing_patterns() {
    let packs = common::shipped_packs();
    let mut types = TypeSystem::defaults().unwrap();
    types.extend_with(toml::from_str("[types.handler]\nextends = \"function\"\ndescription = \"Handles a request.\"\n").unwrap()).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust/sample");
    let dir = tempfile_dir();
    std::fs::write(dir.join("project.toml"), PROJECT_RULES).unwrap();
    let mut rules = CodeRules::defaults(&packs, &types).unwrap();
    let defaults = rules.rules.len();
    rules.load_dir(&dir, &packs, &types).unwrap();
    assert_eq!(rules.rules.len(), defaults + 3, "no-dbg replaced, three added");
    let dbg: Vec<_> = rules.rules.iter().filter(|r| r.spec.id == "no-dbg").collect();
    assert_eq!((dbg.len(), dbg[0].spec.severity), (1, RuleSeverity::Error));
    // The rules match while the files are read, on the same parse.
    let (extracts, resolved, model) = read_all(&root, &packs, &rules, &types);
    assert!(extracts.iter().any(|e| !e.rule_hits.is_empty()) && extracts.iter().all(|e| e.rules_fingerprint == rules.fingerprint("rust")));
    let facts = |file: &arch_graph::RepoPath| model.get(file);
    // The project treats Store's methods as handlers (a specialization of function).
    let mut graph = Graph::default();
    for c in resolved.graph.components() {
        let mut c = c.clone();
        if [id("sample::store::Store::save"), id("sample::store::Store::new")].contains(&c.id) {
            c.type_name = "handler".into();
        }
        graph.upsert_component(c, Provenance::OBSERVED);
    }
    for r in resolved.graph.relations() {
        graph.upsert_relation(r.clone());
    }

    // Checking reads no file: the matches kept with the facts are enough.
    let findings = rules.check(graph.components(), &graph, &facts, &types);
    let got: Vec<(FindingCode, ComponentId, Severity, Option<u32>)> =
        findings.iter().map(|f| (f.code, f.component().unwrap().clone(), f.severity, f.at.as_ref().map(|a| a.span.start_line))).collect();
    assert_eq!(
        got,
        vec![
            // log's println (not reported again on the module that contains log).
            (FindingCode::ForbiddenPattern, id("sample::log"), Severity::Error, Some(30)),
            // new doesn't call self.check(); save does. Plain functions aren't handlers.
            (FindingCode::MissingRequiredPattern, id("sample::store::Store::new"), Severity::Error, Some(6)),
        ]
    );
    assert!(findings[0].message.starts_with("[no-println]") && findings[1].message.starts_with("[handlers-check]"));
    // An anchor from an index that's behind its file isn't judged on code it no longer describes.
    let mut stale = graph.share();
    let mut log = graph.component(&id("sample::log")).unwrap().clone();
    if let Some(anchor) = log.anchor.as_mut() {
        anchor.content_hash = "indexed-before-the-last-edit".into();
    }
    stale.upsert_component(log, Provenance::OBSERVED);
    let findings = rules.check(stale.components(), &stale, &facts, &types);
    assert!(findings.iter().all(|f| f.component() != Some(&id("sample::log"))), "{findings:?}");
    // Facts read with other rules aren't judged by these.
    let other = CodeRules::defaults(&packs, &types).unwrap();
    assert!(other.check(graph.components(), &graph, &facts, &types).is_empty(), "read with other rules: nothing said");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn bad_rules_are_rejected_with_a_reason() {
    let packs = common::shipped_packs();
    let types = TypeSystem::defaults().unwrap();
    let rule = |language: &str, types_list: &str, body: &str| {
        format!("[[rule]]\nid = \"r\"\nlanguage = \"{language}\"\ntypes = [{types_list}]\nkind = \"forbidden\"\nrule = {body}\nmessage = \"m\"\n")
    };
    let cases = [
        (rule("cobol", "\"function\"", "{ pattern = \"x\" }"), "no language pack 'cobol'"),
        (rule("rust", "\"widget\"", "{ pattern = \"x\" }"), "widget"),
        (rule("rust", "", "{ pattern = \"x\" }"), "no types"),
        (rule("rust", "\"function\"", "{ nonsense = 1 }"), "rule 'r'"),
    ];
    for (text, needle) in cases {
        let err = CodeRules::default().add_toml(&text, &packs, &types).unwrap_err().to_string();
        assert!(err.contains(needle), "{err} should mention {needle}");
    }
}

fn tempfile_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("nah-rules-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every file under `root` read with its pack, then linked.
fn read_all(root: &Path, packs: &PackSet, rules: &code_analyzer::CodeRules, types: &TypeSystem) -> (Vec<FileExtract>, Observed, code_analyzer::Model) {
    let extracts: Vec<FileExtract> = code_analyzer::walk(root, root, packs)
        .unwrap()
        .into_iter()
        .filter_map(|f| {
            let pack = packs.for_path(&f.absolute)?;
            Some(code_analyzer::extract_file(pack, rules, f.path, f.module, &std::fs::read_to_string(&f.absolute).unwrap()))
        })
        .collect();
    let read = extracts
        .iter()
        .map(|e| {
            let version = code_analyzer::rows::read_version(&e.pack_fingerprint, &e.rules_fingerprint);
            let file = code_analyzer::rows::FileRow { path: e.file.clone(), language: e.language.clone(), size: 0, modified: 0, content_hash: e.content_hash.clone(), read_version: version };
            (file, e.clone())
        })
        .collect();
    let mut model = code_analyzer::Model::default();
    model.apply(packs, types, code_analyzer::Change { read, ..code_analyzer::Change::default() });
    let observed = code_analyzer::observed(model.files(), packs);
    (extracts, observed, model)
}
