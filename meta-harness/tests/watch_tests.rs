//! The watcher: its debounce window comes
//! from limits.toml, a burst of saves is one batch, an ignored directory
//! says nothing, and Changed is sent only when the graph changed.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use arch_graph::{ArchEvent, TypeSystem};
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

fn service(root: &Path) -> Arc<ArchitectureService> {
    let packs = common::shipped_packs();
    let limits = arch_service::Limits::defaults().unwrap();
    let service = ArchitectureService::open(
        root,
        packs,
        TypeSystem::defaults().unwrap(),
        Box::new(MemoryPlans::default()),
        arch_service::open_facts(root, &limits).unwrap(),
    )
    .unwrap();
    service.ingest(root).unwrap();
    Arc::new(service)
}

fn changed_events(service: &ArchitectureService) -> Arc<AtomicUsize> {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    service.subscribe(move |e| {
        if matches!(e, ArchEvent::Changed { .. }) {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    });
    count
}

#[test]
fn the_debounce_window_comes_from_limits_and_no_env_var() {
    assert!(arch_service::Limits::defaults().unwrap().watch.debounce_ms > 0);
    for file in ["src/main.rs", "src/watch.rs"] {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
        assert!(!text.contains("NAH_WATCH_MS"), "{file} still reads the window from the environment");
    }
    // The project's limits decide it: a window of 0 turns watching off.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/code-analyzer/tests/fixtures/rust/sample"), &root);
    let limits = arch_service::Limits::with_overrides("[limits.watch]\ndebounce_ms = 0\n").unwrap();
    let packs = common::shipped_packs();
    let arch = Arc::new(
        ArchitectureService::open(
            &root,
            packs,
            TypeSystem::defaults().unwrap(),
            Box::new(MemoryPlans::default()),
            arch_service::temporary_facts(&arch_service::Limits::defaults().unwrap()).unwrap(),
        )
        .unwrap()
        .with_limits(limits),
    );
    arch.ingest(&root).unwrap();
    let changed = changed_events(&arch);
    meta_harness::watch::watch(arch.clone()).unwrap();
    let util = root.join("src/util.rs");
    std::fs::write(&util, std::fs::read_to_string(&util).unwrap() + "\npub fn unseen() {}\n").unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(changed.load(Ordering::SeqCst), 0, "not watching");
}

#[test]
fn a_burst_of_saves_is_one_batch_and_an_ignored_directory_says_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/code-analyzer/tests/fixtures/rust/sample"), &root);
    std::fs::write(root.join(".gitignore"), "build_output/\n").unwrap();
    let arch = service(&root);
    let changed = changed_events(&arch);
    meta_harness::watch::watch(arch.clone()).unwrap();
    let window = Duration::from_millis(arch.limits().watch.debounce_ms);
    std::thread::sleep(Duration::from_millis(200));
    // An ignored directory appearing and filling: nothing.
    std::fs::create_dir_all(root.join("build_output/deep")).unwrap();
    std::fs::write(root.join("build_output/deep/x.rs"), "pub fn x() {}\n").unwrap();
    std::thread::sleep(window * 4);
    assert_eq!(changed.load(Ordering::SeqCst), 0, "an ignored directory changed nothing");
    assert!(arch.index_status().stale.is_empty(), "{:?}", arch.index_status().stale);
    // Three saves in a burst: one batch, one Changed.
    for (file, text) in
        [("src/util.rs", "\npub fn one() {}\n"), ("src/store.rs", "\npub fn two() {}\n"), ("src/util.rs", "\npub fn three() {}\n")]
    {
        let path = root.join(file);
        let old = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{old}{text}")).unwrap();
        std::thread::sleep(window / 5);
    }
    std::thread::sleep(window * 6);
    assert_eq!(changed.load(Ordering::SeqCst), 1, "one batch for one burst");
    assert!(arch.observed().unwrap().components().any(|c| c.name == "three"));
}

#[test]
fn changed_is_sent_only_when_the_graph_changed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/code-analyzer/tests/fixtures/rust/sample"), &root);
    let arch = service(&root);
    let changed = changed_events(&arch);
    // Reading the repo again with nothing changed.
    arch.ingest(&root).unwrap();
    // A file looked at again, unchanged.
    arch.refresh(&[root.join("src/util.rs")]).unwrap();
    // A file touched: a new time, the same content.
    let touched = std::fs::File::options().write(true).open(root.join("src/util.rs")).unwrap();
    touched.set_modified(std::time::SystemTime::now() - Duration::from_secs(60)).unwrap();
    drop(touched);
    let report = arch.refresh(&[root.join("src/util.rs")]).unwrap();
    assert!(report.ingest.changed.is_empty(), "{:?}", report.ingest.changed);
    assert_eq!(changed.load(Ordering::SeqCst), 0, "nothing changed, nothing said");
    let util = root.join("src/util.rs");
    let old = std::fs::read_to_string(&util).unwrap();
    std::fs::write(&util, format!("{old}\npub fn added() {{}}\n")).unwrap();
    arch.refresh(&[util]).unwrap();
    assert_eq!(changed.load(Ordering::SeqCst), 1);
}

#[test]
fn a_build_writing_ignored_output_never_holds_back_a_save() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/code-analyzer/tests/fixtures/rust/sample"), &root);
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    let arch = service(&root);
    meta_harness::watch::watch(arch.clone()).unwrap();
    let window = Duration::from_millis(arch.limits().watch.debounce_ms);
    std::thread::sleep(Duration::from_millis(200));
    // A build keeps writing its output for a long time.
    let building = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let writer = {
        let (building, target) = (building.clone(), root.join("target/debug"));
        std::fs::create_dir_all(&target).unwrap();
        std::thread::spawn(move || {
            let mut i = 0;
            while building.load(Ordering::SeqCst) {
                std::fs::write(target.join(format!("out_{}.o", i % 20)), format!("{i}")).unwrap();
                i += 1;
                std::thread::sleep(window / 6);
            }
        })
    };
    std::thread::sleep(window);
    let util = root.join("src/util.rs");
    let old = std::fs::read_to_string(&util).unwrap();
    std::fs::write(&util, format!("{old}\npub fn saved_mid_build() {{}}\n")).unwrap();
    let deadline = std::time::Instant::now() + window * 8;
    while !arch.observed().unwrap().components().any(|c| c.name == "saved_mid_build") && std::time::Instant::now() < deadline {
        std::thread::sleep(window / 4);
    }
    let read = arch.observed().unwrap().components().any(|c| c.name == "saved_mid_build");
    building.store(false, Ordering::SeqCst);
    writer.join().unwrap();
    assert!(read, "the save was read while the build was still writing");
}

#[test]
fn a_rule_above_the_project_changes_what_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let top = dir.path().join("repo");
    assert!(std::process::Command::new("git").args(["init", "-q"]).arg(&top).status().unwrap().success());
    let root = top.join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/code-analyzer/tests/fixtures/rust/sample"), &root);
    std::fs::create_dir_all(root.join("src/extra")).unwrap();
    std::fs::write(root.join("src/extra/more.rs"), "pub fn from_extra() {}\n").unwrap();
    let arch = service(&root);
    assert!(arch.observed().unwrap().components().any(|c| c.name == "from_extra"));
    meta_harness::watch::watch(arch.clone()).unwrap();
    let window = Duration::from_millis(arch.limits().watch.debounce_ms);
    std::thread::sleep(Duration::from_millis(200));
    // The repository's own .gitignore, above the project, starts ignoring it.
    std::fs::write(top.join(".gitignore"), "sample/src/extra/\n").unwrap();
    let deadline = std::time::Instant::now() + window * 10;
    while arch.observed().unwrap().components().any(|c| c.name == "from_extra") && std::time::Instant::now() < deadline {
        std::thread::sleep(window / 4);
    }
    assert!(!arch.observed().unwrap().components().any(|c| c.name == "from_extra"), "a rule above the project was seen");
    // The rule taken back: read again.
    std::fs::write(top.join(".gitignore"), "").unwrap();
    let deadline = std::time::Instant::now() + window * 10;
    while !arch.observed().unwrap().components().any(|c| c.name == "from_extra") && std::time::Instant::now() < deadline {
        std::thread::sleep(window / 4);
    }
    assert!(arch.observed().unwrap().components().any(|c| c.name == "from_extra"), "dropped again, read again");
}

#[test]
fn a_touch_seen_by_the_watcher_says_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/code-analyzer/tests/fixtures/rust/sample"), &root);
    let arch = service(&root);
    let events = Arc::new(AtomicUsize::new(0));
    let seen = events.clone();
    arch.subscribe(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    meta_harness::watch::watch(arch.clone()).unwrap();
    let window = Duration::from_millis(arch.limits().watch.debounce_ms);
    std::thread::sleep(Duration::from_millis(200));
    let util = root.join("src/util.rs");
    std::fs::File::options().write(true).open(&util).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(30)).unwrap();
    std::thread::sleep(window * 6);
    assert_eq!(events.load(Ordering::SeqCst), 0, "a touch: no event of any kind");
}
