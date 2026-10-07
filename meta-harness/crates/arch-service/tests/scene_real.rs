//! The scene engine on this repository's real graph: whatever level you're
//! at, nothing overlaps, parts sit inside their modules, text fits its
//! card, no line runs through a card, and lines cross each other rarely.

use std::collections::BTreeSet;
use std::path::Path;

use arch_graph::view::scene::{Measure, Metrics, Options, Scene, build, crossings, problems};
use arch_graph::{ComponentId, TypeSystem, ViewCatalog, ViewResult};
use arch_index::PackSet;
use arch_service::{ArchitectureService, MemoryIndex, MemoryPlans};

fn metrics() -> Metrics {
    Metrics { grid: 32.0, gap_x: 96.0, gap_y: 32.0, pad: 16.0, header: 32.0, column: 10, lane: 16.0, bend: 3.0, card_min_w: 96.0, card_max_w: 320.0, char_width: 0.6, line_height: 1.4 }
}

const NAME: f32 = 14.0;
const SMALL: f32 = 11.0;

/// Cards as the cockpit sizes them: the name, then the type (and for a shut
/// module, how much is inside).
struct Cards<'a>(&'a ViewResult);

impl Cards<'_> {
    fn lines(&self, id: &ComponentId, closed: bool) -> (String, String) {
        let c = &self.0.components[id];
        let members = self.0.view.node(id).map_or(0, |n| n.members);
        (c.name.clone(), if closed { format!("{} · {members}", c.type_name) } else { c.type_name.clone() })
    }
}

impl Measure for Cards<'_> {
    fn card(&self, id: &ComponentId, closed: bool) -> (f32, f32) {
        let (name, meta) = self.lines(id, closed);
        metrics().card(&[(&name, NAME), (&meta, SMALL)])
    }
    fn outside(&self, group: &ComponentId, reached: u32) -> (f32, f32) {
        let name = self.0.components.get(group).map_or_else(|| group.to_string(), |c| c.name.clone());
        metrics().card(&[(&format!("{reached} more in {name}"), SMALL)])
    }
    fn more(&self, hidden: usize) -> (f32, f32) {
        metrics().card(&[(&format!("+{hidden} more"), SMALL)])
    }
}

/// With NAH_SCENE_SVG set to a directory, each checked scene is drawn there
/// (to look at what the numbers say).
fn draw(what: &str, result: &ViewResult, scene: &Scene) {
    let Ok(dir) = std::env::var("NAH_SCENE_SVG") else { return };
    let Some(b) = scene.bounds() else { return };
    let mut svg = format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='{} {} {} {}' width='{}' height='{}' style='background:#080a0f;font-family:sans-serif'>", b.x - 40.0, b.y - 40.0, b.w + 80.0, b.h + 80.0, b.w + 80.0, b.h + 80.0);
    for c in &scene.containers {
        let r = scene.rects[c];
        svg += &format!("<rect x='{}' y='{}' width='{}' height='{}' rx='6' fill='#11151d' stroke='#2a3240'/><text x='{}' y='{}' fill='#8892a0' font-size='13'>{}</text>", r.x, r.y, r.w, r.h, r.x + 10.0, r.y + 20.0, result.components.get(c).map_or("?", |x| &x.name));
    }
    for line in &scene.lines {
        let pts: Vec<String> = line.points.iter().map(|(x, y)| format!("{x},{y}")).collect();
        svg += &format!("<polyline points='{}' fill='none' stroke='#3ee6ff' stroke-opacity='0.55' stroke-width='{}'/>", pts.join(" "), 1.0 + (line.count as f32).log2().min(3.0));
    }
    for (id, r) in scene.cards() {
        svg += &format!("<rect x='{}' y='{}' width='{}' height='{}' rx='6' fill='#151a24' stroke='#3a4455'/><text x='{}' y='{}' fill='#e6edf3' font-size='14'>{}</text>", r.x, r.y, r.w, r.h, r.x + 16.0, r.y + 24.0, result.components.get(id).map_or("?", |x| &x.name));
    }
    for (g, (r, n)) in &scene.outside {
        svg += &format!("<rect x='{}' y='{}' width='{}' height='{}' rx='6' fill='#1a1530' stroke='#7a5cff'/><text x='{}' y='{}' fill='#c8b8ff' font-size='11'>{n} more in {}</text>", r.x, r.y, r.w, r.h, r.x + 16.0, r.y + 20.0, result.components.get(g).map_or("?", |x| &x.name));
    }
    svg += "</svg>";
    let name: String = what.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    std::fs::write(Path::new(&dir).join(format!("{name}.svg")), svg).unwrap();
}

/// The rules, and text that fits its card (or is cut at the widest card).
fn check(what: &str, result: &ViewResult, scene: &Scene) {
    draw(what, result, scene);
    let found = problems(scene);
    assert!(found.is_empty(), "{what}: {found:#?}");
    let m = metrics();
    let cards = Cards(result);
    for (id, r) in scene.cards() {
        let closed = scene.parent.values().any(|p| p == id);
        let (name, meta) = cards.lines(id, closed);
        for (text, size) in [(name, NAME), (meta, SMALL)] {
            let fits = m.text_width(&text, size) <= r.w - m.pad * 2.0 + 0.5;
            assert!(fits || r.w >= m.card_max_w, "{what}: `{text}` fits its card ({} wide)", r.w);
        }
    }
    let lines = scene.lines.len();
    let crossed = crossings(&scene.lines);
    assert!(crossed <= lines * 2 + 10, "{what}: {crossed} crossings for {lines} lines");
}

fn scene(result: &ViewResult, open: &BTreeSet<ComponentId>) -> Scene {
    build(&result.view, &Options { open, selection: None, max_cards: 60, show_all: false }, &Cards(result), &metrics(), None)
}

// req: G.9, G.7
#[test]
fn every_level_of_this_repository_lays_out_cleanly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let service = ArchitectureService::open(
        &root,
        PackSet::built_in().unwrap(),
        TypeSystem::defaults().unwrap(),
        ViewCatalog::defaults().unwrap(),
        Box::new(MemoryPlans::default()),
        Box::new(MemoryIndex::default()),
    )
    .unwrap();
    service.ingest(&root).unwrap();
    let none = BTreeSet::new();

    // The top level: every part shut, lines between them counted.
    let top = service.named_view("structure", None, vec![], None).unwrap();
    let s = scene(&top, &none);
    check("top level", &top, &s);
    assert!(s.lines.iter().any(|l| l.count > 1), "relations between parts merge");

    // The biggest part, as its own level.
    let biggest = top.view.nodes.iter().filter(|n| n.depth == Some(0)).max_by_key(|n| n.members).unwrap().id.clone();
    let level = service.named_view("structure", None, vec![biggest.clone()], None).unwrap();
    let s = scene(&level, &none);
    check(&format!("inside {biggest}"), &level, &s);
    assert_eq!(s.frame.as_ref(), Some(&biggest));
    assert!(!s.outside.is_empty(), "it reaches outside itself");

    // Its modules opened in place, a level deeper.
    let deeper = service.named_view("structure", None, vec![biggest.clone()], Some(2)).unwrap();
    let modules: BTreeSet<ComponentId> = deeper.view.edges.iter().filter(|e| e.kind == arch_graph::RelationKind::Contains && e.source != biggest).map(|e| e.source.clone()).take(4).collect();
    assert!(!modules.is_empty());
    let s = scene(&deeper, &modules);
    check(&format!("inside {biggest}, modules open"), &deeper, &s);

    // Calls, left to right.
    let entry = ComponentId::for_symbol("rust", "arch_service::service::ArchitectureService::validate");
    let calls = service.named_view("calls", None, vec![entry.clone()], Some(3)).unwrap();
    let s = scene(&calls, &none);
    check("calls from validate", &calls, &s);
    let x = |id: &ComponentId| s.rects[id].x;
    for e in calls.view.edges.iter().filter(|e| e.source == entry) {
        assert!(x(&e.target) > x(&entry), "{} is right of what calls it", e.target);
    }
}
