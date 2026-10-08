//! Performance on real code: this repository and a pinned big one (tokio).
//! Each measured step is timed and checked against its limit.

use std::path::{Path, PathBuf};
use std::time::Instant;

use arch_graph::tree::{TreeRequest, ViewKind, Way, problems};
use arch_graph::TypeSystem;
use arch_index::PackSet;
use arch_service::{ArchitectureService, MemoryPlans};

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
    let facts = arch_service::open_facts(root, &arch_service::Limits::defaults().unwrap()).unwrap();
    ArchitectureService::open(root, PackSet::built_in().unwrap(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), Box::new(facts)).unwrap()
}

fn ms(since: Instant) -> u64 {
    since.elapsed().as_millis() as u64
}

/// Every view, both ways, at depths 1, 4 and 12, from its natural starting
/// points and from the card with the most under it: within the time limit,
/// the card budget, and laid out clean.
fn trees(name: &str, service: &ArchitectureService) {
    let limits = service.limits().clone();
    let check = |request: TreeRequest| {
        let t = Instant::now();
        let tree = service.tree(&request).unwrap();
        let took = ms(t);
        let bytes = serde_json::to_vec(&tree).unwrap().len();
        println!("perf {name} tree view={} way={:?} root={} depth={:?} ms={took} cards={} lines={} bytes={bytes} cut={:?}", request.view.name(), request.way, request.root.is_some(), request.depth, tree.cards.len(), tree.lines.len(), tree.cut.as_ref().map(|c| (c.shown, c.asked, c.hidden)));
        assert!(took <= limits.perf.tree_ms, "{name}: {request:?} took {took} ms (limit {})", limits.perf.tree_ms);
        assert!(tree.cards.len() <= limits.cards_per_answer, "{name}: {} cards", tree.cards.len());
        let found = problems(&tree, &service.trees().layout);
        assert!(found.is_empty(), "{name}: {request:?}: {:#?}", &found[..found.len().min(10)]);
        tree
    };
    for view in ViewKind::ALL {
        for way in [Way::Down, Way::Up] {
            for depth in [1, 4, 12] {
                let tree = check(TreeRequest { view, way, depth: Some(depth), ..TreeRequest::default() });
                // The busiest card as a root.
                if let Some(busiest) = tree.cards.iter().max_by_key(|c| c.more).filter(|c| c.more > 0) {
                    check(TreeRequest { view, way, depth: Some(depth), root: Some(busiest.key.clone()), ..TreeRequest::default() });
                    let t = Instant::now();
                    service.expand(&TreeRequest { view, way, depth: Some(1), root: Some(busiest.key.clone()), ..TreeRequest::default() }, &busiest.key, 1).unwrap();
                    assert!(ms(t) <= limits.perf.tree_ms * 2, "{name}: expanding {} took {} ms", busiest.key, ms(t));
                }
            }
        }
    }
}

fn measure(name: &str, root: &Path) {
    let packs = PackSet::built_in().unwrap();
    let copy = copy_sources(root, &packs);
    let root = copy.path();
    let service = open(root);
    let limits = service.limits().clone();
    let t = Instant::now();
    let report = service.ingest(root).unwrap();
    let took = ms(t);
    println!("perf {name} full_index ms={took} files={} components={} relations={}", report.files, report.components, report.relations);
    assert!(took <= limits.perf.full_index_ms, "{name}: full index took {took} ms");

    // One file changed: the biggest Rust file gets a comment.
    let file = arch_index::walk(root, root, &packs).unwrap().into_iter().filter(|f| f.path.as_str().ends_with(".rs")).max_by_key(|f| std::fs::metadata(&f.absolute).map(|m| m.len()).unwrap_or(0)).unwrap();
    let text = std::fs::read_to_string(&file.absolute).unwrap();
    std::fs::write(&file.absolute, format!("{text}\n// changed\n")).unwrap();
    let t = Instant::now();
    let one = service.ingest(&file.absolute).unwrap();
    let took = ms(t);
    println!("perf {name} one_file ms={took} file={} parsed={}", file.path, one.parsed);
    assert!(took <= limits.perf.one_file_ms, "{name}: one file took {took} ms");
    // The same change found by re-checking the whole repo.
    std::fs::write(&file.absolute, format!("{text}\n// changed again\n")).unwrap();
    let t = Instant::now();
    let whole = service.ingest(root).unwrap();
    let took = ms(t);
    println!("perf {name} one_file_in_whole_repo ms={took} read={} parsed={}", whole.read, whole.parsed);
    assert_eq!(whole.parsed, 1, "{name}: {whole:?}");
    assert!(took <= limits.perf.one_file_ms, "{name}: one file found in the whole repo took {took} ms");
    drop(service);
    // Starting again from the kept facts (no file read).
    let t = Instant::now();
    let service = open(root);
    let took = ms(t);
    println!("perf {name} restart ms={took} components={}", service.observed().unwrap().len().0);
    assert!(took <= limits.perf.full_index_ms, "{name}: restart took {took} ms");

    trees(name, &service);
    let t = Instant::now();
    let hits = service.search("view", &arch_graph::Source::Code, 25).unwrap();
    let took = ms(t);
    println!("perf {name} search ms={took} hits={}", hits.len());
    assert!(took <= limits.perf.search_ms, "{name}: search took {took} ms");
}

// req: M.1, M.7, G.7, G.9
#[test]
fn this_repo_and_tokio_stay_within_their_limits() {
    let this = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    measure("this_repo", &this);
    measure("tokio", &big_repo());
}
