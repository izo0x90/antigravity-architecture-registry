//! What the repository holds: no
//! screenshots lying around, no build output in git, and a server that
//! says so when the cockpit isn't built.

use std::path::Path;

fn repo() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

#[test]
fn no_screenshots_in_the_repository() {
    for dir in [repo(), Path::new(env!("CARGO_MANIFEST_DIR"))] {
        let pngs: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".png")).collect();
        assert!(pngs.is_empty(), "{}: {pngs:?}", dir.display());
    }
}

#[test]
fn the_cockpit_build_is_not_kept_in_git() {
    let ignore = std::fs::read_to_string(repo().join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l.trim() == "meta-harness/static/wasm/"), "ignored");
    let tracked = std::process::Command::new("git").args(["ls-files", "--cached", "meta-harness/static/wasm"]).current_dir(repo()).output().unwrap();
    assert!(String::from_utf8_lossy(&tracked.stdout).trim().is_empty(), "not in the index");
}

#[test]
fn the_server_wont_start_without_a_cockpit_build() {
    let empty = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_meta-harness"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("STATIC_DIR", empty.path())
        .env("PORT", "0")
        .env("HOST", "127.0.0.1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("the cockpit isn't built") && said.contains("make ui"), "{said}");
}
