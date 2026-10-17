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
    let mut g = Graph::default();
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
    entries: Vec<EntryPoint>,
}

impl Fixture {
    /// `app::run` is the program's main; `app::step` handles a route.
    fn new(graph: Graph) -> Self {
        let entries = vec![EntryPoint { id: id("app::run"), why: "main".into() }, EntryPoint { id: id("app::step"), why: "http /api/step".into() }];
        Fixture { graph, plan: None, others: BTreeMap::new(), config: TreeConfig::defaults().unwrap(), types: TypeSystem::defaults().unwrap(), budget: 400, link_budget: 300, entries }
    }

    fn input(&self) -> Input<'_, Graph> {
        Input { graph: &self.graph, plan: self.plan.as_ref(), other_plans: &self.others, config: &self.config, types: &self.types, budget: self.budget, link_budget: self.link_budget, out_of_time: &|| false, layout_out_of_time: &|| false, flows: None, entries: &self.entries }
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
    // Without a root: the entry points to pick from (the main, the route's
    // handler), not every function nothing calls.
    let entries = f.tree(ask(ViewKind::Calls, None, Way::Down, 1));
    assert_eq!(entries.roots, vec![id("app::run").0, id("app::step").0]);
    let none = Fixture { entries: Vec::new(), ..Fixture::new(sample()) };
    assert!(none.tree(ask(ViewKind::Calls, None, Way::Down, 1)).roots.is_empty(), "no entry points: nothing to start from");
    // The depth cuts, and the cut cards say how much more there is.
    let one = f.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 1));
    assert_eq!(one.card(&id("app::step").0).unwrap().more, 2);
    assert!(one.cut.is_none(), "depth isn't a cut: nothing went over a limit");
}

#[test]
fn data_flow_has_types_as_cards_and_functions_on_the_lines() {
    let f = Fixture::new(sample());
    let starts = f.tree(ask(ViewKind::DataFlow, None, Way::Down, 3));
    assert_eq!(starts.roots, vec!["type:Plan".to_owned()], "the data the entry points take (the route's handler takes a Plan)");
    let labels = keys(&starts);
    assert_eq!(labels, vec!["Plan", "Report", "save"]);
    let plan_to_report = line(&starts, "Plan", "Report").unwrap();
    assert_eq!(plan_to_report.pills.iter().map(|p| (p.text.as_str(), p.role)).collect::<Vec<_>>(), vec![("step", PillRole::Step)]);
    assert_eq!(starts.cards[2].kind, "end", "save takes a Report and makes nothing: the flow ends there");
    assert_eq!(starts.card("type:Plan").unwrap().component, Some(id("lib::Plan")), "a data card links to its type");
    // How a flow is known: these were matched by name (no analyzer said so).
    assert_eq!(plan_to_report.evidence, Some(Evidence::Named));
    // Up: where it came from.
    let up = f.tree(TreeRequest { root: Some("type:Report".into()), ..ask(ViewKind::DataFlow, None, Way::Up, 3) });
    assert_eq!(keys(&up), vec!["Report", "Plan", "run"]);
    clean(&starts);
}

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

#[test]
fn opening_a_card_adds_its_branch_and_expand_and_context_agree_with_a_deeper_tree() {
    let f = Fixture::new(sample());
    let base = ask(ViewKind::Calls, Some("app::run"), Way::Down, 1);
    let opened = f.tree(TreeRequest { opened: vec![Opened { card: id("app::step").0, levels: 1 }], ..base.clone() });
    assert!(opened.card(&id("app::save").0).is_some() && opened.card(&id("app::step").0).unwrap().open);
    // Expand into the tree held: only what's new comes as cards, every
    // card's place with it (laid out here, the whole tree again).
    let held = f.tree(base.clone());
    let (now, added) = expand(&f.input(), &held, &id("app::step").0, 1, 1000);
    let mut added_keys: Vec<String> = added.cards.iter().map(|c| c.label.clone()).collect();
    added_keys.sort();
    assert_eq!(added_keys, vec!["save", "util"]);
    assert_eq!(added.places.len(), now.cards.len(), "every card held has its place");
    assert_eq!(keys(&now), keys(&opened), "the same cards as the tree with it opened");
    clean(&now);
    // The base plus every expanded card is the deeper tree.
    let deeper = f.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 2));
    let mut together: Vec<String> = held.cards.iter().map(|c| c.key.clone()).collect();
    for card in ["app::step", "app::log"] {
        together.extend(expand(&f.input(), &held, &id(card).0, 1, 1000).1.cards.into_iter().map(|c| c.key));
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
    // A card nothing holds: only its own branch, not every top.
    let top = context(&f.input(), &base, &id("lib").0);
    let mut held_by_lib = keys(&top);
    held_by_lib.sort();
    assert_eq!(held_by_lib, vec!["Plan", "Report", "lib", "util"]);
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

#[test]
fn project_config_overrides_some_tree_settings() {
    let c = TreeConfig::with_overrides("[views.calls]\ndepth = 7\n[layout]\ngap_x = 80.0\n").unwrap();
    assert_eq!(c.view(ViewKind::Calls).depth, 7);
    assert_eq!(c.layout.gap_x, 80.0);
    assert_eq!(c.view(ViewKind::Contains).depth, TreeConfig::defaults().unwrap().view(ViewKind::Contains).depth);
    assert!(TreeConfig::with_overrides("[layout]\ngap_x = \"wide\"\n").is_err());
}

#[test]
fn call_lines_say_how_theyre_known_and_the_text_marks_name_matches() {
    let mut g = sample();
    // run -> step was confirmed by the language's analyzer; the rest only matched by name.
    let mut proven = call("app::run", "app::step", 0);
    proven.proven_by = Some("rust-analyzer 1.90.0".into());
    g.upsert_relation(proven);
    let f = Fixture::new(g);
    let tree = f.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 1));
    assert_eq!(line(&tree, "run", "step").unwrap().evidence, Some(Evidence::Proven("rust-analyzer 1.90.0".into())));
    assert_eq!(line(&tree, "run", "log").unwrap().evidence, Some(Evidence::Named));
    let text = render(&tree, &f.config);
    let said = |name: &str| text.lines().find(|l| l.trim_start().starts_with(name)).unwrap().to_owned();
    assert!(said("log").contains("(matched by name, unconfirmed)"), "{text}");
    assert!(!said("step").contains("matched by name"), "{text}");
    // Structure isn't a matter of evidence.
    let boxes = f.tree(ask(ViewKind::Contains, Some("app"), Way::Down, 1));
    assert!(boxes.lines.iter().all(|l| l.evidence.is_none()));
}

#[test]
fn every_kind_of_link_found_in_the_code_says_how_it_is_known() {
    let mut contains = rel(RelationKind::Contains, "app", "app::run");
    contains.provenance = Provenance::OBSERVED;
    assert_eq!(Evidence::of(&contains), Some(Evidence::Read), "what holds what is read");
    for kind in [RelationKind::Calls, RelationKind::DataFlow, RelationKind::Implements] {
        let mut r = rel(kind, "app::run", "app::step");
        r.provenance = Provenance::OBSERVED;
        assert_eq!(Evidence::of(&r), Some(Evidence::Named), "{kind:?}: only matched by name");
        r.proven_by = Some("rust-analyzer 1.90.0".into());
        assert_eq!(Evidence::of(&r).map(|e| e.describe()), Some("confirmed by rust-analyzer 1.90.0".into()));
    }
    let planned = rel(RelationKind::Calls, "app::run", "app::step");
    assert_eq!(Evidence::of(&Relation { provenance: Provenance::PLANNED, ..planned }), None, "only a plan says so: not found in the code");
    assert_eq!(Evidence::Proven("x".into()).weaker(Evidence::Named), Evidence::Named);
}

#[test]
fn calls_are_a_tidy_tree_and_contains_a_treemap() {
    let f = Fixture::new(sample());
    // Tidy: each card centred on its children; a small branch tucks in.
    let down = f.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 3));
    clean(&down);
    let centre = |label: &str| down.cards.iter().find(|c| c.label == label).unwrap().rect.center().1;
    for parent in down.cards.iter() {
        let kids: Vec<&Card> = down.cards.iter().filter(|c| c.parent.as_deref() == Some(parent.key.as_str())).collect();
        if let (Some(first), Some(last)) = (kids.first(), kids.last()) {
            let middle = (first.rect.center().1 + last.rect.center().1) / 2.0;
            assert!((parent.rect.center().1 - middle).abs() < 0.5, "{} sits in the middle of its children", parent.label);
        }
    }
    assert!(centre("log") > centre("step"), "in call order");
    // Pills are placed by the layout: just before the card their line
    // arrives at, as wide as their text.
    let with_pills: Vec<&Line> = down.lines.iter().filter(|l| !l.pills.is_empty()).collect();
    assert!(!with_pills.is_empty(), "the sample's calls carry what goes in and comes back");
    for line in with_pills {
        let to = down.card(&line.to).unwrap().rect;
        for pill in &line.pills {
            let at = pill.rect.unwrap_or_else(|| panic!("{} on {} -> {} is placed", pill.text, line.from, line.to));
            assert!(at.right() <= to.x && at.right() > to.x - f.config.layout.gap_x, "{}: in the gap before {}", pill.text, line.to);
            assert!(at.w >= f.config.layout.text_width(&pill.text, f.config.layout.small_size), "{}: as wide as its text", pill.text);
        }
    }
    // Contains: every row of parts fills its box's width.
    let boxes = f.tree(ask(ViewKind::Contains, None, Way::Down, 2));
    clean(&boxes);
    let layout = &f.config.layout;
    for holder in boxes.cards.iter() {
        let parts: Vec<&Card> = boxes.cards.iter().filter(|c| c.parent.as_deref() == Some(holder.key.as_str())).collect();
        if parts.is_empty() {
            continue;
        }
        let right = holder.rect.right() - layout.pad;
        // The last part of each row (nothing to its right at its height) reaches the box's inner edge.
        for p in &parts {
            let last_in_row = !parts.iter().any(|q| q.rect.y == p.rect.y && q.rect.x > p.rect.x);
            if last_in_row {
                assert!((p.rect.right() - right).abs() < 0.5, "{} fills its row in {}", p.label, holder.label);
            }
        }
    }
}

#[test]
fn the_layout_has_its_own_time_limit_and_a_cut_answer_says_so() {
    let f = Fixture::new(sample());
    // Out of time from the start: the first starting point is still laid
    // out (an answer always shows something), the rest are left out.
    let late = || true;
    for view in [ViewKind::Contains, ViewKind::Calls] {
        let mut tree = walk_tree(&f.input(), &ask(view, None, Way::Down, 3));
        let starts = tree.roots.len();
        assert!(starts > 1, "{view:?}: more than one starting point to lay out");
        lay_out(&mut tree, &f.config.layout, &late);
        let cut = tree.cut.clone().expect("the layout cut it");
        assert_eq!((cut.reason, cut.starts_left_out as usize), (CutReason::Time, starts - tree.roots.len()), "{view:?}");
        assert!(tree.roots.len() < starts, "{view:?}: what had no time is left out");
        clean(&tree);
    }
    // One starting point: time is checked at every level, so even one
    // branch is cut where time ran out, and the cards that lost parts count
    // them as more.
    for view in [ViewKind::Contains, ViewKind::Calls] {
        let root = if view == ViewKind::Contains { "app" } else { "app::run" };
        let mut tree = walk_tree(&f.input(), &ask(view, Some(root), Way::Down, 3));
        let (cards, more) = (tree.cards.len(), tree.cards.iter().map(|c| c.more).sum::<u32>());
        lay_out(&mut tree, &f.config.layout, &late);
        let cut = tree.cut.clone().expect("the layout cut it");
        assert_eq!((cut.reason, cut.starts_left_out), (CutReason::Time, 0), "{view:?}");
        assert!(tree.cards.len() < cards && cut.hidden as usize == cards - tree.cards.len(), "{view:?}: what had no time is left out and counted");
        assert!(tree.cards.iter().map(|c| c.more).sum::<u32>() > more, "{view:?}: the cards it was under say there's more");
        clean(&tree);
    }
    // In time: nothing left out.
    let mut tree = walk_tree(&f.input(), &ask(ViewKind::Calls, None, Way::Down, 3));
    lay_out(&mut tree, &f.config.layout, &|| false);
    assert!(tree.cut.is_none());
}

#[test]
fn only_the_components_asked_for_show_with_what_holds_them() {
    let f = Fixture::new(sample());
    let only = |view: ViewKind| TreeRequest { view, only: Some(vec![id("app::step").0, id("lib::util").0]), ..TreeRequest::default() };
    let mut boxes = keys(&f.tree(only(ViewKind::Contains)));
    boxes.sort();
    assert_eq!(boxes, vec!["app", "lib", "step", "util"], "the changed ones and their boxes, nothing else");
    // A box asked for itself shows only what's asked for in it.
    let with_box = TreeRequest { only: Some(vec![id("app").0, id("app::step").0]), ..only(ViewKind::Contains) };
    let mut boxes = keys(&f.tree(with_box));
    boxes.sort();
    assert_eq!(boxes, vec!["app", "step"]);
    let calls = f.tree(only(ViewKind::Calls));
    assert!(keys(&calls).contains(&"step".to_owned()) && !keys(&calls).contains(&"run".to_owned()), "{:?}", keys(&calls));
    // With a plan: of its items, only those asked for.
    let mut on = with_plan();
    on.budget = 400;
    let mut boxes = keys(&on.tree(TreeRequest { only: Some(vec![id("app::run").0]), ..on_plan(ViewKind::Contains, None) }));
    boxes.sort();
    assert_eq!(boxes, vec!["app", "run"]);
    assert!(f.tree(TreeRequest { only: Some(Vec::new()), ..ask(ViewKind::Contains, None, Way::Down, 2) }).cards.is_empty(), "none asked for: nothing");
}

#[test]
fn opening_recounts_what_every_card_has_left_to_open() {
    // run calls step and log; step calls save and util; log calls util.
    let f = Fixture::new(sample());
    let held = f.tree(ask(ViewKind::Calls, Some("app::run"), Way::Down, 1));
    let more = |tree: &Tree, key: &str| tree.card(&id(key).0).map(|c| (c.open, c.more));
    assert_eq!((more(&held, "app::step"), more(&held, "app::log")), (Some((false, 2)), Some((false, 1))));
    // Opening step draws util: log's one call is drawn now, nothing left to open.
    let (now, change) = expand(&f.input(), &held, &id("app::step").0, 1, 400);
    assert_eq!(more(&now, "app::log"), Some((false, 0)), "util is drawn under step");
    assert!(change.places.iter().any(|p| p.key == id("app::log").0 && p.more == 0), "and the client is told");
    // Opening log adds no card, only its line to util; it has nothing more.
    let (after, change) = expand(&f.input(), &now, &id("app::log").0, 1, 400);
    assert!(change.cards.is_empty(), "nothing new");
    assert_eq!(more(&after, "app::log"), Some((true, 0)));
    assert!(line(&after, "log", "util").is_some(), "a line to where util is drawn");
}

#[test]
fn a_card_opened_inside_another_opened_card_opens_too() {
    let f = Fixture::new(sample());
    let base = ask(ViewKind::Calls, Some("app::run"), Way::Down, 0);
    let open = |cards: &[&str]| TreeRequest { opened: cards.iter().map(|c| Opened { card: id(c).0, levels: 1 }).collect(), ..base.clone() };
    // step only shows once run is open; it was opened after run.
    let t = f.tree(open(&["app::run", "app::step"]));
    assert!(t.card(&id("app::save").0).is_some() && t.card(&id("app::step").0).unwrap().open, "{:?}", keys(&t));
    assert!(t.dropped.is_empty());
    // A card under a closed one isn't there: it stays as asked, not dropped.
    let shut = f.tree(TreeRequest { closed: vec![id("app::run").0], ..open(&["app::run", "app::step"]) });
    assert!(shut.card(&id("app::step").0).is_none() && shut.dropped.is_empty());
    // The inner one doesn't fit: only it is closed again, and said.
    let mut tight = Fixture::new(sample());
    tight.budget = 3;
    let t = tight.tree(open(&["app::run", "app::step"]));
    assert_eq!(t.dropped, vec![id("app::step").0]);
    assert!(t.card(&id("app::step").0).is_some() && t.card(&id("app::save").0).is_none());
}
