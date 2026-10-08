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
    let mut g = Graph::new();
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
    let input = Input { graph, plan, other_plans: &others, config: &config, types: &types, budget: 400, link_budget: 300, out_of_time: &|| false };
    arch_graph::tree::build(&input, &request)
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
    };
    g.details.insert((key.to_owned(), g.nav.plan.clone()), Detail::Ready(Box::new(detail)));
}

/// A graph waiting for its first tree (nothing in flight, nothing shown).
pub(crate) fn loading() -> GraphState {
    GraphState { config: Some(TreeConfig::defaults().unwrap()), config_requested: true, plans_stale: false, stale: false, load: Load::Loading, ..Default::default() }
}

/// A graph showing `tree`, settled where it asked (nothing to fetch).
pub(crate) fn showing(tree: Tree) -> GraphState {
    let r = &tree.request;
    let nav = Nav { view: r.view, plan: r.source.plan().cloned(), root: r.root.clone(), way: r.way, depth: r.depth, opened: r.opened.clone(), closed: r.closed.clone(), full_code: r.full_code };
    let mut g = GraphState { config: Some(TreeConfig::defaults().unwrap()), config_requested: true, plans_stale: false, stale: false, nav, ..Default::default() };
    g.set_tree(tree);
    g
}

fn ids(g: &GraphState) -> Vec<String> {
    g.actions().into_iter().map(|a| a.id).collect()
}

// req: M.8
#[test]
fn the_graph_offers_one_small_set_of_actions() {
    let mut g = showing(sample_tree());
    let mut all = ids(&g);
    all.sort();
    assert_eq!(all, ["graph.depth_less", "graph.depth_more", "graph.find", "graph.fit", "graph.flip", "graph.legend", "graph.next_view", "graph.plan", "graph.refresh", "graph.view", "graph.zoom_in", "graph.zoom_out"]);
    // A selection adds going into it (and closing its details).
    g.select(Some("rust:a::step".into()));
    assert!(ids(&g).contains(&"graph.go_in".to_owned()) && ids(&g).contains(&"graph.close_inspector".to_owned()));
    // A plan adds the full-code switch; the agent's move, the way back.
    g.nav.plan = Some(PlanId::new("p").unwrap());
    g.agent = Some(AgentMove { what: "x".into(), select: None, return_to: Nav::default(), return_selection: None });
    assert!(ids(&g).contains(&"graph.full_code".to_owned()) && ids(&g).contains(&"graph.agent_return".to_owned()));
    // The old ways of seeing more or less are gone.
    for gone in ["graph.focus", "graph.unfocus", "graph.level", "graph.coarser", "graph.finer", "graph.show_all", "graph.story", "graph.reached_by", "graph.follow", "graph.next_call", "graph.up"] {
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

// req: M.8
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

// req: M.8
#[test]
fn opening_and_closing_a_card_ask_the_server_and_nothing_moves_by_itself() {
    let mut g = showing(tree_of(&sample_graph(), None, TreeRequest { view: ViewKind::Calls, root: Some("rust:a::run".into()), depth: Some(1), ..TreeRequest::default() }));
    g.select(Some("rust:a::step".into()));
    assert!(g.selected_card().unwrap().more > 0);
    g.fold(Fold::Open);
    assert_eq!(g.nav.opened.len(), 1);
    assert!(g.stale);
    g.set_tree(tree_of(&sample_graph(), None, g.nav.request()));
    assert!(g.card("rust:a::save").is_some(), "its branch is in");
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

// req: M.9
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

// req: M.8, G.10
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

// req: M.2, M.8
#[test]
fn a_missing_plan_or_root_falls_back_and_says_so() {
    let mut g = showing(sample_tree());
    g.nav.plan = Some(PlanId::new("gone").unwrap());
    g.failed("not found: plan gone".into());
    assert_eq!(g.nav.plan, None, "the code instead");
    assert!(g.note.as_deref().is_some_and(|n| n.contains("Plan gone isn't there")));
    g.failed("not found: rust:a::run isn't in the calls view (code)".into());
    assert_eq!(g.nav.root, None, "the starting points instead");
    assert!(g.note.as_deref().is_some_and(|n| n.contains("isn't in this view")));
    g.failed("boom".into());
    assert_eq!(g.load, Load::Failed("boom".into()));
}

// req: M.10, P9.7
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

// req: M.8, G.12
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

// req: M.6
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

// req: P9.6, M.8
#[test]
fn fingers_select_go_in_offer_actions_pan_zoom_and_walk_back() {
    use crate::touch::{Gesture, Swipe};
    let mut g = showing(sample_tree());
    let at = |g: &GraphState, key: &str| g.to_screen(&g.card(key).unwrap().rect).center();
    let step = at(&g, "rust:a::step");
    g.on_gesture(Gesture::Tap(step));
    assert_eq!(g.selection.as_deref(), Some("rust:a::step"), "a tap selects");
    g.on_gesture(Gesture::LongPress(step));
    assert!(g.context.as_ref().is_some_and(|(k, _)| k == "rust:a::step"), "a long press offers its actions");
    g.on_gesture(Gesture::DoubleTap(step));
    assert_eq!(g.nav.root.as_deref(), Some("rust:a::step"), "a double tap goes in");
    let before = g.camera();
    g.on_gesture(Gesture::Pan(egui::vec2(30.0, 0.0)));
    g.on_gesture(Gesture::Pinch { factor: 1.5, center: egui::Pos2::new(100.0, 100.0) });
    assert!(g.camera() != before, "two fingers move the camera");
    g.on_gesture(Gesture::Swipe(Swipe::Right));
    assert_eq!(g.requests.run.as_ref().map(|(id, _)| id.as_str()), Some("nav.back"), "a swipe walks the jump list");
}
