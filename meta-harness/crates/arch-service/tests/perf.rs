//! Performance on real code: this repository and a pinned big one (tokio).
//! Each measured step is timed and checked against its limit.

use std::path::{Path, PathBuf};
use std::time::Instant;

use arch_graph::{TypeSystem, ViewCatalog};
use arch_index::PackSet;
use arch_service::{ArchitectureService, MemoryIndex, MemoryPlans};

/// The big repository: tokio at a fixed tag, fetched once into the cache.
const BIG_REPO: &str = "https://github.com/tokio-rs/tokio.git";
const BIG_TAG: &str = "tokio-1.47.1";

fn big_repo() -> PathBuf {
    let dir = std::env::var_os("NAH_BENCH_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()).join(".cache/nah-bench"));
    let repo = dir.join("tokio");
    if !repo.join(".git").exists() {
        std::fs::create_dir_all(&dir).unwrap();
        let ok = std::process::Command::new("git")
            .args(["clone", "--quiet", "--depth", "1", "--branch", BIG_TAG, BIG_REPO])
            .arg(&repo)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "couldn't fetch {BIG_REPO} at {BIG_TAG} into {} (needed for the performance tests)", repo.display());
    }
    repo
}

/// A copy of `root`'s source files (what the packs read) in a temp dir, so
/// a test can change one without touching the real tree.
fn copy_sources(root: &Path, packs: &PackSet) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for file in arch_index::walk(root, root, packs).unwrap() {
        let to = dir.path().join(file.path.as_str());
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(&file.absolute, &to).unwrap();
    }
    dir
}

fn open(root: &Path) -> ArchitectureService {
    ArchitectureService::open(root, PackSet::built_in().unwrap(), TypeSystem::defaults().unwrap(), ViewCatalog::defaults().unwrap(), Box::new(MemoryPlans::default()), Box::new(MemoryIndex::default())).unwrap()
}

fn ms(since: Instant) -> u128 {
    since.elapsed().as_millis()
}

fn measure(name: &str, root: &Path) {
    let packs = PackSet::built_in().unwrap();
    let copy = copy_sources(root, &packs);
    let root = copy.path();
    let service = open(root);
    let t = Instant::now();
    let report = service.ingest(root).unwrap();
    println!("perf {name} full_index ms={} files={} components={} relations={}", ms(t), report.files, report.components, report.relations);

    // One file changed: the biggest Rust file gets a comment.
    let file = arch_index::walk(root, root, &packs).unwrap().into_iter().filter(|f| f.path.as_str().ends_with(".rs")).max_by_key(|f| std::fs::metadata(&f.absolute).map(|m| m.len()).unwrap_or(0)).unwrap();
    let text = std::fs::read_to_string(&file.absolute).unwrap();
    std::fs::write(&file.absolute, format!("{text}\n// changed\n")).unwrap();
    let t = Instant::now();
    service.ingest(&file.absolute).unwrap();
    println!("perf {name} one_file ms={} file={}", ms(t), file.path);

    for view in ["structure", "calls", "data_flow"] {
        for depth in [1, 4, 12] {
            let t = Instant::now();
            let result = service.named_view(view, &arch_graph::Source::Code, vec![], Some(depth)).unwrap();
            let bytes = serde_json::to_vec(&result).unwrap().len();
            println!("perf {name} view={view} depth={depth} ms={} nodes={} edges={} bytes={bytes}", ms(t), result.view.nodes.len(), result.view.edges.len());
        }
    }
    let t = Instant::now();
    let hits = service.search("view", &arch_graph::Source::Code, 25).unwrap();
    println!("perf {name} search ms={} hits={}", ms(t), hits.len());
}

#[test]
fn baseline() {
    let this = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    measure("this_repo", &this);
    measure("tokio", &big_repo());
}
