//! The hot spots, on this repo's own code: linking every file's facts into
//! the graph, walking a tree, laying it out, and matching names.
//! `cargo bench -p arch-service --bench hot`

use std::path::Path;

use arch_graph::TypeSystem;
use arch_graph::tree::{TreeRequest, ViewKind};
use code_analyzer::{CodeRules, FileExtract, PackSet, combine, extract_file, walk};
use arch_service::{ArchitectureService, MemoryPlans, MemoryStore};
use criterion::{Criterion, criterion_group, criterion_main};

fn repo() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn extracts(root: &Path, packs: &PackSet, rules: &CodeRules) -> Vec<FileExtract> {
    walk(root, root, packs)
        .unwrap()
        .into_iter()
        .filter_map(|f| {
            let pack = packs.for_path(&f.absolute)?;
            let content = std::fs::read_to_string(&f.absolute).ok()?;
            Some(extract_file(pack, rules, f.path, f.module, &content))
        })
        .collect()
}

fn hot(c: &mut Criterion) {
    let root = repo();
    let packs = code_analyzer::PackSet::from_shipped_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")).unwrap();
    let types = TypeSystem::defaults().unwrap();
    let rules = CodeRules::defaults(&packs, &types).unwrap();
    let facts = extracts(&root, &packs, &rules);
    c.bench_function("link", |b| b.iter(|| combine(&facts, &[], &packs, &types)));

    let service = ArchitectureService::open(&root, code_analyzer::PackSet::from_shipped_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")).unwrap(), types.clone(), Box::new(MemoryPlans::default()), Box::new(MemoryStore::default())).unwrap();
    service.ingest(&root).unwrap();
    for (view, depth) in [(ViewKind::Contains, 4), (ViewKind::Calls, 4), (ViewKind::DataFlow, 4)] {
        let request = TreeRequest { view, depth: Some(depth), ..TreeRequest::default() };
        c.bench_function(&format!("tree {} depth {depth}", view.name()), |b| b.iter(|| service.tree(&request).unwrap()));
    }

    let graph = service.observed().unwrap();
    let names: Vec<String> = graph.components().map(|c| c.name.clone()).collect();
    let matching = arch_search::Matching::shared();
    c.bench_function("match names", |b| b.iter(|| arch_search::rank("rebuild", &names, matching)));
}

criterion_group!(benches, hot);
criterion_main!(benches);
