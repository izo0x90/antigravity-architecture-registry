//! The real rust-analyzer, not a stand-in: it confirms the Rust fixture's
//! calls, the types its data carries and how its fields are used, and a
//! warm server confirms one changed file within limits.toml's confirm_ms.
//! Machine-dependent: it needs rust-analyzer in the shipped pack's
//! supported range (installed, or on PATH), and without it says so and skips.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arch_graph::{ComponentId, RelationKind, TypeSystem};
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

fn id(s: &str) -> ComponentId {
    ComponentId::for_symbol("rust", s)
}

/// The relation's prover (None: no such relation; Some(None): not proven).
fn prover(service: &ArchitectureService, kind: RelationKind, from: &str, to: &str) -> Option<Option<String>> {
    service.observed().unwrap().outgoing(&id(from), Some(kind)).find(|r| r.target == id(to)).map(|r| r.proven_by.clone())
}

fn by_rust_analyzer(p: Option<Option<String>>) -> bool {
    p.flatten().is_some_and(|by| by.starts_with("rust-analyzer"))
}

fn wait_until(what: &str, limit: Duration, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

const COUNTER: &str = "pub struct Counter {\n    pub count: u32,\n}\n\nimpl Counter {\n    pub fn bump(&mut self) {\n        self.count += 1;\n    }\n\n    pub fn total(&self) -> u32 {\n        self.count\n    }\n}\n\npub fn reset(c: &mut Counter) {\n    c.count = 0;\n}\n\npub fn peek(c: &Counter) -> u32 {\n    c.count\n}\n";

#[test]
fn rust_analyzer_confirms_the_fixtures_links_and_a_warm_change_in_time_needs_rust_analyzer() {
    let dir = tempfile::tempdir().unwrap();
    let root: PathBuf = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample"), &root);
    std::fs::write(root.join("src/counter.rs"), COUNTER).unwrap();
    let lib = root.join("src/lib.rs");
    std::fs::write(&lib, format!("pub mod counter;\n{}", std::fs::read_to_string(&lib).unwrap())).unwrap();
    // The shipped packs, the server's caches in this test's own folder.
    let places = nah_packs::Places::isolated(&dir.path().join("home"));
    let catalog = nah_packs::Catalog::load(&nah_packs::Distribution::from_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")).unwrap(), &places).unwrap();
    match nah_packs::state(&catalog, &nah_packs::Lock::default(), "rust-analyzer") {
        Ok(state) if state.is_ready() => {}
        other => return eprintln!("skipped: rust-analyzer isn't usable here ({other:?})"),
    }
    let limits = arch_service::Limits::defaults().unwrap();
    let service = ArchitectureService::open(&root, code_analyzer::PackSet::from_catalog(&catalog).unwrap(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), arch_service::temporary_facts(&limits).unwrap())
        .unwrap()
        .with_limits(limits.clone())
        .with_servers(&catalog);
    service.ingest(&root).unwrap();
    let ready = Duration::from_secs(limits.servers.ready_s);
    // Calls: on a value made here, through a typed parameter, through a path.
    // (`ambiguous()` calls a `helper` two glob imports bring: Rust itself
    // rejects that, so the server has nothing to say.)
    wait_until("run's call to Store::save is confirmed", ready, || by_rust_analyzer(prover(&service, RelationKind::Calls, "sample::run", "sample::store::Store::save")));
    wait_until("keep's call through its parameter", ready, || by_rust_analyzer(prover(&service, RelationKind::Calls, "sample::keep", "sample::store::Store::save")));
    wait_until("apply's call through util::", ready, || by_rust_analyzer(prover(&service, RelationKind::Calls, "sample::Upper::apply", "sample::util::shout")));
    // Fields: written and read through `self`, and through a parameter.
    wait_until("bump writes Counter", ready, || by_rust_analyzer(prover(&service, RelationKind::DataFlow, "sample::counter::Counter::bump", "sample::counter::Counter")));
    wait_until("total reads Counter", ready, || by_rust_analyzer(prover(&service, RelationKind::DataFlow, "sample::counter::Counter", "sample::counter::Counter::total")));
    wait_until("reset writes Counter through `c`", ready, || by_rust_analyzer(prover(&service, RelationKind::DataFlow, "sample::counter::reset", "sample::counter::Counter")));
    wait_until("peek reads Counter through `c`", ready, || by_rust_analyzer(prover(&service, RelationKind::DataFlow, "sample::counter::Counter", "sample::counter::peek")));
    // A value made, of the type the server says.
    wait_until("Store::new makes a Store", ready, || by_rust_analyzer(prover(&service, RelationKind::DataFlow, "sample::store::Store::new", "sample::store::Store")));
    // Imports and paths written: which component each names, as it says.
    wait_until("lib.rs's `use crate::store::Store` names Store", ready, || service.references(&id("sample::store::Store")).unwrap().iter().any(|r| r.file == "src/lib.rs" && r.line == 5 && r.evidence == "server"));
    let shout_line = std::fs::read_to_string(&lib).unwrap().lines().position(|l| l.contains("util::shout(input)")).unwrap() as u32 + 1;
    wait_until("`util` in `util::shout` names the module", ready, || service.references(&id("sample::util")).unwrap().iter().any(|r| r.file == "src/lib.rs" && r.kind == "path" && r.line == shout_line && r.evidence == "server"));
    // Warm: one changed file confirmed again within confirm_ms, from the
    // change on (read, link, ask, answer).
    assert!(prover(&service, RelationKind::Calls, "sample::yelled", "sample::store::helper").is_none());
    std::fs::write(&lib, std::fs::read_to_string(&lib).unwrap().replace("    yell(input)\n", "    store::helper();\n    yell(input)\n")).unwrap();
    let started = Instant::now();
    service.refresh(std::slice::from_ref(&lib)).unwrap();
    wait_until("the change is confirmed", Duration::from_secs(60), || by_rust_analyzer(prover(&service, RelationKind::Calls, "sample::yelled", "sample::store::helper")));
    let took = started.elapsed();
    println!("rust-analyzer confirmed the changed file in {took:?} (limit {} ms)", limits.servers.confirm_ms);
    assert!(took <= Duration::from_millis(limits.servers.confirm_ms), "{took:?} over confirm_ms ({} ms)", limits.servers.confirm_ms);
}
