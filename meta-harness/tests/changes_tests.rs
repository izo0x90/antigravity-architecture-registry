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

/// Outside git, a project says so; two texts (a call's change) still
/// compare, as a file's sides do, and too much at once is refused.
#[tokio::test]
async fn a_project_outside_git_says_so_and_still_compares_two_texts() {
    let (addr, _) = spawn_app_with(fixture_architecture()).await;
    let (status, body) = http(addr, "GET", "/api/changes", None).await;
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("git repository"), "{body}");
    let pairs = serde_json::json!({ "pairs": [{ "old": "a\nb\n", "new": "a\nB\n", "context": 1 }, { "old": "", "new": "x\n" }] }).to_string();
    let (status, body) = http(addr, "POST", "/api/changes/texts", Some(&pairs)).await;
    assert_eq!(status, 200, "{body}");
    let compared: Vec<Vec<harness_contracts::changes::Hunk>> = serde_json::from_str(&body).unwrap();
    let kinds: Vec<Vec<(LineKind, &str)>> = compared.iter().map(|hunks| hunks.iter().flat_map(|h| &h.lines).map(|l| (l.kind, l.text.as_str())).collect()).collect();
    assert_eq!(kinds, [vec![(LineKind::Context, "a"), (LineKind::Removed, "b"), (LineKind::Added, "B")], vec![(LineKind::Added, "x")]]);
    let big = serde_json::json!({ "pairs": [{ "old": "x".repeat(3 * 1024 * 1024), "new": "" }] }).to_string();
    let (status, _) = http(addr, "POST", "/api/changes/texts", Some(&big)).await;
    assert_eq!(status, 413);
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

#[tokio::test]
async fn the_timeline_has_a_window_per_turn_boundary_and_each_turns_changes() {
    use harness_contracts::{AdapterEvent, ClientEvent, ThreadId, Turn, TurnId, TurnStatus};
    let arch = committed();
    let root = arch.root().to_path_buf();
    let (addr, state) = spawn_app_with(arch).await;
    let turn = |status| ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new("u1"), thread_id: ThreadId::new("t1"), ordinal: 1, status, failure: None, usage: None } } };
    let timeline = |addr| async move {
        let (status, body) = http(addr, "GET", "/api/changes/timeline", None).await;
        assert_eq!(status, 200, "{body}");
        serde_json::from_str::<harness_contracts::changes::Timeline>(&body).unwrap()
    };
    state.harness.hub.publish(turn(TurnStatus::Running));
    let mut started = timeline(addr).await;
    for _ in 0..50 {
        if !started.turns.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        started = timeline(addr).await;
    }
    assert_eq!(started.turns.len(), 1, "the turn's start was seen: {started:?}");
    std::fs::write(root.join("src/util.rs"), "pub fn helper() {}\n").unwrap();
    state.harness.hub.publish(turn(TurnStatus::Completed));
    let mut ended = timeline(addr).await;
    for _ in 0..50 {
        if ended.turn("u1").is_some_and(|t| t.comparison.to != harness_contracts::changes::Point::Files) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        ended = timeline(addr).await;
    }
    let u1 = ended.turn("u1").unwrap();
    assert_eq!(u1.files, 1, "{ended:?}");
    assert_eq!(ended.windows.len(), 2, "its window, and the open one since");
    assert_eq!(ended.windows[0].working.iter().map(|w| w.turn.as_str()).collect::<Vec<_>>(), ["u1"]);
    let (status, body) = http(addr, "GET", &format!("/api/changes?from={}&to={}", u1.comparison.from, u1.comparison.to), None).await;
    assert!(status == 200 && body.contains("src/util.rs"), "its comparison reads: {body}");
}

#[tokio::test]
async fn the_user_stages_and_unstages_and_agents_read_changes_but_cannot_stage() {
    let arch = committed();
    let util = arch.root().join("src/util.rs");
    std::fs::write(&util, "pub fn shout(s: &str) -> String {\n    s.trim().to_uppercase()\n}\n\npub fn helper() {}\n").unwrap();
    arch.refresh(std::slice::from_ref(&util)).unwrap();
    let (addr, _) = spawn_app_with(arch).await;
    let groups = |body: String| serde_json::from_str::<ChangeList>(&body).unwrap().files.into_iter().map(|f| (f.group, f.path)).collect::<Vec<_>>();
    let (status, body) = http(addr, "POST", "/api/changes/stage", Some(r#"{"path":"src/util.rs"}"#)).await;
    assert_eq!(status, 204, "{body}");
    assert_eq!(groups(http(addr, "GET", "/api/changes", None).await.1), [(Group::Staged, "src/util.rs".into())]);
    let (status, _) = http(addr, "POST", "/api/changes/unstage", Some(r#"{"path":"src/util.rs"}"#)).await;
    assert_eq!(status, 204);
    assert_eq!(groups(http(addr, "GET", "/api/changes", None).await.1), [(Group::Unstaged, "src/util.rs".into())]);
    let (status, body) = http(addr, "POST", "/api/changes/stage", Some(r#"{"path":"src/util.rs","hunk":{"old_start":90,"new_start":90,"context":3}}"#)).await;
    assert_eq!(status, 400, "a change that isn't there: {body}");
    // No agent role may stage; every one may read.
    let access = nah_access::Access::defaults();
    for role in ["lead", "sub", "worker", "external"] {
        assert!(!access.allows(&nah_access::Principal::agent(role), nah_access::Capability::ChangesStage), "{role}");
    }
    let mut mcp = Mcp::lead(addr).await;
    let (err, text) = mcp.tool("changes", serde_json::json!({})).await;
    assert!(!err && text.contains("[unstaged] modified src/util.rs +1 -1"), "{text}");
    let (err, text) = mcp.tool("changes", serde_json::json!({ "path": "src/util.rs" })).await;
    assert!(!err && text.contains("+    s.trim().to_uppercase()") && text.contains("-    s.to_uppercase()"), "{text}");
    let (err, text) = mcp.tool("changes", serde_json::json!({ "components": true })).await;
    assert!(!err && text.contains("util::shout (src/util.rs) +1 -1"), "{text}");
    let (err, text) = mcp.tool("changes", serde_json::json!({ "changed": "HEAD..nowhere" })).await;
    assert!(err, "{text}");
    let (err, text) = mcp.tool("changes", serde_json::json!({ "timeline": true })).await;
    assert!(!err && text.contains("Windows"), "{text}");
}

/// Code as notes see it: a component (followed by the index) and lines of
/// a file, kept where they are as the file is saved with lines added
/// before them (the index tells, as the watcher has it do), read as they
/// were once a change is among them or a definition is gone; nothing of
/// it in the logs; the snapshots no note points to any more let go.
#[tokio::test]
async fn tagged_code_follows_the_saved_file_and_reads_as_it_was_once_changed_or_gone() {
    let arch = committed();
    let root = arch.root().to_path_buf();
    let util = root.join("src/util.rs");
    let before = std::fs::read_to_string(&util).unwrap();
    let shout = arch.graph(&arch_graph::Source::Code).unwrap().components().find(|c| c.name == "shout" && c.anchor.as_ref().is_some_and(|a| a.file.as_str() == "src/util.rs")).map(|c| c.id.clone()).expect("shout in util.rs");
    let (addr, state) = spawn_app_with(arch.clone()).await;
    let mut lead = Mcp::lead(addr).await;
    let (error, said) = lead.tool("tag", serde_json::json!({ "tag": "here", "subject": "src/util.rs lines 2-3" })).await;
    assert!(!error, "{said}");
    let (error, said) = lead.tool("tag", serde_json::json!({ "tag": "here", "subject": format!("component {shout}") })).await;
    assert!(!error, "{said}");
    let (_, read) = lead.tool("tag_read", serde_json::json!({ "tag": "here" })).await;
    let lines: Vec<&str> = before.lines().collect();
    assert!(read.contains(&format!("1. src/util.rs lines 2-3 (tagged by lead; as it is now):\n{}\n{}", lines[1], lines[2])), "{read}");
    assert!(read.contains(&format!("2. component {shout} (tagged by lead; as it is now):\n{}", lines[0])), "the component, by its id: {read}");
    // Saved with two lines added before them: tagged where they are now
    // before the index has caught up, they're the same lines (not twice).
    std::fs::write(&util, format!("// one\n// two\n{before}")).unwrap();
    let (error, said) = lead.tool("tag", serde_json::json!({ "tag": "here", "subject": "src/util.rs lines 4-5" })).await;
    assert!(!error && said.starts_with("here (on 2 things)"), "{said}");
    // The index told (as the watcher tells it): they're where they went,
    // and so is the definition.
    let saved = |text: String| {
        std::fs::write(&util, text).unwrap();
        arch.refresh(std::slice::from_ref(&util)).unwrap();
    };
    saved(format!("// one\n// two\n{before}"));
    let (_, read) = lead.tool("tag_read", serde_json::json!({ "tag": "here" })).await;
    assert!(read.contains(&format!("1. src/util.rs lines 4-5 (tagged by lead; as it is now):\n{}\n{}", lines[1], lines[2])), "{read}");
    assert!(read.contains(&format!("2. component {shout} (tagged by lead; as it is now):\n{}", lines[0])), "{read}");
    // A change among them: as they were, said so.
    saved(std::fs::read_to_string(&util).unwrap().replacen(lines[1], "    s.trim().to_uppercase()", 1));
    let (_, read) = lead.tool("tag_read", serde_json::json!({ "tag": "here" })).await;
    assert!(read.contains(&format!("1. src/util.rs lines 4-5 (tagged by lead; as it was when noted: changed or gone since):\n{}\n{}", lines[1], lines[2])), "{read}");
    // The definition deleted: what it was, from where it was when tagged.
    saved("// one\n// two\npub fn helper() {}\n".into());
    let (_, read) = lead.tool("tag_read", serde_json::json!({ "tag": "here" })).await;
    assert!(read.contains(&format!("2. component {shout} (tagged by lead; as it was when noted: changed or gone since):\n{}\n{}\n{}", lines[0], lines[1], lines[2])), "{read}");
    // Lines a file doesn't have can't be tagged.
    let (error, said) = lead.tool("tag", serde_json::json!({ "tag": "peek", "subject": "src/util.rs lines 900-901" })).await;
    assert!(error && said.contains("isn't there"), "{said}");
    // Only the project's own files: not one outside it, however named.
    std::fs::write(root.parent().unwrap().join("outside.txt"), "secret\n").unwrap();
    for outside in ["/etc/passwd line 1", "../outside.txt line 1", "src/../../outside.txt line 1", "file ../outside.txt"] {
        let (error, said) = lead.tool("tag", serde_json::json!({ "tag": "peek", "subject": outside })).await;
        assert!(error && said.contains("isn't a file of the project"), "{outside}: {said}");
    }
    // A whole file (#12): all of it, as it is now.
    let (error, said) = lead.tool("tag", serde_json::json!({ "tag": "whole", "subject": "file src/util.rs" })).await;
    assert!(!error, "{said}");
    let (_, read) = lead.tool("tag_read", serde_json::json!({ "tag": "whole" })).await;
    assert!(read.contains("1. file src/util.rs (tagged by lead; as it is now):\n// one\n// two\npub fn helper() {}"), "{read}");
    // Code lasts: nothing of it was logged.
    assert!(state.tags.tags.find("here").unwrap().on.iter().all(|t| t.kept.is_none()));
    // Taken off everything: the snapshots kept for them go (but the last,
    // used again for the next).
    let changes = nah_changes::Changes::open(&root).unwrap();
    assert!(changes.snapshots_for("tagged lines").unwrap().len() >= 2, "one when tagged, another where they moved");
    let (error, said) = lead.tool("tag_edit", serde_json::json!({ "tag": "here", "delete": true })).await;
    assert!(!error, "{said}");
    assert_eq!(changes.snapshots_for("tagged lines").unwrap().len(), 1);
}
