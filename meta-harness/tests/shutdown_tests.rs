//! Stopping the server: SIGTERM stops it the way Ctrl-C does, even while an
//! agent holds an event stream open.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

#[cfg(unix)]
#[test]
fn sigterm_stops_the_server_while_an_agents_event_stream_is_open() {
    let home = tempfile::tempdir().unwrap();
    // A project, a stand-in cockpit build, and a home with no harness.
    let project = home.path().join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
    std::fs::write(project.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    let static_dir = home.path().join("static");
    let build = static_dir.join(meta_harness::server::COCKPIT_BUILD);
    std::fs::create_dir_all(build.parent().unwrap()).unwrap();
    std::fs::write(&build, b"").unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut server = std::process::Command::new(env!("CARGO_BIN_EXE_meta-harness"))
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("NAH_REPO_ROOT", &project)
        .env("STATIC_DIR", &static_dir)
        .env_remove("PORT")
        .env("METAHARNESS_PORT", port.to_string())
        .env("HOST", "127.0.0.1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // An agent's event stream, open and answered.
    let started = Instant::now();
    let mut stream = loop {
        if let Ok(s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            break s;
        }
        assert!(server.try_wait().unwrap().is_none(), "the server exited on start");
        assert!(started.elapsed() < Duration::from_secs(60), "the server didn't listen");
        std::thread::sleep(Duration::from_millis(100));
    };
    write!(stream, "GET /api/mcp/sse HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n").unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut seen = Vec::new();
    let mut buf = [0u8; 1024];
    while !String::from_utf8_lossy(&seen).contains("event: endpoint") {
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0, "the stream ended before it began: {}", String::from_utf8_lossy(&seen));
        seen.extend_from_slice(&buf[..n]);
    }
    let _ = std::process::Command::new("kill").args(["-TERM", &server.id().to_string()]).status();
    let stopping = Instant::now();
    while server.try_wait().unwrap().is_none() && stopping.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let exited = server.try_wait().unwrap();
    if exited.is_none() {
        let _ = server.kill();
        let _ = server.wait();
    }
    assert!(exited.is_some_and(|s| s.success()), "stopped cleanly within 10 s with the stream open: {exited:?}");
}
