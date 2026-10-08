//! The index stage with a stand-in indexer pack: it hands back a SCIP index
//! made here, and only what still describes the files is kept.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use arch_graph::RepoPath;
use protobuf::Message;

fn shipped() -> nah_packs::Distribution {
    nah_packs::Distribution::from_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")).unwrap()
}

/// A SCIP index with one document: `name` defined at `def` and referenced at `uses` (line, start, end).
fn index(path: &str, symbol: &str, def: (i32, i32, i32), uses: &[(i32, i32, i32)]) -> Vec<u8> {
    let mut doc = scip::types::Document::new();
    doc.relative_path = path.into();
    doc.position_encoding = scip::types::PositionEncoding::UTF8CodeUnitOffsetFromLineStart.into();
    let occ = |(line, s, e): (i32, i32, i32), roles: i32| {
        let mut o = scip::types::Occurrence::new();
        o.range = vec![line, s, e];
        o.symbol = symbol.into();
        o.symbol_roles = roles;
        o
    };
    doc.occurrences.push(occ(def, 1));
    doc.occurrences.extend(uses.iter().map(|u| occ(*u, 0)));
    let mut info = scip::types::SymbolInformation::new();
    info.symbol = symbol.into();
    let mut rel = scip::types::Relationship::new();
    rel.symbol = "trait Shouts#".into();
    rel.is_implementation = true;
    info.relationships.push(rel);
    doc.symbols.push(info);
    let mut i = scip::types::Index::new();
    i.documents.push(doc);
    i.write_to_bytes().unwrap()
}

struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
    places: nah_packs::Places,
}

/// A project with `files`, and a stand-in indexer pack that copies `scip` to its output.
fn project(files: &[(&str, &str)], scip: &[u8]) -> Project {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    for (path, text) in files {
        std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        std::fs::write(root.join(path), text).unwrap();
    }
    let places = nah_packs::Places { project: root.clone(), ..nah_packs::Places::isolated(&dir.path().join("user")) };
    let pack = root.join(".nah/packs/fake-indexer");
    std::fs::create_dir_all(&pack).unwrap();
    let fixture = dir.path().join("index.scip");
    std::fs::write(&fixture, scip).unwrap();
    let tool = dir.path().join("fake-indexer");
    std::fs::write(&tool, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'fake 1.0.0'; else cp '{}' \"$2\"; fi\n", fixture.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(
        pack.join("pack.toml"),
        format!(
            "[pack]\nname = \"fake-indexer\"\nkind = \"indexer\"\nversion = \"1.0.0\"\ndescription = \"x\"\n[tool]\nname = \"fake\"\nsupports = \">=1.0.0\"\n[tool.check]\ncommand = \"{t}\"\nargs = [\"--version\"]\nversion = '^fake (\\d+\\.\\d+\\.\\d+)'\n[[install]]\nplatforms = [\"linux-x86_64\", \"linux-arm64\", \"macos-arm64\", \"macos-x86_64\"]\n[[install.steps]]\nrun = {{ command = \"true\" }}\n[run]\ncommand = \"{t}\"\nargs = [\"{{root}}\", \"{{out}}\"]\noutput = \"scip\"\ntimeout_s = 30\n",
            t = tool.display()
        ),
    )
    .unwrap();
    Project { _dir: dir, root, places }
}

fn hashes(root: &Path, files: &[&str]) -> BTreeMap<RepoPath, String> {
    files.iter().map(|f| (RepoPath::new(f).unwrap(), code_analyzer::content_hash(&std::fs::read_to_string(root.join(f)).unwrap()))).collect()
}

// req: AP.7, AP.8
#[test]
fn an_indexers_output_is_kept_per_file_only_while_it_describes_the_file() {
    let code = "pub fn shout() {}\npub fn run() { shout(); }\n";
    let p = project(&[("src/lib.rs", code), ("src/other.rs", "fn x() {}\n")], &index("src/lib.rs", "cargo m 1.0 shout().", (0, 7, 12), &[(1, 15, 20)]));
    let catalog = nah_packs::Catalog::load(&shipped(), &p.places).unwrap();
    let lock = nah_packs::Lock::default();
    let run = code_analyzer::index::run(&catalog, &lock, "fake-indexer", &p.root, &hashes(&p.root, &["src/lib.rs", "src/other.rs"])).unwrap();
    assert_eq!((run.indexer.as_str(), run.tool_version.as_deref()), ("fake-indexer", Some("1.0.0")));
    assert_eq!(run.files.len(), 1, "only files the index covers");
    let file = &run.files[0];
    let at = |[s, e, ..]: [u32; 4]| &code[s as usize..e as usize];
    assert_eq!(file.occurrences.iter().map(|o| (at(*o), o[3] & code_analyzer::index::ROLE_DEFINITION != 0)).collect::<Vec<_>>(), [("shout", true), ("shout", false)]);
    assert_eq!(file.symbols, ["cargo m 1.0 shout()."]);
    assert_eq!(file.defined[0].implements, ["trait Shouts#"]);

    // The file changed after the hashes were taken: nothing about it is kept.
    let stale = hashes(&p.root, &["src/lib.rs"]);
    std::fs::write(p.root.join("src/lib.rs"), "pub fn shout() {}\n").unwrap();
    assert!(code_analyzer::index::run(&catalog, &lock, "fake-indexer", &p.root, &stale).unwrap().files.is_empty());
}

// req: AP.7
#[test]
fn an_indexer_that_fails_or_isnt_ready_says_so() {
    let p = project(&[("src/lib.rs", "fn a() {}\n")], b"not a scip index");
    let catalog = nah_packs::Catalog::load(&shipped(), &p.places).unwrap();
    let err = code_analyzer::index::run(&catalog, &nah_packs::Lock::default(), "fake-indexer", &p.root, &hashes(&p.root, &["src/lib.rs"])).unwrap_err().to_string();
    assert!(err.contains("SCIP"), "{err}");
    let err = code_analyzer::index::run(&catalog, &nah_packs::Lock::default(), "scip-python", &p.root, &BTreeMap::new());
    assert!(err.is_err(), "an indexer that isn't installed isn't run");
}
