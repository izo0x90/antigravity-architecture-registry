//! Where the views start with no root picked: Calls from the entry points (a main, a route's handler), Data
//! flow from the types those take; not every function nothing calls.

use arch_graph::TypeSystem;
use arch_graph::tree::{TreeRequest, ViewKind};
use arch_service::{ArchitectureService, FsPlans};

mod common;

const APP: &str = r#"pub struct Filter { pub name: String }

fn main() {
    serve();
}

fn serve() {
    let app = Router::new().route("/api/items", get(list_items)).route("/api/page", get(page_of));
}

fn list_items(filter: Filter) -> usize {
    filter.name.len()
}

pub struct Query<T>(pub T);

pub struct Page {
    pub at: u32,
}

fn page_of(q: Query<Page>) -> u32 {
    q.0.at
}

fn tidy() {
    helper();
    emit(Event::Saved);
}

fn helper() {}

pub enum Event {
    Saved,
}

fn emit(_e: Event) {}

// An event handled inside: not a way in.
fn on_event(e: Event) {
    match e {
        Event::Saved => tidy(),
    }
}

// The tools an MCP server lists, and the call that takes each.
fn tools() -> Vec<&'static str> {
    vec![tool("search"), tool("inspect")]
}

fn tool(name: &'static str) -> &'static str {
    name
}

fn call(name: &str) {
    match name {
        "search" => helper(),
        "inspect" => {}
        _ => {}
    }
}

// A match on a string no tool declares: not a way in.
fn pick(view: &str) {
    match view {
        "calls" => helper(),
        _ => {}
    }
}

// A match that takes one tool's name among other keys (a query's fields):
// it reads keys, it doesn't take tools.
fn field(key: &str) {
    match key {
        "search" => helper(),
        "depth" => {}
        _ => {}
    }
}
"#;

#[test]
fn calls_start_at_the_entry_points_and_data_flow_at_what_they_take() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("app");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::write(root.join("src/main.rs"), APP).unwrap();
    let s = ArchitectureService::open(&root, common::shipped_packs(), TypeSystem::defaults().unwrap(), Box::new(FsPlans::new(&root).unwrap()), arch_service::open_facts(&root, &arch_service::Limits::defaults().unwrap()).unwrap()).unwrap();
    s.ingest(&root).unwrap();
    let calls = s.tree(&TreeRequest { view: ViewKind::Calls, depth: Some(0), ..TreeRequest::default() }).unwrap();
    assert_eq!(calls.roots, ["rust:app::main::main", "rust:app::main::list_items", "rust:app::main::page_of", "rust:app::main::call"], "the main, the route's handler, the call that takes the MCP tools; not `tidy` (it calls something), an event's handler a match on a string no tool declares, or one that takes a tool's name among keys of its own");
    let data = s.tree(&TreeRequest { view: ViewKind::DataFlow, depth: Some(0), ..TreeRequest::default() }).unwrap();
    assert_eq!(data.roots, ["type:Filter", "type:Page"], "what the handlers take, out of its wrapper (`Query<Page>`), the wrapper itself not");
    // Contains starts at the repo: what nothing holds (here its one module).
    let contains = s.tree(&TreeRequest { view: ViewKind::Contains, depth: Some(0), ..TreeRequest::default() }).unwrap();
    assert_eq!(contains.roots, ["rust:app::main"]);
}
