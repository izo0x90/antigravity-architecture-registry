//! Attempts to get past the verifier on a temp copy of the Rust fixture:
//! the plan says `validate(input: &str) -> bool` must call `Store::new`;
//! the user approves; the "agent" writes code that shouldn't pass.

use std::path::{Path, PathBuf};

use arch_graph::{ComponentId, FindingCode, Param, PlanId, RelationKind, Severity, Signature, Stage, TypeSystem};
use arch_index::{CodeRules, PackSet};
use arch_service::{Actor, ArchitectureService, FsIndex, FsPlans, PlanOp};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t)
        } else {
            std::fs::copy(e.path(), t).unwrap();
        }
    }
}

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    s: ArchitectureService,
}

fn repo() -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../arch-index/tests/fixtures/rust/sample"), &root);
    let (packs, types) = (PackSet::built_in().unwrap(), TypeSystem::defaults().unwrap());
    let rules = CodeRules::defaults(&packs, &types).unwrap();
    let s = ArchitectureService::open(&root, packs, types, Box::new(FsPlans::new(&root).unwrap()), Box::new(FsIndex::new(&root).unwrap()))
        .unwrap()
        .with_rules(rules);
    s.ingest(&root).unwrap();
    Repo { _dir: dir, root, s }
}

fn id(s: &str) -> ComponentId {
    ComponentId::for_symbol("rust", s)
}

fn pid() -> PlanId {
    PlanId::new("p").unwrap()
}

fn user() -> Actor {
    Actor::User { name: "izo".into() }
}

fn add(r: &Repo, name: &str) {
    let sig = Signature { params: vec![Param { name: "input".into(), ty: Some("&str".into()), optional: false, variadic: false }], returns: Some("bool".into()), qualifiers: vec![] };
    r.s.apply(&pid(), PlanOp::AddComponent { name: name.into(), type_name: "function".into(), description: format!("{name} input."), parent: Some(id("sample")), file: None, signature: Some(sig), schemas: None, spec: None })
        .unwrap();
}

fn relate(r: &Repo, a: &str, b: &str) {
    r.s.apply(&pid(), PlanOp::Relate { kind: RelationKind::Calls, source: id(a), target: id(b), order: None, ports: None, note: None, expects: None }).unwrap();
}

fn append(r: &Repo, file: &str, text: &str) {
    let mut c = std::fs::read_to_string(r.root.join(file)).unwrap();
    c.push_str(text);
    std::fs::write(r.root.join(file), c).unwrap();
}

/// The plan, approved by the user: validate calls Store::new.
fn approved() -> Repo {
    let r = repo();
    r.s.create_plan("p", "P", "").unwrap();
    add(&r, "validate");
    relate(&r, "sample::validate", "sample::store::Store::new");
    r.s.approve(&pid(), &id("sample::validate"), Stage::ArchApproved, &user()).unwrap();
    r.s.approve(&pid(), &id("sample::validate"), Stage::PlanApproved, &user()).unwrap();
    r
}

/// The error codes verifying validate to Implemented reports, after writing
/// `lib` into lib.rs and `extra` into other files.
fn attempt(lib: &str, extra: &[(&str, &str)]) -> Vec<FindingCode> {
    let r = approved();
    append(&r, "src/lib.rs", lib);
    for (file, text) in extra {
        append(&r, file, text);
    }
    r.s.ingest(&r.root).unwrap();
    let v = r.s.verify(&pid(), &id("sample::validate"), Stage::Implemented).unwrap();
    let errors: Vec<FindingCode> = v.report.findings.iter().filter(|f| f.severity == Severity::Error).map(|f| f.code).collect();
    assert_eq!(v.advanced_to.is_some(), errors.is_empty(), "{:?}", v.report);
    errors
}

fn body(code: &str) -> String {
    format!("\npub fn validate(input: &str) -> bool {{\n{code}\n}}\n")
}

const CALLS: &str = "    crate::store::Store::new();";

// req: P6.10
#[test]
fn placeholders_are_found_however_they_are_spelled() {
    assert!(attempt(&body(&format!("{CALLS}\n    !input.is_empty()")), &[]).is_empty(), "the honest version passes");
    let caught = |name: &str, lib: String, extra: &[(&str, &str)]| {
        let errors = attempt(&lib, extra);
        assert!(errors.contains(&FindingCode::PlaceholderRemains), "{name}: {errors:?}");
    };
    caught("path", body(&format!("{CALLS}\n    std::todo!()")), &[]);
    caught("use alias", format!("\nuse std::todo as later;{}", body(&format!("{CALLS}\n    later!()"))), &[]);
    caught("macro alias to a panic", format!("\nmacro_rules! nyi {{ () => {{ panic!(\"not implemented\") }} }}{}", body(&format!("{CALLS}\n    nyi!()"))), &[]);
    caught("nested macro aliases", format!("\nmacro_rules! aa {{ () => {{ todo!() }} }}\nmacro_rules! bb {{ () => {{ aa!() }} }}{}", body(&format!("{CALLS}\n    bb!()"))), &[]);
    caught("exported alias in another file", body(&format!("{CALLS}\n    crate::wip!()")), &[("src/util.rs", "\n#[macro_export]\nmacro_rules! wip { () => { todo!() } }\n")]);
    caught("ends by panicking", body(&format!("{CALLS}\n    panic!(\"unfinished\")")), &[]);
    caught("ends with a bare panic", body(&format!("{CALLS}\n    panic!();")), &[]);
    caught("helper through a fn pointer", format!("\nfn stubbed(_: &str) -> bool {{ todo!() }}{}", body(&format!("{CALLS}\n    let f: fn(&str) -> bool = stubbed;\n    f(input)"))), &[]);
    caught("helper two hops away", format!("\nfn h1(i: &str) -> bool {{ h2(i) }}\nfn h2(_: &str) -> bool {{ unimplemented!() }}{}", body(&format!("{CALLS}\n    h1(input)"))), &[]);
    // A function meant to diverge isn't a placeholder; neither is unreachable!().
    let r = repo();
    append(&r, "src/util.rs", "\npub fn fail() -> ! {\n    panic!(\"boom\")\n}\n\npub fn never(x: u8) -> u8 {\n    match x {\n        0 => 1,\n        _ => unreachable!(),\n    }\n}\n");
    r.s.ingest(&r.root).unwrap();
    let graph = r.s.observed().unwrap();
    for f in ["sample::util::fail", "sample::util::never"] {
        assert!(graph.component(&id(f)).unwrap().placeholders.is_empty(), "{f}");
    }
}

// req: P6.2
#[test]
fn calls_in_code_that_never_runs_dont_count() {
    let missing = |name: &str, code: &str| {
        let errors = attempt(&body(&format!("{code}\n    !input.is_empty()")), &[]);
        assert!(errors.contains(&FindingCode::MissingCallSite), "{name}: {errors:?}");
    };
    missing("if false", "    if false { crate::store::Store::new(); }");
    missing("if (false)", "    if (false) { crate::store::Store::new(); }");
    missing("if !true", "    if !true { crate::store::Store::new(); }");
    missing("while false", "    while false { crate::store::Store::new(); }");
    missing("if cfg!(any())", "    if cfg!(any()) { crate::store::Store::new(); }");
    missing("cfg(any())", "    #[cfg(any())]\n    crate::store::Store::new();");
    missing("cfg( any() )", "    #[cfg( any() )]\n    crate::store::Store::new();");
    missing("cfg(not(all()))", "    #[cfg(not(all()))]\n    crate::store::Store::new();");
    missing("cfg(all(any()))", "    #[cfg(all(any()))]\n    { crate::store::Store::new(); }");
    let after_return = "\npub fn validate(input: &str) -> bool {\n    return !input.is_empty();\n    #[allow(unreachable_code)]\n    crate::store::Store::new();\n}\n";
    assert!(attempt(after_return, &[]).contains(&FindingCode::MissingCallSite), "after return");
    // Conditions that may hold at runtime count as calls.
    for live in ["    if input.len() > 3 { crate::store::Store::new(); }", "    #[cfg(not(any()))]\n    crate::store::Store::new();", "    if true || input.is_empty() { crate::store::Store::new(); }"] {
        assert!(attempt(&body(&format!("{live}\n    !input.is_empty()")), &[]).is_empty(), "{live}");
    }
}

// req: P6.3
#[test]
fn signature_qualifiers_include_the_abi() {
    let errors = attempt(&format!("\npub extern \"C\" fn validate(input: &str) -> bool {{\n{CALLS}\n    !input.is_empty()\n}}\n"), &[]);
    assert_eq!(errors, vec![FindingCode::SignatureMismatch]);
}

// req: P6.6
#[test]
fn a_design_change_revokes_the_plan_approval_of_what_builds_on_it() {
    let r = repo();
    r.s.create_plan("p", "P", "").unwrap();
    add(&r, "check_all");
    add(&r, "validate");
    relate(&r, "sample::validate", "sample::check_all");
    relate(&r, "sample::check_all", "sample::store::Store::new");
    for c in ["sample::check_all", "sample::validate"] {
        r.s.approve(&pid(), &id(c), Stage::ArchApproved, &user()).unwrap();
        r.s.approve(&pid(), &id(c), Stage::PlanApproved, &user()).unwrap();
    }
    // The agent changes check_all's design: it and what calls it fall back.
    r.s.apply(&pid(), PlanOp::Modify { id: id("sample::check_all"), description: Some("changed".into()), signature: None, schemas: None, spec: None }).unwrap();
    let plan = r.s.plan(&pid()).unwrap();
    assert!(!plan.plan_approved(&id("sample::validate")) && !plan.plan_approved(&id("sample::check_all")));
    let revoked: Vec<&arch_graph::Approval> = plan.approvals.iter().filter(|a| a.kind == arch_graph::ApprovalKind::Revoked).collect();
    assert_eq!(revoked.len(), 2, "both recorded: {revoked:?}");
    assert!(revoked.iter().all(|a| a.by == arch_graph::REVOKED_BY));
    // The user re-approves only check_all; validate can't be implemented on its old approval.
    r.s.approve(&pid(), &id("sample::check_all"), Stage::ArchApproved, &user()).unwrap();
    r.s.approve(&pid(), &id("sample::check_all"), Stage::PlanApproved, &user()).unwrap();
    append(&r, "src/lib.rs", "\npub fn validate(input: &str) -> bool {\n    check_all(input)\n}\n\npub fn check_all(input: &str) -> bool {\n    crate::store::Store::new();\n    !input.is_empty()\n}\n");
    assert_eq!(r.s.verify(&pid(), &id("sample::check_all"), Stage::Implemented).unwrap().advanced_to, Some(Stage::Implemented));
    assert_eq!(r.s.verify(&pid(), &id("sample::validate"), Stage::Skeleton).unwrap().advanced_to, Some(Stage::Skeleton));
    let v = r.s.verify(&pid(), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, None);
    assert!(v.report.findings.iter().any(|f| f.code == FindingCode::ApprovalRequired), "{:?}", v.report);
    // Once the user approves its plan again, it can.
    r.s.approve(&pid(), &id("sample::validate"), Stage::PlanApproved, &user()).unwrap();
    assert_eq!(r.s.verify(&pid(), &id("sample::validate"), Stage::Implemented).unwrap().advanced_to, Some(Stage::Implemented));
    // Verifying code that broke takes Implemented away; its plan approval stands.
    let lib = r.root.join("src/lib.rs");
    std::fs::write(&lib, std::fs::read_to_string(&lib).unwrap().replace("    check_all(input)\n", "    todo!()\n")).unwrap();
    let v = r.s.verify(&pid(), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, None);
    let plan = r.s.plan(&pid()).unwrap();
    assert_eq!(plan.component(&id("sample::validate")).unwrap().stage, Stage::Skeleton);
    assert!(plan.plan_approved(&id("sample::validate")), "code falling back doesn't revoke the design");
}

// req: P6.8
#[test]
fn helpers_regressing_after_implemented_are_caught_on_change() {
    let r = approved();
    append(&r, "src/util.rs", "\npub fn real_helper(i: &str) -> bool {\n    !i.is_empty()\n}\n");
    append(&r, "src/lib.rs", &body(&format!("{CALLS}\n    crate::util::real_helper(input)")));
    r.s.ingest(&r.root).unwrap();
    assert_eq!(r.s.verify(&pid(), &id("sample::validate"), Stage::Implemented).unwrap().advanced_to, Some(Stage::Implemented));
    // The unplanned helper it leans on goes back to a placeholder.
    let util = r.root.join("src/util.rs");
    std::fs::write(&util, std::fs::read_to_string(&util).unwrap().replace("    !i.is_empty()\n", "    todo!()\n")).unwrap();
    let refresh = r.s.refresh(std::slice::from_ref(&util)).unwrap();
    let validate = refresh.reverified.iter().find(|x| x.component == id("sample::validate")).expect("re-verified on the helper's change");
    assert_eq!(validate.demoted_to, Some(Stage::Skeleton));
    assert!(validate.report.findings.iter().any(|f| f.code == FindingCode::PlaceholderRemains && f.message.contains("real_helper")));
    // Verifying without a refresh re-reads the helper too.
    std::fs::write(&util, std::fs::read_to_string(&util).unwrap().replace("    todo!()\n", "    !i.is_empty()\n")).unwrap();
    let v = r.s.verify(&pid(), &id("sample::validate"), Stage::Implemented).unwrap();
    assert_eq!(v.advanced_to, Some(Stage::Implemented), "{:?}", v.report);
    std::fs::write(&util, std::fs::read_to_string(&util).unwrap().replace("    !i.is_empty()\n", "    todo!()\n")).unwrap();
    let v = r.s.verify(&pid(), &id("sample::validate"), Stage::Implemented).unwrap();
    assert!(v.report.findings.iter().any(|f| f.code == FindingCode::PlaceholderRemains), "{:?}", v.report);
}
