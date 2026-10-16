//! The service asks the one definition of "ignored" for everything it does
//! with paths: what the watcher marks out of
//! date, what search lists, and where a plan may put code.

use std::path::Path;

use arch_graph::{PlanId, TypeSystem};
use arch_service::{ArchitectureService, MemoryPlans, PlanOp, SearchKind};

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

#[test]
fn the_service_skips_what_the_parent_repos_ignore_files_and_nah_ignore_skip() {
    let dir = tempfile::tempdir().unwrap();
    let top = dir.path();
    assert!(std::process::Command::new("git").args(["init", "-q"]).arg(top).status().unwrap().success());
    // The project is below the repository's top; target/ is ignored there.
    std::fs::write(top.join(".gitignore"), "target/\n").unwrap();
    let root = top.join("proj");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample"), &root);
    std::fs::create_dir_all(root.join(".nah")).unwrap();
    std::fs::write(root.join(".nah/ignore"), "generated/\n").unwrap();
    for f in ["target/debug/build.rs", "generated/out.rs"] {
        std::fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
        std::fs::write(root.join(f), "pub fn hidden_away() {}\n").unwrap();
    }
    let packs = common::shipped_packs();
    let service = ArchitectureService::open(
        &root,
        packs,
        TypeSystem::defaults().unwrap(),
        Box::new(MemoryPlans::default()),
        arch_service::open_facts(&root, &arch_service::Limits::defaults().unwrap()).unwrap(),
    )
    .unwrap();
    service.ingest(&root).unwrap();
    assert!(service.observed().unwrap().components().all(|c| c.name != "hidden_away"), "ignored files aren't read");
    // The watcher's marks.
    service.mark_stale(&[root.join("target/debug/build.rs"), root.join("generated/out.rs"), root.join("target")]).unwrap();
    assert!(service.index_status().stale.is_empty(), "ignored paths are never out of date: {:?}", service.index_status().stale);
    // Search.
    let found = service.search("out", SearchKind::Files, &arch_graph::Source::Code, Some(50), None).unwrap();
    assert!(found.hits.iter().all(|h| !format!("{h:?}").contains("generated/") && !format!("{h:?}").contains("target/")), "{:?}", found.hits);
    // Where a plan may put code.
    let plan = PlanId::new("p").unwrap();
    service.create_plan("p", "P", "", arch_graph::PlanKind::Change).unwrap();
    let placed = service.apply(
        &plan,
        PlanOp::AddComponent { name: "later".into(), type_name: "function".into(), description: "x".into(), parent: None, file: Some("generated/later.rs".into()), signature: None, schemas: None, spec: None },
    );
    assert!(placed.is_err_and(|e| e.to_string().contains("ignored")), "a plan can't put code where ingestion never reads");
}

fn open(root: &Path) -> ArchitectureService {
    ArchitectureService::open(
        root,
        common::shipped_packs(),
        TypeSystem::defaults().unwrap(),
        Box::new(MemoryPlans::default()),
        arch_service::open_facts(root, &arch_service::Limits::defaults().unwrap()).unwrap(),
    )
    .unwrap()
}

fn found(service: &ArchitectureService, query: &str) -> Vec<String> {
    service.search(query, SearchKind::Files, &arch_graph::Source::Code, Some(5), None).unwrap().hits.into_iter().filter_map(|h| h.file).collect()
}

#[test]
fn search_lists_every_file_however_many_arrive_and_whole_folders_moved_in() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample"), &root);
    // In a git repository, hidden files are searched (CI files, plans); what
    // the rules leave out isn't, nor git's own folder.
    assert!(std::process::Command::new("git").arg("init").arg("-q").arg(&root).status().unwrap().success());
    std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
    std::fs::write(root.join(".github/workflows/ci.yml"), "on: push\n").unwrap();
    std::fs::write(root.join(".gitignore"), "secret.txt\n").unwrap();
    std::fs::write(root.join("secret.txt"), "x\n").unwrap();
    let service = open(&root);
    assert!(!found(&service, "lib.rs").is_empty(), "the list is made");
    assert_eq!(found(&service, "ci.yml").first().map(String::as_str), Some(".github/workflows/ci.yml"));
    assert!(found(&service, "secret.txt").iter().all(|f| f != "secret.txt"), "a .gitignore rule still leaves it out");
    assert!(found(&service, "HEAD").iter().all(|f| !f.starts_with(".git/")), "not git's own files");
    // Hidden files that come and go later, as the watcher passes them on.
    std::fs::write(root.join(".github/workflows/release.yml"), "on: tag\n").unwrap();
    std::fs::remove_file(root.join(".github/workflows/ci.yml")).unwrap();
    service.refresh(&[root.join(".github/workflows/release.yml"), root.join(".github/workflows/ci.yml")]).unwrap();
    assert_eq!(found(&service, "release.yml").first().map(String::as_str), Some(".github/workflows/release.yml"));
    assert!(found(&service, "ci.yml").iter().all(|f| f != ".github/workflows/ci.yml"), "removed");
    // More new files than fff-search takes in one by one, as the watcher
    // sees them.
    std::fs::create_dir_all(root.join("notes")).unwrap();
    for i in 0..1100 {
        let path = root.join(format!("notes/note_{i:05}.md"));
        std::fs::write(&path, "x\n").unwrap();
        service.refresh(std::slice::from_ref(&path)).unwrap();
    }
    assert_eq!(found(&service, "note_01099").first().map(String::as_str), Some("notes/note_01099.md"));
    // A folder moved in from outside: one event for the folder.
    let outside = dir.path().join("elsewhere/moved");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("arrived.md"), "x\n").unwrap();
    std::fs::rename(&outside, root.join("moved")).unwrap();
    service.refresh(&[root.join("moved")]).unwrap();
    assert_eq!(found(&service, "arrived").first().map(String::as_str), Some("moved/arrived.md"));
}

#[test]
fn a_changed_ignore_rule_changes_what_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../code-analyzer/tests/fixtures/rust/sample"), &root);
    let service = open(&root);
    service.ingest(&root).unwrap();
    let has = |service: &ArchitectureService, file: &str| service.observed().unwrap().components().any(|c| c.file().is_some_and(|f| f.as_str() == file));
    assert!(has(&service, "src/util.rs"));
    // The project starts ignoring a file: it leaves the graph and search.
    std::fs::create_dir_all(root.join(".nah")).unwrap();
    std::fs::write(root.join(".nah/ignore"), "src/util.rs\n").unwrap();
    service.refresh(&[root.join(".nah/ignore")]).unwrap();
    assert!(!has(&service, "src/util.rs"), "an ignored file leaves the graph");
    assert!(found(&service, "util.rs").iter().all(|f| f != "src/util.rs"));
    // A folder's own rule file, then the rule dropped: it comes back.
    std::fs::write(root.join(".nah/ignore"), "").unwrap();
    std::fs::write(root.join("src/.gitignore"), "store.rs\n").unwrap();
    service.refresh(&[root.join(".nah/ignore"), root.join("src/.gitignore")]).unwrap();
    assert!(has(&service, "src/util.rs"), "no longer ignored, read again");
    assert!(!has(&service, "src/store.rs"), "src/.gitignore applies to src/");
}
