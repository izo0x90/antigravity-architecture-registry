//! The languages' servers confirm links, with a scripted server standing in for rust-analyzer: it's asked
//! only about the changed and re-linked files' refs, what it answers is
//! kept as proven, a link into a changed file stays (unconfirmed) until
//! it's asked again, each language's state is kept with why, and a full
//! re-index asks again.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arch_graph::{ComponentId, RelationKind, ServerState, TypeSystem};
use arch_service::{ArchitectureService, MemoryPlans};

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

struct Setup {
    _dir: tempfile::TempDir,
    root: PathBuf,
    log: PathBuf,
    catalog: nah_packs::Catalog,
}

/// The Rust fixture, and the shipped packs with Rust's server swapped for
/// the scripted one (installed).
fn setup(flags: &[&str]) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample"), &root);
    let packs_dir = dir.path().join("packs");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs"), &packs_dir);
    let rust = packs_dir.join("language/rust/pack.toml");
    std::fs::write(&rust, std::fs::read_to_string(&rust).unwrap().replace("server = \"rust-analyzer\"", "server = \"fake-lsp\"")).unwrap();
    let log = dir.path().join("server.log");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_lsp.py");
    let mut args = vec![script.display().to_string(), "--log".into(), log.display().to_string()];
    args.extend(flags.iter().map(|f| (*f).to_owned()));
    let args = args.iter().map(|a| format!("{a:?}")).collect::<Vec<_>>().join(", ");
    std::fs::create_dir_all(packs_dir.join("server/fake")).unwrap();
    std::fs::write(
        packs_dir.join("server/fake/pack.toml"),
        format!(
            r#"[pack]
name = "fake-lsp"
kind = "server"
version = "1.0.0"
description = "A scripted language server for tests."

[tool]
name = "python3"
supports = ">=3.0.0"
[tool.check]
command = "python3"
args = ["--version"]
version = 'Python (\d+\.\d+\.\d+)'

[[install]]
platforms = ["linux-x86_64", "linux-arm64", "macos-arm64", "macos-x86_64"]
[[install.steps]]
run = {{ command = "true" }}

[server]
command = "python3"
args = [{args}]
requires = ["textDocument/definition"]
ready_timeout_s = 30
request_timeout_s = 30
"#
        ),
    )
    .unwrap();
    let places = nah_packs::Places::isolated(&dir.path().join("home"));
    let catalog = nah_packs::Catalog::load(&nah_packs::Distribution::from_dir(&packs_dir).unwrap(), &places).unwrap();
    let mut lock = nah_packs::Lock::default();
    let platform = catalog.platforms.current().unwrap().name.clone();
    lock.packs.insert("fake-lsp".into(), nah_packs::Installed { pack_version: "1.0.0".into(), tool_version: None, platform, dest: dir.path().join("dest"), sha256: vec![], installed_at: 0 });
    lock.write(&catalog.lock_path).unwrap();
    Setup { root, log, catalog, _dir: dir }
}

fn open(s: &Setup) -> ArchitectureService {
    let facts = arch_service::open_facts(&s.root, &arch_service::Limits::defaults().unwrap()).unwrap();
    ArchitectureService::open(&s.root, code_analyzer::PackSet::from_catalog(&s.catalog).unwrap(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), facts).unwrap().with_servers(&s.catalog)
}

fn id(s: &str) -> ComponentId {
    ComponentId::for_symbol("rust", s)
}

/// The call's relation's prover (None: matched by name; Some(None):
/// unconfirmed; absent: no call).
fn proven(service: &ArchitectureService, from: &str, to: &str) -> Option<Option<String>> {
    service.observed().unwrap().outgoing(&id(from), Some(RelationKind::Calls)).find(|r| r.target == id(to)).map(|r| r.proven_by.clone())
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn asked(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().filter_map(|l| l.strip_prefix("definition ")).map(|l| l.split(' ').next().unwrap().to_owned()).collect()
}

#[test]
fn the_server_confirms_links_and_is_asked_only_about_the_changed_and_relinked_files() {
    let s = setup(&["--prefer", "src/util.rs"]);
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    wait_until("run's call to Store::save is confirmed", || proven(&service, "sample::run", "sample::store::Store::save") == Some(Some("fake-lsp 9.9.9".into())));
    // What names couldn't settle, the server does: two helpers share the name.
    wait_until("ambiguous() calls util::helper", || proven(&service, "sample::ambiguous", "sample::util::helper").is_some());
    assert!(service.ambiguous_calls().unwrap().iter().all(|c| c.callee != "helper"), "no longer ambiguous");
    // A change: only the changed file and the files it re-links are asked.
    std::fs::write(&s.log, "").unwrap();
    let util = s.root.join("src/util.rs");
    std::fs::write(&util, std::fs::read_to_string(&util).unwrap().replace("s.to_uppercase()", "s.to_lowercase()")).unwrap();
    let report = service.refresh(std::slice::from_ref(&util)).unwrap();
    wait_until("the change is confirmed", || !asked(&s.log).is_empty() && proven(&service, "sample::Upper::apply", "sample::util::shout") == Some(Some("fake-lsp 9.9.9".into())));
    std::thread::sleep(Duration::from_millis(300));
    let files: std::collections::BTreeSet<String> = asked(&s.log).into_iter().collect();
    assert!(files.contains("src/util.rs"), "{files:?}");
    assert!(!files.contains("src/store.rs"), "store.rs uses nothing util.rs defines: {files:?} ({report:?})");
    assert!(std::fs::read_to_string(&s.log).unwrap().lines().any(|l| l == "changed src/util.rs 2"), "told of the change");
    // A new file is told as created, a removed one as deleted.
    let fresh = s.root.join("src/fresh.rs");
    std::fs::write(&fresh, "pub fn fresh() {}\n").unwrap();
    service.refresh(std::slice::from_ref(&fresh)).unwrap();
    std::fs::remove_file(&fresh).unwrap();
    service.refresh(std::slice::from_ref(&fresh)).unwrap();
    wait_until("told of the new file and its removal", || {
        let log = std::fs::read_to_string(&s.log).unwrap();
        log.lines().any(|l| l == "changed src/fresh.rs 1") && log.lines().any(|l| l == "changed src/fresh.rs 3")
    });
}

#[test]
fn a_link_into_a_changed_file_stays_unconfirmed_until_the_server_answers_again() {
    let s = setup(&["--answer-ms", "300", "--prefer", "src/util.rs"]);
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    wait_until("confirmed", || proven(&service, "sample::run", "sample::store::Store::save") == Some(Some("fake-lsp 9.9.9".into())));
    // A link only the server makes: names alone leave `helper()` ambiguous.
    let only_the_server = || proven(&service, "sample::ambiguous", "sample::util::helper");
    wait_until("ambiguous() calls util::helper", || only_the_server() == Some(Some("fake-lsp 9.9.9".into())));
    for file in ["src/store.rs", "src/util.rs"] {
        let path = s.root.join(file);
        std::fs::write(&path, format!("// moved down\n{}", std::fs::read_to_string(&path).unwrap())).unwrap();
        service.refresh(std::slice::from_ref(&path)).unwrap();
    }
    // At once: the links are there, unconfirmed (not dropped, not external,
    // not ambiguous again).
    assert_eq!(proven(&service, "sample::run", "sample::store::Store::save"), Some(None), "kept, unconfirmed, while it's asked again");
    assert_eq!(only_the_server(), Some(None), "the server's link is kept, unconfirmed");
    assert!(service.ambiguous_calls().unwrap().iter().all(|c| c.callee != "helper"), "not ambiguous again");
    wait_until("confirmed again", || proven(&service, "sample::run", "sample::store::Store::save") == Some(Some("fake-lsp 9.9.9".into())) && only_the_server() == Some(Some("fake-lsp 9.9.9".into())));
}

#[test]
fn an_answer_in_the_repo_at_no_component_says_local() {
    let s = setup(&[]);
    std::fs::write(s.root.join("src/generic.rs"), "pub fn first<Item: Clone>(items: &[Item]) -> Item {\n    items[0].clone()\n}\n").unwrap();
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    let item = || {
        let rows = service.file_rows().into_iter().find(|r| r.file.path.as_str() == "src/generic.rs").unwrap();
        rows.refs.iter().filter(|r| r.name == "Item").map(|r| (r.evidence, r.prover.clone(), r.target.clone())).collect::<Vec<_>>()
    };
    wait_until("the server answered for Item", || !item().is_empty() && item().iter().all(|(_, prover, _)| prover.is_some()));
    assert!(item().iter().all(|r| *r == (code_analyzer::rows::Evidence::Local, Some("fake-lsp 9.9.9".into()), None)), "a generic in the repo: local, not \"not found\": {:?}", item());
}

#[test]
fn each_languages_server_state_is_kept_with_why_and_unreadable_places_are_counted() {
    let s = setup(&["--bad-positions"]);
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    let rust = |service: &ArchitectureService| service.index_status().servers.into_iter().find(|v| v.language == "rust").unwrap();
    wait_until("ready", || rust(&service).state == ServerState::Ready);
    assert_eq!(rust(&service).version.as_deref(), Some("9.9.9"));
    wait_until("places it gave couldn't be read", || rust(&service).dropped_positions > 0);
    // Nothing it said was taken: links stay as names made them.
    assert_eq!(proven(&service, "sample::run", "sample::store::Store::save"), Some(None));
    let python = service.index_status().servers.into_iter().find(|v| v.language == "python").unwrap();
    assert_eq!(python.state, ServerState::NotInstalled);
    assert!(python.reason.as_deref().is_some_and(|r| r.contains("meta-harness pack install basedpyright")), "{python:?}");
    drop(service);
    // Kept: a restart shows them before any server has started.
    let facts = arch_service::open_facts(&s.root, &arch_service::Limits::defaults().unwrap()).unwrap();
    let again = ArchitectureService::open(&s.root, code_analyzer::PackSet::from_catalog(&s.catalog).unwrap(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), facts).unwrap();
    let kept = again.index_status().servers;
    assert!(kept.iter().any(|v| v.language == "rust" && v.state == ServerState::Ready), "{kept:?}");
    assert!(kept.iter().any(|v| v.language == "python" && v.state == ServerState::NotInstalled && v.reason.is_some()), "{kept:?}");
}

#[test]
fn a_full_reindex_asks_the_servers_again() {
    let s = setup(&[]);
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    wait_until("confirmed", || proven(&service, "sample::run", "sample::store::Store::save") == Some(Some("fake-lsp 9.9.9".into())));
    std::fs::write(&s.log, "").unwrap();
    service.reindex().unwrap();
    wait_until("confirmed again after the re-index", || proven(&service, "sample::run", "sample::store::Store::save") == Some(Some("fake-lsp 9.9.9".into())));
    let files: std::collections::BTreeSet<String> = asked(&s.log).into_iter().collect();
    assert!(files.contains("src/lib.rs") && files.contains("src/store.rs") && files.contains("src/util.rs"), "every file asked again: {files:?}");
}

#[test]
fn a_full_reindex_with_no_server_running_keeps_what_servers_said_until_they_can_be_asked() {
    let s = setup(&["--prefer", "src/util.rs"]);
    {
        let service = open(&s);
        service.ingest(&s.root).unwrap();
        wait_until("confirmed", || proven(&service, "sample::run", "sample::store::Store::save") == Some(Some("fake-lsp 9.9.9".into())));
        assert!(service.wait_for_servers(Duration::from_secs(30)).unwrap());
    }
    // No server this time: the re-index keeps the answer, unconfirmed.
    {
        let facts = arch_service::open_facts(&s.root, &arch_service::Limits::defaults().unwrap()).unwrap();
        let service = ArchitectureService::open(&s.root, code_analyzer::PackSet::from_catalog(&s.catalog).unwrap(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), facts).unwrap();
        service.reindex().unwrap();
        assert_eq!(proven(&service, "sample::run", "sample::store::Store::save"), Some(None), "kept, not wiped");
        let lib = service.file_rows().into_iter().find(|r| r.file.path.as_str() == "src/lib.rs").unwrap();
        let save = lib.refs.iter().find(|r| r.name == "save" && r.detail.call.is_some()).unwrap();
        assert_eq!((save.prover.as_deref(), save.evidence), (Some("fake-lsp 9.9.9"), code_analyzer::rows::Evidence::Name), "the server's answer, waiting to be asked again");
    }
    // With the server again: asked, and proven.
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    wait_until("confirmed again", || proven(&service, "sample::run", "sample::store::Store::save") == Some(Some("fake-lsp 9.9.9".into())));
}

/// Definition requests logged since `from` lines.
fn requests_since(log: &Path, from: usize) -> usize {
    std::fs::read_to_string(log).unwrap_or_default().lines().skip(from).filter(|l| l.starts_with("definition ")).count()
}

fn log_len(log: &Path) -> usize {
    std::fs::read_to_string(log).unwrap_or_default().lines().count()
}

#[test]
fn what_waited_at_shutdown_is_asked_after_a_restart_and_a_restart_with_nothing_left_asks_nothing() {
    let s = setup(&["--prefer", "src/util.rs", "--progress-ms", "2500"]);
    {
        let service = open(&s);
        service.ingest(&s.root).unwrap();
        // Closed while the server still loads: nothing was asked.
        assert_eq!(asked(&s.log).len(), 0);
    }
    // Opened again: nothing changed, nothing re-linked, but what no server
    // confirmed yet is asked once it's ready.
    let service = open(&s);
    let report = service.ingest(&s.root).unwrap();
    assert!(report.linked.is_empty(), "{report:?}");
    wait_until("run's call is confirmed after the restart", || proven(&service, "sample::run", "sample::store::Store::save") == Some(Some("fake-lsp 9.9.9".into())));
    assert!(service.wait_for_servers(Duration::from_secs(30)).unwrap());
    drop(service);
    // Everything answered: a restart with nothing changed asks nothing.
    let from = log_len(&s.log);
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    assert!(service.wait_for_servers(Duration::from_secs(30)).unwrap());
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(requests_since(&s.log, from), 0, "{}", std::fs::read_to_string(&s.log).unwrap());
}

#[test]
fn a_refused_answer_is_asked_again_until_it_is_answered() {
    let s = setup(&["--prefer", "src/util.rs", "--refuse-first", "3"]);
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    wait_until("confirmed after the refusals", || proven(&service, "sample::run", "sample::store::Store::save") == Some(Some("fake-lsp 9.9.9".into())));
}

#[test]
fn a_warm_server_confirms_a_change_within_confirm_ms_asking_only_its_refs_and_the_counts_are_kept() {
    let s = setup(&["--prefer", "src/util.rs"]);
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    wait_until("warm", || proven(&service, "sample::Upper::apply", "sample::util::shout") == Some(Some("fake-lsp 9.9.9".into())));
    assert!(service.wait_for_servers(Duration::from_secs(30)).unwrap());
    let limit = service.limits().servers.confirm_ms;
    let from = log_len(&s.log);
    let util = s.root.join("src/util.rs");
    std::fs::write(&util, std::fs::read_to_string(&util).unwrap().replace("s.to_uppercase()", "s.to_lowercase()")).unwrap();
    let report = service.refresh(std::slice::from_ref(&util)).unwrap().ingest;
    let started = Instant::now();
    assert!(service.wait_for_servers(Duration::from_secs(30)).unwrap());
    let took = started.elapsed().as_millis() as u64;
    println!("confirm one changed file: {took} ms (limit {limit})");
    assert!(took <= limit, "confirming took {took} ms, over confirm_ms {limit}");
    // Exactly the names of the files re-linked were asked: their calls
    // (with a name), the types they name, the fields they use, and the
    // imports and paths they write.
    let rows = service.file_rows();
    let asked = |x: &code_analyzer::rows::RefRow| match &x.detail.call {
        Some((_, c)) => c.name_span.is_some(),
        None => {
            use code_analyzer::rows::RefKind;
            matches!(x.kind, RefKind::Type | RefKind::Field | RefKind::Import | RefKind::Path)
        }
    };
    let names: usize = rows.iter().filter(|r| report.linked.contains(&r.file.path)).map(|r| r.refs.iter().filter(|x| asked(x)).count()).sum();
    assert_eq!(requests_since(&s.log, from), names, "requests for {:?}", report.linked);
    // The language's files and proven links, live and as kept.
    let live = service.index_status().servers.into_iter().find(|x| x.language == "rust").unwrap();
    assert_eq!(live.files, 3);
    assert!(live.proven > 0, "{live:?}");
    wait_until("the counts are kept", || {
        let store = arch_service::open_facts(&s.root, &arch_service::Limits::defaults().unwrap()).unwrap();
        store.servers().unwrap().into_iter().any(|k| k.language == "rust" && k.files == live.files && k.proven == live.proven)
    });
}

/// The data flow's prover (None: matched by name; Some(None): unconfirmed;
/// absent: no such flow).
fn flow_proven(service: &ArchitectureService, from: &str, to: &str) -> Option<Option<String>> {
    service.observed().unwrap().outgoing(&id(from), Some(RelationKind::DataFlow)).find(|r| r.target == id(to)).map(|r| r.proven_by.clone())
}

/// Fields read and written: through `self`, through a parameter (only the
/// server can say which type `c` is), and taken by a `&mut` borrow (a
/// write the server sees and our reader doesn't).
const COUNTER: &str = "pub struct Counter {\n    pub count: u32,\n}\n\nimpl Counter {\n    pub fn bump(&mut self) {\n        self.count += 1;\n    }\n\n    pub fn total(&self) -> u32 {\n        self.count\n    }\n\n    pub fn fresh() -> Counter {\n        Counter { count: 0 }\n    }\n\n    pub fn drain(&mut self) -> u32 {\n        std::mem::take(&mut self.count)\n    }\n}\n\npub fn reset(c: &mut Counter) {\n    c.count = 0;\n}\n";

/// The service over the fixture with the counter module added.
fn with_counter(s: &Setup) -> ArchitectureService {
    std::fs::write(s.root.join("src/counter.rs"), COUNTER).unwrap();
    let lib = s.root.join("src/lib.rs");
    std::fs::write(&lib, format!("pub mod counter;\n{}", std::fs::read_to_string(&lib).unwrap())).unwrap();
    let service = open(s);
    service.ingest(&s.root).unwrap();
    service
}

#[test]
fn the_server_confirms_field_reads_and_writes_and_the_types_data_carries() {
    let s = setup(&[]);
    std::fs::write(s.root.join("src/counter.rs"), COUNTER).unwrap();
    let lib = s.root.join("src/lib.rs");
    std::fs::write(&lib, format!("pub mod counter;\n{}", std::fs::read_to_string(&lib).unwrap())).unwrap();
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    let by = Some(Some("fake-lsp 9.9.9".to_owned()));
    // A field written, a field read: which field (its type) and how it's
    // used, both as the server says.
    wait_until("bump's write is confirmed", || flow_proven(&service, "sample::counter::Counter::bump", "sample::counter::Counter") == by);
    wait_until("total's read is confirmed", || flow_proven(&service, "sample::counter::Counter", "sample::counter::Counter::total") == by);
    // The type a value is made of, as the server says.
    wait_until("fresh's Counter is confirmed", || flow_proven(&service, "sample::counter::Counter::fresh", "sample::counter::Counter") == by);
    let log = std::fs::read_to_string(&s.log).unwrap();
    assert!(log.lines().any(|l| l.starts_with("highlight src/counter.rs")), "fields are asked how they're used:\n{log}");
    // The type named in a signature is the server's too.
    let refs = service.references(&id("sample::counter::Counter")).unwrap();
    assert!(refs.iter().any(|r| r.file == "src/counter.rs" && r.line == 14 && r.evidence == "server"), "the return type names Counter, as the server says: {refs:?}");
    // Its answer decides how a field is used: a `&mut` borrow writes, though
    // the reader alone would say it reads.
    wait_until("drain's write is the server's", || flow_proven(&service, "sample::counter::Counter::drain", "sample::counter::Counter") == by);
    assert_eq!(flow_proven(&service, "sample::counter::Counter", "sample::counter::Counter::drain"), None, "not a read");
    // A field of a parameter: which type it's on, only the server says.
    wait_until("reset's write through `c` is confirmed", || flow_proven(&service, "sample::counter::reset", "sample::counter::Counter") == by);
}

#[test]
fn a_field_use_the_server_gives_no_kind_for_stays_unconfirmed() {
    let s = setup(&["--no-kinds"]);
    let service = with_counter(&s);
    // The server says which field (the type is confirmed), not how it's
    // used: the read or write is the reader's guess, so it isn't proven.
    wait_until("the types are confirmed", || flow_proven(&service, "sample::counter::Counter::fresh", "sample::counter::Counter") == Some(Some("fake-lsp 9.9.9".into())));
    // `c`'s type comes only from the server: the link appears once it has answered.
    wait_until("reset's field is the server's", || flow_proven(&service, "sample::counter::reset", "sample::counter::Counter").is_some());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(flow_proven(&service, "sample::counter::reset", "sample::counter::Counter"), Some(None), "reset's write: the field's type is the server's, how it's used isn't");
    assert_eq!(flow_proven(&service, "sample::counter::Counter::bump", "sample::counter::Counter"), Some(None), "bump's write is the reader's");
    assert_eq!(flow_proven(&service, "sample::counter::Counter", "sample::counter::Counter::total"), Some(None), "total's read is the reader's");
    // The ref keeps the guess for display, and that the server said nothing
    // of it: linking again from the rows can't take the guess for the server's.
    let rows = service.file_rows().into_iter().find(|r| r.file.path.as_str() == "src/counter.rs").unwrap();
    let fields: Vec<_> = rows.refs.iter().filter(|r| r.kind == code_analyzer::rows::RefKind::Field).collect();
    assert!(!fields.is_empty() && fields.iter().all(|r| r.detail.access.is_some() && r.detail.server_access.is_none()), "{fields:#?}");
}

#[test]
fn a_file_pointing_into_an_ignored_file_is_settled_and_neither_a_change_elsewhere_nor_a_restart_asks_again() {
    let s = setup(&[]);
    // util.rs is ignored: the server's answers that point into it name no
    // component we keep: `local`, settled.
    std::fs::create_dir_all(s.root.join(".nah")).unwrap();
    std::fs::write(s.root.join(".nah/ignore"), "src/util.rs\n").unwrap();
    std::fs::write(s.root.join("src/lonely.rs"), "pub fn alone() {}\n").unwrap();
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    wait_until("run's call is confirmed", || proven(&service, "sample::run", "sample::store::Store::save") == Some(Some("fake-lsp 9.9.9".into())));
    std::thread::sleep(Duration::from_millis(500));
    // A change to a file nothing else depends on: only it is linked again,
    // and only it may be asked about.
    std::fs::write(&s.log, "").unwrap();
    let lonely = s.root.join("src/lonely.rs");
    std::fs::write(&lonely, "pub fn alone() {}\n\npub fn apart() {\n    alone();\n}\n").unwrap();
    service.refresh(std::slice::from_ref(&lonely)).unwrap();
    wait_until("the change is confirmed", || proven(&service, "sample::lonely::apart", "sample::lonely::alone") == Some(Some("fake-lsp 9.9.9".into())));
    std::thread::sleep(Duration::from_millis(500));
    let files: std::collections::BTreeSet<String> = asked(&s.log).into_iter().collect();
    assert_eq!(files, std::collections::BTreeSet::from(["src/lonely.rs".to_owned()]), "only the changed file");
    let lib = service.file_rows().into_iter().find(|r| r.file.path.as_str() == "src/lib.rs").unwrap();
    let into_util = lib.refs.iter().find(|r| r.name == "shout" && r.detail.call.is_some()).unwrap();
    assert_eq!((into_util.evidence, into_util.prover.as_deref()), (code_analyzer::rows::Evidence::Local, Some("fake-lsp 9.9.9")), "an answer into a file that isn't read");
    // A restart with nothing changed asks nothing.
    assert!(service.wait_for_servers(Duration::from_secs(30)).unwrap());
    drop(service);
    std::fs::write(&s.log, "").unwrap();
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(service.wait_for_servers(Duration::from_secs(30)).unwrap());
    assert!(asked(&s.log).is_empty(), "a restart asked again: {:?}", asked(&s.log));
}

#[test]
fn a_manifest_the_server_watches_is_sent_when_it_changes() {
    let s = setup(&[]);
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    wait_until("ready", || proven(&service, "sample::run", "sample::store::Store::save") == Some(Some("fake-lsp 9.9.9".into())));
    // No language reads Cargo.toml, but the server asked to hear of it.
    let manifest = s.root.join("Cargo.toml");
    std::fs::write(&manifest, format!("{}\n[dependencies]\n", std::fs::read_to_string(&manifest).unwrap())).unwrap();
    service.refresh(std::slice::from_ref(&manifest)).unwrap();
    wait_until("the server heard of it", || std::fs::read_to_string(&s.log).unwrap().lines().any(|l| l == "changed Cargo.toml 2"));
    // What it didn't ask for, it isn't told.
    let readme = s.root.join("README.md");
    std::fs::write(&readme, "# sample\n").unwrap();
    service.refresh(std::slice::from_ref(&readme)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!std::fs::read_to_string(&s.log).unwrap().contains("changed README.md"));
}

#[test]
fn an_implementation_the_server_confirms_is_kept_as_proven() {
    let s = setup(&[]);
    let service = open(&s);
    service.ingest(&s.root).unwrap();
    let implements = || service.observed().unwrap().outgoing(&id("sample::Upper"), Some(RelationKind::Implements)).find(|r| r.target == id("sample::Step")).map(|r| r.proven_by.clone());
    wait_until("Upper implements Step, as the server says", || implements() == Some(Some("fake-lsp 9.9.9".into())));
}
