//! Language servers are packs: kind
//! `server` replaces `indexer`; its manifest gives how to start it, what to
//! send it, what it must support, its time limits, and its caches, which
//! never go into the project.

use std::path::Path;

use nah_packs::{Catalog, Distribution, Kind, Lock, Places};

fn shipped(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs").join(name)).unwrap()
}

fn distribution(packs: &[(&str, &str)]) -> Distribution {
    let mut files = vec![("index.toml".to_owned(), shipped("index.toml").into_bytes()), ("platforms.toml".to_owned(), shipped("platforms.toml").into_bytes())];
    files.extend(packs.iter().map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec())));
    Distribution::new(files)
}

fn places(dir: &Path) -> Places {
    Places { project: dir.join("project"), home: dir.join("home"), config_home: dir.join("config"), data_home: dir.join("data") }
}

const SERVER: &str = r#"[pack]
name = "fake-server"
kind = "server"
version = "1.0.0"
description = "A language server for tests."

[tool]
name = "fake-server"
supports = ">=1.0.0, <2.0.0"
[tool.check]
command = "fake-server"
args = ["--version"]
version = '^fake-server (\d+\.\d+\.\d+)'

[[install]]
platforms = ["linux-x86_64", "linux-arm64", "macos-arm64", "macos-x86_64"]
[[install.steps]]
run = { command = "true" }

[server]
command = "fake-server"
args = ["--stdio"]
env = { CACHE_DIR = "{cache}/data" }
requires = ["textDocument/definition"]
ready_timeout_s = 120
request_timeout_s = 30
[server.initialization_options]
checkOnSave = false
[server.settings]
fake = { level = 2 }
"#;

fn load(dir: &Path, packs: &[(&str, &str)]) -> Result<Catalog, String> {
    Catalog::load(&distribution(packs), &places(dir)).map_err(|e| e.to_string())
}

#[test]
fn a_server_manifest_is_checked_strictly() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = load(dir.path(), &[("server/fake/pack.toml", SERVER)]).unwrap();
    let pack = catalog.get("fake-server").unwrap();
    assert_eq!(pack.kind(), Kind::Server);
    let server = pack.manifest.server.as_ref().unwrap();
    assert_eq!(server.requires, ["textDocument/definition"]);
    assert_eq!((server.ready_timeout_s, server.request_timeout_s), (120, 30));
    let bad = |from: &str, to: &str| load(dir.path(), &[("server/fake/pack.toml", &SERVER.replacen(from, to, 1))]).unwrap_err();
    let without_server = SERVER.split("[server]").next().unwrap().to_owned();
    assert!(load(dir.path(), &[("server/fake/pack.toml", &without_server)]).unwrap_err().contains("server packs need [server]"));
    assert!(bad("requires = [\"textDocument/definition\"]", "requires = [\"textDocument/hover\"]").contains("textDocument/definition"), "a server must answer definitions");
    assert!(bad("{cache}/data", "{root}/.cache").contains("outside the project"), "caches never go into the project");
    assert!(bad("args = [\"--stdio\"]", "args = [\"--cache={root}/.cache\"]").contains("outside it"), "nor through an argument");
    assert!(load(dir.path(), &[("server/fake/pack.toml", &SERVER.replacen("args = [\"--stdio\"]", "args = [\"--root={root}\"]", 1))]).is_ok(), "naming the project itself is fine");
    assert!(bad("{cache}/data", "{nope}").contains("unknown placeholder {nope}"));
    assert!(bad("ready_timeout_s = 120", "ready_timeout_s = 0").contains("ready_timeout_s"));
    assert!(bad("request_timeout_s = 30", "request_timeout_s = 0").contains("request_timeout_s"));
    // The indexer kind is gone.
    assert!(bad("kind = \"server\"", "kind = \"indexer\"").contains("unknown variant"));
}

#[test]
fn the_shipped_servers_load_and_every_language_names_one() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::load(&Distribution::new(walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs"))), &places(dir.path())).unwrap();
    let servers: Vec<&str> = catalog.of_kind(Kind::Server).map(|p| p.name()).collect();
    for wanted in ["rust-analyzer", "typescript-server", "basedpyright"] {
        assert!(servers.contains(&wanted), "{wanted} is shipped: {servers:?}");
    }
    for language in catalog.of_kind(Kind::Language) {
        let named = language.manifest.language.as_ref().unwrap().server.as_deref().unwrap_or_else(|| panic!("{} names no server", language.name()));
        assert_eq!(catalog.get(named).unwrap().kind(), Kind::Server, "{} names {named}", language.name());
    }
}

#[test]
fn a_servers_launch_is_filled_in_with_its_cache_outside_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = load(dir.path(), &[("server/fake/pack.toml", SERVER)]).unwrap();
    let project = dir.path().join("project");
    let launch = nah_packs::server_launch(&catalog, &Lock::default(), "fake-server", &project).unwrap();
    assert_eq!(launch.program, "fake-server");
    assert_eq!(launch.args, ["--stdio"]);
    let cache = &launch.env.iter().find(|(k, _)| k == "CACHE_DIR").unwrap().1;
    assert!(Path::new(cache).starts_with(dir.path().join("data")), "{cache} is under the user's data folder");
    assert!(!Path::new(cache).starts_with(&project), "{cache} isn't in the project");
    assert_eq!(launch.initialization_options["checkOnSave"], serde_json::json!(false));
    assert_eq!(launch.settings["fake"]["level"], serde_json::json!(2));
    assert_eq!((launch.ready_timeout.as_secs(), launch.request_timeout.as_secs()), (120, 30));
    // Two projects never share a cache.
    let other = nah_packs::server_launch(&catalog, &Lock::default(), "fake-server", &dir.path().join("elsewhere")).unwrap();
    assert_ne!(other.env, launch.env);
    // A cache is kept while its project exists; one for a project that's
    // gone is removed when the server starts for another.
    std::fs::create_dir_all(&project).unwrap();
    launch.keep_cache(&project).unwrap();
    let gone = dir.path().join("gone");
    let temporary = nah_packs::server_launch(&catalog, &Lock::default(), "fake-server", &gone).unwrap();
    std::fs::create_dir_all(&gone).unwrap();
    temporary.keep_cache(&gone).unwrap();
    std::fs::remove_dir_all(&gone).unwrap();
    other.keep_cache(&dir.path().join("elsewhere")).unwrap();
    assert!(launch.cache.is_dir(), "its project is there: kept");
    assert!(!temporary.cache.exists(), "its project is gone: removed");
}

/// Every file under `dir`, by path relative to it.
fn walk(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push((path.strip_prefix(dir).unwrap().to_string_lossy().into_owned(), std::fs::read(&path).unwrap()));
            }
        }
    }
    out
}
