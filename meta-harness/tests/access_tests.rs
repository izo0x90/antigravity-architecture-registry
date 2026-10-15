//! Every route says what it needs: each handler takes `Needs<…>` first,
//! except the few that are open on purpose, and every route that isn't
//! open refuses a request with no credentials.

mod common;

use common::{http_anonymous, spawn_app};

/// Open on purpose: health, the launch link, the agents' endpoint (their
/// session's key says who they are), and the sockets (a ticket each).
const OPEN_HANDLERS: &[&str] = &["health_check", "crate::cockpit::launch", "mcp_sse_proxy", "mcp_messages_proxy", "ws_events_handler", "crate::remote::socket"];
const OPEN_PATHS: &[&str] = &["/health", "/cockpit", "/api/mcp/sse", "/api/mcp/messages", "/ws/events", "/ws/cockpit"];

#[test]
fn every_routed_handler_takes_needs_first() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<_> = std::fs::read_dir(root.join("routes")).unwrap().flatten().map(|e| e.path()).collect();
    files.extend(["arch.rs", "graph.rs", "queue.rs"].map(|f| root.join(f)));
    let route = regex::Regex::new(r#"\.route\("([^"]+)",\s*([^\n]+)\)"#).unwrap();
    let handler = regex::Regex::new(r"\b(?:get|post|put|delete)\(([\w:]+)\)").unwrap();
    let mut checked = 0;
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for r in route.captures_iter(&text) {
            for h in handler.captures_iter(&r[2]) {
                let name = &h[1];
                if OPEN_HANDLERS.contains(&name) {
                    continue;
                }
                let (source, fn_name) = match name.rsplit_once("::") {
                    Some((module, f)) => (std::fs::read_to_string(root.join(format!("{}.rs", module.trim_start_matches("crate::").replace("::", "/")))).unwrap(), f.to_owned()),
                    None => (text.clone(), name.to_owned()),
                };
                let at = source.find(&format!("\nasync fn {fn_name}(")).or_else(|| source.find(&format!("\npub async fn {fn_name}("))).unwrap_or_else(|| panic!("{}: no handler {name} for {}", file.display(), &r[1]));
                let params = source[at..].split_once('(').unwrap().1.trim_start();
                assert!(params.starts_with("_: Needs<"), "{} {}: {name} must take Needs<…> first", file.display(), &r[1]);
                checked += 1;
            }
        }
    }
    assert!(checked > 60, "every route was looked at ({checked})");
}

#[tokio::test]
async fn a_request_with_no_credentials_is_refused_everywhere_but_the_open_routes() {
    let (addr, _) = spawn_app().await;
    let pinned = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/http_routes.txt")).unwrap();
    let mut refused = 0;
    for line in pinned.lines() {
        let (path, methods) = line.split_once(' ').unwrap();
        if OPEN_PATHS.contains(&path) {
            continue;
        }
        for method in methods.split(',').filter(|m| *m != "HEAD") {
            let (status, body) = http_anonymous(addr, method, path, Some("{}")).await;
            assert_eq!(status, 403, "{method} {path}: {body}");
            refused += 1;
        }
    }
    assert!(refused > 60, "{refused}");
}

/// The sockets take a one-time ticket, which only the cockpit's session
/// gets: none, a made-up one, or one used already is refused.
#[tokio::test]
async fn the_sockets_take_a_ticket_good_once() {
    let (addr, _) = spawn_app().await;
    for path in ["/ws/events", "/ws/cockpit"] {
        assert!(tokio_tungstenite::connect_async(format!("ws://{addr}{path}")).await.is_err(), "{path}: no ticket");
        assert!(tokio_tungstenite::connect_async(format!("ws://{addr}{path}?ticket=made-up")).await.is_err(), "{path}: made up");
        let ticket = common::ticket(addr).await;
        let opened = tokio_tungstenite::connect_async(format!("ws://{addr}{path}?ticket={ticket}")).await;
        assert!(opened.is_ok(), "{path}: a good ticket");
        assert!(tokio_tungstenite::connect_async(format!("ws://{addr}{path}?ticket={ticket}")).await.is_err(), "{path}: used once");
    }
}

/// A request's log line has its path, never its query: sessions' keys and
/// sockets' tickets travel there.
#[test]
fn request_logs_never_carry_a_key() {
    #[derive(Clone, Default)]
    struct Buffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt().with_writer(move || writer.clone()).with_ansi(false).finish();
    tracing::subscriber::with_default(subscriber, || {
        let request = axum::http::Request::get("/api/mcp/sse?key=secret-key&ticket=secret-ticket").body(()).unwrap();
        let span = meta_harness::server::request_span(&request);
        let _inside = span.enter();
        tracing::info!("answered");
    });
    let logged = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    assert!(logged.contains("/api/mcp/sse") && !logged.contains("secret"), "{logged}");
}
