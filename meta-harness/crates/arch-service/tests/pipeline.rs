//! The one pipeline through the service: read, index (a language's own
//! indexer, here a stand-in pack), combine, store; start-up and every reader
//! use the stored result.

use std::path::{Path, PathBuf};

use arch_graph::{ComponentId, RelationKind, RepoPath, TypeSystem};
use arch_service::{ArchitectureService, Limits, MemoryPlans, open_facts};
use protobuf::Message;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample")
}

fn shipped() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")
}

fn id(s: &str) -> ComponentId {
    ComponentId::for_symbol("rust", s)
}

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    places: nah_packs::Places,
}

/// A copy of the Rust fixture, and a stand-in Rust indexer pack in the
/// project that writes an index saying `store.save(&out)` in run() is
/// Store::save (as rust-analyzer would).
fn repo() -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sample");
    for file in ["Cargo.toml", "src/lib.rs", "src/store.rs", "src/util.rs"] {
        std::fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
        std::fs::copy(fixture().join(file), root.join(file)).unwrap();
    }
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    let store = std::fs::read_to_string(root.join("src/store.rs")).unwrap();
    let at = |text: &str, needle: &str, nth: usize| -> (i32, i32, i32) {
        let start = text.match_indices(needle).nth(nth).unwrap().0;
        let line = text[..start].matches('\n').count() as i32;
        let col = (start - text[..start].rfind('\n').map_or(0, |i| i + 1)) as i32;
        (line, col, col + needle.len() as i32)
    };
    let save = "rust-analyzer cargo sample 0.1.0 store/Store#save().";
    let occurrence = |(line, s, e): (i32, i32, i32), roles: i32| {
        let mut o = scip::types::Occurrence::new();
        o.range = vec![line, s, e];
        o.symbol = save.into();
        o.symbol_roles = roles;
        o
    };
    let document = |path: &str, occurrences: Vec<scip::types::Occurrence>| {
        let mut d = scip::types::Document::new();
        d.relative_path = path.into();
        d.position_encoding = scip::types::PositionEncoding::UTF8CodeUnitOffsetFromLineStart.into();
        d.occurrences = occurrences;
        d
    };
    let mut index = scip::types::Index::new();
    index.documents.push(document("src/store.rs", vec![occurrence(at(&store, "save", 0), 1)]));
    index.documents.push(document("src/lib.rs", vec![occurrence(at(&lib, "save", 0), 0)]));
    let scip_file = dir.path().join("index.scip");
    std::fs::write(&scip_file, index.write_to_bytes().unwrap()).unwrap();
    let tool = dir.path().join("fake-rust-analyzer");
    std::fs::write(&tool, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'fake 1.0.0'; else cp '{}' \"$3\"; fi\n", scip_file.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // The project's own rust-analyzer pack replaces the shipped one.
    let pack = root.join(".nah/packs/rust-analyzer");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(
        pack.join("pack.toml"),
        format!(
            "[pack]\nname = \"rust-analyzer\"\nkind = \"indexer\"\nversion = \"1.0.0\"\ndescription = \"A stand-in.\"\n[tool]\nname = \"fake\"\nsupports = \">=1.0.0\"\n[tool.check]\ncommand = \"{t}\"\nargs = [\"--version\"]\nversion = '^fake (\\d+\\.\\d+\\.\\d+)'\n[[install]]\nplatforms = [\"linux-x86_64\", \"linux-arm64\", \"macos-arm64\", \"macos-x86_64\"]\n[[install.steps]]\nrun = {{ command = \"true\" }}\n[run]\ncommand = \"{t}\"\nargs = [\"{{root}}\", \"--output\", \"{{out}}\"]\noutput = \"scip\"\ntimeout_s = 30\n",
            t = tool.display()
        ),
    )
    .unwrap();
    let places = nah_packs::Places { project: root.clone(), ..nah_packs::Places::isolated(&dir.path().join("user")) };
    Repo { _dir: dir, root, places }
}

fn open(repo: &Repo, types: TypeSystem) -> ArchitectureService {
    let catalog = nah_packs::Catalog::load(&nah_packs::Distribution::from_dir(&shipped()).unwrap(), &repo.places).unwrap();
    let packs = code_analyzer::PackSet::from_catalog(&catalog).unwrap();
    let facts = open_facts(&repo.root, &Limits::defaults().unwrap()).unwrap();
    ArchitectureService::open(&repo.root, packs, types, Box::new(MemoryPlans::default()), Box::new(facts)).unwrap().with_indexers(catalog)
}

fn proven_by(service: &ArchitectureService, from: &str, to: &str) -> Option<Option<String>> {
    service.observed().unwrap().relations().find(|r| r.kind == RelationKind::Calls && r.source == id(from) && r.target == id(to)).map(|r| r.proven_by.clone())
}

fn open_stages(service: &ArchitectureService) -> Vec<String> {
    service.perf().unwrap().recent.iter().find(|t| t.what == "open").unwrap().stages.iter().map(|s| s.stage.clone()).collect()
}

// req: AP.7, AP.9
#[test]
fn read_index_combine_store_and_start_up_reads_the_stored_result() {
    let repo = repo();
    let service = open(&repo, TypeSystem::defaults().unwrap());
    service.ingest(&repo.root).unwrap();
    assert_eq!(proven_by(&service, "sample::run", "sample::store::Store::save"), Some(None), "matched by name until the indexer runs");

    // The language's indexer runs (in the background, on a server).
    service.index_languages().unwrap();
    assert_eq!(proven_by(&service, "sample::run", "sample::store::Store::save"), Some(Some("rust-analyzer 1.0.0".into())));
    let status = service.index_status();
    let rust = status.indexers.iter().find(|s| s.language == "rust").unwrap();
    assert_eq!((rust.state, rust.files, rust.proven), (arch_graph::IndexerState::Ready, 2, 1));
    let index_run = service.perf().unwrap().recent.into_iter().find(|t| t.what == "index").unwrap();
    assert_eq!(index_run.stages.iter().map(|s| s.stage.as_str()).collect::<Vec<_>>(), ["indexer", "combine", "store", "graph"], "the indexer stage is timed");
    // What each named place refers to, from the store.
    let places = service.occurrences(&RepoPath::new("src/lib.rs").unwrap()).unwrap();
    assert!(places.iter().any(|o| o.target == Some(id("sample::store::Store::save")) && o.symbol.is_some()));
    drop(service);

    // Start-up reads the stored result: nothing is combined, the proof stays.
    let again = open(&repo, TypeSystem::defaults().unwrap());
    assert_eq!(open_stages(&again), ["load"]);
    assert_eq!(proven_by(&again, "sample::run", "sample::store::Store::save"), Some(Some("rust-analyzer 1.0.0".into())));
    // Nothing changed on disk: reading again keeps the result as it is.
    again.ingest(&repo.root).unwrap();
    assert_eq!(proven_by(&again, "sample::run", "sample::store::Store::save"), Some(Some("rust-analyzer 1.0.0".into())));

    // lib.rs is edited: what the indexer said about it no longer holds.
    let lib = repo.root.join("src/lib.rs");
    std::fs::write(&lib, std::fs::read_to_string(&lib).unwrap() + "\npub fn added() {}\n").unwrap();
    again.ingest(&lib).unwrap();
    assert_eq!(proven_by(&again, "sample::run", "sample::store::Store::save"), Some(None), "back to a name match until it's indexed again");
    drop(again);

    // Other types: the kept result was made another way, so it's made again.
    let mut types = TypeSystem::defaults().unwrap();
    types.extend_from_toml("[types.handler]\nextends = \"function\"\ndescription = \"Handles a request.\"\n").unwrap();
    let other = open(&repo, types);
    assert_eq!(open_stages(&other), ["combine", "store", "graph"]);
}
