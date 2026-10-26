//! The generic LSP client against a
//! scripted server: it starts in the background, is ready when the server's
//! progress says so (and nothing waits for it), asks definitions together
//! and turns positions to bytes and back in the server's encoding,
//! restarts a crashed server with backoff, drains stderr, sends changes for
//! the changed files only, and stops the server when dropped.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{Change, Client, LspError, ServerSpec, ServerState};

struct Fake {
    _dir: tempfile::TempDir,
    root: PathBuf,
    log: PathBuf,
}

fn fake() -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let log = dir.path().join("server.log");
    Fake { root, log, _dir: dir }
}

fn spec(f: &Fake, flags: &[&str]) -> ServerSpec {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_server.py");
    let mut args = vec![script.to_string_lossy().into_owned(), "--log".into(), f.log.to_string_lossy().into_owned()];
    args.extend(flags.iter().map(|s| (*s).to_owned()));
    ServerSpec {
        name: "fake-server".into(),
        program: "python3".into(),
        args,
        env: Vec::new(),
        root: f.root.clone(),
        initialization_options: serde_json::json!({}),
        settings: serde_json::json!({}),
        requires: vec!["textDocument/definition".into()],
        ready_timeout: Duration::from_secs(20),
        settle: Duration::from_millis(200),
        request_timeout: Duration::from_secs(10),
        in_flight: 64,
        backoff: (Duration::from_millis(50), Duration::from_millis(400)),
        shutdown: Duration::from_secs(2),
    }
}

fn logged(f: &Fake) -> Vec<String> {
    std::fs::read_to_string(&f.log).unwrap_or_default().lines().map(str::to_owned).collect()
}

/// Byte offsets of every name in `text` (what the reader's refs give).
fn names(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut prev = None;
    for (i, c) in text.char_indices() {
        let word = c.is_alphanumeric() || c == '_';
        if word && prev != Some(true) {
            out.push(i);
        }
        prev = Some(word);
    }
    out
}

const TEXT: &str = "fn main() {\n    let é = run(1);\n    let ü = é + other();\n}\n";

#[test]
fn it_starts_in_the_background_and_answers_definitions() {
    let f = fake();
    std::fs::write(f.root.join("src/main.rs"), TEXT).unwrap();
    let t = Instant::now();
    let client = Client::start(spec(&f, &["--start-ms", "500"]));
    assert!(t.elapsed() < Duration::from_millis(200), "starting doesn't wait for the server");
    assert!(matches!(client.state(), ServerState::Starting));
    assert!(client.wait_ready(Duration::from_secs(10)));
    assert!(matches!(client.state(), ServerState::Ready { version: Some(ref v) } if v == "1.2.3"));
    let at = names(TEXT);
    let places = client.definitions(&f.root.join("src/main.rs"), TEXT, &at).unwrap();
    assert_eq!(places.len(), at.len());
    for (offset, place) in at.iter().zip(&places) {
        let place = place.place().unwrap();
        assert_eq!(
            (place.file.as_path(), place.start),
            (f.root.join("src/main.rs").as_path(), *offset),
            "the server's answer comes back as bytes"
        );
    }
    assert_eq!(logged(&f).iter().filter(|m| *m == "textDocument/definition").count(), at.len());
}

#[test]
fn more_names_than_the_limit_are_sent_in_batches_and_every_one_is_answered() {
    let f = fake();
    std::fs::write(f.root.join("src/main.rs"), TEXT).unwrap();
    // The server holds what it's asked until nothing more comes, and logs
    // how many it held: never more than the limit.
    let mut s = spec(&f, &["--batch-ms", "100"]);
    s.in_flight = 2;
    let client = Client::start(s);
    assert!(client.wait_ready(Duration::from_secs(10)));
    let at = names(TEXT);
    assert!(at.len() > 2, "more than the limit: {at:?}");
    let places = client.definitions(&f.root.join("src/main.rs"), TEXT, &at).unwrap();
    let starts: Vec<usize> = places.iter().map(|p| p.place().unwrap().start).collect();
    assert_eq!(starts, at, "each answer for its own name, in order, across batches");
    assert_eq!(logged(&f).iter().filter(|m| *m == "textDocument/definition").count(), at.len());
    let batches: Vec<usize> = logged(&f).iter().filter_map(|m| m.strip_prefix("batch ")).map(|n| n.parse().unwrap()).collect();
    assert_eq!(batches.iter().sum::<usize>(), at.len());
    assert!(batches.len() > 1 && batches.iter().all(|n| *n <= 2), "never more than the limit at once: {batches:?}");
}

#[test]
fn until_the_servers_progress_ends_it_is_not_ready_and_nothing_waits() {
    let f = fake();
    let client = Client::start(spec(&f, &["--progress-ms", "1200"]));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !logged(&f).iter().any(|m| m == "initialized") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(!client.is_ready(), "still loading");
    let t = Instant::now();
    assert!(matches!(client.definitions(&f.root.join("src/main.rs"), TEXT, &[0]), Err(LspError::NotReady)));
    assert!(t.elapsed() < Duration::from_millis(50), "asking before it's ready doesn't wait");
    assert!(client.wait_ready(Duration::from_secs(10)));
}

#[test]
fn positions_turn_into_bytes_in_the_servers_encoding() {
    for encoding in ["utf-16", "utf-8"] {
        let f = fake();
        std::fs::write(f.root.join("src/main.rs"), TEXT).unwrap();
        let client = Client::start(spec(&f, &["--encoding", encoding]));
        assert!(client.wait_ready(Duration::from_secs(10)));
        // Names after non-ASCII letters on their line.
        let at: Vec<usize> = ["other", "run"].iter().map(|n| TEXT.find(n).unwrap()).collect();
        let places = client.definitions(&f.root.join("src/main.rs"), TEXT, &at).unwrap();
        let back: Vec<usize> = places.iter().map(|p| p.place().unwrap().start).collect();
        assert_eq!(back, at, "{encoding}");
        assert_eq!(client.encoding(), Some(encoding.to_owned()));
    }
}

#[test]
fn a_crashed_server_is_restarted_with_backoff() {
    let f = fake();
    std::fs::write(f.root.join("src/main.rs"), TEXT).unwrap();
    let client = Client::start(spec(&f, &["--crash-after", "2"]));
    assert!(client.wait_ready(Duration::from_secs(10)));
    let file = f.root.join("src/main.rs");
    assert!(client.definitions(&file, TEXT, &[0, 3]).is_ok());
    // The third request kills it.
    assert!(client.definitions(&file, TEXT, &[0]).is_err());
    assert!(client.wait_ready(Duration::from_secs(10)), "restarted: {:?}", client.state());
    assert_eq!(client.restarts(), 1);
    assert!(client.definitions(&file, TEXT, &[0]).is_ok(), "answers again");
}

#[test]
fn a_server_that_floods_stderr_still_answers() {
    let f = fake();
    std::fs::write(f.root.join("src/main.rs"), TEXT).unwrap();
    let client = Client::start(spec(&f, &["--stderr-mb", "8"]));
    assert!(client.wait_ready(Duration::from_secs(20)), "{:?}", client.state());
    assert!(client.definitions(&f.root.join("src/main.rs"), TEXT, &[0]).is_ok());
}

#[test]
fn changes_are_sent_for_the_changed_files_only() {
    let f = fake();
    let client = Client::start(spec(&f, &[]));
    assert!(client.wait_ready(Duration::from_secs(10)));
    let a = f.root.join("src/a.rs");
    let b = f.root.join("src/b.rs");
    std::fs::write(&a, "fn a() {}\n").unwrap();
    client.changed(&[(a.clone(), Change::Changed), (b.clone(), Change::Deleted)]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while logged(&f).iter().filter(|l| l.starts_with("changed ")).count() < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let changed: Vec<String> = logged(&f).into_iter().filter(|l| l.starts_with("changed ")).collect();
    assert_eq!(changed.len(), 2, "{changed:?}");
    assert!(changed.iter().any(|l| l.ends_with("src/a.rs 2")) && changed.iter().any(|l| l.ends_with("src/b.rs 3")), "{changed:?}");
}

#[test]
fn dropping_the_client_stops_the_server() {
    let f = fake();
    let client = Client::start(spec(&f, &[]));
    assert!(client.wait_ready(Duration::from_secs(10)));
    let pid = client.pid().unwrap();
    drop(client);
    // Gone (or a zombie no one waits for): signal 0 can't reach it.
    let alive = || {
        std::process::Command::new("kill").args(["-0", &pid.to_string()]).status().is_ok_and(|s| s.success())
            && !std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default().contains(") Z ")
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(), "the server process ended");
    assert!(logged(&f).iter().any(|m| m == "shutdown"), "asked to shut down first");
}

/// Waits until `f`'s log has a line `pred` accepts.
fn logs(f: &Fake, pred: impl Fn(&str) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if logged(f).iter().any(|l| pred(l)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn a_server_that_reports_its_status_is_ready_only_once_quiescent() {
    let f = fake();
    let client = Client::start(spec(&f, &["--status-ms", "1000"]));
    assert!(logs(&f, |l| l == "initialized"));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!client.is_ready(), "initialized, no progress open, but not quiescent yet");
    assert!(client.wait_ready(Duration::from_secs(10)));
}

#[test]
fn a_refusal_is_not_an_answer_and_asking_again_gets_one() {
    let f = fake();
    std::fs::write(f.root.join("src/main.rs"), TEXT).unwrap();
    let client = Client::start(spec(&f, &["--refuse-first", "1"]));
    assert!(client.wait_ready(Duration::from_secs(10)));
    let file = f.root.join("src/main.rs");
    assert!(matches!(client.definitions(&file, TEXT, &[0]), Err(LspError::Refused(why)) if why.contains("content modified")));
    assert!(client.definitions(&file, TEXT, &[0]).unwrap()[0].place().is_some());
}

#[test]
fn a_server_that_hangs_is_started_again() {
    let f = fake();
    std::fs::write(f.root.join("src/main.rs"), TEXT).unwrap();
    let mut s = spec(&f, &["--hang-after", "1"]);
    s.request_timeout = Duration::from_millis(500);
    let client = Client::start(s);
    assert!(client.wait_ready(Duration::from_secs(10)));
    let file = f.root.join("src/main.rs");
    assert!(client.definitions(&file, TEXT, &[0]).is_ok());
    let t = Instant::now();
    assert!(matches!(client.definitions(&file, TEXT, &[0]), Err(LspError::Lost(_))));
    assert!(t.elapsed() < Duration::from_secs(3), "within the request limit");
    assert!(client.wait_ready(Duration::from_secs(10)), "started again: {:?}", client.state());
    assert_eq!(client.restarts(), 1);
    assert!(client.definitions(&file, TEXT, &[0]).is_ok(), "the new run answers");
}

#[test]
fn a_server_not_ready_in_time_shows_as_failed_until_it_is() {
    let f = fake();
    let mut s = spec(&f, &["--progress-ms", "1500"]);
    s.ready_timeout = Duration::from_millis(500);
    let client = Client::start(s);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !matches!(client.state(), ServerState::Failed { .. }) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(matches!(client.state(), ServerState::Failed { ref reason } if reason.contains("not ready within")), "{:?}", client.state());
    assert!(client.wait_ready(Duration::from_secs(10)), "it gets there: {:?}", client.state());
    // One that never ends its progress stays failed, with why.
    let g = fake();
    let mut s = spec(&g, &["--never-ready"]);
    s.ready_timeout = Duration::from_millis(300);
    let stuck = Client::start(s);
    assert!(!stuck.wait_ready(Duration::from_secs(1)));
    assert!(matches!(stuck.state(), ServerState::Failed { ref reason } if reason.contains("still loading")), "{:?}", stuck.state());
}

#[test]
fn a_server_without_what_it_must_answer_fails_and_isnt_restarted() {
    let f = fake();
    let client = Client::start(spec(&f, &["--no-definition"]));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !matches!(client.state(), ServerState::Failed { .. }) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        matches!(client.state(), ServerState::Failed { ref reason } if reason.contains("doesn't offer textDocument/definition")),
        "{:?}",
        client.state()
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(client.restarts(), 0);
}

#[test]
fn settings_are_answered_when_the_server_asks_for_them() {
    let f = fake();
    let mut s = spec(&f, &["--ask-config", "fake.check"]);
    s.settings = serde_json::json!({"fake": {"check": {"command": "clippy"}}});
    let client = Client::start(s);
    assert!(client.wait_ready(Duration::from_secs(10)));
    assert!(logs(&f, |l| l == r#"config [{"command": "clippy"}]"#), "{:?}", logged(&f));
}

#[test]
fn changes_made_while_it_loads_are_sent() {
    let f = fake();
    let client = Client::start(spec(&f, &["--progress-ms", "2000"]));
    assert!(logs(&f, |l| l == "initialized"));
    assert!(!client.is_ready());
    client.changed(&[(f.root.join("src/a.rs"), Change::Changed)]);
    assert!(logs(&f, |l| l.starts_with("changed ") && l.ends_with("src/a.rs 2")), "sent while loading");
}

#[test]
fn dropping_a_client_that_is_still_starting_doesnt_wait_for_it() {
    let f = fake();
    let client = Client::start(spec(&f, &["--start-ms", "8000"]));
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    drop(client);
    assert!(t.elapsed() < Duration::from_secs(1), "took {:?}", t.elapsed());
    // And while it waits to start a crashed server again.
    let g = fake();
    let mut s = spec(&g, &[]);
    s.program = "/nonexistent/server".into();
    s.backoff = (Duration::from_secs(30), Duration::from_secs(60));
    let failing = Client::start(s);
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    drop(failing);
    assert!(t.elapsed() < Duration::from_secs(1), "took {:?}", t.elapsed());
}

#[test]
fn it_says_whether_a_place_reads_or_writes_what_is_named_there() {
    let f = fake();
    let file = f.root.join("src/main.rs");
    std::fs::write(&file, TEXT).unwrap();
    let at = names(TEXT);
    for (kind, access) in [("2", Some(crate::Access::Read)), ("3", Some(crate::Access::Write)), ("1", None)] {
        let client = Client::start(spec(&f, &["--highlight-kind", kind]));
        assert!(client.wait_ready(Duration::from_secs(10)));
        assert!(client.answers_highlights());
        assert_eq!(client.accesses(&file, TEXT, &at).unwrap(), vec![access; at.len()], "kind {kind}");
    }
    // A server without highlights says so.
    let client = Client::start(spec(&f, &[]));
    assert!(client.wait_ready(Duration::from_secs(10)));
    assert!(!client.answers_highlights());
}

#[test]
fn highlight_places_that_cant_be_converted_are_counted() {
    let f = fake();
    let file = f.root.join("src/main.rs");
    std::fs::write(&file, TEXT).unwrap();
    let at = names(TEXT);
    let client = Client::start(spec(&f, &["--highlight-kind", "3", "--bad-highlights"]));
    assert!(client.wait_ready(Duration::from_secs(10)));
    assert_eq!(client.accesses(&file, TEXT, &at).unwrap(), vec![None; at.len()], "no kind it can place");
    assert_eq!(client.dropped_positions(), at.len(), "each counted");
}

#[test]
fn a_server_that_stops_reading_never_blocks_the_client() {
    let f = fake();
    let text = "fn main() {}\n".repeat(4096);
    std::fs::write(f.root.join("src/main.rs"), &text).unwrap();
    // Asked all at once, more than a pipe holds: sending them waits on the
    // server reading, and this one has stopped.
    let at: Vec<usize> = (0..4096).map(|line| line * 13 + 3).collect();
    let mut s = spec(&f, &["--deaf"]);
    s.request_timeout = Duration::from_secs(2);
    s.in_flight = at.len();
    let client = std::sync::Arc::new(Client::start(s));
    assert!(client.wait_ready(Duration::from_secs(10)));
    let asking = {
        let (client, file) = (client.clone(), f.root.join("src/main.rs"));
        std::thread::spawn(move || client.definitions(&file, &text, &at))
    };
    std::thread::sleep(Duration::from_millis(300));
    // While that waits on the server, everything else still answers.
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let client = client.clone();
        std::thread::spawn(move || tx.send(client.state()).unwrap());
    }
    assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok(), "the client's state answers while a send waits on the server");
    assert!(matches!(asking.join().unwrap(), Err(LspError::Lost(_))), "and the questions give up in time");
}
