//! The code view: one request gives a
//! file's text and its analysis for that same text, and a component's uses
//! come from the refs linked to it, across files.

use std::path::{Path, PathBuf};

use arch_graph::{ComponentId, Source, TypeSystem};
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

fn sample(dir: &Path) -> (PathBuf, ArchitectureService) {
    let root = dir.join("sample");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample"), &root);
    let facts = arch_service::open_facts(&root, &arch_service::Limits::defaults().unwrap()).unwrap();
    let service = ArchitectureService::open(&root, common::shipped_packs(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), facts).unwrap();
    service.ingest(&root).unwrap();
    (root, service)
}

#[test]
fn one_request_gives_the_text_and_its_analysis_for_the_same_content() {
    let dir = tempfile::tempdir().unwrap();
    let (root, service) = sample(dir.path());
    let view = service.code_analysis("src/lib.rs").unwrap();
    let text = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert_eq!(view.text, text);
    assert_eq!(view.content_hash, code_analyzer::content_hash(&text));
    assert!(!view.stale && !view.occurrences.is_empty() && !view.highlights.runs.is_empty());
    // The gutter's components come with it: no second request.
    assert!(view.view.components.iter().any(|(c, _)| c.name == "run") && !view.view.stale, "{:?}", view.view.components.len());
    // Every place it names is in the text it came with.
    for o in &view.occurrences {
        assert!(view.text.get(o.start as usize..o.end as usize).is_some_and(|w| !w.is_empty()), "{o:?}");
    }
    // Changed and not read again: the new text, said to be unanalysed.
    std::fs::write(root.join("src/lib.rs"), format!("// edited\n{text}")).unwrap();
    let view = service.code_analysis("src/lib.rs").unwrap();
    assert!(view.stale && view.text.starts_with("// edited") && view.occurrences.is_empty());
    assert!(view.view.stale && !view.view.components.is_empty(), "where things were, said to be out of date");
    // A file no pack reads: its text, nothing else.
    let manifest = service.code_analysis("Cargo.toml").unwrap();
    assert!(manifest.stale && manifest.text.contains("[package]"));
    // Nothing outside the repo, however it's named.
    std::fs::write(dir.path().join("secret.txt"), "no").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path().join("secret.txt"), root.join("link.txt")).unwrap();
    assert!(service.code_analysis("../secret.txt").is_err());
    #[cfg(unix)]
    assert!(service.code_analysis("link.txt").is_err(), "a link out of the repo");
}

#[test]
fn a_components_uses_are_every_ref_to_it_across_files_with_their_lines() {
    let dir = tempfile::tempdir().unwrap();
    let (root, service) = sample(dir.path());
    let store = ComponentId::for_symbol("rust", "sample::store::Store");
    let uses = service.component(&store, &Source::Code).unwrap().references;
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    let store_rs = std::fs::read_to_string(root.join("src/store.rs")).unwrap();
    let found: Vec<(String, String, u32)> = uses.iter().map(|r| (r.file.clone(), r.kind.clone(), r.line)).collect();
    // The imports, the parameter's type and the path in lib.rs; the loop's
    // `Vec<Store>` in store.rs.
    assert!(found.iter().any(|(f, k, _)| f == "src/lib.rs" && k == "import"), "{found:?}");
    assert!(found.iter().any(|(f, k, _)| f == "src/lib.rs" && k == "type"), "{found:?}");
    assert!(found.iter().any(|(f, k, _)| f == "src/lib.rs" && k == "path"), "{found:?}");
    assert!(found.iter().any(|(f, k, _)| f == "src/store.rs" && k == "type"), "{found:?}");
    // Each at its line, naming it there (`Self` in its own impl names it too).
    for r in &uses {
        let text = if r.file == "src/lib.rs" { &lib } else { &store_rs };
        assert!(["Store", "Self"].contains(&&text[r.start as usize..r.end as usize]), "{r:?}");
        assert_eq!(text[..r.start as usize].matches('\n').count() as u32 + 1, r.line, "{r:?}");
        assert!(r.within.is_some() && !r.stale);
    }
    // A file edited since it was read: its uses' lines aren't known until
    // it's read again (never a wrong line).
    std::fs::write(root.join("src/lib.rs"), format!("// moved down\n{lib}")).unwrap();
    let uses = service.component(&store, &Source::Code).unwrap().references;
    assert!(uses.iter().filter(|r| r.file == "src/lib.rs").all(|r| r.stale && r.line == 0), "{uses:?}");
    assert!(uses.iter().filter(|r| r.file == "src/store.rs").all(|r| !r.stale && r.line > 0));
}
