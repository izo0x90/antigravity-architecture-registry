//! Performance on real code: this repository and a pinned big one (tokio).
//! Each measured step is timed and checked against its limit; a one-file
//! change is held to an absolute limit here and to a share of the full
//! index on tokio, with a plan loaded.

use std::path::{Path, PathBuf};
use std::time::Instant;

use arch_graph::tree::{TreeRequest, ViewKind, Way, problems};
use arch_graph::TypeSystem;
use code_analyzer::PackSet;
use arch_service::{ArchitectureService, FsPlans, SearchKind};

mod common;

/// The big repository: tokio at a fixed tag, fetched once into the cache.
const BIG_REPO: &str = "https://github.com/tokio-rs/tokio.git";
const BIG_TAG: &str = "tokio-1.47.1";

/// Tokio, fetched over the network the first time; why not, when it
/// can't be.
fn big_repo() -> Result<PathBuf, String> {
    let dir = std::env::var_os("NAH_BENCH_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()).join(".cache/nah-bench"));
    let repo = dir.join("tokio");
    if !repo.join(".git").exists() {
        std::fs::create_dir_all(&dir).unwrap();
        let ok = std::process::Command::new("git")
            .args(["clone", "--quiet", "--depth", "1", "--branch", BIG_TAG, BIG_REPO])
            .arg(&repo)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            let _ = std::fs::remove_dir_all(&repo);
            return Err(format!("couldn't fetch {BIG_REPO} at {BIG_TAG} into {} (git and the network)", repo.display()));
        }
    }
    Ok(repo)
}

/// A copy of `root`'s sources and manifests in a temp dir, so a test can
/// change one without touching the real tree (see `common::copy_sources`).
fn copy_sources(root: &Path, packs: &PackSet) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    common::copy_sources(root, dir.path(), packs, std::time::SystemTime::now() - std::time::Duration::from_secs(3600));
    dir
}

/// How a one-file change is held to its gate.
#[derive(Clone, Copy)]
enum OneFile {
    /// At most `one_file_ms`.
    Absolute,
    /// At most `one_file_share` of the full index.
    Share,
}

/// A plan that adds a function under the code's first module, so every
/// measurement runs with a plan loaded.
fn load_plan(service: &ArchitectureService) -> arch_graph::PlanId {
    let module = service.observed().unwrap().components().find(|c| c.type_name == "module").map(|c| c.id.clone()).unwrap();
    let plan = service.create_plan("perf", "Perf", "", arch_graph::PlanKind::Change).unwrap();
    service
        .apply(
            &plan.id,
            arch_service::PlanOp::AddComponent {
                name: "planned_for_perf".into(),
                type_name: "function".into(),
                description: "Planned, to measure with a plan loaded.".into(),
                parent: Some(module),
                file: None,
                signature: None,
                schemas: None,
                spec: None,
            },
        )
        .unwrap();
    plan.id
}

/// The service over `root`, its plans kept in the copy (so a restart has them).
fn open(root: &Path) -> ArchitectureService {
    let facts = arch_service::open_facts(root, &arch_service::Limits::defaults().unwrap()).unwrap();
    ArchitectureService::open(root, common::shipped_packs(), TypeSystem::defaults().unwrap(), Box::new(FsPlans::new(root).unwrap()), facts).unwrap()
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

fn measure(name: &str, root: &Path, gate: OneFile) {
    let packs = common::shipped_packs();
    let copy = copy_sources(root, &packs);
    let root = copy.path();
    let service = open(root);
    let limits = service.limits().clone();
    let t = Instant::now();
    let report = service.ingest(root).unwrap();
    let took = ms(t);
    println!("perf {name} full_index ms={took} files={} components={} relations={}", report.files, report.components, report.relations);
    assert!(took <= limits.perf.full_index_ms, "{name}: full index took {took} ms");
    let full = took;
    let plan = load_plan(&service);
    let one_file_limit = match gate {
        OneFile::Absolute => limits.perf.one_file_ms,
        OneFile::Share => (full as f64 * limits.perf.one_file_share) as u64,
    };

    // One file changed: the biggest Rust file gets a comment, and the
    // watcher's path takes it (with the plan loaded).
    let file = code_analyzer::walk(root, root, &packs).unwrap().into_iter().filter(|f| f.path.as_str().ends_with(".rs")).max_by_key(|f| std::fs::metadata(&f.absolute).map(|m| m.len()).unwrap_or(0)).unwrap();
    let text = std::fs::read_to_string(&file.absolute).unwrap();
    std::fs::write(&file.absolute, format!("{text}\n// changed\n")).unwrap();
    let t = Instant::now();
    let one = service.refresh(std::slice::from_ref(&file.absolute)).unwrap().ingest;
    let took = ms(t);
    println!("perf {name} one_file ms={took} limit={one_file_limit} full={full} file={} parsed={} read={}", file.path, one.parsed, one.read);
    assert_eq!((one.read, one.parsed), (1, 1), "{name}: {one:?}");
    assert!(took <= one_file_limit, "{name}: one file took {took} ms (limit {one_file_limit}; full index {full} ms)");
    // A change others can see: a new function (its name is new to every
    // file that might use it).
    std::fs::write(&file.absolute, format!("{text}\n// changed\npub fn added_by_the_perf_test() {{}}\n")).unwrap();
    let t = Instant::now();
    let item = service.refresh(std::slice::from_ref(&file.absolute)).unwrap().ingest;
    let took = ms(t);
    println!("perf {name} one_file_new_item ms={took} limit={one_file_limit} linked={}", item.linked.len());
    assert!(took <= one_file_limit, "{name}: a new function took {took} ms (limit {one_file_limit})");
    // The same change found by re-checking the whole repo.
    std::fs::write(&file.absolute, format!("{text}\n// changed again\n")).unwrap();
    let t = Instant::now();
    let whole = service.ingest(root).unwrap();
    let took = ms(t);
    println!("perf {name} one_file_in_whole_repo ms={took} read={} parsed={}", whole.read, whole.parsed);
    assert_eq!((whole.read, whole.parsed), (1, 1), "{name}: only the changed file is read: {whole:?}");
    assert!(took <= one_file_limit, "{name}: one file found in the whole repo took {took} ms (limit {one_file_limit})");
    drop(service);
    // Starting again from the kept facts (no file read), until the graph is
    // there to read (opening loads the rows on first use).
    let t = Instant::now();
    let service = open(root);
    let components = service.observed().unwrap().len().0;
    let took = ms(t);
    println!("perf {name} restart ms={took} components={components}");
    assert!(took <= limits.perf.full_index_ms, "{name}: restart took {took} ms");

    trees(name, &service);
    // The plan's own trees.
    for view in ViewKind::ALL {
        let t = Instant::now();
        let tree = service.tree(&TreeRequest { view, source: arch_graph::Source::Plan(plan.clone()), depth: Some(4), ..TreeRequest::default() }).unwrap();
        println!("perf {name} plan_tree view={} ms={} cards={}", view.name(), ms(t), tree.cards.len());
        assert!(ms(t) <= limits.perf.tree_ms, "{name}: the plan's {} tree took {} ms", view.name(), ms(t));
    }
    for (kind, query) in [(SearchKind::Symbols, "view"), (SearchKind::Symbols, "servce::serch"), (SearchKind::Files, "service"), (SearchKind::Text, "fn new")] {
        let t = Instant::now();
        let found = service.search(query, kind, &arch_graph::Source::Code, None, None).unwrap();
        let took = ms(t);
        println!("perf {name} search kind={} query={query:?} ms={took} hits={} total={}", kind.name(), found.hits.len(), found.total);
        assert!(took <= limits.perf.search_ms, "{name}: searching {} for {query:?} took {took} ms", kind.name());
    }
}

#[test]
fn this_repo_and_tokio_stay_within_their_limits_needs_network() {
    let this = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    measure("this_repo", &this, OneFile::Absolute);
    // Machine-dependent: tokio comes over the network the first time.
    let tokio = match big_repo() {
        Ok(tokio) => tokio,
        Err(why) => return eprintln!("skipped tokio: {why}"),
    };
    // The corpus is pinned: tokio at its tag, read by the shipped packs.
    let packs = common::shipped_packs();
    assert_eq!(code_analyzer::walk(&tokio, &tokio, &packs).unwrap().len(), 717, "the tokio corpus changed");
    measure("tokio", &tokio, OneFile::Share);
}
