//! Reading the code again after a change: only what changed is read, the
//! result is the same as reading everything, and the facts survive a restart.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use arch_graph::{Graph, TypeSystem};
use arch_service::{ArchitectureService, Limits, MemoryPlans, MemoryStore, open_facts};

fn sample() -> PathBuf {
    let root = tempfile::tempdir().unwrap().keep().join("sample");
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample");
    for entry in walk(&from) {
        let to = root.join(entry.strip_prefix(&from).unwrap());
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(&entry, &to).unwrap();
    }
    age(&root, LAST_YEAR);
    root
}

/// A time well before any test runs (seconds since the epoch).
const LAST_YEAR: u64 = 1_700_000_000;

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { out.extend(walk(&path)) } else { out.push(path) }
    }
    out
}

/// Files written a moment ago aren't trusted by their time; these are dated
/// back, as files are that weren't just edited.
fn age(root: &Path, at: u64) {
    for file in walk(root).into_iter().filter(|f| !f.components().any(|c| c.as_os_str() == ".nah")) {
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(at)).unwrap();
    }
}

fn open(root: &Path, kept: bool) -> ArchitectureService {
    let facts: Box<dyn arch_service::FactStore> = match kept {
        true => Box::new(open_facts(root, &Limits::defaults().unwrap()).unwrap()),
        false => Box::new(MemoryStore::default()),
    };
    ArchitectureService::open(root, code_analyzer::PackSet::from_shipped_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")).unwrap(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), facts).unwrap()
}

/// The graph as text, in a fixed order.
fn text(graph: &Graph) -> String {
    let mut components: Vec<String> = graph.components().map(|c| serde_json::to_string(c).unwrap()).collect();
    let mut relations: Vec<String> = graph.relations().map(|r| serde_json::to_string(r).unwrap()).collect();
    components.sort();
    relations.sort();
    format!("{}\n{}", components.join("\n"), relations.join("\n"))
}

// req: M.12
#[test]
fn a_one_file_change_reads_only_that_file_and_gives_the_same_graph_as_reading_everything() {
    let root = sample();
    let service = open(&root, true);
    let first = service.ingest(&root).unwrap();
    assert!(first.parsed >= 3, "{first:?}");

    // Nothing changed: no file is read at all.
    let again = service.ingest(&root).unwrap();
    assert_eq!((again.read, again.parsed, again.cached), (0, 0, first.files), "{again:?}");

    // One file changes (and gains a function that calls into another file).
    let util = root.join("src/util.rs");
    std::fs::write(&util, "pub fn shout(s: &str) -> String {\n    s.to_uppercase()\n}\n\npub fn helper() {}\n\npub fn louder(s: &str) -> String {\n    shout(&shout(s))\n}\n").unwrap();
    age(&root, LAST_YEAR);
    let one = service.ingest(&root).unwrap();
    assert_eq!((one.read, one.parsed), (1, 1), "only util.rs is read: {one:?}");
    assert_eq!(one.changed.iter().map(|f| f.as_str()).collect::<Vec<_>>(), ["src/util.rs"]);

    // The same graph as reading everything afresh.
    let fresh = open(&root, false);
    fresh.ingest(&root).unwrap();
    assert_eq!(text(&service.observed().unwrap()), text(&fresh.observed().unwrap()));
    assert!(text(&service.observed().unwrap()).contains("louder"));

    // Touched without changing: read once to compare, then trusted again.
    age(&root, LAST_YEAR + 60);
    let touched = service.ingest(&root).unwrap();
    assert_eq!((touched.read, touched.parsed), (first.files, 0), "{touched:?}");
    assert_eq!(service.ingest(&root).unwrap().read, 0);

    // A file that's gone leaves.
    std::fs::remove_file(&util).unwrap();
    let gone = service.ingest(&root).unwrap();
    assert_eq!(gone.removed, 1);
    assert!(!text(&service.observed().unwrap()).contains("louder"));
}

// req: M.12
#[test]
fn facts_kept_from_the_last_run_give_the_graph_without_reading_a_file() {
    let root = sample();
    let before = {
        let service = open(&root, true);
        service.ingest(&root).unwrap();
        text(&service.observed().unwrap())
    };
    let service = open(&root, true);
    assert_eq!(text(&service.observed().unwrap()), before, "linked from the kept facts at start-up");
    assert_eq!(service.ingest(&root).unwrap().read, 0, "and nothing is read to catch up");
    assert!(root.join(".nah/cache/index.sqlite").is_file());
}

// req: M.11
#[test]
fn the_old_json_cache_is_removed() {
    let root = sample();
    std::fs::create_dir_all(root.join(".nah/cache/index")).unwrap();
    std::fs::write(root.join(".nah/cache/index/src_sa.rs.json"), "{}").unwrap();
    open(&root, true);
    assert!(!root.join(".nah/cache/index").exists());
}

// req: M.12
#[test]
fn a_file_changed_just_now_isnt_trusted_by_its_time() {
    let root = sample();
    let service = open(&root, true);
    service.ingest(&root).unwrap();
    // Same size, written now: its time can't tell a second edit in the same tick.
    std::fs::write(root.join("src/util.rs"), std::fs::read_to_string(root.join("src/util.rs")).unwrap().replace("helper", "helpme")).unwrap();
    let first = service.ingest(&root).unwrap();
    assert_eq!(first.parsed, 1);
    std::fs::write(root.join("src/util.rs"), std::fs::read_to_string(root.join("src/util.rs")).unwrap().replace("helpme", "helpus")).unwrap();
    let second = service.ingest(&root).unwrap();
    assert_eq!(second.parsed, 1, "the second edit is seen: {second:?}");
    assert!(text(&service.observed().unwrap()).contains("helpus"));
}
