//! How the server runs: its parts (`--no-project`, `--no-cockpit`), where
//! it may listen, and its secret.

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

const SECRET: &str = "the-servers-secret";

/// A server started as the user would, in its own home, with `args`; the
/// project (when one is given) and a stand-in cockpit build.
struct Server {
    child: std::process::Child,
    port: u16,
    home: tempfile::TempDir,
}

impl Server {
    fn start(project: bool, args: &[&str]) -> Self {
        Self::start_in(project, args, "place")
    }

    /// The same, started from `cwd` (in its home; the project is still
    /// `place`, as `NAH_REPO_ROOT` says).
    fn start_in(project: bool, args: &[&str], cwd: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let place = home.path().join("place");
        std::fs::create_dir_all(place.join("src")).unwrap();
        if project {
            std::fs::write(place.join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
            std::fs::write(place.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
        }
        let page = home.path().join("static");
        let build = page.join(meta_harness::server::COCKPIT_BUILD);
        std::fs::create_dir_all(build.parent().unwrap()).unwrap();
        std::fs::write(&build, b"").unwrap();
        std::fs::write(page.join("index.html"), "<title>NAH</title>").unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let child = Self::command(home.path())
            .current_dir(home.path().join(cwd))
            .args(args)
            .args(["--listen", &format!("127.0.0.1:{port}")])
            .env("STATIC_DIR", &page)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut server = Self { child, port, home };
        let started = Instant::now();
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(server.child.try_wait().unwrap().is_none(), "the server exited on start");
            assert!(started.elapsed() < Duration::from_secs(60), "the server didn't listen");
            std::thread::sleep(Duration::from_millis(100));
        }
        server
    }

    /// The program as the user runs it in `home`'s place.
    fn command(home: &Path) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_meta-harness"));
        command
            .current_dir(home.join("place"))
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("NAH_REPO_ROOT", home.join("place"))
            .env("NAH_COCKPIT_TOKEN", SECRET);
        command
    }

    /// A GET from the cockpit (with its cookie and `headers`): the status,
    /// the headers, the body.
    fn get(&self, path: &str, headers: &str) -> (u16, String, Vec<u8>) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\nCookie: nah_cockpit={SECRET}\r\n{headers}Connection: close\r\n\r\n").unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer);
        let split = answer.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&answer[..split]).into_owned();
        let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        (status, head, answer[split + 4..].to_vec())
    }

    fn status(&self, path: &str) -> u16 {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        answer.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    fn place(&self) -> std::path::PathBuf {
        self.home.path().join("place")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn exists(path: &Path) -> bool {
    path.exists()
}

#[test]
fn a_server_with_no_project_serves_the_cockpit_and_reads_no_code() {
    let server = Server::start(false, &["--no-project"]);
    assert_eq!(server.status("/health"), 200);
    assert_eq!(server.status(&format!("/cockpit?nah={SECRET}")), 200, "the cockpit opens");
    assert_eq!(server.status("/index.html"), 200, "its page is served");
    for project_route in ["/api/graph/config", "/api/instances", "/api/slots", "/api/notifications"] {
        assert_eq!(server.status(project_route), 404, "{project_route}: no project, no harnesses");
    }
    assert!(exists(&server.home.path().join("data/nah/home/state.sqlite")), "its state is the user's, not a project's");
    assert!(!exists(&server.place().join(".nah")), "no project read, watched or analysed where it started");
}

#[test]
fn a_server_without_the_cockpit_serves_the_project_but_not_the_page() {
    let server = Server::start(true, &["serve", "--no-cockpit"]);
    assert_eq!(server.status("/health"), 200);
    for page in ["/index.html", &format!("/cockpit?nah={SECRET}"), "/api/ui/state"] {
        assert_eq!(server.status(page), 404, "{page}");
    }
    assert_eq!(server.status("/api/instances"), 403, "the project's API is there (for those it admits)");
}

#[test]
fn the_server_says_its_secret_and_refuses_an_address_the_internet_reaches() {
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_meta-harness")).args(args).env("HOME", home.path()).env("XDG_CONFIG_HOME", home.path().join("config")).env_remove("NAH_COCKPIT_TOKEN").output().unwrap()
    };
    let first = run(&["token"]);
    let secret = String::from_utf8(first.stdout).unwrap();
    assert!(first.status.success() && secret.trim().len() >= 32, "{secret:?}");
    assert_eq!(String::from_utf8(run(&["token"]).stdout).unwrap(), secret, "the same one each time");
    let refused = run(&["--no-cockpit", "--no-project"]);
    assert!(!refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("nothing to serve"));
    let public = run(&["--no-project", "--listen", "0.0.0.0:0"]);
    let said = String::from_utf8_lossy(&public.stderr);
    assert!(!public.status.success() && said.contains("0.0.0.0") && said.contains("--listen-public"), "{said}");
}

#[test]
fn the_servers_a_home_links_to_are_kept_from_the_command_line() {
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| std::process::Command::new(env!("CARGO_BIN_EXE_meta-harness")).arg("servers").args(args).env("HOME", home.path()).env("XDG_DATA_HOME", home.path().join("data")).output().unwrap();
    let added = run(&["add", "cloud", "http://100.64.0.7:8080", "its-secret"]);
    assert!(added.status.success(), "{}", String::from_utf8_lossy(&added.stderr));
    let listed = String::from_utf8(run(&["list"]).stdout).unwrap();
    assert_eq!(listed, "cloud\thttp://100.64.0.7:8080\n", "listed without its secret");
    let taken = run(&["add", "here", "http://127.0.0.1:1", "x"]);
    assert!(!taken.status.success() && String::from_utf8_lossy(&taken.stderr).contains("this server's own"));
    assert!(run(&["remove", "cloud"]).status.success());
    let again = run(&["remove", "cloud"]);
    assert!(!again.status.success() && String::from_utf8_lossy(&again.stderr).contains("no server named"));
}

/// A font pack's face, as the cockpit asks for it: before it's installed,
/// what installs it; installed (here, as a project's own pack, found from
/// the project as `pack install` finds it, wherever the server started),
/// its file; asked again unchanged, "not modified" (not sent again);
/// outdated or with its lock unreadable, said as such.
#[test]
fn a_font_packs_face_is_served_once_installed_and_not_sent_again_unchanged() {
    use sha2::Digest;
    let server = Server::start_in(false, &["--no-project"], "");
    let face = b"a face, standing in for a font's bytes".to_vec();
    let file = server.home.path().join("Fake-Regular.ttf");
    std::fs::write(&file, &face).unwrap();
    let sum: String = sha2::Sha256::digest(&face).iter().map(|b| format!("{b:02x}")).collect();
    let pack = server.place().join(".nah/packs/fake-font");
    std::fs::create_dir_all(&pack).unwrap();
    let manifest = format!(
        r#"[pack]
name = "fake-font"
kind = "font"
version = "1.0.0"
description = "A font for tests."

[tool]
name = "Fake Mono"
version = "1.0.0"
supports = ">=1.0.0, <2.0.0"

[[install]]
platforms = ["linux-x86_64", "linux-arm64", "linux-arm64-16k", "macos-arm64", "macos-x86_64"]
holds = ["Fake-Regular.ttf"]
[[install.steps]]
download = {{ url = "file://{}", sha256 = "{sum}", save_as = "Fake-Regular.ttf" }}

[font]
face = "Fake-Regular.ttf"
"#,
        file.display()
    );
    std::fs::write(pack.join("pack.toml"), manifest).unwrap();
    let (status, _, body) = server.get("/api/ui/fonts/fake-font", "");
    let said = String::from_utf8_lossy(&body);
    assert_eq!(status, 404, "{said}");
    assert!(said.contains("meta-harness pack install fake-font"), "{said}");

    let installed = Server::command(server.home.path()).args(["pack", "install", "fake-font"]).output().unwrap();
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));
    let (status, head, body) = server.get("/api/ui/fonts/fake-font", "");
    assert_eq!((status, body), (200, face));
    let header = |name: &str| head.lines().find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_owned()));
    assert_eq!(header("content-type").as_deref(), Some("font/ttf"));
    assert_eq!(header("cache-control").as_deref(), Some("no-cache"), "asked again each time it's used");
    let changed = header("last-modified").expect("when it last changed");
    let (status, _, body) = server.get("/api/ui/fonts/fake-font", &format!("If-Modified-Since: {changed}\r\n"));
    assert_eq!((status, body.len()), (304, 0), "unchanged: not sent again");
    assert_eq!(server.get("/api/ui/fonts/rust", "").0, 400, "not a font");

    // A newer pack, not installed yet: outdated, and how to update.
    std::fs::write(pack.join("pack.toml"), std::fs::read_to_string(pack.join("pack.toml")).unwrap().replacen("version = \"1.0.0\"", "version = \"1.1.0\"", 1)).unwrap();
    let (status, _, body) = server.get("/api/ui/fonts/fake-font", "");
    assert_eq!(status, 409, "{}", String::from_utf8_lossy(&body));
    assert!(String::from_utf8_lossy(&body).contains("meta-harness pack update fake-font"));
    // The server's own lock unreadable: its fault, not the cockpit's.
    // (The user's data folder: XDG's here, or macOS's own under its home.)
    let data = if cfg!(target_os = "macos") { server.home.path().join("Library/Application Support") } else { server.home.path().join("data") };
    let lock = data.join("nah/packs.lock");
    assert!(lock.is_file(), "the lock is where the server's packs are");
    std::fs::write(&lock, "not a lock").unwrap();
    assert_eq!(server.get("/api/ui/fonts/fake-font", "").0, 500);
}
