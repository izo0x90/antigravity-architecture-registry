//! The code view isn't blocked by a write, on a copy of this repository's sources. Alone in
//! its binary: the other gate tests reading the repo at the same time would
//! take the cores it measures.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use arch_graph::TypeSystem;
use arch_service::{ArchitectureService, MemoryPlans};

mod common;

fn packs() -> code_analyzer::PackSet {
    common::shipped_packs()
}

fn open(root: &Path) -> ArchitectureService {
    let facts = arch_service::open_facts(root, &arch_service::Limits::defaults().unwrap()).unwrap();
    ArchitectureService::open(root, packs(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), facts).unwrap()
}

/// This repository's sources and manifests in a temp dir, dated an hour back, read once.
fn this_repo() -> (tempfile::TempDir, ArchitectureService) {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let dir = tempfile::tempdir().unwrap();
    common::copy_sources(&here, dir.path(), &packs(), SystemTime::now() - Duration::from_secs(3600));
    let service = open(dir.path());
    service.ingest(dir.path()).unwrap();
    (dir, service)
}

/// Rust files by how much other files use what they define (from the graph).
fn files_by_use(service: &ArchitectureService) -> Vec<String> {
    let graph = service.observed().unwrap();
    let file_of = |id| graph.component(id).and_then(|c| c.anchor.as_ref()).map(|a| a.file.as_str().to_owned());
    let mut uses: BTreeMap<String, usize> = BTreeMap::new();
    for r in graph.relations() {
        if let (Some(from), Some(to)) = (file_of(&r.source), file_of(&r.target))
            && from != to
            && to.ends_with(".rs")
        {
            *uses.entry(to).or_default() += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = uses.into_iter().collect();
    ranked.sort_by_key(|(f, n)| (std::cmp::Reverse(*n), f.clone()));
    ranked.into_iter().map(|(f, _)| f).collect()
}

#[test]
fn the_code_view_answers_while_a_write_runs() {
    let (dir, service) = this_repo();
    let service = Arc::new(service);
    let file = files_by_use(&service)[0].clone();
    let path: PathBuf = dir.path().join(&file);
    let text = std::fs::read_to_string(&path).unwrap();
    let other = service.observed().unwrap().components().filter_map(|c| c.anchor.as_ref()).map(|a| a.file.as_str().to_owned()).find(|f| f != &file && f.ends_with(".rs")).unwrap();
    // The same reads with nothing being written.
    let mut idle = Duration::ZERO;
    for _ in 0..100 {
        let t = Instant::now();
        service.code_analysis(&other).unwrap();
        idle = idle.max(t.elapsed());
    }
    let windows: Arc<Mutex<Vec<(Instant, Instant)>>> = Arc::default();
    let writing = Arc::new(AtomicBool::new(true));
    let started = Arc::new(AtomicBool::new(false));
    let writer = {
        let (service, writing, started, windows, path) = (service.clone(), writing.clone(), started.clone(), windows.clone(), path.clone());
        std::thread::spawn(move || {
            while !started.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            for n in 0..6 {
                std::fs::write(&path, format!("{text}\n// change {n}\n")).unwrap();
                let t = Instant::now();
                service.refresh(std::slice::from_ref(&path)).unwrap();
                windows.lock().unwrap().push((t, Instant::now()));
            }
            writing.store(false, Ordering::SeqCst);
        })
    };
    let mut reads = Vec::new();
    started.store(true, Ordering::SeqCst);
    while writing.load(Ordering::SeqCst) {
        let t = Instant::now();
        service.code_analysis(&other).unwrap();
        reads.push((t, Instant::now()));
    }
    writer.join().unwrap();
    let windows = windows.lock().unwrap().clone();
    let during: Vec<Duration> = reads.iter().filter(|(s, e)| windows.iter().any(|(ws, we)| s < we && e > ws)).map(|(s, e)| *e - *s).collect();
    assert!(during.len() >= 3, "too few reads overlapped a write to tell ({} of {})", during.len(), reads.len());
    let slowest = during.iter().max().copied().unwrap();
    // Not blocked: no read during a write is slower
    // than three times the slowest read with nothing written, plus 5 ms.
    let bound = idle * 3 + Duration::from_millis(5);
    assert!(slowest <= bound, "a read waited for a write: slowest {slowest:?} during writes {:?}; slowest with none {idle:?}", windows.iter().map(|(s, e)| *e - *s).collect::<Vec<_>>());
}
