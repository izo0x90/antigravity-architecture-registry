//! The hot spots, on this repo's own code: linking every file's facts into
//! the graph, walking a tree, laying it out, and matching names.
//! `cargo bench -p arch-service --bench hot`; CI runs each once
//! (`-- --test`, .github/workflows/check.yml).

use std::path::Path;

use arch_graph::TypeSystem;
use arch_graph::tree::{TreeRequest, ViewKind};
use arch_service::{ArchitectureService, MemoryPlans};
use code_analyzer::{CodeRules, FileExtract, PackSet, extract_file, walk};
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
    let packs = shipped_packs();
    let types = TypeSystem::defaults().unwrap();
    let rules = CodeRules::defaults(&packs, &types).unwrap();
    let facts = extracts(&root, &packs, &rules);
    c.bench_function("link", |b| {
        b.iter(|| {
            let read = facts
                .iter()
                .map(|e| {
                    (
                        code_analyzer::rows::FileRow {
                            path: e.file.clone(),
                            language: e.language.clone(),
                            size: 0,
                            modified: 0,
                            content_hash: e.content_hash.clone(),
                            read_version: String::new(),
                        },
                        e.clone(),
                    )
                })
                .collect();
            code_analyzer::Model::default().apply(&packs, &types, code_analyzer::Change { read, ..code_analyzer::Change::default() })
        })
    });

    let service = ArchitectureService::open(
        &root,
        shipped_packs(),
        types.clone(),
        Box::new(MemoryPlans::default()),
        arch_service::temporary_facts(&arch_service::Limits::defaults().unwrap()).unwrap(),
    )
    .unwrap();
    service.ingest(&root).unwrap();
    for (view, depth) in [(ViewKind::Contains, 4), (ViewKind::Calls, 4), (ViewKind::DataFlow, 4)] {
        let request = TreeRequest { view, depth: Some(depth), ..TreeRequest::default() };
        c.bench_function(&format!("tree {} depth {depth}", view.name()), |b| b.iter(|| service.tree(&request).unwrap()));
        // The layout alone, on the tree that walk gave.
        let walked = service.tree(&request).unwrap();
        let layout = service.trees().layout.clone();
        // The copy each run lays out is made outside the timing.
        c.bench_function(&format!("layout {} depth {depth}", view.name()), |b| {
            b.iter_batched_ref(
                || walked.clone(),
                |tree| arch_graph::tree::lay_out(tree, &layout, &|| false),
                criterion::BatchSize::SmallInput,
            )
        });
    }

    let graph = service.observed().unwrap();
    let names: Vec<String> = graph.components().map(|c| c.name.clone()).collect();
    let matching = arch_search::Matching::shared();
    c.bench_function("match names", |b| b.iter(|| arch_search::rank("rebuild", &names, matching)));
}

criterion_group!(benches, hot);
criterion_main!(benches);

/// The language packs shipped in this repo, without any project's or
/// user's own (tests must not depend on the machine).
pub fn shipped_packs() -> code_analyzer::PackSet {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
    let places = nah_packs::Places::isolated(&std::env::temp_dir().join(format!("nah-test-packs-{}", std::process::id())));
    let catalog = nah_packs::Catalog::load(&nah_packs::Distribution::from_dir(&dir).unwrap(), &places).unwrap();
    code_analyzer::PackSet::from_catalog(&catalog).unwrap()
}
