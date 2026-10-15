//! What agents and the cockpit can reach, pinned: the MCP tools (names,
//! order, descriptions, schemas) and every HTTP route with its methods.
//! Refactors must leave these unchanged; a change that means to alter them
//! updates the golden files on purpose (`UPDATE_SNAPSHOTS=1`) and says so.

mod common;

use std::net::SocketAddr;
use std::path::PathBuf;

use common::{Mcp, spawn_app};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Compares `actual` with `tests/snapshots/<name>`, or rewrites it when
/// `UPDATE_SNAPSHOTS` is set.
fn golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_SNAPSHOTS=1 to create it)", path.display()));
    if expected != actual {
        let first = expected.lines().zip(actual.lines()).position(|(a, b)| a != b).unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} changed at line {}:\n  pinned: {:?}\n  now:    {:?}\n(if the change is intended, rerun with UPDATE_SNAPSHOTS=1)",
            first + 1,
            expected.lines().nth(first),
            actual.lines().nth(first)
        );
    }
}

#[tokio::test]
async fn the_mcp_tools_are_as_pinned() {
    let (addr, _) = spawn_app().await;
    let mut mcp = Mcp::lead(addr).await;
    let tools = mcp.rpc("tools/list", json!({})).await;
    let mut out = String::new();
    for tool in tools["result"]["tools"].as_array().unwrap() {
        // One tool per block: its name, description and schema (keys sorted).
        out.push_str(&format!("## {}\n{}\n{}\n\n", tool["name"].as_str().unwrap(), tool["description"].as_str().unwrap(), serde_json::to_string_pretty(&tool["inputSchema"]).unwrap()));
    }
    golden("mcp_tools.txt", &out);
}

/// Every route, with path parameters filled in.
const ROUTES: &[&str] = &[
    "/health",
    "/cockpit",
    "/api/ui/config",
    "/api/ui/state",
    "/api/instances",
    "/api/instances/fake/refresh",
    "/api/instances/fake/sessions",
    "/api/instances/fake/sessions/n1",
    "/api/instances/fake/auth/oauth",
    "/api/instances/fake/auth/complete",
    "/api/instances/fake/auth/logout",
    "/api/threads",
    "/api/threads/t1",
    "/api/threads/t1/turns",
    "/api/threads/t1/inject",
    "/api/threads/t1/steer",
    "/api/threads/t1/interrupt",
    "/api/threads/t1/requests/r1",
    "/api/slots",
    "/api/slots/s1",
    "/api/slots/s1/harness",
    "/api/slots/s1/settings",
    "/api/slots/s1/sessions",
    "/api/slots/s1/sessions/k1",
    "/api/slots/s1/current",
    "/api/slots/s1/prompt",
    "/api/fs/read",
    "/api/dev/feedback",
    "/api/mcp/servers",
    "/api/mcp/sse",
    "/api/mcp/messages",
    "/ws/events",
    "/ws/cockpit",
    "/api/cockpit/ticket",
    "/api/graph/tree",
    "/api/graph/expand",
    "/api/graph/context",
    "/api/graph/details/c1",
    "/api/graph/config",
    "/api/perf",
    "/api/arch/types",
    "/api/search",
    "/api/arch/components/c1",
    "/api/arch/at",
    "/api/arch/file",
    "/api/code/analysis",
    "/api/arch/ingest",
    "/api/arch/index",
    "/api/arch/refresh",
    "/api/arch/tools",
    "/api/arch/reviewers",
    "/api/arch/ambiguous",
    "/api/arch/unproven",
    "/api/arch/navigate",
    "/api/arch/plans",
    "/api/arch/import",
    "/api/arch/plans/p1",
    "/api/arch/plans/p1/ops",
    "/api/arch/plans/p1/scope",
    "/api/arch/plans/p1/validate",
    "/api/arch/plans/p1/next",
    "/api/arch/plans/p1/components/c1/contract",
    "/api/arch/plans/p1/components/c1/verify",
    "/api/arch/plans/p1/components/c1/approve",
    "/api/arch/plans/p1/tools",
    "/api/arch/plans/p1/components/c1/review",
    "/api/arch/plans/p1/comments",
    "/api/arch/plans/p1/comments/k1/replies",
    "/api/arch/plans/p1/comments/k1/accept",
    "/api/arch/plans/p1/comments/k1/reopen",
    "/api/arch/plans/p1/reviewed",
    "/api/arch/plans/p1/diff",
    "/api/arch/plans/p1/queue",
    "/api/arch/plans/p1/queue/start",
    "/api/arch/plans/p1/queue/stop",
    "/api/arch/plans/p1/queue/retry",
];

/// The methods a path takes, from the `Allow` header of a request with a
/// method no route uses: a route answers 405 with its methods, without
/// running any handler; the static-file fallback answers 405 with none.
async fn allowed(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("PATCH {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let status = response.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let allow = response
        .lines()
        .find_map(|l| l.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("allow")).map(|(_, v)| v.trim().to_owned()))
        .unwrap_or_default();
    let mut methods: Vec<&str> = allow.split(',').map(str::trim).filter(|m| !m.is_empty()).collect();
    methods.sort_unstable();
    (status, methods.join(","))
}

#[tokio::test]
async fn the_http_routes_are_as_pinned() {
    let (addr, _) = spawn_app().await;
    let mut out = String::new();
    for path in ROUTES {
        let (status, methods) = allowed(addr, path).await;
        assert!(status == 405 && !methods.is_empty(), "{path} is a route (PATCH is no route's method): {status} [{methods}]");
        out.push_str(&format!("{path} {methods}\n"));
    }
    let (_, methods) = allowed(addr, "/api/no/such/route").await;
    assert!(methods.is_empty(), "an unknown path isn't mistaken for a route: [{methods}]");
    golden("http_routes.txt", &out);
}
