//! Gaps the final audit found, on small repos written per test: methods
//! verify with their receivers, the found symbol's kind must match the plan,
//! Python and TypeScript placeholder bodies and dead code are caught,
//! ignored or hidden files can't be planned into, component names are
//! single identifiers, loops order after what they depend on, and scoped
//! re-reads follow renamed directories and honor ignores.

use std::path::PathBuf;

use arch_graph::{Component, ComponentId, FindingCode, Param, Plan, PlanId, Provenance, ProposedLocation, Relation, RelationKind, RepoPath, Schemas, Signature, Stage, Status, TypeSystem};
use arch_index::PackSet;
use arch_service::{Actor, ArchitectureService, FsPlans, PlanOp, PlanRepository};

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    s: ArchitectureService,
}

fn repo(files: &[(&str, &str)]) -> Repo {
    repo_typed(files, TypeSystem::defaults().unwrap())
}

fn repo_typed(files: &[(&str, &str)], types: TypeSystem) -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("app");
    for (path, text) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let s = ArchitectureService::open(
        &root,
        PackSet::built_in().unwrap(),
        types,
        Box::new(FsPlans::new(&root).unwrap()),
        Box::new(arch_service::open_facts(&root, &arch_service::Limits::defaults().unwrap()).unwrap()),
    )
    .unwrap();
    s.ingest(&root).unwrap();
    Repo { _dir: dir, root, s }
}

fn pid() -> PlanId {
    PlanId::new("p").unwrap()
}

fn user() -> Actor {
    Actor::User { name: "izo".into() }
}

fn param(name: &str, ty: &str) -> Param {
    Param { name: name.into(), ty: Some(ty.into()), optional: false, variadic: false }
}

fn sig(params: Vec<Param>, returns: &str) -> Option<Signature> {
    Some(Signature { params, returns: Some(returns.into()), qualifiers: vec![] })
}

fn add(r: &Repo, name: &str, type_name: &str, parent: &ComponentId, signature: Option<Signature>) -> Result<Plan, arch_service::ServiceError> {
    r.s.apply(
        &pid(),
        PlanOp::AddComponent { name: name.into(), type_name: type_name.into(), description: format!("{name} does it."), parent: Some(parent.clone()), file: None, signature, schemas: None, spec: None },
    )
}

fn relate(r: &Repo, a: &ComponentId, b: &ComponentId) {
    r.s.apply(&pid(), PlanOp::Relate { kind: RelationKind::Calls, source: a.clone(), target: b.clone(), order: None, ports: None, note: None, expects: None }).unwrap();
}

fn approve(r: &Repo, id: &ComponentId) {
    r.s.approve(&pid(), id, Stage::ArchApproved, &user()).unwrap();
    r.s.approve(&pid(), id, Stage::PlanApproved, &user()).unwrap();
}

fn write(r: &Repo, file: &str, text: &str) {
    std::fs::write(r.root.join(file), text).unwrap();
}

/// Error codes from verifying `id` to Implemented.
fn verify(r: &Repo, id: &ComponentId) -> (Option<Stage>, Vec<FindingCode>) {
    let v = r.s.verify(&pid(), id, Stage::Implemented).unwrap();
    (v.advanced_to, v.report.findings.iter().filter(|f| f.severity == arch_graph::Severity::Error).map(|f| f.code).collect())
}

// req: P6.3
#[test]
fn methods_verify_without_their_receiver_in_the_plan() {
    // Rust: planned `flush(n: usize) -> usize`, written with `&self`.
    let r = repo(&[("Cargo.toml", "[package]\nname = \"app\"\n"), ("src/lib.rs", "pub struct Store;\n\nimpl Store {\n    fn check(&self) {}\n}\n")]);
    let rs = |s: &str| ComponentId::for_symbol("rust", s);
    r.s.create_plan("p", "P", "").unwrap();
    add(&r, "flush", "function", &rs("app::Store"), sig(vec![param("n", "usize")], "usize")).unwrap();
    relate(&r, &rs("app::Store::flush"), &rs("app::Store::check"));
    approve(&r, &rs("app::Store::flush"));
    write(&r, "src/lib.rs", "pub struct Store;\n\nimpl Store {\n    fn check(&self) {}\n\n    pub fn flush(&self, n: usize) -> usize {\n        self.check();\n        n\n    }\n}\n");
    assert_eq!(verify(&r, &rs("app::Store::flush")), (Some(Stage::Implemented), vec![]));
    // A real difference after the receiver still shows, with a diff.
    write(&r, "src/lib.rs", "pub struct Store;\n\nimpl Store {\n    fn check(&self) {}\n\n    pub fn flush(&mut self, n: u8) -> usize {\n        self.check();\n        n as usize\n    }\n}\n");
    let v = r.s.verify(&pid(), &rs("app::Store::flush"), Stage::Implemented).unwrap();
    let mismatch = v.report.findings.iter().find(|f| f.code == FindingCode::SignatureMismatch).expect("type differs");
    assert!(mismatch.message.contains("parameter 'n': planned type usize, code has u8"), "{}", mismatch.message);
    // Python: planned `flush(n: int) -> int`, written with `self`.
    let r = repo(&[("counter.py", "class Counter:\n    def check(self):\n        return 1\n")]);
    let py = |s: &str| ComponentId::for_symbol("python", s);
    r.s.create_plan("p", "P", "").unwrap();
    add(&r, "flush", "function", &py("counter.Counter"), sig(vec![param("n", "int")], "int")).unwrap();
    relate(&r, &py("counter.Counter.flush"), &py("counter.Counter.check"));
    approve(&r, &py("counter.Counter.flush"));
    write(&r, "counter.py", "class Counter:\n    def check(self):\n        return 1\n\n    def flush(self, n: int) -> int:\n        self.check()\n        return n\n");
    assert_eq!(verify(&r, &py("counter.Counter.flush")), (Some(Stage::Implemented), vec![]));
}

// req: P6.1
#[test]
fn the_symbol_found_must_be_the_kind_planned() {
    let r = repo(&[("Cargo.toml", "[package]\nname = \"app\"\n"), ("src/lib.rs", "pub fn run() {}\n")]);
    let rs = |s: &str| ComponentId::for_symbol("rust", s);
    r.s.create_plan("p", "P", "").unwrap();
    r.s.apply(&pid(), PlanOp::SetGreenfield { greenfield: true }).unwrap();
    add(&r, "Saver", "interface", &rs("app"), None).unwrap();
    approve(&r, &rs("app::Saver"));
    write(&r, "src/lib.rs", "pub fn run() {}\n\npub struct Saver;\n");
    let (advanced, codes) = verify(&r, &rs("app::Saver"));
    assert_eq!((advanced, codes), (None, vec![FindingCode::WrongType]), "a struct where an interface was planned");
    write(&r, "src/lib.rs", "pub fn run() {}\n\npub trait Saver {}\n");
    assert_eq!(verify(&r, &rs("app::Saver")).0, Some(Stage::Implemented));
    // A project's specialized type is checked as the base the code can show.
    let mut types = TypeSystem::defaults().unwrap();
    types.extend_from_toml("[types.handler]\nextends = \"function\"").unwrap();
    let r = repo_typed(&[("Cargo.toml", "[package]\nname = \"app\"\n"), ("src/lib.rs", "pub fn run() {}\n")], types);
    r.s.create_plan("p", "P", "").unwrap();
    r.s.apply(&pid(), PlanOp::SetGreenfield { greenfield: true }).unwrap();
    add(&r, "on_event", "handler", &rs("app"), sig(vec![], "bool")).unwrap();
    approve(&r, &rs("app::on_event"));
    write(&r, "src/lib.rs", "pub fn run() {}\n\npub fn on_event() -> bool {\n    true\n}\n");
    assert_eq!(verify(&r, &rs("app::on_event")), (Some(Stage::Implemented), vec![]), "a handler written as a function");
    // A type the language's pack can't tell apart (a Python interface is a class) isn't judged.
    let r = repo(&[("ports.py", "def run():\n    return 1\n")]);
    let py = |s: &str| ComponentId::for_symbol("python", s);
    r.s.create_plan("p", "P", "").unwrap();
    r.s.apply(&pid(), PlanOp::SetGreenfield { greenfield: true }).unwrap();
    add(&r, "Saver", "interface", &py("ports"), None).unwrap();
    approve(&r, &py("ports.Saver"));
    write(&r, "ports.py", "from typing import Protocol\n\ndef run():\n    return 1\n\nclass Saver(Protocol):\n    pass\n");
    assert_eq!(verify(&r, &py("ports.Saver")).0, Some(Stage::Implemented));
}

/// A planned Python function `run(text: str) -> str` that must call `clean`.
fn python_run_calls_clean() -> Repo {
    let r = repo(&[("helpers.py", "def clean(text: str) -> str:\n    return text.strip()\n"), ("app.py", "from helpers import clean\n")]);
    let py = |s: &str| ComponentId::for_symbol("python", s);
    r.s.create_plan("p", "P", "").unwrap();
    add(&r, "run", "function", &py("app"), sig(vec![param("text", "str")], "str")).unwrap();
    relate(&r, &py("app.run"), &py("helpers.clean"));
    approve(&r, &py("app.run"));
    r
}

// req: P6.10, P6.2
#[test]
fn python_and_typescript_stubs_and_dead_calls_dont_pass() {
    let py = |s: &str| ComponentId::for_symbol("python", s);
    let head = "from helpers import clean\n\n\n";
    for (case, body) in [
        ("ellipsis", "def run(text: str) -> str:\n    clean(text)\n    ...\n"),
        ("docstring then ellipsis", "def run(text: str) -> str:\n    \"\"\"Runs.\"\"\"\n    clean(text)\n    ...\n"),
    ] {
        let r = python_run_calls_clean();
        write(&r, "app.py", &format!("{head}{body}"));
        assert_eq!(verify(&r, &py("app.run")), (None, vec![FindingCode::PlaceholderRemains]), "{case}");
        assert_eq!(r.s.verify(&pid(), &py("app.run"), Stage::Skeleton).unwrap().advanced_to, Some(Stage::Skeleton), "{case}: a skeleton is fine");
    }
    let r = python_run_calls_clean();
    write(&r, "app.py", &format!("{head}def run(text: str) -> str:\n    pass\n"));
    assert!(verify(&r, &py("app.run")).1.contains(&FindingCode::PlaceholderRemains), "pass where a value is promised");
    for (case, body) in [
        ("after return", "def run(text: str) -> str:\n    return text\n    clean(text)\n"),
        ("while False", "def run(text: str) -> str:\n    while False:\n        clean(text)\n    return text\n"),
        ("if not True", "def run(text: str) -> str:\n    if not True:\n        clean(text)\n    return text\n"),
        ("if False or False", "def run(text: str) -> str:\n    if False or False:\n        clean(text)\n    return text\n"),
        ("after raise", "def run(text: str) -> str:\n    raise ValueError(text)\n    clean(text)\n"),
        ("while 0", "def run(text: str) -> str:\n    while 0:\n        clean(text)\n    return text\n"),
    ] {
        let r = python_run_calls_clean();
        write(&r, "app.py", &format!("{head}{body}"));
        assert_eq!(verify(&r, &py("app.run")), (None, vec![FindingCode::MissingCallSite]), "{case}");
    }
    let r = python_run_calls_clean();
    write(&r, "app.py", &format!("{head}def run(text: str) -> str:\n    if text:\n        return clean(text)\n    return text\n"));
    assert_eq!(verify(&r, &py("app.run")), (Some(Stage::Implemented), vec![]), "a runtime condition counts as live");
    // Interface stubs aren't placeholders.
    let r = repo(&[("proto.py", "from typing import Protocol\n\n\nclass Sink(Protocol):\n    def put(self, x: int) -> int:\n        ...\n")]);
    assert!(r.s.observed().unwrap().component(&py("proto.Sink.put")).unwrap().placeholders.is_empty());

    // TypeScript.
    let ts = |s: &str| ComponentId::for_symbol("typescript", s);
    for (case, body, ok) in [
        ("while (false)", "export function run(text: string): string {\n  while (false) {\n    clean(text);\n  }\n  return text;\n}\n", false),
        ("after return", "export function run(text: string): string {\n  return text;\n  clean(text);\n}\n", false),
        ("if (!true)", "export function run(text: string): string {\n  if (!true) {\n    clean(text);\n  }\n  return text;\n}\n", false),
        ("live", "export function run(text: string): string {\n  return clean(text);\n}\n", true),
    ] {
        let r = repo(&[("helpers.ts", "export function clean(text: string): string {\n  return text.trim();\n}\n"), ("app.ts", "import { clean } from \"./helpers\";\n")]);
        r.s.create_plan("p", "P", "").unwrap();
        add(&r, "run", "function", &ts("app"), sig(vec![param("text", "string")], "string")).unwrap();
        relate(&r, &ts("app.run"), &ts("helpers.clean"));
        approve(&r, &ts("app.run"));
        write(&r, "app.ts", &format!("import {{ clean }} from \"./helpers\";\n\n{body}"));
        let (advanced, codes) = verify(&r, &ts("app.run"));
        assert_eq!(advanced.is_some(), ok, "{case}: {codes:?}");
        if !ok {
            assert_eq!(codes, vec![FindingCode::MissingCallSite], "{case}");
        }
    }
}

fn rust_repo() -> Repo {
    repo(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        (".gitignore", "gen/\n"),
        ("src/lib.rs", "pub mod feat;\n\npub fn run() {}\n"),
        ("src/feat/mod.rs", "pub fn alpha() {}\n"),
        ("gen/out.rs", "pub fn generated() {}\n"),
    ])
}

// req: P5.7
#[test]
fn ignored_or_hidden_files_cant_be_planned_into() {
    let r = rust_repo();
    std::fs::create_dir_all(r.root.join(".hidden")).unwrap();
    let rs = |s: &str| ComponentId::for_symbol("rust", s);
    r.s.create_plan("p", "P", "").unwrap();
    r.s.apply(&pid(), PlanOp::SetGreenfield { greenfield: true }).unwrap();
    for file in ["gen/x.rs", ".hidden/y.rs"] {
        let err = r.s
            .apply(&pid(), PlanOp::AddComponent { name: "z".into(), type_name: "module".into(), description: "Z.".into(), parent: None, file: Some(file.into()), signature: None, schemas: None, spec: None })
            .unwrap_err()
            .to_string();
        assert!(err.contains("git-ignored or hidden"), "{file}: {err}");
    }
    // A plan written by hand gets the finding.
    let mut plan = r.s.plan(&pid()).unwrap();
    plan.components.push(Component {
        id: rs("app::gen_x"),
        name: "gen_x".into(),
        type_name: "function".into(),
        description: "Generated.".into(),
        status: Status::New,
        stage: Stage::Declared,
        anchor: None,
        proposed: Some(ProposedLocation { language: "rust".into(), file: RepoPath::new("gen/x.rs").unwrap(), parent: None, symbol: "app::gen_x".into() }),
        signature: Some(Signature::default()),
        schemas: Schemas::default(),
        spec: None,
        placeholders: vec![],
    });
    FsPlans::new(&r.root).unwrap().save(&plan).unwrap();
    let report = r.s.validate(&pid()).unwrap();
    assert!(report.findings.iter().any(|f| f.code == FindingCode::InvalidLocation && f.message.contains("gen/x.rs is git-ignored or hidden")), "{report:?}");
}

// req: P5.1
#[test]
fn component_names_are_single_identifiers_of_the_language() {
    let r = rust_repo();
    let rs = |s: &str| ComponentId::for_symbol("rust", s);
    r.s.create_plan("p", "P", "").unwrap();
    for bad in ["Store::extra", "a.b", "1abc", "fn", "two words", ""] {
        let err = add(&r, bad, "function", &rs("app"), Some(Signature::default())).map(|_| ()).unwrap_err().to_string();
        assert!(err.contains("component name") || err.contains("single identifier"), "{bad:?}: {err}");
    }
    add(&r, "fine_name", "function", &rs("app"), Some(Signature::default())).unwrap();
    // Python's keywords are its own.
    let r = repo(&[("m.py", "def f():\n    return 1\n")]);
    r.s.create_plan("p", "P", "").unwrap();
    let py = |s: &str| ComponentId::for_symbol("python", s);
    assert!(add(&r, "lambda", "function", &py("m"), Some(Signature::default())).is_err());
    add(&r, "fn", "function", &py("m"), Some(Signature::default())).unwrap();
}

// req: P5.10
#[test]
fn loops_come_after_what_they_depend_on_and_before_their_dependents() {
    let r = rust_repo();
    let rs = |s: &str| ComponentId::for_symbol("rust", s);
    r.s.create_plan("p", "P", "").unwrap();
    // a calls x; x and y call each other; y calls z.
    for name in ["a", "x", "y", "z"] {
        add(&r, name, "function", &rs("app"), Some(Signature::default())).unwrap();
    }
    for (from, to) in [("a", "x"), ("x", "y"), ("y", "x"), ("y", "z")] {
        relate(&r, &rs(&format!("app::{from}")), &rs(&format!("app::{to}")));
    }
    let plan = r.s.plan(&pid()).unwrap();
    let graph = r.s.graph(&arch_graph::Source::Plan(pid())).unwrap();
    let order: Vec<String> = arch_service::topological(&plan, &graph, r.s.types()).into_iter().map(|i| i.0).collect();
    assert_eq!(order, vec!["rust:app::z", "rust:app::x", "rust:app::y", "rust:app::a"]);
    let mut lone = Plan::new("lone", "Lone");
    lone.components.push(plan.component(&rs("app::a")).unwrap().clone());
    lone.relations.push(Relation::new(RelationKind::Calls, rs("app::a"), rs("app::a"), Provenance::PLANNED));
    assert_eq!(arch_service::topological(&lone, &arch_graph::Graph::merge(arch_graph::Graph::new(), std::slice::from_ref(&lone)), r.s.types()).len(), 1);
}

// req: P4.7
#[test]
fn scoped_rereads_follow_renamed_directories_and_honor_ignores() {
    let r = rust_repo();
    let rs = |s: &str| ComponentId::for_symbol("rust", s);
    let has = |id: &str| r.s.observed().unwrap().component(&rs(id)).is_some();
    assert!(has("app::feat::alpha") && !has("app::generated"), "the ignored dir isn't read");
    // A directory renamed: the watcher reports both paths.
    std::fs::rename(r.root.join("src/feat"), r.root.join("src/feat2")).unwrap();
    write(&r, "src/lib.rs", "pub mod feat2;\n\npub fn run() {}\n");
    let refresh = r.s.refresh(&[r.root.join("src/feat"), r.root.join("src/feat2"), r.root.join("src/lib.rs")]).unwrap();
    assert!(refresh.ingest.changed.iter().any(|p| p.as_str() == "src/feat/mod.rs"), "the old files leave: {:?}", refresh.ingest.changed);
    assert!(!has("app::feat::alpha") && has("app::feat2::alpha"));
    // Scoped ingest reads from the root: ignores apply, hidden dirs stay out,
    // and a dir named like a hidden one isn't confused with it.
    std::fs::create_dir_all(r.root.join(".github")).unwrap();
    std::fs::create_dir_all(r.root.join("github")).unwrap();
    write(&r, ".github/hook.py", "def hook():\n    pass\n");
    write(&r, "github/real.py", "def real():\n    pass\n");
    r.s.ingest(&r.root.join("github")).unwrap();
    let py = |s: &str| ComponentId::for_symbol("python", s);
    assert!(r.s.observed().unwrap().component(&py("github.real")).is_some());
    let report = r.s.ingest(&r.root.join(".github")).unwrap();
    assert_eq!((report.files, report.removed), (0, 0), "a hidden scope reads nothing and drops nothing");
    assert!(r.s.observed().unwrap().component(&py("github.real")).is_some(), "github/ is untouched");
    let report = r.s.ingest(&r.root.join("gen")).unwrap();
    assert_eq!(report.files, 0, "an ignored scope reads nothing");
    assert!(!has("app::generated"));
}
