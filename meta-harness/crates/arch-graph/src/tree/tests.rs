use std::collections::BTreeMap;

use super::*;
use crate::component::tests::{anchor_for, planned};
use crate::{Graph, Param, Provenance, Relation, Signature, Stage, Status, TypeSystem};

fn id(s: &str) -> ComponentId {
    ComponentId::new(format!("rust:{s}"))
}

fn code(symbol: &str, ty: &str) -> crate::Component {
    let mut c = planned(&format!("rust:{symbol}"), Stage::Implemented);
    c.status = Status::Existing;
    c.type_name = ty.into();
    c.anchor = Some(anchor_for(symbol));
    c
}

fn rel(kind: RelationKind, a: &str, b: &str) -> Relation {
    Relation::new(kind, id(a), id(b), Provenance::OBSERVED)
}

fn call(a: &str, b: &str, order: u32) -> Relation {
    let mut r = rel(RelationKind::Calls, a, b);
    r.order = Some(order);
    r
}

fn flow(a: &str, b: &str, data: &str) -> Relation {
    let mut r = rel(RelationKind::DataFlow, a, b);
    r.data = Some(data.into());
    r
}

/// app { run, step, save, log }, lib { util, Plan, Report }.
/// run -> step (first) -> save -> step (a loop), run -> log (second),
/// step -> util and log -> util (util reached twice).
/// Plan flows run -> step; step turns it into a Report for save.
fn sample() -> Graph {
    let mut g = Graph::new();
    for (s, ty) in [("app", "module"), ("app::run", "function"), ("app::step", "function"), ("app::save", "function"), ("app::log", "function"), ("lib", "module"), ("lib::util", "function"), ("lib::Plan", "class"), ("lib::Report", "class")] {
        let mut c = code(s, ty);
        if s == "app::step" {
            c.signature = Some(Signature { params: vec![Param { name: "plan".into(), ty: Some("&Plan".into()), optional: false, variadic: false }], returns: Some("Report".into()), qualifiers: vec![] });
        }
        g.upsert_component(c, Provenance::OBSERVED);
    }
    for (a, b) in [("app", "app::run"), ("app", "app::step"), ("app", "app::save"), ("app", "app::log"), ("lib", "lib::util"), ("lib", "lib::Plan"), ("lib", "lib::Report")] {
        g.upsert_relation(rel(RelationKind::Contains, a, b));
    }
    for r in [call("app::run", "app::step", 0), call("app::run", "app::log", 1), call("app::step", "app::save", 0), call("app::save", "app::step", 0), call("app::step", "lib::util", 1), call("app::log", "lib::util", 0)] {
        g.upsert_relation(r);
    }
    for r in [flow("app::run", "app::step", "&Plan"), flow("app::step", "app::save", "Report"), flow("app::step", "app::log", "()")] {
        g.upsert_relation(r);
    }
    g
}

struct Fixture {
    graph: Graph,
    plan: Option<Plan>,
    others: BTreeMap<ComponentId, Vec<PlanId>>,
    config: TreeConfig,
    types: TypeSystem,
    budget: usize,
    link_budget: usize,
}

impl Fixture {
    fn new(graph: Graph) -> Self {
        Fixture { graph, plan: None, others: BTreeMap::new(), config: TreeConfig::defaults().unwrap(), types: TypeSystem::defaults().unwrap(), budget: 400, link_budget: 300 }
    }

    fn input(&self) -> Input<'_, Graph> {
        Input { graph: &self.graph, plan: self.plan.as_ref(), other_plans: &self.others, config: &self.config, types: &self.types, budget: self.budget, link_budget: self.link_budget, out_of_time: &|| false }
    }

    fn tree(&self, request: TreeRequest) -> Tree {
        build(&self.input(), &request)
    }
}

fn ask(view: ViewKind, root: Option<&str>, way: Way, depth: u32) -> TreeRequest {
    TreeRequest { view, root: root.map(|r| id(r).0), way, depth: Some(depth), ..TreeRequest::default() }
}

fn keys(tree: &Tree) -> Vec<String> {
    tree.cards.iter().map(|c| c.label.clone()).collect()
}

fn line<'t>(tree: &'t Tree, from: &str, to: &str) -> Option<&'t Line> {
    let key = |label: &str| tree.cards.iter().find(|c| c.label == label).map(|c| c.key.clone());
    let (a, b) = (key(from)?, key(to)?);
    tree.lines.iter().find(|l| l.from == a && l.to == b)
}

fn clean(tree: &Tree) {
    let found = problems(tree, &TreeConfig::defaults().unwrap().layout);
    assert!(found.is_empty(), "{found:#?}");
}

// req: M.4
#[test]
fn contains_is_boxes_in_boxes_from_the_top() {
    let f = Fixture::new(sample());
    let top = f.tree(ask(ViewKind::Contains, None, Way::Down, 1));
    assert_eq!(top.roots, vec![id("app").0, id("lib").0], "the top-level parts");
    assert_eq!(top.cards.len(), 9);
    assert!(top.lines.is_empty(), "boxes, no lines");
    let shallow = f.tree(ask(ViewKind::Contains, None, Way::Down, 0));
    assert_eq!(shallow.card(&id("app").0).unwrap().more, 4, "a shut box says how much is inside");
    let app = top.card(&id("app").0).unwrap();
    for part in top.cards.iter().filter(|c| c.parent.as_deref() == Some(app.key.as_str())) {
        assert!(app.rect.contains(&part.rect), "{} sits inside app", part.label);
    }
    clean(&top);
    // Up: what holds it.
    let up = f.tree(ask(ViewKind::Contains, Some("app::step"), Way::Up, 3));
    assert_eq!(keys(&up), vec!["step", "app"]);
    assert_eq!(f.tree(ask(ViewKind::Contains, Some("app::step"), Way::Down, 1)).path.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), vec!["app"]);
}

// req: M.4
#[test]
fn calls_go_down_in_order_and_up_with_repeats_loops_and_pills() {
    let f = Fixture::new(sample());
    let down = f.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 3));
    assert_eq!(keys(&down), vec!["run", "step", "log", "save", "util"], "breadth first, each in call order, each once");
    assert_eq!(line(&down, "run", "step").unwrap().order, Some(0));
    assert_eq!(line(&down, "log", "util").unwrap().kind, LineKind::Repeat, "util is drawn once, under step; log points at it");
    assert_eq!(line(&down, "save", "step").unwrap().kind, LineKind::Loop, "save calls back up its own path");
    let pills: Vec<(&str, PillRole)> = line(&down, "run", "step").unwrap().pills.iter().map(|p| (p.text.as_str(), p.role)).collect();
    assert_eq!(pills, vec![("plan: &Plan", PillRole::In), ("Report", PillRole::Out)], "what goes in and comes back");
    clean(&down);
    // Up: what calls it.
    let up = f.tree(ask(ViewKind::Calls, Some("lib::util"), Way::Up, 2));
    assert_eq!(keys(&up), vec!["util", "log", "step", "run", "save"]);
    // Without a root: the entry points (nothing calls them).
    let entries = f.tree(ask(ViewKind::Calls, None, Way::Down, 1));
    assert_eq!(entries.roots, vec![id("app::run").0]);
    // The depth cuts, and the cut cards say how much more there is.
    let one = f.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 1));
    assert_eq!(one.card(&id("app::step").0).unwrap().more, 2);
    assert!(one.cut.is_none(), "depth isn't a cut: nothing went over a limit");
}

// req: M.4
#[test]
fn data_flow_has_types_as_cards_and_functions_on_the_lines() {
    let f = Fixture::new(sample());
    let starts = f.tree(ask(ViewKind::DataFlow, None, Way::Down, 3));
    assert_eq!(starts.roots, vec!["type:Plan".to_owned()], "data nothing makes comes first; `()` isn't data");
    let labels = keys(&starts);
    assert_eq!(labels, vec!["Plan", "Report", "save"]);
    let plan_to_report = line(&starts, "Plan", "Report").unwrap();
    assert_eq!(plan_to_report.pills.iter().map(|p| (p.text.as_str(), p.role)).collect::<Vec<_>>(), vec![("step", PillRole::Step)]);
    assert_eq!(starts.cards[2].kind, "end", "save takes a Report and makes nothing: the flow ends there");
    assert_eq!(starts.card("type:Plan").unwrap().component, Some(id("lib::Plan")), "a data card links to its type");
    // Up: where it came from.
    let up = f.tree(TreeRequest { root: Some("type:Report".into()), ..ask(ViewKind::DataFlow, None, Way::Up, 3) });
    assert_eq!(keys(&up), vec!["Report", "Plan", "run"]);
    clean(&starts);
}

// req: M.4
#[test]
fn a_tree_bigger_than_the_budget_is_cut_at_a_whole_level_and_says_so() {
    let mut g = sample();
    for i in 0..30 {
        let name = format!("app::helper{i:02}");
        g.upsert_component(code(&name, "function"), Provenance::OBSERVED);
        g.upsert_relation(call("app::save", &name, 10 + i));
    }
    let mut f = Fixture::new(g);
    f.budget = 10;
    let tree = f.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 4));
    assert!(tree.cards.len() <= 10);
    let cut = tree.cut.as_ref().expect("cut");
    assert_eq!((cut.reason, cut.shown, cut.asked), (CutReason::Budget, 2, 4), "the third level (30 helpers) didn't fit");
    assert_eq!(tree.card(&id("app::save").0).unwrap().more, 30, "the cut card counts what it hides");
    assert!(cut.hidden >= 30);
    assert!(render(&tree, &f.config).contains("showing 2 of 4 levels"));
    // Too many starting points: the first ones, and the rest counted.
    let mut f2 = Fixture::new(sample());
    f2.budget = 1;
    let top = f2.tree(ask(ViewKind::Contains, None, Way::Down, 1));
    assert_eq!((top.cards.len(), top.cut.as_ref().unwrap().shown), (1, 0));
    // Lines to cards shown elsewhere have their own limit; the rest are counted.
    let mut f4 = Fixture::new(sample());
    f4.link_budget = 0;
    let no_links = f4.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 3));
    assert!(no_links.lines.iter().all(|l| l.kind == LineKind::Tree));
    assert_eq!(no_links.card(&id("app::log").0).unwrap().links_left_out, 1, "log's line to util (shown under step) is counted, not drawn");
    // Without a root, a view shows its first starting points and counts the rest.
    let mut f3 = Fixture::new(sample());
    f3.config.views.get_mut(&ViewKind::Contains).unwrap().starts = 1;
    let first = f3.tree(ask(ViewKind::Contains, None, Way::Down, 1));
    assert_eq!(first.roots, vec![id("app").0]);
    assert_eq!(first.cut.as_ref().unwrap().starts_left_out, 1);
    assert!(render(&first, &f3.config).contains("first 1 starting points of 2"));
}

// req: M.5
#[test]
fn opening_a_card_adds_its_branch_and_expand_and_context_agree_with_a_deeper_tree() {
    let f = Fixture::new(sample());
    let base = ask(ViewKind::Calls, Some("app::run"), Way::Down, 1);
    let opened = f.tree(TreeRequest { opened: vec![Opened { card: id("app::step").0, levels: 1 }], ..base.clone() });
    assert!(opened.card(&id("app::save").0).is_some() && opened.card(&id("app::step").0).unwrap().open);
    // Expand returns only what's new.
    let added = expand(&f.input(), &base, &id("app::step").0, 1);
    let mut added_keys: Vec<String> = added.cards.iter().map(|c| c.label.clone()).collect();
    added_keys.sort();
    assert_eq!(added_keys, vec!["save", "util"]);
    // The base plus every expanded card is the deeper tree.
    let deeper = f.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 2));
    let mut together: Vec<String> = f.tree(base.clone()).cards.iter().map(|c| c.key.clone()).collect();
    for card in ["app::step", "app::log"] {
        together.extend(expand(&f.input(), &base, &id(card).0, 1).cards.into_iter().map(|c| c.key));
    }
    together.sort();
    together.dedup();
    let mut deeper_keys: Vec<String> = deeper.cards.iter().map(|c| c.key.clone()).collect();
    deeper_keys.sort();
    assert_eq!(together, deeper_keys);
    // Closing a card hides its children and counts them.
    let closed = f.tree(TreeRequest { closed: vec![id("app::step").0], ..ask(ViewKind::Calls, Some("app::run"), Way::Down, 3) });
    assert!(closed.card(&id("app::save").0).is_none());
    assert_eq!(closed.card(&id("app::step").0).unwrap().more, 1, "save hidden; util is still shown under log");
    // Context: what holds it, what else is there, its lines to those.
    let around = context(&f.input(), &base, &id("app::step").0);
    let mut beside = keys(&around);
    beside.sort();
    assert_eq!(beside, vec!["app", "log", "run", "save", "step"]);
    assert!(around.lines.iter().any(|l| l.from == id("app::step").0 && l.to == id("app::save").0), "step's call to save, beside it");
    // A branch that doesn't fit is closed again and reported.
    let mut tight = Fixture::new(sample());
    tight.budget = 3;
    let t = tight.tree(TreeRequest { opened: vec![Opened { card: id("app::step").0, levels: 2 }], ..base });
    assert_eq!(t.dropped, vec![id("app::step").0]);
    assert!(t.card(&id("app::save").0).is_none());
}

/// A plan on the sample: a new `audit` that run calls, `log` changed, and a
/// planned call from run to save the code doesn't make.
fn plan() -> Plan {
    let mut plan = Plan::new("p", "P");
    plan.components.push(planned("rust:app::audit", Stage::Declared));
    let mut log = code("app::log", "function");
    log.status = Status::Modifying;
    log.stage = Stage::ArchApproved;
    plan.components.push(log);
    let mut run = code("app::run", "function");
    run.status = Status::Modifying;
    run.stage = Stage::Implemented;
    plan.components.push(run);
    for (a, b) in [("app::run", "app::audit"), ("app::run", "app::save")] {
        plan.relations.push(Relation::new(RelationKind::Calls, id(a), id(b), Provenance::PLANNED));
    }
    plan.relations.push(Relation::new(RelationKind::Contains, id("app"), id("app::audit"), Provenance::PLANNED));
    plan
}

fn with_plan() -> Fixture {
    let p = plan();
    let mut f = Fixture::new(Graph::merge(sample(), std::slice::from_ref(&p)));
    f.others.insert(id("app::log"), vec![PlanId::new("q").unwrap()]);
    f.plan = Some(p);
    f
}

fn on_plan(view: ViewKind, depth: Option<u32>) -> TreeRequest {
    TreeRequest { view, source: Source::Plan(PlanId::new("p").unwrap()), depth, ..TreeRequest::default() }
}

// req: M.6
#[test]
fn picking_a_plan_starts_from_its_own_items_and_shows_only_the_code_it_plugs_into() {
    let f = with_plan();
    // Calls: from the plan's functions nothing else in the plan calls.
    let calls = f.tree(on_plan(ViewKind::Calls, Some(2)));
    assert_eq!(calls.roots, vec![id("app::run").0], "run calls the plan's other items; nothing in the plan calls run");
    let card = |k: &str| calls.card(&id(k).0).unwrap();
    assert_eq!((card("app::audit").mark, card("app::log").mark, card("app::run").mark), (Mark::New, Mark::Changed, Mark::Done));
    assert!(card("app::step").outside_plan && !card("app::step").open, "step is code the plan plugs into: shown, not walked");
    assert!(calls.card(&id("app::save").0).is_some_and(|c| !c.open), "save is there (the plan says run calls it) but not walked");
    assert_eq!(line(&calls, "run", "save").unwrap().mark, Mark::Mismatch, "planned, the code doesn't make that call");
    assert_eq!(line(&calls, "run", "step").unwrap().mark, Mark::Code, "the real call is shown beside it");
    assert_eq!(card("app::log").other_plans, vec![PlanId::new("q").unwrap()], "another plan also changes log");
    assert!(render(&calls, &f.config).contains("also changed by q"));
    // Contains: the plan's items in the boxes they go into, nothing else.
    let boxes = f.tree(on_plan(ViewKind::Contains, None));
    let mut shown = keys(&boxes);
    shown.sort();
    assert_eq!(shown, vec!["app", "audit", "log", "run"], "lib and app's other parts aren't part of it");
    assert!(boxes.card(&id("app").0).unwrap().outside_plan);
    // Show it in the full code: everything, marks kept.
    let full = f.tree(TreeRequest { full_code: true, ..on_plan(ViewKind::Contains, Some(1)) });
    assert!(full.card(&id("lib").0).is_some());
    assert_eq!(full.card(&id("app::audit").0).unwrap().mark, Mark::New);
    // Code: no marks.
    let mut code_only = Fixture::new(sample());
    code_only.plan = None;
    assert!(code_only.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 2)).cards.iter().all(|c| c.mark == Mark::Code));
}

// req: M.6
#[test]
fn a_plans_data_flow_starts_from_the_data_it_brings() {
    let mut p = plan();
    p.components.push({
        let mut c = planned("rust:lib::Audit", Stage::Declared);
        c.type_name = "class".into();
        c
    });
    let mut f2 = flow("app::audit", "app::save", "Audit");
    f2.provenance = Provenance::PLANNED;
    p.relations.push(f2);
    let mut f3 = flow("app::run", "app::audit", "&Plan");
    f3.provenance = Provenance::PLANNED;
    p.relations.push(f3);
    let mut f = Fixture::new(Graph::merge(sample(), std::slice::from_ref(&p)));
    f.plan = Some(p);
    let tree = f.tree(on_plan(ViewKind::DataFlow, Some(2)));
    assert_eq!(tree.roots, vec!["type:Plan".to_owned()], "Plan comes into the plan from outside it; the plan makes Audit itself");
    let audit = tree.card("type:Audit").expect("Plan becomes an Audit via audit");
    assert_eq!(audit.mark, Mark::New);
    assert!(!audit.outside_plan);
    let report = tree.card("type:Report").unwrap();
    assert!(report.outside_plan && !report.open, "step (not in the plan) makes a Report from it: shown, not walked");
}

// req: M.7
#[test]
fn trees_lay_out_spaced_and_lines_stay_out_of_cards() {
    let f = Fixture::new(sample());
    for request in [ask(ViewKind::Calls, Some("app::run"), Way::Down, 4), ask(ViewKind::Calls, Some("lib::util"), Way::Up, 4), ask(ViewKind::DataFlow, None, Way::Down, 4), ask(ViewKind::Contains, None, Way::Down, 3)] {
        let tree = f.tree(request);
        clean(&tree);
        assert!(tree.size.0 > 0.0 && tree.size.1 > 0.0);
        for c in &tree.cards {
            for (text, size) in card_lines(c, tree.request.view, &f.config.layout) {
                let fits = f.config.layout.text_width(&text, size) <= c.rect.w - f.config.layout.pad * 2.0 + 0.5;
                assert!(fits || c.rect.w >= f.config.layout.card_max_w, "`{text}` fits its card");
            }
        }
    }
    // Columns: each level to the right of the one before, children after their parent.
    let tree = f.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 2));
    let x = |k: &str| tree.card(&id(k).0).unwrap().rect.x;
    assert!(x("app::run") < x("app::step") && x("app::step") < x("app::save"));
    let lines_back = tree.lines.iter().filter(|l| l.kind != LineKind::Tree).count();
    assert!(lines_back >= 1);
}

// req: M.4
#[test]
fn project_config_overrides_some_tree_settings() {
    let c = TreeConfig::with_overrides("[views.calls]\ndepth = 7\n[layout]\ngap_x = 80.0\n").unwrap();
    assert_eq!(c.view(ViewKind::Calls).depth, 7);
    assert_eq!(c.layout.gap_x, 80.0);
    assert_eq!(c.view(ViewKind::Contains).depth, TreeConfig::defaults().unwrap().view(ViewKind::Contains).depth);
    assert!(TreeConfig::with_overrides("[layout]\ngap_x = \"wide\"\n").is_err());
}
