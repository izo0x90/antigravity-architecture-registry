use std::collections::BTreeMap;

use arch_graph::tree::{Input, Mark, Tree, TreeConfig, TreeRequest, ViewKind, Way};
use arch_graph::{CodeAnchor, Component, ComponentId, Graph, Plan, PlanId, Provenance, Relation, RelationKind, RepoPath, Span, Stage, Status, TypeSystem};

use super::*;
use crate::actions::ActionProvider;
use crate::nav::{Fold, Navigable, Step};

fn component(symbol: &str, ty: &str) -> Component {
    Component {
        id: ComponentId::new(format!("rust:{symbol}")),
        name: symbol.rsplit("::").next().unwrap().into(),
        type_name: ty.into(),
        description: String::new(),
        status: Status::Existing,
        stage: Stage::Implemented,
        anchor: Some(CodeAnchor { language: "rust".into(), file: RepoPath::new("src/a.rs").unwrap(), symbol: symbol.into(), syntax_kind: "function_item".into(), span: Span { start_line: 1, end_line: 3, start_byte: 0, end_byte: 9 }, content_hash: String::new() }),
        proposed: None,
        signature: None,
        schemas: Default::default(),
        spec: None,
        placeholders: vec![],
    }
}

/// a { run, step, save, log }, b { util }: run -> step -> save, run -> log.
pub(crate) fn sample_graph() -> Graph {
    let mut g = Graph::default();
    for (s, ty) in [("a", "module"), ("a::run", "function"), ("a::step", "function"), ("a::save", "function"), ("a::log", "function"), ("b", "module"), ("b::util", "function")] {
        g.upsert_component(component(s, ty), Provenance::OBSERVED);
    }
    let id = |s: &str| ComponentId::new(format!("rust:{s}"));
    for (a, b) in [("a", "a::run"), ("a", "a::step"), ("a", "a::save"), ("a", "a::log"), ("b", "b::util")] {
        g.upsert_relation(Relation::new(RelationKind::Contains, id(a), id(b), Provenance::OBSERVED));
    }
    for (i, (a, b)) in [("a::run", "a::step"), ("a::step", "a::save"), ("a::run", "a::log")].into_iter().enumerate() {
        let mut r = Relation::new(RelationKind::Calls, id(a), id(b), Provenance::OBSERVED);
        r.order = Some(i as u32);
        g.upsert_relation(r);
    }
    g
}

/// The tree the server would send for `request` over `graph`.
pub(crate) fn tree_of(graph: &Graph, plan: Option<&Plan>, request: TreeRequest) -> Tree {
    let (config, types, others) = (TreeConfig::defaults().unwrap(), TypeSystem::defaults().unwrap(), BTreeMap::new());
    let input = Input { graph, plan, other_plans: &others, config: &config, types: &types, budget: 400, link_budget: 300, flows: None, out_of_time: &|| false, layout_out_of_time: &|| false, entries: &entries() };
    arch_graph::tree::build(&input, &request)
}

/// What the server answers when `card` is opened one level in `held`,
/// keeping to `cap` cards.
pub(crate) fn change_of(graph: &Graph, held: &Tree, card: &str, cap: usize) -> arch_graph::tree::Expanded {
    let (config, types, others) = (TreeConfig::defaults().unwrap(), TypeSystem::defaults().unwrap(), BTreeMap::new());
    let input = Input { graph, plan: None, other_plans: &others, config: &config, types: &types, budget: 400, link_budget: 300, flows: None, out_of_time: &|| false, layout_out_of_time: &|| false, entries: &entries() };
    arch_graph::tree::expand(&input, held, card, 1, cap).1
}

/// The sample's entry point: `run` is its main.
fn entries() -> Vec<arch_graph::tree::EntryPoint> {
    vec![arch_graph::tree::EntryPoint { id: ComponentId::new("rust:a::run"), why: "main".into() }]
}

/// Calls down from run, three levels.
pub(crate) fn sample_tree() -> Tree {
    tree_of(&sample_graph(), None, TreeRequest { view: ViewKind::Calls, root: Some("rust:a::run".into()), depth: Some(3), ..TreeRequest::default() })
}

/// Contains from the top, two levels.
pub(crate) fn boxes_tree() -> Tree {
    tree_of(&sample_graph(), None, TreeRequest { view: ViewKind::Contains, depth: Some(2), ..TreeRequest::default() })
}

/// Gives a card its details as if fetched: its component from the sample,
/// placed in `file` at `start..end`, with its relations.
pub(crate) fn with_detail(g: &mut GraphState, key: &str, file: &str, start: u32, end: u32) {
    let graph = sample_graph();
    let id = ComponentId::new(key);
    let mut c = graph.component(&id).unwrap().clone();
    c.anchor = Some(CodeAnchor { language: "rust".into(), file: RepoPath::new(file).unwrap(), symbol: key.trim_start_matches("rust:").into(), syntax_kind: "function_item".into(), span: Span { start_line: start, end_line: end, start_byte: 0, end_byte: 1 }, content_hash: String::new() });
    let detail = arch_graph::ComponentDetail {
        alignment: arch_graph::Alignment::Observed,
        outgoing: graph.outgoing(&id, None).cloned().collect(),
        incoming: graph.incoming(&id, None).cloned().collect(),
        component: c,
        code: None,
        stale: false,
            references: Vec::new(),
    };
    g.details.insert((key.to_owned(), g.nav.source.clone()), Detail::Ready(Box::new(detail)));
}

/// A graph waiting for its first tree (nothing in flight, nothing shown).
pub(crate) fn loading() -> GraphState {
    GraphState { config: Some(TreeConfig::defaults().unwrap()), config_requested: true, plans_stale: false, stale: false, load: Load::Loading, ..Default::default() }
}

/// A graph showing `tree`, settled where it asked (nothing to fetch).
pub(crate) fn showing(tree: Tree) -> GraphState {
    let r = &tree.request;
    let nav = Nav { view: r.view, source: r.source.clone(), root: r.root.clone(), way: r.way, depth: r.depth, opened: r.opened.clone(), closed: r.closed.clone(), full_code: r.full_code, only_changed: r.only.is_some() };
    let mut g = GraphState { config: Some(TreeConfig::defaults().unwrap()), config_requested: true, plans_stale: false, stale: false, nav, ..Default::default() };
    g.set_tree(tree);
    g
}

fn ids(g: &GraphState) -> Vec<String> {
    g.actions().into_iter().map(|a| a.id).collect()
}

#[test]
fn the_graph_offers_one_small_set_of_actions() {
    let mut g = showing(sample_tree());
    let mut all = ids(&g);
    all.sort();
    // Depth, flip, switch view, pick source; finding where to start; the camera;
    // back along the path; the legend.
    assert_eq!(all, ["graph.depth_less", "graph.depth_more", "graph.find", "graph.fit", "graph.flip", "graph.legend", "graph.path", "graph.plan", "graph.view", "graph.zoom_in", "graph.zoom_out"]);
    // A selection adds going into it.
    g.select(Some("rust:a::step".into()));
    assert!(ids(&g).contains(&"graph.go_in".to_owned()));
    // A plan adds the full-code switch; the agent's move, the way back.
    g.nav.source = Source::Plan(PlanId::new("p").unwrap());
    g.agent = Some(AgentMove { what: "x".into(), select: None, return_to: Nav::default(), return_selection: None });
    assert!(ids(&g).contains(&"graph.full_code".to_owned()) && ids(&g).contains(&"graph.agent_return".to_owned()));
    // The old ways of seeing more or less are gone.
    for gone in ["graph.focus", "graph.unfocus", "graph.level", "graph.coarser", "graph.finer", "graph.show_all", "graph.story", "graph.reached_by", "graph.follow", "graph.next_call", "graph.up", "graph.next_view", "graph.refresh", "graph.close_inspector"] {
        assert!(!ids(&g).contains(&gone.to_owned()), "{gone}");
    }
    // The keys for them are gone from the keymap too.
    let keymap = include_str!("../../config/default.toml");
    for gone in ["\"graph.focus\"", "\"graph.level\"", "\"graph.coarser\"", "\"graph.next_call\""] {
        assert!(!keymap.contains(gone), "{gone} still bound");
    }
    for bound in ["\"graph.flip\"", "\"graph.full_code\"", "\"graph.go_in\""] {
        assert!(keymap.contains(bound), "{bound} bound");
    }
}

#[test]
fn going_in_and_back_walk_a_trail_of_roots() {
    let mut g = showing(sample_tree());
    g.select(Some("rust:a::step".into()));
    assert_eq!(g.enter(), Step::Done);
    assert_eq!(g.nav.root.as_deref(), Some("rust:a::step"));
    assert!(g.stale, "the new root is fetched");
    assert_eq!(g.trail_roots(), vec![Some("rust:a::run".to_owned())]);
    g.set_tree(tree_of(&sample_graph(), None, g.nav.request()));
    assert_eq!(g.selection.as_deref(), Some("rust:a::step"), "what was gone into is selected");
    assert_eq!(g.leave(), Step::Done);
    assert_eq!(g.nav.root.as_deref(), Some("rust:a::run"), "back to where going in started");
    g.set_tree(tree_of(&sample_graph(), None, g.nav.request()));
    assert_eq!(g.selection.as_deref(), Some("rust:a::step"), "with what was selected then");
    // No trail: out to what holds the root (Contains), else the starting points.
    let mut boxes = showing(tree_of(&sample_graph(), None, TreeRequest { view: ViewKind::Contains, root: Some("rust:a".into()), depth: Some(1), ..TreeRequest::default() }));
    assert!(boxes.leave() == Step::Done && boxes.nav.root.is_none());
    assert!(matches!(boxes.leave(), Step::Nothing(_)), "already at the top");
    // The trail's crumbs go back to any step.
    let mut g = showing(sample_tree());
    for card in ["rust:a::step", "rust:a::save"] {
        g.navigate(NavChange::GoIn(card.into()));
    }
    g.back_to(0);
    assert_eq!((g.nav.root.as_deref(), g.trail_roots().len()), (Some("rust:a::run"), 0));
}

#[test]
fn opening_and_closing_a_card_ask_the_server_and_nothing_moves_by_itself() {
    let mut g = showing(tree_of(&sample_graph(), None, TreeRequest { view: ViewKind::Calls, root: Some("rust:a::run".into()), depth: Some(1), ..TreeRequest::default() }));
    g.select(Some("rust:a::step".into()));
    assert!(g.selected_card().unwrap().more > 0);
    let before = g.tree.as_ref().unwrap().cards.len();
    g.fold(Fold::Open);
    assert_eq!(g.nav.opened.len(), 1);
    // Opening asks for its branch alone (against the tree held), not the
    // whole tree again.
    let held = g.tree.clone().unwrap();
    assert!(!g.stale && g.expanding.as_ref().is_some_and(|(card, request)| card == "rust:a::step" && *request == held.request));
    g.take_change(change_of(&sample_graph(), &held, "rust:a::step", 1000));
    assert!(g.card("rust:a::save").is_some(), "its branch is in");
    assert!(g.tree.as_ref().unwrap().cards.len() > before && g.card("rust:a::step").unwrap().open);
    g.fold(Fold::Close);
    assert_eq!((g.nav.opened.len(), g.nav.closed.len()), (0, 1));
    g.set_tree(tree_of(&sample_graph(), None, g.nav.request()));
    g.fit = false;
    // A card with nothing more says so instead of fetching.
    g.stale = false;
    g.select(Some("rust:a::log".into()));
    g.fold(Fold::Open);
    assert!(!g.stale && g.note.as_deref().is_some_and(|n| n.contains("nothing more")));
    // Zooming only zooms; a code change refetches the same tree and keeps the camera.
    let before = g.nav.clone();
    g.zoom_by(1.5, egui::Pos2::new(100.0, 100.0));
    assert_eq!(g.nav, before);
    let camera = g.camera();
    g.on_event(&arch_graph::ArchEvent::Changed { reason: arch_graph::ChangeReason::Ingested, plans: vec![], components: vec![] });
    assert!(g.stale && !g.refit, "refetched in place");
    g.set_tree(tree_of(&sample_graph(), None, g.nav.request()));
    assert!(!g.fit && g.camera() == camera, "the camera stays where the user put it");
    // Depth and flip are the user's.
    g.command(actions::GraphCommand::Depth(1), None);
    assert_eq!(g.nav.depth, Some(2));
    g.command(actions::GraphCommand::Flip, None);
    assert_eq!(g.nav.way, Way::Up);
}

#[test]
fn the_cockpit_holds_one_tree_and_forgets_what_the_server_closed() {
    let mut g = showing(sample_tree());
    let first: Vec<String> = g.tree.as_ref().unwrap().cards.iter().map(|c| c.key.clone()).collect();
    g.navigate(NavChange::GoIn("rust:a::save".into()));
    g.set_tree(tree_of(&sample_graph(), None, g.nav.request()));
    let now: Vec<String> = g.tree.as_ref().unwrap().cards.iter().map(|c| c.key.clone()).collect();
    assert_eq!(now, vec!["rust:a::save".to_owned()], "only the tree on screen, not everything seen before ({first:?})");
    // A branch the server couldn't fit is closed here too, and said.
    g.nav.opened.push(arch_graph::tree::Opened { card: "rust:a::x".into(), levels: 1 });
    let mut tree = sample_tree();
    tree.dropped = vec!["rust:a::x".into()];
    g.set_tree(tree);
    assert!(g.nav.opened.is_empty());
    assert!(g.note.as_deref().is_some_and(|n| n.contains("Closed")));
    // Off screen isn't drawn.
    g.canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    g.camera = Camera { pan: egui::vec2(0.0, 0.0), zoom: 1.0 };
    assert!(!g.on_screen().is_empty());
    g.camera = Camera { pan: egui::vec2(-50_000.0, -50_000.0), zoom: 1.0 };
    assert!(g.on_screen().is_empty(), "nothing on screen, nothing drawn");
}

#[test]
fn selecting_lights_up_what_its_tied_to() {
    let mut g = showing(sample_tree());
    assert!(g.related().is_none(), "nothing selected: nothing dims");
    g.select(Some("rust:a::step".into()));
    let lit = g.related().unwrap();
    assert!(lit.contains("rust:a::run") && lit.contains("rust:a::save") && lit.contains("rust:a::step"));
    assert!(!lit.contains("rust:a::log"), "log isn't tied to step");
    assert!(g.touches_selection("rust:a::run", "rust:a::step") && !g.touches_selection("rust:a::run", "rust:a::log"));
    // In boxes: what holds it and what's inside it.
    let mut b = showing(boxes_tree());
    b.select(Some("rust:a".into()));
    let lit = b.related().unwrap();
    assert!(lit.contains("rust:a::run") && !lit.contains("rust:b::util"));
}

#[test]
fn a_missing_plan_or_root_falls_back_and_says_so() {
    let mut g = showing(sample_tree());
    let (gone, kept) = (PlanId::new("gone").unwrap(), PlanId::new("kept").unwrap());
    // Under a plan, a root that isn't there: the plan stays, the root goes.
    g.nav.source = Source::Plan(kept.clone());
    g.failed(missing("rust:a::run isn't in the calls view (plan kept)"));
    assert_eq!((g.nav.source.plan(), g.nav.root.as_deref()), (Some(&kept), None), "the plan's starting points instead");
    assert!(g.note.as_deref().is_some_and(|n| n.contains("isn't in this view") && !n.contains("Plan")));
    // Not found with no root: the plans are asked; one that's still there stays.
    g.note = None;
    g.failed(missing("something"));
    g.plans_arrived(vec![(kept.clone(), arch_graph::PlanKind::Change)]);
    assert_eq!(g.nav.source.plan(), Some(&kept), "a plan that's there isn't called gone");
    assert!(g.note.is_none());
    // A plan that's gone: the code instead, and it says so.
    g.nav.source = Source::Plan(gone.clone());
    g.failed(missing("plan gone"));
    assert_eq!(g.nav.source.plan(), Some(&gone), "not before the plans say so");
    g.plans_arrived(vec![(kept, arch_graph::PlanKind::Change)]);
    assert_eq!(g.nav.source, Source::Code, "the code instead");
    assert!(g.note.as_deref().is_some_and(|n| n.contains("Plan gone isn't there")));
    // Any other failure says so; words in it don't make it "not found".
    g.failed(crate::net::NetError { status: Some(500), message: "the file wasn't found on disk".into() });
    assert_eq!(g.load, Load::Failed("the file wasn't found on disk".into()));
    assert_eq!(g.nav.source, Source::Code);
}

/// The server's 404, with its message.
fn missing(message: &str) -> crate::net::NetError {
    crate::net::NetError { status: Some(404), message: message.into() }
}

#[test]
fn the_agent_moves_the_view_and_the_user_can_return() {
    let mut g = showing(sample_tree());
    g.select(Some("rust:a::log".into()));
    let request = arch_graph::ViewRequest { view: Some(ViewKind::Contains), root: Some("rust:a".into()), select: Some("rust:a::save".into()), ..Default::default() };
    g.on_event(&arch_graph::ArchEvent::ViewRequested { request: Box::new(request), by: arch_graph::Requester::Agent });
    assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::Contains, Some("rust:a")));
    let agent = g.agent.clone().unwrap();
    assert!(agent.what.contains("contains view") && agent.select.as_deref() == Some("rust:a::save"));
    g.set_tree(tree_of(&sample_graph(), None, g.nav.request()));
    assert_eq!(g.selection.as_deref(), Some("rust:a::save"), "what the agent pointed at");
    g.command(actions::GraphCommand::AgentReturn, None);
    assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::Calls, Some("rust:a::run")));
    g.set_tree(sample_tree());
    assert_eq!(g.selection.as_deref(), Some("rust:a::log"), "back where the user was");
}

#[test]
fn switching_views_keeps_what_the_new_view_can_show() {
    let mut g = showing(sample_tree());
    g.select(Some("rust:a::step".into()));
    assert_eq!(g.root_for(ViewKind::Contains).as_deref(), Some("rust:a::step"));
    assert_eq!(g.root_for(ViewKind::DataFlow), None, "a function isn't data: data flow starts afresh");
    g.command(actions::GraphCommand::SetView, Some("contains".into()));
    assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::Contains, Some("rust:a::step")), "the selection comes along");
    // Revealing something not on screen goes into it; data flow shows its calls.
    let mut d = showing(sample_tree());
    d.nav.view = ViewKind::DataFlow;
    d.reveal(ComponentId::new("rust:b::util"));
    assert_eq!((d.nav.view, d.nav.root.as_deref()), (ViewKind::Calls, Some("rust:b::util")));
    // Going into something that holds nothing shows it inside what holds it.
    let mut c = showing(boxes_tree());
    c.navigate(NavChange::GoIn("rust:a::log".into()));
    c.set_tree(tree_of(&sample_graph(), None, c.nav.request()));
    assert_eq!(c.nav.root.as_deref(), Some("rust:a"), "inside its box");
    c.set_tree(tree_of(&sample_graph(), None, c.nav.request()));
    assert_eq!(c.selection.as_deref(), Some("rust:a::log"), "selected there");
}

#[test]
fn plan_marks_show_on_cards() {
    let mut plan = Plan::new("p", "P");
    let mut new = component("a::audit", "function");
    new.status = Status::New;
    new.stage = Stage::Declared;
    new.anchor = None;
    plan.components.push(new);
    plan.relations.push(Relation::new(RelationKind::Calls, ComponentId::new("rust:a::run"), ComponentId::new("rust:a::audit"), Provenance::PLANNED));
    let graph = Graph::merge(sample_graph(), std::slice::from_ref(&plan));
    let tree = tree_of(&graph, Some(&plan), TreeRequest { view: ViewKind::Calls, source: arch_graph::Source::Plan(plan.id.clone()), root: Some("rust:a::run".into()), depth: Some(1), full_code: true, ..TreeRequest::default() });
    let g = showing(tree);
    assert_eq!(g.card("rust:a::audit").unwrap().mark, Mark::New);
    assert!(g.actions().iter().any(|a| a.id == "graph.full_code"));
}

#[test]
fn fingers_select_go_in_offer_actions_pan_zoom_and_walk_back() {
    use crate::touch::Gesture;
    let mut g = showing(sample_tree());
    let at = |g: &GraphState, key: &str| g.to_screen(&g.card(key).unwrap().rect).center();
    let step = at(&g, "rust:a::step");
    g.on_gesture(Gesture::Tap(step));
    assert_eq!(g.selection.as_deref(), Some("rust:a::step"), "a tap selects");
    g.on_gesture(Gesture::LongPress(step));
    assert_eq!(g.selection.as_deref(), Some("rust:a::step"), "a long press selects (its controls show under it)");
    g.on_gesture(Gesture::DoubleTap(step));
    assert_eq!(g.nav.root.as_deref(), Some("rust:a::step"), "a double tap goes in");
    let before = g.camera();
    g.on_gesture(Gesture::Pan(egui::vec2(30.0, 0.0)));
    g.on_gesture(Gesture::Pinch { factor: 1.5, center: egui::Pos2::new(100.0, 100.0) });
    assert!(g.camera() != before, "two fingers move the camera");

}

#[test]
fn the_graph_is_split_as_agreed() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/graph");
    let mut files: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    // What was asked, the cards held, drawing, the selection, the details
    // card, the Code/plan picker, the actions; the state that ties them, and
    // the tests.
    assert_eq!(files, ["actions.rs", "cards.rs", "details.rs", "draw.rs", "mod.rs", "request.rs", "select.rs", "source.rs", "tests.rs"]);
    // Each job in its file.
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();
    for (file, jobs) in [
        ("request.rs", &["pub fn apply(", "pub fn navigate(", "pub fn back(", "pub fn back_to(", "struct Step"][..]),
        ("cards.rs", &["pub fn take(", "fn take_change("][..]),
        ("draw.rs", &["fn draw_pills(", "fn visible("][..]),
        ("select.rs", &["pub fn select(", "pub fn reveal(", "fn related(", "pub fn move_selection("][..]),
        ("details.rs", &["fn render_card_context(", "fn want_details("][..]),
        ("source.rs", &["fn source_choices", "fn switch_full_code"][..]),
        ("actions.rs", &["impl ActionProvider for GraphState", "fn command("][..]),
    ] {
        let text = read(file);
        for job in jobs {
            assert!(text.contains(job), "{job} lives in {file}");
        }
    }
    // Answers are asked for and taken in one place (the tick), not by the
    // actions, the selection or the drawing.
    for file in ["actions.rs", "select.rs", "draw.rs", "request.rs", "cards.rs", "source.rs"] {
        assert!(!read(file).contains("net."), "{file} asks the server for nothing");
    }
}

#[test]
fn a_tree_too_big_to_fit_opens_on_its_root() {
    // A root that calls many: the tidy tree centres it on them, far below the top.
    let mut graph = sample_graph();
    let id = |s: &str| ComponentId::new(format!("rust:{s}"));
    for i in 0..40 {
        let name = format!("a::f{i}");
        graph.upsert_component(component(&name, "function"), Provenance::OBSERVED);
        let mut r = Relation::new(RelationKind::Calls, id("a::run"), id(&name), Provenance::OBSERVED);
        r.order = Some(10 + i);
        graph.upsert_relation(r);
    }
    let mut g = showing(tree_of(&graph, None, TreeRequest { view: ViewKind::Calls, root: Some("rust:a::run".into()), depth: Some(1), ..TreeRequest::default() }));
    g.canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    g.fit_view();
    let root = g.to_screen(&g.card("rust:a::run").unwrap().rect);
    assert!(g.canvas.contains_rect(root), "the root is on screen: {root:?}");
}

#[test]
fn a_card_text_is_measured_once_until_the_zoom_changes() {
    let mut fitted = super::draw::Fitted::default();
    let measured = std::cell::Cell::new(0);
    let fit = |t: &str, _: f32| {
        measured.set(measured.get() + 1);
        Some(t.to_owned())
    };
    for _frame in 0..3 {
        fitted.get("run", 12.0, 80.0, 1.0, fit);
        fitted.get("step", 12.0, 80.0, 1.0, fit);
    }
    assert_eq!(measured.get(), 2, "each text once, however many frames");
    fitted.get("run", 18.0, 120.0, 1.5, fit);
    assert_eq!(measured.get(), 3, "zoomed: measured again");
}

#[test]
fn walkthroughs_are_picked_like_plans_and_named_as_walkthroughs() {
    let mut g = showing(sample_tree());
    let (plan, walk) = (PlanId::new("add-cache").unwrap(), PlanId::new("how-run-works").unwrap());
    g.plans_arrived(vec![(walk.clone(), arch_graph::PlanKind::Walkthrough), (plan.clone(), arch_graph::PlanKind::Change)]);
    let labels: Vec<String> = g.source_choices().iter().map(|c| c.label.clone()).collect();
    assert_eq!(labels, ["code", "add-cache", "walkthrough how-run-works"], "plans, then walkthroughs");
    g.nav.source = Source::Plan(walk);
    assert_eq!(g.source_label(), "walkthrough how-run-works");
    g.nav.source = Source::Plan(plan);
    assert_eq!(g.source_label(), "plan add-cache");
}

#[test]
fn the_camera_zooms_and_fits_from_keys_and_buttons() {
    let mut g = showing(sample_tree());
    let has = |g: &GraphState, id: &str| g.actions().iter().any(|a| a.id == id);
    assert!(has(&g, "graph.fit") && has(&g, "graph.zoom_in") && has(&g, "graph.zoom_out"));
    let before = g.camera.zoom;
    g.command(actions::GraphCommand::Zoom(g.ui.zoom_step), None);
    assert!(g.camera.zoom > before, "zoomed in");
    g.command(actions::GraphCommand::Zoom(1.0 / g.ui.zoom_step), None);
    assert!((g.camera.zoom - before).abs() < 1e-4, "and back out");
    g.command(actions::GraphCommand::Zoom(g.ui.zoom_step * 4.0), None);
    g.command(actions::GraphCommand::Fit, None);
    assert!(g.camera.zoom <= 1.0, "fit brings the drawing back to the screen");
    let keymap = include_str!("../../config/default.toml");
    for bound in ["\"graph.zoom_in\" = { leader = \"g =\", keys = [\"z +\"] }", "\"graph.fit\" = { leader = \"g z\", keys = [\"z z\"] }"] {
        assert!(keymap.contains(bound), "{bound}");
    }
}

#[test]
fn changed_components_get_a_badge_and_only_they_show_when_asked() {
    let mut g = showing(sample_tree());
    assert!(!ids(&g).contains(&"graph.only_changed".to_owned()), "nothing changed: no switch");
    let comparison = harness_contracts::changes::Comparison::default();
    let step = ComponentId::new("rust:a::step");
    let changed = |id: &ComponentId, direct: bool| harness_contracts::changes::ComponentChanges { component: id.as_str().into(), file: "src/a.rs".into(), added: 2, removed: 1, runs: vec![(2, 2)], direct };
    let module = ComponentId::new("rust:a");
    g.set_changed(BTreeMap::from([(step.clone(), changed(&step, true)), (module.clone(), changed(&module, false))]), &comparison, 1);
    assert_eq!((g.changed(&step), g.changed_read(), g.stale), (Some((2, 1)), 1, false), "a badge; the tree as it was");
    assert!(ids(&g).contains(&"graph.only_changed".to_owned()));
    g.command(super::actions::GraphCommand::OnlyChanged, None);
    assert!(g.nav.only_changed && g.stale, "only changed: asked again");
    assert_eq!(g.changed(&module), Some((2, 1)), "what holds it: a badge too");
    assert_eq!(g.nav.asking(g.changed_directly()).only, Some(vec!["rust:a::step".to_owned()]), "only what changed itself (what holds it comes along)");
    g.stale = false;
    g.set_changed(BTreeMap::new(), &comparison, 2);
    assert!(g.stale, "what changed moved: asked again");
    assert_eq!(g.nav.asking(g.changed_directly()).only, Some(Vec::new()), "nothing changed: nothing shown");
    g.command(super::actions::GraphCommand::OnlyChanged, Some("false".into()));
    assert_eq!(g.nav.asking(g.changed_directly()).only, None);
}
