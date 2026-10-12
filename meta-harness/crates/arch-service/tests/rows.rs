//! The store's rows and per-file linking: the reader's output is rows; a change re-links only what it can
//! affect, and gives the same result as reading everything again; links
//! into a changed file stay; unresolved refs keep why; one writer.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arch_graph::TypeSystem;
use arch_service::{ArchitectureService, MemoryPlans};

mod common;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn open(root: &Path) -> ArchitectureService {
    let facts = arch_service::open_facts(root, &arch_service::Limits::defaults().unwrap()).unwrap();
    ArchitectureService::open(root, common::shipped_packs(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), facts).unwrap()
}

/// Every fixture language in one project.
fn fixtures(dir: &Path) -> PathBuf {
    let root = dir.join("proj");
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures");
    for (language, folder) in [("rust", "sample"), ("python", "app"), ("typescript", "app")] {
        copy_dir(&from.join(language).join(folder), &root.join(language));
    }
    root
}

/// The graph as comparable text: every component and relation, and every
/// call that wasn't linked, with why.
fn result(service: &ArchitectureService) -> BTreeSet<String> {
    let graph = service.observed().unwrap();
    let mut out: BTreeSet<String> = graph.components().map(|c| format!("component {}", serde_json::to_string(c).unwrap())).collect();
    out.extend(graph.relations().map(|r| format!("relation {}", serde_json::to_string(r).unwrap())));
    out.extend(service.ambiguous_calls().unwrap().iter().map(|c| format!("ambiguous {}", serde_json::to_string(c).unwrap())));
    out.extend(service.unproven_calls().unwrap().iter().map(|c| format!("unproven {}", serde_json::to_string(c).unwrap())));
    // Every ref as stored, with its call's place among the file's calls
    // (what a server's answers are kept by).
    for f in service.file_rows() {
        for r in &f.refs {
            out.insert(format!("ref {} {}-{} {} {:?} {:?} {} {:?}", f.file.path.as_str(), r.start, r.end, r.name, r.kind, r.target, r.evidence.name(), r.detail.call.as_ref().map(|(i, _)| i)));
        }
    }
    out
}

/// What reading `root` from nothing gives (in a copy, with its own store).
fn from_scratch(root: &Path) -> BTreeSet<String> {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join(root.file_name().unwrap());
    copy_dir(root, &copy);
    let _ = std::fs::remove_dir_all(copy.join(".nah"));
    let service = open(&copy);
    service.ingest(&copy).unwrap();
    result(&service)
}

fn same(a: &BTreeSet<String>, b: &BTreeSet<String>, after: &str) {
    let only_a: Vec<_> = a.difference(b).take(5).collect();
    let only_b: Vec<_> = b.difference(a).take(5).collect();
    assert!(only_a.is_empty() && only_b.is_empty(), "after {after}: kept differs from read again.\nonly kept: {only_a:#?}\nonly read again: {only_b:#?}");
}

/// A named change to the files.
type Step<'a> = (&'a str, Box<dyn Fn() + 'a>);

fn edit(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains(from), "{} has no {from:?}", path.display());
    std::fs::write(path, text.replacen(from, to, 1)).unwrap();
}

fn db(root: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(root.join(".nah/cache/index.sqlite")).unwrap()
}

#[test]
fn every_change_gives_what_reading_everything_again_gives() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures(dir.path());
    let service = open(&root);
    service.ingest(&root).unwrap();
    same(&result(&service), &from_scratch(&root), "the first read");
    let rust = root.join("rust/src");
    let steps: Vec<Step> = vec![
        ("a body edited", Box::new(|| edit(&rust.join("util.rs"), "s.to_uppercase()", "s.to_lowercase()"))),
        ("a function added", Box::new(|| edit(&rust.join("util.rs"), "pub fn helper() {}", "pub fn helper() {}\n\npub fn added() -> u32 { 1 }"))),
        ("a called function renamed", Box::new(|| edit(&rust.join("util.rs"), "pub fn shout(", "pub fn holler("))),
        ("the call follows it", Box::new(|| edit(&rust.join("lib.rs"), "util::shout(input)", "util::holler(input)"))),
        ("a new file that calls into others", Box::new(|| std::fs::write(rust.join("extra.rs"), "use crate::store::Store;\n\npub fn more() { let s = Store::new(); s.save(\"x\"); crate::util::helper(); }\n").unwrap())),
        ("its module declared", Box::new(|| edit(&rust.join("lib.rs"), "mod util;", "mod util;\nmod extra;"))),
        ("a type's methods moved to another file", Box::new(|| std::fs::write(rust.join("extra.rs"), "use crate::store::Store;\n\nimpl Store {\n    pub fn flush(&self) {}\n}\n\npub fn more() { let s = Store::new(); s.flush(); }\n").unwrap())),
        ("a file deleted", Box::new(|| std::fs::remove_file(rust.join("extra.rs")).unwrap())),
        ("a placeholder added", Box::new(|| edit(&rust.join("util.rs"), "pub fn helper() {}", "pub fn helper() { todo!() }"))),
        ("a Python body edited", Box::new(|| {
            let py = root.join("python");
            let file = walk_all(&py).into_iter().find(|p| p.extension().is_some_and(|e| e == "py")).unwrap();
            let text = std::fs::read_to_string(&file).unwrap();
            std::fs::write(&file, format!("{text}\n\ndef added_here():\n    return 1\n")).unwrap();
        })),
    ];
    for (what, step) in steps {
        step();
        let changed: Vec<PathBuf> = walk_all(&root);
        service.refresh(&changed).unwrap();
        same(&result(&service), &from_scratch(&root), what);
    }
}

/// Every path under `root` (outside .nah), as a watcher batch would hold.
fn walk_all(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|n| n == ".nah") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.push(root.join("rust/src/extra.rs"));
    out
}

#[test]
fn a_change_reads_only_the_changed_file_and_relinks_its_dependents_from_rows() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures(dir.path());
    let service = open(&root);
    service.ingest(&root).unwrap();
    let calls = |service: &ArchitectureService, from: &str, to: &str| {
        let graph = service.observed().unwrap();
        graph.outgoing(&arch_graph::ComponentId::for_symbol("rust", from), Some(arch_graph::RelationKind::Calls)).any(|r| r.target == arch_graph::ComponentId::for_symbol("rust", to))
    };
    assert!(calls(&service, "sample::Upper::apply", "sample::util::shout"));
    // The callee is renamed: the caller's link goes, without the caller being read.
    let util = root.join("rust/src/util.rs");
    edit(&util, "pub fn shout(", "pub fn holler(");
    let report = service.refresh(std::slice::from_ref(&util)).unwrap().ingest;
    assert_eq!((report.read, report.parsed), (1, 1), "only the changed file is read: {report:?}");
    assert!(!calls(&service, "sample::Upper::apply", "sample::util::shout"));
    // And back: the caller is linked again from its rows.
    edit(&util, "pub fn holler(", "pub fn shout(");
    let report = service.refresh(std::slice::from_ref(&util)).unwrap().ingest;
    assert_eq!((report.read, report.parsed), (1, 1), "{report:?}");
    assert!(calls(&service, "sample::Upper::apply", "sample::util::shout"));
}

#[test]
fn a_change_relinks_only_files_of_its_own_language() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures(dir.path());
    // A Python file using a name the Rust file defines.
    std::fs::write(root.join("python/shouts.py"), "def loud(s):\n    return shout(s)\n").unwrap();
    let service = open(&root);
    service.ingest(&root).unwrap();
    let util = root.join("rust/src/util.rs");
    edit(&util, "pub fn shout(", "pub fn holler(");
    let linked = service.refresh(std::slice::from_ref(&util)).unwrap().ingest.linked;
    let names: Vec<&str> = linked.iter().map(|p| p.as_str()).collect();
    assert!(names.contains(&"rust/src/lib.rs"), "the Rust file using it is linked again: {names:?}");
    assert!(!names.contains(&"python/shouts.py"), "names link within one language: {names:?}");
    // A body edit leaves what other files see of util.rs as it was: only
    // the files whose links go into it are linked again, and the graph is
    // what reading everything again gives.
    edit(&util, "s.to_uppercase()", "s.to_lowercase()");
    let linked = service.refresh(std::slice::from_ref(&util)).unwrap().ingest.linked;
    let graph = service.observed().unwrap();
    let into_util = |file: &str| service.file_rows().iter().find(|r| r.file.path.as_str() == file).is_some_and(|r| r.refs.iter().any(|x| x.target.as_ref().and_then(|t| graph.component(t)).is_some_and(|c| c.file().is_some_and(|f| f.as_str() == "rust/src/util.rs"))));
    assert!(linked.iter().all(|p| p.as_str() == "rust/src/util.rs" || into_util(p.as_str())), "{linked:?}");
    same(&result(&service), &from_scratch(&root), "a body edit");
}

#[test]
fn a_link_into_a_changed_file_stays() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures(dir.path());
    let service = open(&root);
    service.ingest(&root).unwrap();
    let link = |service: &ArchitectureService| {
        let graph = service.observed().unwrap();
        graph.outgoing(&arch_graph::ComponentId::for_symbol("rust", "sample::run"), Some(arch_graph::RelationKind::Calls)).find(|r| r.target == arch_graph::ComponentId::for_symbol("rust", "sample::store::Store::save")).cloned()
    };
    let before = link(&service).expect("run calls Store::save");
    // The callee's file changes around it; the call into it isn't dropped.
    edit(&root.join("rust/src/store.rs"), "pub struct Store", "/// Keeps things.\npub struct Store");
    service.refresh(&[root.join("rust/src/store.rs")]).unwrap();
    let after = link(&service).expect("the link into the changed file is still there");
    assert_eq!((after.sites, after.order), (before.sites, before.order));
}

#[test]
fn every_unresolved_ref_keeps_why() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures(dir.path());
    let service = open(&root);
    service.ingest(&root).unwrap();
    let conn = db(&root);
    let rows: Vec<(String, String, Option<String>, String, String)> = conn
        .prepare("SELECT file, name, target, evidence, detail FROM refs WHERE file LIKE 'rust/%'")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, Option<String>>(4)?.unwrap_or_default())))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let evidence = |name: &str| rows.iter().filter(|r| r.1 == name).map(|r| r.3.clone()).collect::<BTreeSet<_>>();
    // `helper()` in ambiguous(): two helpers share the name.
    let helper = rows.iter().find(|r| r.0 == "rust/src/lib.rs" && r.1 == "helper" && r.3 == "ambiguous").expect("helper() is ambiguous");
    assert!(helper.4.contains("sample::util::helper") && helper.4.contains("sample::store::helper"), "the candidates are kept: {}", helper.4);
    assert!(evidence("to_uppercase").contains("external"), "a call outside the repo: {:?}", evidence("to_uppercase"));
    assert!(evidence("save").contains("name"), "a call linked by name");
    for row in &rows {
        match row.3.as_str() {
            "name" | "server" => assert!(row.2.is_some(), "linked without a target: {row:?}"),
            "ambiguous" | "unproven" | "external" | "not_found" => assert!(row.2.is_none(), "unresolved with a target: {row:?}"),
            other => panic!("a ref without a reason: {other:?} in {row:?}"),
        }
    }
}

#[test]
fn the_readers_output_is_rows_and_the_version_is_the_databases() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures(dir.path());
    let service = open(&root);
    service.ingest(&root).unwrap();
    drop(service);
    let conn = db(&root);
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    let files = count("SELECT count(*) FROM files");
    assert!(files >= 6, "{files}");
    assert_eq!(count("SELECT count(*) FROM components WHERE name = 'run' AND file = 'rust/src/lib.rs' AND name_start IS NOT NULL"), 1, "a definition with its name's place");
    assert!(count("SELECT count(*) FROM refs WHERE file = 'rust/src/lib.rs' AND kind = 'call'") >= 5, "calls with their names' places");
    assert!(count("SELECT count(*) FROM flows WHERE file = 'rust/src/lib.rs'") >= 1, "data flow");
    assert!(count("SELECT count(*) FROM placeholders WHERE file = 'rust/src/lib.rs'") >= 1, "todo!() is a placeholder");
    assert_eq!(count("SELECT count(*) FROM highlights"), files, "one row of colours per file");
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert!(version > 0, "the schema's version is the database's");
    assert_eq!(count("SELECT count(*) FROM meta WHERE key IN ('schema_version', 'fingerprints')"), 2);
    // Another version: the rows are made again from the files.
    conn.execute_batch("PRAGMA user_version = 1").unwrap();
    drop(conn);
    let service = open(&root);
    let report = service.ingest(&root).unwrap();
    assert_eq!(report.parsed as i64, files, "every file is read again under another schema: {report:?}");
}

#[test]
fn every_ref_spans_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures(dir.path());
    // Callees the reader keeps as text, and a call of what a call returns.
    std::fs::write(
        root.join("rust/src/odd.rs"),
        "pub mod checks {\n    pub fn is_data(t: u8) -> impl Fn(u8) -> bool {\n        move |x| x == t\n    }\n}\n\npub fn odd() -> bool {\n    let _ = <crate::store::Store as Default>::default();\n    checks::is_data(1)(2)\n}\n",
    )
    .unwrap();
    edit(&root.join("rust/src/lib.rs"), "mod util;", "mod util;\npub mod odd;");
    let service = open(&root);
    service.ingest(&root).unwrap();
    let conn = db(&root);
    let rows: Vec<(String, u32, u32, String, String)> = conn
        .prepare("SELECT file, start, \"end\", name, kind FROM refs")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let text = |file: &str| std::fs::read_to_string(root.join(file)).unwrap();
    let wrong: Vec<_> = rows.iter().filter(|(file, start, end, name, _)| text(file).get(*start as usize..*end as usize) != Some(name.as_str())).collect();
    assert!(wrong.is_empty(), "refs not spanning their names: {wrong:#?}");
    assert!(rows.iter().any(|r| r.0 == "rust/src/odd.rs" && r.3 == "default"), "`<T as Trait>::default` is named `default`");
    assert!(rows.iter().any(|r| r.0 == "rust/src/odd.rs" && r.3 == "is_data"));
}

#[test]
fn changes_from_many_threads_end_as_reading_everything_again() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures(dir.path());
    let service = Arc::new(open(&root));
    service.ingest(&root).unwrap();
    let rust = root.join("rust/src");
    edit(&rust.join("util.rs"), "pub fn helper() {}", "pub fn helper() {}\npub fn one() {}");
    edit(&rust.join("store.rs"), "pub struct Store", "pub struct Two;\npub struct Store");
    let threads: Vec<_> = [rust.join("util.rs"), rust.join("store.rs"), root.clone()]
        .into_iter()
        .map(|path| {
            let service = service.clone();
            std::thread::spawn(move || {
                for _ in 0..3 {
                    if path.is_dir() {
                        service.ingest(&path).unwrap();
                    } else {
                        service.refresh(std::slice::from_ref(&path)).unwrap();
                    }
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    same(&result(&service), &from_scratch(&root), "changes from three threads");
}

#[test]
fn a_full_reindex_is_one_change_readers_see_whole() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures(dir.path());
    let service = Arc::new(open(&root));
    service.ingest(&root).unwrap();
    let file = "rust/src/lib.rs";
    assert!(!service.code_analysis(file).unwrap().highlights.is_empty());
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reindex = {
        let (service, done) = (service.clone(), done.clone());
        std::thread::spawn(move || {
            for _ in 0..3 {
                service.reindex().unwrap();
            }
            done.store(true, std::sync::atomic::Ordering::SeqCst);
        })
    };
    let mut reads = 0;
    while !done.load(std::sync::atomic::Ordering::SeqCst) {
        let seen = service.code_analysis(file).unwrap();
        assert!(seen.stale || !seen.highlights.is_empty(), "a reader saw the store half replaced: no colours for {file}");
        reads += 1;
    }
    reindex.join().unwrap();
    assert!(reads > 0);
}

#[test]
fn edits_whose_effect_goes_through_imports_impls_and_type_names_give_what_reading_everything_again_gives() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures(dir.path());
    let rust = root.join("rust/src");
    let files = [
        ("bell.rs", "pub trait Loud {\n    fn loud(&self) -> u32;\n}\n\npub struct Bell;\n\npub fn ring(b: &Bell) -> u32 {\n    b.loud()\n}\n\npub fn ring_any(b: &dyn Loud) -> u32 {\n    b.loud()\n}\n\npub fn ring_new() -> u32 {\n    let b = Bell;\n    b.loud()\n}\n"),
        ("imp.rs", "use crate::bell::{Bell, Loud};\n\nimpl Loud for Bell {\n    fn loud(&self) -> u32 {\n        1\n    }\n}\n"),
        ("g.rs", "use crate::util::fresh as fr;\nuse std::fmt::Display;\n\npub fn calls_fresh() {\n    fr();\n}\n\npub fn shows(x: &dyn Display) -> std::fmt::Result {\n    crate::util::helper();\n    crate::util::nope();\n    Ok(())\n}\n"),
        ("g2.rs", "use crate::util::*;\n\npub fn keep2(s: &Store) {\n    s.save(\"x\");\n}\n"),
        ("h.rs", "use crate::store::Store;\n\nimpl Store {\n    pub fn flush(&self) {}\n}\n"),
        ("g3.rs", "use crate::store::Store;\n\npub fn use_flush(s: &Store) {\n    s.flush();\n}\n"),
        ("reex.rs", "pub use crate::bell::ring;\n"),
        ("g5.rs", "fn make_fn() -> fn() {\n    crate::util::helper\n}\n\npub fn chain() {\n    make_fn()();\n    crate::util::helper();\n}\n"),
        ("kc.rs", "pub trait Chime {\n    fn chime(&self) -> u32 {\n        1\n    }\n\n    fn strike(&self);\n}\n\npub trait Other {\n    fn chime(&self) -> u32 {\n        2\n    }\n\n    fn strike(&self);\n}\n\npub struct Gong;\n\npub struct Holder<T>(pub T);\n"),
        ("kg.rs", "use crate::kc::{Chime, Gong, Other};\n\nimpl Chime for Gong {\n    fn strike(&self) {}\n}\n"),
        ("ku.rs", "use crate::kc::Gong;\n\npub fn hit(g: &Gong) -> u32 {\n    g.chime()\n}\n"),
        ("kg2.rs", "use crate::kc::{Chime, Holder, Other};\n\nimpl<T> Chime for Holder<T> {\n    fn strike(&self) {}\n}\n"),
        ("hu.rs", "use crate::kc::Holder;\n\npub fn hit2(h: &Holder<u32>) -> u32 {\n    h.chime()\n}\n"),
        ("pot.rs", "pub struct Pot;\n\npub fn make() -> Pot {\n    Pot\n}\n"),
        ("stir.rs", "use crate::pot::Pot;\n\nimpl Pot {\n    pub fn stir(&self) {}\n}\n"),
        ("cook.rs", "pub fn cook() {\n    crate::pot::make().stir();\n}\n"),
        ("g4.rs", "pub fn rings(b: &crate::bell::Bell) -> u32 {\n    crate::reex::ring(b)\n}\n"),
    ];
    for (name, text) in files {
        std::fs::write(rust.join(name), text).unwrap();
    }
    edit(&rust.join("lib.rs"), "mod util;", "mod util;\nmod bell;\nmod imp;\nmod g;\nmod g2;\nmod h;\nmod g3;\nmod reex;\nmod g4;\nmod g5;\nmod kc;\nmod kg;\nmod ku;\nmod pot;\nmod stir;\nmod cook;\nmod kg2;\nmod hu;");
    // A base two levels up.
    std::fs::write(root.join("python/pz.py"), "class Base0:\n    def m(self):\n        return 1\n\n\nclass Base1:\n    def m(self):\n        return 2\n").unwrap();
    std::fs::write(root.join("python/pmid.py"), "from pz import Base0, Base1\n\n\nclass Mid(Base0):\n    pass\n").unwrap();
    std::fs::write(root.join("python/pleaf.py"), "from pmid import Mid\n\n\nclass Leaf(Mid):\n    pass\n").unwrap();
    std::fs::write(root.join("python/pu2.py"), "from pleaf import Leaf\n\n\ndef run2(b: Leaf):\n    return b.m()\n").unwrap();
    std::fs::write(root.join("typescript/src/tz.ts"), "export class Base0 {\n  m(): number {\n    return 1;\n  }\n}\n\nexport class Base1 {\n  m(): number {\n    return 2;\n  }\n}\n").unwrap();
    std::fs::write(root.join("typescript/src/tmid.ts"), "import { Base0, Base1 } from \"./tz\";\n\nexport class Mid extends Base0 {}\n").unwrap();
    std::fs::write(root.join("typescript/src/tleaf.ts"), "import { Mid } from \"./tmid\";\n\nexport class Leaf extends Mid {}\n").unwrap();
    std::fs::write(root.join("typescript/src/tu2.ts"), "import { Leaf } from \"./tleaf\";\n\nexport function run2(b: Leaf): number {\n  return b.m();\n}\n").unwrap();
    // Python: a call through a class whose base another file names.
    std::fs::write(root.join("python/pa.py"), "class A:\n    def m(self):\n        return 1\n\n\nclass C:\n    def m(self):\n        return 2\n").unwrap();
    std::fs::write(root.join("python/pb.py"), "from pa import A, C\n\n\nclass B(A):\n    pass\n").unwrap();
    std::fs::write(root.join("python/pu.py"), "from pb import B\n\n\ndef run(b: B):\n    return b.m()\n").unwrap();
    // TypeScript: the same through `extends`.
    std::fs::write(root.join("typescript/src/ta.ts"), "export class A {\n  m(): number {\n    return 1;\n  }\n}\n\nexport class C {\n  m(): number {\n    return 2;\n  }\n}\n").unwrap();
    std::fs::write(root.join("typescript/src/tb.ts"), "import { A, C } from \"./ta\";\n\nexport class B extends A {}\n").unwrap();
    std::fs::write(root.join("typescript/src/tu.ts"), "import { B } from \"./tb\";\n\nexport function run(b: B): number {\n  return b.m();\n}\n").unwrap();
    let service = open(&root);
    service.ingest(&root).unwrap();
    same(&result(&service), &from_scratch(&root), "the first read");
    // Imports and the types a signature names are refs, each with why.
    let refs = |file: &str| -> BTreeSet<(String, String, String)> {
        db(&root)
            .prepare("SELECT name, kind, evidence FROM refs WHERE file = ?1")
            .unwrap()
            .query_map([file], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let has = |file: &str, row: (&str, &str, &str)| {
        let rows = refs(file);
        assert!(rows.contains(&(row.0.to_owned(), row.1.to_owned(), row.2.to_owned())), "{file} has no {row:?}: {rows:#?}");
    };
    has("rust/src/g.rs", ("util", "import", "name"));
    has("rust/src/g.rs", ("fresh", "import", "not_found"));
    has("rust/src/g.rs", ("fr", "call", "not_found"));
    has("rust/src/g3.rs", ("Store", "import", "name"));
    has("rust/src/g3.rs", ("Store", "type", "name"));
    has("rust/src/bell.rs", ("u32", "type", "external"));
    // Outside crates are outside; a path written from the repo that leads
    // nowhere is missing; paths are refs too.
    has("rust/src/g.rs", ("std", "import", "external"));
    has("rust/src/g.rs", ("Display", "import", "external"));
    has("rust/src/g.rs", ("Display", "type", "external"));
    has("rust/src/g.rs", ("util", "path", "name"));
    has("rust/src/g.rs", ("nope", "call", "not_found"));
    has("rust/src/g.rs", ("helper", "call", "name"));
    // Each ref spans its name only.
    let text = std::fs::read_to_string(rust.join("g.rs")).unwrap();
    let spans: Vec<(String, u32, u32)> = db(&root).prepare("SELECT name, start, \"end\" FROM refs WHERE file = 'rust/src/g.rs'").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
    for (name, start, end) in &spans {
        assert_eq!(&text[*start as usize..*end as usize], name, "the ref spans its name");
    }
    has("rust/src/util.rs", ("to_uppercase", "call", "external"));
    let impl_text = "impl Loud for Bell {\n    fn loud(&self) -> u32 {\n        1\n    }\n}\n";
    let steps: Vec<Step> = vec![
        ("a trait's impl removed in another file", Box::new(|| edit(&rust.join("imp.rs"), impl_text, ""))),
        ("the impl added back", Box::new(|| edit(&rust.join("imp.rs"), "Loud};\n\n", &format!("Loud}};\n\n{impl_text}")))),
        ("a name an alias imports defined later", Box::new(|| edit(&rust.join("util.rs"), "pub fn helper() {}", "pub fn helper() {}\n\npub fn fresh() {}"))),
        ("a same-named type added where a glob import reaches", Box::new(|| edit(&rust.join("util.rs"), "pub fn helper() {}", "pub fn helper() {}\n\npub struct Store;"))),
        ("a type renamed while its methods live in another file", Box::new(|| edit(&rust.join("store.rs"), "pub struct Store {", "pub struct Repo {"))),
        ("a re-export pointed elsewhere", Box::new(|| edit(&rust.join("reex.rs"), "crate::bell::ring;", "crate::bell::ring_new as ring;"))),
        ("a re-export removed", Box::new(|| edit(&rust.join("reex.rs"), "pub use crate::bell::ring_new as ring;\n", ""))),
        ("a type's impl switched to another trait in another file", Box::new(|| edit(&rust.join("kg.rs"), "impl Chime for Gong", "impl Other for Gong"))),
        ("a Python class's base changed in another file", Box::new(|| edit(&root.join("python/pb.py"), "class B(A):", "class B(C):"))),
        ("a TypeScript class's base changed in another file", Box::new(|| edit(&root.join("typescript/src/tb.ts"), "extends A", "extends C"))),
        ("a generic type's impl switched to another trait in another file", Box::new(|| edit(&rust.join("kg2.rs"), "impl<T> Chime for Holder<T>", "impl<T> Other for Holder<T>"))),
        ("a Python base two levels up changed", Box::new(|| edit(&root.join("python/pmid.py"), "class Mid(Base0):", "class Mid(Base1):"))),
        ("a TypeScript base two levels up changed", Box::new(|| edit(&root.join("typescript/src/tmid.ts"), "extends Base0", "extends Base1"))),
        ("a type renamed while its impl lives in another file", Box::new(|| {
            edit(&rust.join("pot.rs"), "pub struct Pot;", "pub struct Kettle;");
            edit(&rust.join("pot.rs"), "-> Pot {\n    Pot", "-> Kettle {\n    Kettle");
        })),
    ];
    let calls = |service: &ArchitectureService, lang: &str, from: &str| -> Vec<String> {
        let graph = service.observed().unwrap();
        graph.outgoing(&arch_graph::ComponentId::for_symbol(lang, from), Some(arch_graph::RelationKind::Calls)).map(|r| r.target.as_str().to_owned()).collect()
    };
    // Calls made through a type whose impl or base lives in another file.
    let through = |service: &ArchitectureService| {
        [
            calls(service, "rust", "sample::ku::hit"),
            calls(service, "python", "python.pu.run"),
            calls(service, "typescript", "typescript.src.tu.run"),
            calls(service, "rust", "sample::hu::hit2"),
            calls(service, "python", "python.pu2.run2"),
            calls(service, "typescript", "typescript.src.tu2.run2"),
        ]
    };
    let ids = |list: [&str; 6]| list.map(|id| vec![id.to_owned()]);
    assert_eq!(through(&service), ids(["rust:sample::kc::Chime::chime", "python:python.pa.A.m", "typescript:typescript.src.ta.A.m", "rust:sample::kc::Chime::chime", "python:python.pz.Base0.m", "typescript:typescript.src.tz.Base0.m"]));
    for (what, step) in steps {
        step();
        service.refresh(&walk_all(&root)).unwrap();
        same(&result(&service), &from_scratch(&root), what);
    }
    // Each now goes where the changed impl or base leads.
    assert_eq!(through(&service), ids(["rust:sample::kc::Other::chime", "python:python.pa.C.m", "typescript:typescript.src.ta.C.m", "rust:sample::kc::Other::chime", "python:python.pz.Base1.m", "typescript:typescript.src.tz.Base1.m"]));
}

#[test]
fn opening_kept_rows_that_must_be_linked_again_is_the_pipeline_threads_work() {
    let dir = tempfile::tempdir().unwrap();
    let root = fixtures(dir.path());
    let first = {
        let service = open(&root);
        service.ingest(&root).unwrap();
        result(&service)
    };
    // What linking depends on changed since: opening links every row again
    // and writes them (a store write anywhere but the pipeline thread fails
    // here, where debug checks run).
    db(&root).execute("UPDATE meta SET value = 'older' WHERE key = 'fingerprints'", []).unwrap();
    let service = open(&root);
    same(&result(&service), &first, "opening again");
    let kept: String = db(&root).query_row("SELECT value FROM meta WHERE key = 'fingerprints'", [], |r| r.get(0)).unwrap();
    assert_ne!(kept, "older", "linked again and kept");
}

#[test]
fn a_reason_says_what_a_name_really_is() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"proj\"\n").unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "mod kinds;\nmod store;\nmod time;\nmod tree;\npub use kinds::Kind;\npub use store::open;\nuse crate::time::Duration;\n\npub enum Failure {\n    Bad(String),\n}\n\n#[derive(Default)]\npub struct Config {\n    pub n: u32,\n}\n\npub fn run() -> Failure {\n    let _c = Config::default();\n    let _k: Kind = crate::open();\n    let _d = Duration::from_secs(1);\n    Failure::Bad(String::new())\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/kinds.rs"),
        "#[derive(Default)]\npub struct Kind {\n    pub n: u32,\n}\n\npub const LIMIT: u32 = 3;\npub type Res = Result<Kind, Shape>;\n\npub enum Shape {\n    Square { side: u32 },\n}\n\nimpl Kind {\n    pub fn one() -> Self {\n        Self::make()\n    }\n\n    fn make() -> Self {\n        Self::default()\n    }\n\n    pub fn square() -> Shape {\n        Shape::Square { side: LIMIT }\n    }\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("src/store.rs"), "use crate::Kind;\nuse crate::kinds::{LIMIT, Res};\n\npub fn open() -> Kind {\n    Kind { n: 1 }\n}\n\npub fn again() -> Kind {\n    crate::Kind::one()\n}\n\npub fn checked() -> Res {\n    let _ = LIMIT;\n    Ok(again())\n}\n").unwrap();
    std::fs::create_dir_all(root.join("src/tree")).unwrap();
    std::fs::write(root.join("src/tree/mod.rs"), "mod held;\nmod layout;\npub use layout::Rect;\n").unwrap();
    std::fs::write(root.join("src/tree/layout.rs"), "pub struct Rect;\n").unwrap();
    std::fs::write(root.join("src/tree/held.rs"), "use super::Rect;\n\npub fn place() -> Rect {\n    Rect\n}\n").unwrap();
    std::fs::write(root.join("src/time.rs"), "pub use std::time::Duration;\n").unwrap();
    let service = open(&root);
    service.ingest(&root).unwrap();
    let rows = service.file_rows();
    let reason = |file: &str, name: &str| -> Vec<(String, Option<String>)> {
        rows.iter().filter(|f| f.file.path.as_str() == file).flat_map(|f| f.refs.iter().filter(|r| r.name == name).map(|r| (r.evidence.name().to_owned(), r.target.as_ref().map(|t| t.as_str().to_owned())))).collect()
    };
    // Through the crate root's re-exports: the real definitions.
    assert!(reason("src/lib.rs", "open").contains(&("name".into(), Some("rust:proj::store::open".into()))), "{:?}", reason("src/lib.rs", "open"));
    assert!(reason("src/store.rs", "Kind").iter().any(|(e, t)| e == "name" && t.as_deref() == Some("rust:proj::kinds::Kind")), "{:?}", reason("src/store.rs", "Kind"));
    // `Self::` in an impl: the type's own; a derived one, a member.
    assert!(reason("src/kinds.rs", "make").contains(&("name".into(), Some("rust:proj::kinds::Kind::make".into()))), "{:?}", reason("src/kinds.rs", "make"));
    assert!(reason("src/kinds.rs", "default").contains(&("member".into(), None)), "{:?}", reason("src/kinds.rs", "default"));
    // `Self` as a type is the enclosing one.
    assert!(reason("src/kinds.rs", "Self").iter().all(|(e, t)| e == "name" && t.as_deref() == Some("rust:proj::kinds::Kind")), "{:?}", reason("src/kinds.rs", "Self"));
    // A variant written as a struct; a constant and a type alias (items no
    // card shows): something of the repo, not missing.
    let square: Vec<_> = reason("src/kinds.rs", "Shape::Square").into_iter().chain(reason("src/kinds.rs", "Square")).collect();
    assert!(!square.is_empty() && square.iter().all(|(e, _)| e == "member"), "{square:?}");
    assert!(!reason("src/store.rs", "LIMIT").is_empty() && reason("src/store.rs", "LIMIT").iter().all(|(e, _)| e == "member"), "{:?}", reason("src/store.rs", "LIMIT"));
    assert!(!reason("src/store.rs", "Res").is_empty() && reason("src/store.rs", "Res").iter().all(|(e, _)| e == "member"), "{:?}", reason("src/store.rs", "Res"));
    // A path through the crate root's re-export, and a re-export one module
    // up (`use super::Rect`, the parent having `pub use layout::Rect`).
    assert!(reason("src/store.rs", "Kind").iter().all(|(e, t)| e == "name" && t.as_deref() == Some("rust:proj::kinds::Kind")), "{:?}", reason("src/store.rs", "Kind"));
    assert!(reason("src/store.rs", "one").contains(&("name".into(), Some("rust:proj::kinds::Kind::one".into()))), "{:?}", reason("src/store.rs", "one"));
    assert!(reason("src/tree/held.rs", "Rect").iter().all(|(e, t)| e == "name" && t.as_deref() == Some("rust:proj::tree::layout::Rect")), "{:?}", reason("src/tree/held.rs", "Rect"));
    // A variant and a derived method: members of a repo type, not components.
    assert!(reason("src/lib.rs", "Bad").contains(&("member".into(), None)), "{:?}", reason("src/lib.rs", "Bad"));
    assert!(reason("src/lib.rs", "default").contains(&("member".into(), None)), "{:?}", reason("src/lib.rs", "default"));
    // A std type a repo module re-exports: outside the repo, not missing.
    assert!(reason("src/lib.rs", "from_secs").contains(&("external".into(), None)), "{:?}", reason("src/lib.rs", "from_secs"));
    // No candidate twice.
    for f in &rows {
        for r in &f.refs {
            let mut c = r.detail.candidates.clone();
            c.dedup();
            assert_eq!(c, r.detail.candidates, "{r:?}");
        }
    }
}
