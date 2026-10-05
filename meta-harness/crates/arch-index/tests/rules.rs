//! Code rules: forbidden and required ast-grep patterns scoped to component
//! types, checked over the Rust fixture.

use std::path::Path;

use arch_graph::{ComponentId, FindingCode, Graph, Provenance, Severity, TypeSystem};
use arch_index::{CodeRules, PackSet, RuleSeverity, ingest};

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

// req: P6.12
#[test]
fn code_rules_are_scoped_to_types_and_report_forbidden_and_missing_patterns() {
    let packs = PackSet::built_in().unwrap();
    let mut types = TypeSystem::defaults().unwrap();
    types.extend_with(toml::from_str("[types.handler]\nextends = \"function\"\ndescription = \"Handles a request.\"\n").unwrap()).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust/sample");
    let (_, resolved) = ingest(&root, Path::new("."), &packs, &types).unwrap();
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

    let dir = tempfile_dir();
    std::fs::write(dir.join("project.toml"), PROJECT_RULES).unwrap();
    let mut rules = CodeRules::defaults(&packs, &types).unwrap();
    let defaults = rules.rules.len();
    rules.load_dir(&dir, &packs, &types).unwrap();
    assert_eq!(rules.rules.len(), defaults + 3, "no-dbg replaced, three added");
    let dbg: Vec<_> = rules.rules.iter().filter(|r| r.spec.id == "no-dbg").collect();
    assert_eq!((dbg.len(), dbg[0].spec.severity), (1, RuleSeverity::Error));

    let findings = rules.check(graph.components(), &graph, &root, &packs, &types);
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
    std::fs::remove_dir_all(&dir).unwrap();
}

// req: P6.12
#[test]
fn bad_rules_are_rejected_with_a_reason() {
    let packs = PackSet::built_in().unwrap();
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
