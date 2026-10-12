//! Packs on the real index and platforms config, with test packs beside the
//! shipped ones, installed from local files.

use std::io::Write;
use std::path::{Path, PathBuf};

use nah_packs::{Catalog, Distribution, Kind, Lock, PackError, Places, State};

fn shipped(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs").join(name)).unwrap()
}

/// The shipped index and platforms, plus `packs` (path, text).
fn distribution(packs: &[(&str, &str)]) -> Distribution {
    let mut files = vec![("index.toml".to_owned(), shipped("index.toml").into_bytes()), ("platforms.toml".to_owned(), shipped("platforms.toml").into_bytes())];
    files.extend(packs.iter().map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec())));
    Distribution::new(files)
}

struct World {
    _dir: tempfile::TempDir,
    places: Places,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let at = |p: &str| dir.path().join(p);
    let places = Places { project: at("project"), home: at("home"), config_home: at("config"), data_home: at("data") };
    World { _dir: dir, places }
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A zip holding `files`.
fn zip(path: &Path, files: &[(&str, &str)]) -> String {
    let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, text) in files {
        w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(text.as_bytes()).unwrap();
    }
    w.finish().unwrap();
    sha256(&std::fs::read(path).unwrap())
}

fn harness(version: &str, url: &Path, sum: &str) -> String {
    format!(
        r#"[pack]
name = "fake-harness"
kind = "harness"
version = "{version}"
description = "A harness for tests."

[tool]
name = "fake"
version = "1.1.1"
supports = ">=1.0.0, <2.0.0"

[[install]]
platforms = ["linux-x86_64", "linux-arm64", "macos-arm64", "macos-x86_64"]
[[install.steps]]
download = {{ url = "file://{url}", sha256 = "{sum}", unpack = "zip" }}
[[install.steps]]
run = {{ command = "touch", args = ["made-by-a-step"] }}

[provides]
launch = {{ program = "{{dest}}/server.bin" }}
driver = "fake"
"#,
        url = url.display()
    )
}

fn load(world: &World, packs: &[(&str, &str)]) -> Result<Catalog, PackError> {
    Catalog::load(&distribution(packs), &world.places)
}

fn bad(world: &World, manifest: &str) -> String {
    load(world, &[("t/pack.toml", manifest)]).unwrap_err().to_string()
}

#[test]
fn manifests_are_checked_strictly_when_loaded() {
    let w = world();
    let good = harness("1.0.0", Path::new("/x.zip"), &"a".repeat(64));
    let catalog = load(&w, &[("harness/fake/pack.toml", &good)]).unwrap();
    assert_eq!(catalog.get("fake-harness").unwrap().kind(), Kind::Harness);

    let with = |from: &str, to: &str| good.replacen(from, to, 1);
    assert!(bad(&w, &with("[provides]", "[provides]\nextra = 1")).contains("unknown field"));
    assert!(bad(&w, &with("[provides]\nlaunch = { program = \"{dest}/server.bin\" }\ndriver = \"fake\"\n", "")).contains("harness packs need [provides]"));
    assert!(bad(&w, &with("{dest}/server.bin", "{dest}/{nope}")).contains("provides.launch: unknown placeholder {nope}"));
    assert!(bad(&w, &with(&"a".repeat(64), "abc")).contains("64 lowercase hex"));
    assert!(bad(&w, &with(&format!(", sha256 = \"{}\"", "a".repeat(64)), "")).contains("missing field `sha256`"), "a download needs a checksum");
    assert!(bad(&w, &with("file:///x.zip", "file:///{nope}.zip")).contains("unknown placeholder {nope}"));
    assert!(bad(&w, &with("\"linux-arm64\"", "\"plan9-mips\"")).contains("no platform 'plan9-mips'"));
    assert!(bad(&w, &with("version = \"1.1.1\"", "version = \"3.0.0\"")).contains("isn't in tool.supports"));
    let e = bad(&w, &with("kind = \"harness\"", "kind = \"server\""));
    assert!(e.contains("server packs need [server]"), "{e}");
    let server = r#"[pack]
name = "fake-server"
kind = "server"
version = "1.0.0"
description = "A language server for tests."
[tool]
name = "fake"
supports = ">=1.0.0"
[tool.check]
command = "fake"
version = '(\d+\.\d+\.\d+)'
[[install]]
platforms = ["linux-x86_64"]
[[install.steps]]
run = { command = "true" }
[server]
command = "fake"
args = ["--stdio"]
requires = ["textDocument/definition"]
ready_timeout_s = 60
request_timeout_s = 10
"#;
    assert!(load(&w, &[("s/pack.toml", server)]).is_ok());
    assert!(bad(&w, &server.replace("[tool.check]\ncommand = \"fake\"\nversion = '(\\d+\\.\\d+\\.\\d+)'\n", "")).contains("needs [tool.check]"));
    let language = "[pack]\nname = \"fake-lang\"\nkind = \"language\"\nversion = \"1.0.0\"\ndescription = \"A language.\"\n[language]\nsyntax = \"syntax.toml\"\nserver = \"fake-server\"\n";
    assert!(bad(&w, language).contains("no file 'syntax.toml'"));
    assert!(load(&w, &[("l/pack.toml", language), ("l/syntax.toml", ""), ("s/pack.toml", server)]).is_ok());
    assert!(load(&w, &[("l/pack.toml", language), ("l/syntax.toml", "")]).unwrap_err().to_string().contains("isn't a server pack"));
}

#[test]
fn platforms_and_sources_come_from_config_and_later_sources_win() {
    let w = world();
    let catalog = load(&w, &[]).unwrap();
    assert_eq!(catalog.platforms.for_target("linux", "aarch64", 4096).unwrap().name, "linux-arm64");
    assert_eq!(catalog.platforms.for_target("macos", "aarch64", 16384).unwrap().vars["arch"], "arm64");
    assert!(catalog.platforms.for_target("plan9", "mips", 4096).is_err());
    // The page size picks the more specific platform, a developer one.
    let sandbox = catalog.platforms.for_target("linux", "aarch64", 16384).unwrap();
    assert_eq!((sandbox.name.as_str(), sandbox.developer), ("linux-arm64-16k", true));
    assert!(!catalog.platforms.for_target("linux", "aarch64", 65536).unwrap().developer, "other page sizes: the general one");
    let names: Vec<&str> = catalog.platforms.and_general(sandbox).map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["linux-arm64-16k", "linux-arm64"], "a pack with no entry for it installs the general one");
    assert!(catalog.platforms.current().is_ok(), "this machine is one of them");
    assert_eq!(catalog.root, w.places.data_home.join("nah/packs"));
    assert_eq!(catalog.lock_path, w.places.data_home.join("nah/packs.lock"));

    // The same pack from the project and the user's folders: the last source wins.
    let shipped = harness("1.0.0", Path::new("/x.zip"), &"a".repeat(64));
    for (folder, version) in [(w.places.project.join(".nah/packs/fake"), "1.1.0"), (w.places.config_home.join("nah/packs/fake"), "1.2.0")] {
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("pack.toml"), harness(version, Path::new("/x.zip"), &"a".repeat(64))).unwrap();
    }
    let catalog = load(&w, &[("harness/fake/pack.toml", &shipped)]).unwrap();
    let pack = catalog.get("fake-harness").unwrap();
    assert_eq!(pack.manifest.pack.version, "1.2.0");
    assert!(pack.origin.contains("config"), "{}", pack.origin);
    // Two packs with one name in one source is a mistake.
    let err = load(&w, &[("a/pack.toml", &shipped), ("b/pack.toml", &shipped)]).unwrap_err().to_string();
    assert!(err.contains("two packs named 'fake-harness'"), "{err}");
}

#[test]
fn installs_run_the_steps_verify_and_record_and_a_failure_keeps_what_was_there() {
    let w = world();
    let downloads = w.places.home.join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let archive = downloads.join("server.zip");
    let sum = zip(&archive, &[("server.bin", "v1")]);
    let catalog = load(&w, &[("harness/fake/pack.toml", &harness("1.0.0", &archive, &sum))]).unwrap();
    let platform = catalog.platforms.current().unwrap().clone();
    let lock = || Lock::read(&catalog.lock_path).unwrap();
    assert_eq!(nah_packs::state(&catalog, &lock(), "fake-harness").unwrap(), State::Missing);

    let installed = nah_packs::install(&catalog, "fake-harness", &platform).unwrap();
    let program = PathBuf::from(nah_packs::harness_launch(&catalog, &lock(), "fake-harness").unwrap().program);
    assert_eq!(std::fs::read_to_string(&program).unwrap(), "v1");
    assert!(installed.dest.join("made-by-a-step").is_file(), "a run step ran in the install folder");
    assert_eq!((installed.sha256.clone(), installed.tool_version.as_deref()), (vec![sum.clone()], Some("1.1.1")));
    assert_eq!(lock().packs["fake-harness"], installed);
    assert_eq!(nah_packs::state(&catalog, &lock(), "fake-harness").unwrap(), State::Ready { tool_version: Some("1.1.1".into()) });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(std::fs::metadata(&program).unwrap().permissions().mode() & 0o111 != 0, "the program is executable");
    }

    // A newer pack whose download doesn't match its checksum: nothing changes.
    let tampered = downloads.join("server2.zip");
    zip(&tampered, &[("server.bin", "v2")]);
    let newer = load(&w, &[("harness/fake/pack.toml", &harness("2.0.0", &tampered, &sum))]).unwrap();
    assert_eq!(nah_packs::state(&newer, &lock(), "fake-harness").unwrap(), State::Outdated { installed: "1.0.0".into(), available: "2.0.0".into() });
    let err = nah_packs::install(&newer, "fake-harness", &platform).unwrap_err().to_string();
    assert!(err.contains("checksum"), "{err}");
    assert_eq!(lock().packs["fake-harness"], installed, "the lock still says the old install");
    assert_eq!(std::fs::read_to_string(&program).unwrap(), "v1", "and it's still there");
    assert!(std::fs::read_dir(&catalog.root).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with('.')), "no staging left behind");

    // The right archive: the new version replaces the old one.
    let good = zip(&tampered, &[("server.bin", "v2")]);
    let newer = load(&w, &[("harness/fake/pack.toml", &harness("2.0.0", &tampered, &good))]).unwrap();
    let updated = nah_packs::install(&newer, "fake-harness", &platform).unwrap();
    assert_eq!(std::fs::read_to_string(nah_packs::harness_launch(&newer, &lock(), "fake-harness").unwrap().program).unwrap(), "v2");
    assert!(!installed.dest.exists() && updated.dest.exists(), "the old version's folder is gone");
}

/// A program that prints a version, for checks.
fn fake_tool(dir: &Path, prints: &str) -> PathBuf {
    let path = dir.join("fake-tool");
    std::fs::write(&path, format!("#!/bin/sh\necho '{prints}'\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

#[test]
fn a_check_compares_the_tools_version_with_the_packs_range() {
    let w = world();
    std::fs::create_dir_all(&w.places.home).unwrap();
    let tool = fake_tool(&w.places.home, "fake-tool 1.4.2 (abc 2026-01-01)");
    let server = |supports: &str| {
        format!(
            "[pack]\nname = \"fake-server\"\nkind = \"server\"\nversion = \"1.0.0\"\ndescription = \"x\"\n[tool]\nname = \"fake\"\nsupports = \"{supports}\"\n[tool.check]\ncommand = \"{}\"\nargs = [\"--version\"]\nversion = '^fake-tool (\\d+\\.\\d+\\.\\d+)'\n[[install]]\nplatforms = [\"linux-x86_64\", \"linux-arm64\", \"macos-arm64\", \"macos-x86_64\"]\n[[install.steps]]\nrun = {{ command = \"true\" }}\n[server]\ncommand = \"{}\"\nrequires = [\"textDocument/definition\"]\nready_timeout_s = 5\nrequest_timeout_s = 5\n",
            tool.display(),
            tool.display()
        )
    };
    let lock = Lock::default();
    let ready = load(&w, &[("s/pack.toml", &server(">=1.0.0, <2.0.0"))]).unwrap();
    assert_eq!(nah_packs::state(&ready, &lock, "fake-server").unwrap(), State::Ready { tool_version: Some("1.4.2".into()) }, "found on its own, already installed");
    let old = load(&w, &[("s/pack.toml", &server(">=2.0.0"))]).unwrap();
    assert_eq!(nah_packs::state(&old, &lock, "fake-server").unwrap(), State::OutOfRange { found: "1.4.2".into(), supports: ">=2.0.0".into() });
    std::fs::remove_file(&tool).unwrap();
    assert_eq!(nah_packs::state(&ready, &lock, "fake-server").unwrap(), State::Missing);
}
