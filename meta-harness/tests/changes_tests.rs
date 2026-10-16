//! The project's changes on the server: the routes over a git repository
//! (what a comparison changes, one file's changes), and a project that
//! isn't one.

mod common;

use std::path::Path;

use common::{Mcp, fixture_architecture, http, spawn_app_with};
use harness_contracts::changes::{ChangeList, ComponentChangeList, FileChanges, Group, LineKind};

fn git(root: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).current_dir(root).output().unwrap();
    assert!(ok.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ok.stderr));
}

/// The fixture project, committed, as a git repository.
fn committed() -> std::sync::Arc<arch_service::ArchitectureService> {
    let arch = fixture_architecture();
    let root = arch.root().to_path_buf();
    git(&root, &["init", "-q"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "first"]);
    arch
}

#[tokio::test]
async fn the_routes_list_a_comparisons_files_and_give_one_files_changes() {
    let arch = committed();
    let util = arch.root().join("src/util.rs");
    std::fs::write(&util, std::fs::read_to_string(&util).unwrap() + "\npub fn added() {}\n").unwrap();
    let (addr, _) = spawn_app_with(arch).await;
    let (status, body) = http(addr, "GET", "/api/changes", None).await;
    assert_eq!(status, 200, "{body}");
    let list: ChangeList = serde_json::from_str(&body).unwrap();
    assert!(list.comparison.is_uncommitted(), "the uncommitted changes when none is asked");
    let util = list.files.iter().find(|f| f.path == "src/util.rs").expect("util.rs changed");
    assert_eq!((util.group, util.added, util.removed), (Group::Unstaged, 2, 0));
    let (status, body) = http(addr, "GET", "/api/changes/file?path=src/util.rs&group=unstaged&context=0", None).await;
    assert_eq!(status, 200, "{body}");
    let file: FileChanges = serde_json::from_str(&body).unwrap();
    assert!(file.hunks[0].lines.iter().any(|l| l.kind == LineKind::Added && l.text == "pub fn added() {}"), "{file:?}");
    let (status, body) = http(addr, "GET", "/api/changes?from=rev:nope&to=HEAD", None).await;
    assert_eq!(status, 400, "a point that isn't there: {body}");
    let (status, _) = http(addr, "GET", "/api/changes?from=nowhere", None).await;
    assert_eq!(status, 400, "a point that can't be read");
}

#[tokio::test]
async fn a_project_outside_git_says_so() {
    let (addr, _) = spawn_app_with(fixture_architecture()).await;
    let (status, body) = http(addr, "GET", "/api/changes", None).await;
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("git repository"), "{body}");
}

#[tokio::test]
async fn changes_land_on_the_components_whose_lines_they_touch_and_out_of_date_files_are_named() {
    let arch = committed();
    let util = arch.root().join("src/util.rs");
    std::fs::write(&util, "pub fn shout(s: &str) -> String {\n    s.trim().to_uppercase()\n}\n\npub fn helper() {}\n").unwrap();
    arch.refresh(std::slice::from_ref(&util)).unwrap();
    let (addr, _) = spawn_app_with(arch.clone()).await;
    let (status, body) = http(addr, "GET", "/api/changes/components", None).await;
    assert_eq!(status, 200, "{body}");
    let list: ComponentChangeList = serde_json::from_str(&body).unwrap();
    let changed: Vec<(&str, u32, u32)> = list.components.iter().map(|c| (c.component.rsplit("::").next().unwrap(), c.added, c.removed)).collect();
    assert!(changed.contains(&("shout", 1, 1)), "{changed:?}");
    let direct: Vec<&str> = list.components.iter().filter(|c| c.direct).map(|c| c.component.rsplit("::").next().unwrap()).collect();
    assert_eq!(direct, ["shout"], "what holds it changed only in it: {:?}", list.components);
    assert!(!changed.iter().any(|(n, ..)| *n == "helper"), "untouched: not listed: {changed:?}");
    assert!(list.out_of_date.is_empty());
    // Changed again, not read again: the index can't say where things are.
    std::fs::write(&util, "pub fn helper() {}\n").unwrap();
    let (_, body) = http(addr, "GET", "/api/changes/components", None).await;
    let list: ComponentChangeList = serde_json::from_str(&body).unwrap();
    assert_eq!(list.out_of_date, ["src/util.rs"], "{body}");
}

#[tokio::test]
async fn search_and_the_agents_views_can_keep_to_what_changed() {
    let arch = committed();
    let util = arch.root().join("src/util.rs");
    std::fs::write(&util, "pub fn shout(s: &str) -> String {\n    s.trim().to_uppercase()\n}\n\npub fn helper() {}\n").unwrap();
    arch.refresh(std::slice::from_ref(&util)).unwrap();
    let (addr, _) = spawn_app_with(arch).await;
    // helper is in util.rs (changed) and store.rs (not).
    let (status, body) = http(addr, "GET", "/api/search?q=helper&kind=symbols", None).await;
    assert!(status == 200 && body.contains("src/store.rs") && body.contains("src/util.rs"), "{body}");
    let (status, body) = http(addr, "GET", "/api/search?q=helper&kind=symbols&changed=HEAD..files", None).await;
    assert!(status == 200 && body.contains("src/util.rs") && !body.contains("src/store.rs"), "only the changed file: {body}");
    let (status, _) = http(addr, "GET", "/api/search?q=helper&changed=nowhere", None).await;
    assert_eq!(status, 400, "a comparison that can't be read");
    let mut mcp = Mcp::lead(addr).await;
    let (err, text) = mcp.tool("search", serde_json::json!({ "query": "helper", "changed": "HEAD..files" })).await;
    assert!(!err && text.contains("util::helper") && !text.contains("store::helper"), "{text}");
    let (err, text) = mcp.tool("view", serde_json::json!({ "view": "contains", "changed": "HEAD..files" })).await;
    assert!(!err && text.contains("shout") && !text.contains("save"), "only what changed and what holds it: {text}");
}
