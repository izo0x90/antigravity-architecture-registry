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
        let child = std::process::Command::new(env!("CARGO_BIN_EXE_meta-harness"))
            .args(args)
            .args(["--listen", &format!("127.0.0.1:{port}")])
            .current_dir(&place)
            .env("HOME", home.path())
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("NAH_REPO_ROOT", &place)
            .env("NAH_COCKPIT_TOKEN", SECRET)
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
