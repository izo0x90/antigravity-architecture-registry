//! The explainer page: served by the app, its reference lists generated from
//! the real system, and notes the user leaves on its items.

mod common;

use common::{http, http_cockpit, spawn_app};
use serde_json::{Value, json};

/// Compares a generated reference file with the one saved under
/// `static/explainer/data/` (`UPDATE_EXPLAINER=1` writes it).
fn up_to_date(name: &str, value: &Value) {
    // Keys sorted, whichever map order serde_json was built with.
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                Value::Object(keys.into_iter().map(|k| (k.clone(), sorted(&m[k]))).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    let text = serde_json::to_string_pretty(&sorted(value)).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static/explainer/data").join(name);
    if std::env::var_os("UPDATE_EXPLAINER").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    let saved = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(saved == text, "static/explainer/data/{name} is out of date: run the tests with UPDATE_EXPLAINER=1");
}

// req: E.2
#[test]
fn the_explainer_lists_every_agent_tool_and_view() {
    let views = arch_graph::ViewCatalog::defaults().unwrap();
    let tools: Vec<Value> = meta_harness::mcp::list_tools(&views, &arch_graph::tree::TreeConfig::defaults().unwrap())
        .into_iter()
        .map(|t| {
            let required: Vec<&str> = t.input_schema["required"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let params: Vec<Value> = t.input_schema["properties"]
                .as_object()
                .map(|p| {
                    p.iter()
                        .map(|(name, spec)| json!({ "name": name, "type": spec["type"], "description": spec["description"], "required": required.contains(&name.as_str()) }))
                        .collect()
                })
                .unwrap_or_default();
            json!({ "name": t.name, "description": t.description, "params": params })
        })
        .collect();
    assert!(tools.len() > 10, "{}", tools.len());
    up_to_date("agent-tools.json", &Value::Array(tools));
    up_to_date("views.json", &serde_json::to_value(&views).unwrap());
}

// req: E.1, E.3
#[tokio::test]
async fn the_explainer_is_served_and_keeps_the_users_notes() {
    let (addr, _) = spawn_app().await;
    let (status, page) = http(addr, "GET", "/explainer/", None).await;
    assert_eq!(status, 200);
    assert!(page.contains("<title>"), "the page itself");
    // Nothing yet; the agent (no cookie) can read but not write.
    assert_eq!(http(addr, "GET", "/api/explainer/notes", None).await, (200, "[]".to_owned()));
    let note = json!({ "item": "levels", "kind": "question", "text": "Why two kinds of level?" }).to_string();
    assert_eq!(http(addr, "POST", "/api/explainer/notes", Some(&note)).await.0, 403, "only the user writes notes");
    let (status, body) = http_cockpit(addr, "POST", "/api/explainer/notes", Some(&note)).await;
    assert_eq!(status, 201, "{body}");
    let made: Value = serde_json::from_str(&body).unwrap();
    let id = made["id"].as_str().unwrap().to_owned();
    let (_, listed) = http(addr, "GET", "/api/explainer/notes", None).await;
    let listed: Value = serde_json::from_str(&listed).unwrap();
    assert_eq!((listed[0]["item"].as_str(), listed[0]["text"].as_str(), listed[0]["resolved"].as_bool()), (Some("levels"), Some("Why two kinds of level?"), Some(false)));
    // Resolved, then removed.
    let (status, body) = http_cockpit(addr, "PUT", &format!("/api/explainer/notes/{id}"), Some(r#"{"resolved":true}"#)).await;
    assert_eq!((status, serde_json::from_str::<Value>(&body).unwrap()["resolved"].as_bool()), (200, Some(true)));
    let empty = json!({ "item": "levels", "kind": "question", "text": "  " }).to_string();
    assert_eq!(http_cockpit(addr, "POST", "/api/explainer/notes", Some(&empty)).await.0, 400, "an empty note is refused");
    assert_eq!(http_cockpit(addr, "DELETE", &format!("/api/explainer/notes/{id}"), None).await.0, 204);
    assert_eq!(http(addr, "GET", "/api/explainer/notes", None).await, (200, "[]".to_owned()));
}

/// The quoted names after `key: "` in `text` (as in `a: "graph.focus"`), and
/// those written inline as `{key:name}`.
fn named(text: &str, key: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for open in [format!("{key}: \""), format!("{{{key}:")] {
        let close = if open.ends_with('"') { '"' } else { '}' };
        for (at, _) in text.match_indices(&open) {
            let before = text[..at].chars().last();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            if let Some(name) = text[at + open.len()..].split(close).next() {
                out.insert(name.to_owned());
            }
        }
    }
    out
}

// req: E.1, E.2
#[test]
fn the_explainer_names_only_real_things_and_explains_every_action() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static/explainer");
    // The content without its comments (they describe the syntax).
    let content: String = std::fs::read_to_string(dir.join("content.js")).unwrap().lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    let demos = std::fs::read_to_string(dir.join("demos.js")).unwrap();
    let json = |f: &str| -> Value { serde_json::from_str(&std::fs::read_to_string(dir.join("data").join(f)).unwrap()).unwrap() };
    let actions: std::collections::BTreeSet<String> = json("actions.json").as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_owned()).collect();
    let tools: std::collections::BTreeSet<String> = json("agent-tools.json").as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect();
    let views = json("views.json");
    // Every action, tool, view and demo the page names exists.
    let used = named(&content, "a");
    assert!(used.is_subset(&actions), "unknown actions: {:?}", used.difference(&actions).collect::<Vec<_>>());
    let used_tools = named(&content, "t");
    assert!(used_tools.is_subset(&tools), "unknown tools: {:?}", used_tools.difference(&tools).collect::<Vec<_>>());
    for v in named(&content, "v") {
        assert!(views["views"].get(&v).is_some(), "unknown view {v}");
    }
    for demo in named(&content, "demo") {
        assert!(demos.contains(&format!("D.{demo} = ")), "no demo {demo}");
    }
    // Every item it links to or ties to is one of its items.
    let items = named(&content, "id");
    let ties = content.lines().filter(|l| l.contains("ties: [")).flat_map(|l| l.split("[\"").skip(1).filter_map(|t| t.split('"').next().map(str::to_owned)).collect::<Vec<_>>());
    for link in named(&content, "i").into_iter().chain(named(&content, "item")).chain(ties) {
        assert!(items.contains(&link), "unknown item {link}");
    }
    // Complete: every cockpit action is explained somewhere.
    let unexplained: Vec<&String> = actions.difference(&used).collect();
    assert!(unexplained.is_empty(), "actions no item explains: {unexplained:?}");
}
