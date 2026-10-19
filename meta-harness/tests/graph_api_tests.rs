//! The three views over REST and MCP, on a copy of the Rust fixture repo:
//! the cockpit and the agent get the same trees.

mod common;

use common::{Mcp, http, spawn_app};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

fn keys(tree: &Value) -> Vec<String> {
    let mut k: Vec<String> = tree["cards"].as_array().unwrap().iter().map(|c| c["key"].as_str().unwrap().to_owned()).collect();
    k.sort();
    k
}

#[tokio::test]
async fn the_cockpit_gets_laid_out_trees_and_loads_more_in_pieces() {
    let (addr, _) = spawn_app().await;
    let (status, body) = http(addr, "GET", "/api/graph/tree?view=calls&root=rust:sample::run&depth=1", None).await;
    assert_eq!(status, 200, "{body}");
    let tree = parse(&body);
    assert_eq!(tree["roots"], json!(["rust:sample::run"]));
    let run = tree["cards"].as_array().unwrap().iter().find(|c| c["key"] == "rust:sample::run").unwrap();
    assert!(run["rect"]["w"].as_f64().unwrap() > 0.0, "laid out on the server");
    assert!(tree["lines"].as_array().unwrap().iter().all(|l| l["path"].as_array().unwrap().len() >= 2), "every line has its path");
    // Nothing there: 404, said plainly.
    let (status, body) = http(addr, "GET", "/api/graph/tree?view=calls&root=rust:sample::ghost", None).await;
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("isn't in the calls view"));
    // Opening a card: only what it adds.
    let leaf = tree["cards"].as_array().unwrap().iter().find(|c| c["more"].as_u64().unwrap_or(0) > 0).expect("a card with more under it");
    let card = leaf["key"].as_str().unwrap();
    let (status, body) = http(addr, "GET", &format!("/api/graph/expand?view=calls&root=rust:sample::run&depth=1&card={card}"), None).await;
    assert_eq!(status, 200, "{body}");
    let added = parse(&body);
    let before = keys(&tree);
    assert!(!added["cards"].as_array().unwrap().is_empty());
    assert!(keys(&added).iter().all(|k| !before.contains(k)), "nothing the cockpit already has");
    // Placed by the server: every card held, the ones held before too.
    let placed: Vec<String> = added["places"].as_array().unwrap().iter().map(|p| p["key"].as_str().unwrap().to_owned()).collect();
    assert!(before.iter().chain(&keys(&added)).all(|k| placed.contains(k)), "every card has its place");
    // Where a card sits.
    let (status, body) = http(addr, "GET", "/api/graph/context?view=calls&card=rust:sample::run", None).await;
    assert_eq!(status, 200, "{body}");
    assert!(keys(&parse(&body)).contains(&"rust:sample::run".to_owned()));
    // One card in full, fetched only when asked.
    let (status, body) = http(addr, "GET", "/api/graph/details/rust:sample::log", None).await;
    assert_eq!(status, 200);
    assert!(parse(&body)["code"].as_str().unwrap().contains("println!"));
    // The sizes the cockpit draws with.
    let (status, body) = http(addr, "GET", "/api/graph/config", None).await;
    assert_eq!(status, 200);
    assert!(parse(&body)["layout"]["gap_x"].as_f64().unwrap() > 0.0);
    // Contains, no root: the top-level parts as boxes.
    let (_, body) = http(addr, "GET", "/api/graph/tree?view=contains&depth=1", None).await;
    let boxes = parse(&body);
    assert!(boxes["lines"].as_array().is_none_or(|l| l.is_empty()));
    assert!(!boxes["cards"].as_array().unwrap().is_empty());
    // What a view isn't asked with is refused, not ignored.
    let (status, body) = http(addr, "GET", "/api/graph/tree?view=contains&plan=x", None).await;
    assert!(status == 400 && body.contains("plan: not something a view is asked with"), "{status} {body}");
    // The old ways in are gone.
    assert_eq!(http(addr, "POST", "/api/graph/tree", Some(&json!({ "view": "contains" }).to_string())).await.0, 405);
    assert_eq!(http(addr, "GET", "/api/graph/card?key=rust:sample::log", None).await.0, 404);
}

#[tokio::test]
async fn answers_are_compressed_when_the_browser_accepts_it() {
    let (addr, _) = spawn_app().await;
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = "GET /api/graph/tree?view=contains&depth=3 HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n";
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let head = String::from_utf8_lossy(&raw[..raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap()]).to_lowercase();
    assert!(head.contains("content-encoding: gzip"), "{head}");
}

#[tokio::test]
async fn the_agent_sees_the_same_tree_as_text() {
    let (addr, _) = spawn_app().await;
    let (_, body) = http(addr, "GET", "/api/graph/tree?view=calls&root=rust:sample::run&depth=2", None).await;
    let tree = parse(&body);
    let mut mcp = Mcp::lead(addr).await;
    mcp.rpc("initialize", json!({})).await;
    let (err, text) = mcp.tool("view", json!({ "view": "calls", "root": "rust:sample::run", "depth": 2 })).await;
    assert!(!err, "{text}");
    for key in keys(&tree) {
        assert!(text.contains(&format!("key={key}")), "{key} in the agent's view:\n{text}");
    }
    // Up, and loading more, and where it sits.
    let (err, text) = mcp.tool("view", json!({ "view": "calls", "root": "rust:sample::log", "way": "up", "depth": 1 })).await;
    assert!(!err && text.contains("what calls it") && text.contains("key=rust:sample::run"), "{text}");
    let leaf = tree["cards"].as_array().unwrap().iter().find(|c| c["more"].as_u64().unwrap_or(0) > 0).map(|c| c["key"].as_str().unwrap().to_owned());
    if let Some(card) = leaf {
        let (err, text) = mcp.tool("view", json!({ "view": "calls", "root": "rust:sample::run", "depth": 2, "expand": card })).await;
        assert!(!err, "{text}");
    }
    let (err, text) = mcp.tool("view", json!({ "view": "contains", "context": "rust:sample::run" })).await;
    assert!(!err && text.contains("key=rust:sample::run"), "{text}");
    let (err, text) = mcp.tool("view", json!({ "view": "data_flow" })).await;
    assert!(!err && text.contains("data_flow (code)"), "{text}");
}

#[tokio::test]
async fn the_perf_page_shows_recent_answers_by_stage() {
    let (addr, _) = spawn_app().await;
    let (status, _) = http(addr, "GET", "/api/graph/tree?view=contains&depth=2", None).await;
    assert_eq!(status, 200);
    let (status, body) = http(addr, "GET", "/api/perf", None).await;
    assert_eq!(status, 200, "{body}");
    let perf = parse(&body);
    let tree = perf["recent"].as_array().unwrap().iter().rev().find(|t| t["what"] == "tree").expect("the tree answer is there");
    let stages: Vec<&str> = tree["stages"].as_array().unwrap().iter().map(|s| s["stage"].as_str().unwrap()).collect();
    assert_eq!(stages, ["source", "badges", "walk", "layout", "keep", "encode"]);
    assert_eq!(tree["size"][1], "bytes");
    assert!(perf["graph"]["components"].as_u64().unwrap() > 0 && perf["last_index"]["what"] == "ingest", "{body}");
}
