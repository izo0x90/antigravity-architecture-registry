//! The graph's own actions. Selecting, opening and closing, going in and
//! back are the universal moves (in `select`); these go in, set the depth,
//! flip the way, switch the view and pick the source (with its "show in full
//! code" switch). Besides them: finding something to start from, and going
//! back to where you were before the agent moved the view, and the camera:
//! zoom in or out, or fit the drawing to the screen (it also moves with the
//! pointer and fingers). Keys live in the keymap.
//!
//! Finding anything in the code from the graph: symbols, files and lines,
//! through the server's one search, listed in the palette as they're typed.
//! Picking one makes its card the root, in this view if it can show it.

use arch_graph::tree::{ViewKind, Way};
use arch_graph::Source;

use std::collections::BTreeMap;

use arch_search::{Found, Hit, SearchKind};

use super::{GraphState, NavChange};
use crate::net::NetResult;
use crate::actions::{Action, ActionProvider, Choice, Group, MINOR, PRIMARY, Param, RunCx, SECONDARY, Style};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GraphCommand {
    SetView,
    SetPlan,
    FullCode,
    OnlyChanged,
    Flip,
    Depth(i32),
    GoIn,
    AgentReturn,
    Find,
    /// The camera: zoom by this factor (around the selection), fit the
    /// whole tree, or fit the selected card.
    Zoom(f32),
    Fit,
    FitCard,
    /// Show or hide the legend.
    Legend,
    /// Back along the path: its start (`top`), a root gone through
    /// (`back:<i>`), or what holds the root (`into:<key>`).
    Path,
}

impl GraphState {
    /// The card a view switch starts from: the selection (or the root) as
    /// that view knows it, if it can show it.
    pub(crate) fn root_for(&self, view: ViewKind) -> Option<String> {
        let key = self.selection.as_ref().or(self.nav.root.as_ref())?;
        let card = self.card(key);
        match view {
            // A component; a data card stands for its type's component.
            ViewKind::Contains => card.and_then(|c| c.component.as_ref()).map(|id| id.0.clone()).or_else(|| (!key.starts_with("type:")).then(|| key.clone())),
            // Calls start from something that can be called; anything else starts afresh.
            ViewKind::Calls => card.filter(|c| c.callable).and_then(|c| c.component.as_ref()).map(|id| id.0.clone()),
            // Data flow shows data: a data card stays, anything else starts afresh.
            ViewKind::DataFlow => key.starts_with("type:").then(|| key.clone()),
        }
    }

    /// What flipping shows, in words.
    fn flip_label(&self) -> &'static str {
        match (self.nav.view, self.nav.way.flipped()) {
            (ViewKind::Contains, Way::Up) => "Flip: show what holds it",
            (ViewKind::Contains, Way::Down) => "Flip: show what's inside",
            (ViewKind::Calls, Way::Up) => "Flip: show what calls it",
            (ViewKind::Calls, Way::Down) => "Flip: show what it calls",
            (ViewKind::DataFlow, Way::Up) => "Flip: show where it came from",
            (ViewKind::DataFlow, Way::Down) => "Flip: show where it goes",
        }
    }

}

impl ActionProvider for GraphState {
    fn actions(&self) -> Vec<Action> {
        use GraphCommand as G;
        use Style::{Button, Setting};
        let mut out = self.moves();
        let f = |id: &str, label: &str, c: GraphCommand| Action::new(id, label, c).focused();
        let views = ViewKind::ALL
            .iter()
            .map(|v| {
                let detail = self.config.as_ref().map(|c| c.view(*v).description.clone());
                Choice::new(v.name(), v.name().replace('_', " ")).detail(detail).current(*v == self.nav.view)
            })
            .collect();
        out.push(f("graph.view", "Switch view (contains, calls, data flow)", G::SetView).param(Param::Choice(views)).header(Setting, "View", Some(egui_phosphor::regular::GRAPH)).group(Group::View).jump());
        out.push(f("graph.plan", "Show the code, or one plan", G::SetPlan).param(Param::Choice(self.source_choices())).header(Setting, "Source", None).group(Group::View).jump());
        if self.nav.source != Source::Code {
            out.push(f("graph.full_code", "Show the plan in the full code", G::FullCode).param(Param::Toggle { on: self.nav.full_code }).header(Setting, "Full code", None).group(Group::View).jump());
        }
        if self.nav.only_changed || !self.changed.is_empty() {
            out.push(f("graph.only_changed", "Show only what the changes touch", G::OnlyChanged).param(Param::Toggle { on: self.nav.only_changed }).header(Setting, "Only changed", None).group(Group::View).importance(MINOR).jump());
        }
        out.push(f("graph.flip", self.flip_label(), G::Flip).header(Button, "", Some(egui_phosphor::regular::ARROWS_DOWN_UP)).group(Group::View).importance(SECONDARY).jump());
        out.push(f("graph.depth_less", "Show fewer levels", G::Depth(-1)).header(Button, "", Some(egui_phosphor::regular::MINUS)).group(Group::View).importance(MINOR).jump());
        out.push(f("graph.depth_more", "Show more levels", G::Depth(1)).header(Button, "", Some(egui_phosphor::regular::PLUS)).group(Group::View).importance(MINOR).jump());
        if let Some(card) = self.selected_card()
            && self.nav.root.as_deref() != Some(card.key.as_str())
        {
            out.push(f("graph.go_in", &format!("Make {} the root", card.label), G::GoIn).icon(egui_phosphor::regular::ARROW_SQUARE_IN).group(Group::Navigate).importance(SECONDARY).jump());
        }
        out.push(f("graph.fit", "Fit the whole tree to the screen", G::Fit).header(Button, "", Some(egui_phosphor::regular::CORNERS_OUT)).group(Group::View).importance(MINOR));
        let fit_card = f("graph.fit_card", "Fit the selected card to the screen", G::FitCard).group(Group::View).importance(MINOR);
        out.push(if self.selection.is_some() { fit_card } else { fit_card.when(crate::actions::When::Unavailable) });
        out.push(f("graph.zoom_in", "Zoom in", G::Zoom(self.ui.zoom_step)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_PLUS).group(Group::View).importance(MINOR));
        out.push(f("graph.zoom_out", "Zoom out", G::Zoom(1.0 / self.ui.zoom_step)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_MINUS).group(Group::View).importance(MINOR));
        let (found, waiting) = self.find_choices();
        let find = Param::Search { hint: "a name, a file, or words in a line".into(), found, waiting };
        out.push(f("graph.find", "Find in the code (symbols, files, lines)", G::Find).param(find).header(Button, "", Some(egui_phosphor::regular::MAGNIFYING_GLASS)).group(Group::Navigate).importance(SECONDARY).jump());
        // Too big to show whole: only the levels that fit.
        if let Some(cut) = self.tree.as_ref().and_then(|t| t.cut.as_ref()).filter(|c| c.shown < c.asked && c.shown > 0) {
            let delta = cut.shown as i32 - self.depth() as i32;
            out.push(f("graph.fit_levels", &format!("Show only the {} levels that fit", cut.shown), G::Depth(delta)).header(Button, format!("Show {} levels", cut.shown), None).group(Group::View).importance(SECONDARY).jump());
        }
        if self.agent.is_some() {
            out.push(f("graph.agent_return", "Return to where you were before the agent moved the view", G::AgentReturn).header(Button, "Return", Some(egui_phosphor::regular::ARROW_U_UP_LEFT)).group(Group::Navigate).importance(PRIMARY).jump());
        }
        // The path to the root: each step back along it.
        if let Some(root) = &self.nav.root {
            let name = |key: &str| self.card(key).map_or_else(|| key.rsplit([':', '.']).next().unwrap_or(key).to_owned(), |c| c.label.clone());
            let mut steps = vec![Choice::new("top", "start").detail(Some("The view's starting points".into()))];
            steps.extend(self.trail_roots().iter().enumerate().filter_map(|(i, r)| r.as_ref().map(|r| Choice::new(format!("back:{i}"), name(r)))));
            steps.push(Choice::new(format!("here:{root}"), name(root)).current(true));
            if let Some(tree) = &self.tree {
                steps.extend(tree.path.iter().map(|c| Choice::new(format!("into:{}", c.key), format!("inside {}", c.label)).detail(Some("Show this box".into()))));
            }
            out.push(f("graph.path", "Go back along the path", G::Path).param(Param::Choice(steps)).header(Setting, "Path", None).group(Group::Navigate).importance(MINOR).jump());
        }
        out.push(f("graph.legend", if self.legend_open { "Hide the legend" } else { "Show the legend" }, G::Legend).header(Button, "", Some(egui_phosphor::regular::INFO)).group(Group::View).importance(MINOR));
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, _cx: &mut RunCx<'_>) {
        if let Some(command) = action.command::<GraphCommand>().cloned() {
            self.command(command, value);
        }
    }


    fn search(&mut self, action: &str, query: &str) {
        if action == "graph.find" {
            self.find_query(query);
        }
    }
}

impl GraphState {
    /// What each of the graph's actions does.
    pub(crate) fn command(&mut self, command: GraphCommand, value: Option<String>) {
        match command {
            GraphCommand::SetView => {
                if let Some(view) = value.as_deref().and_then(ViewKind::from_name) {
                    let root = self.root_for(view);
                    self.navigate(NavChange::View { view, root });
                }
            }
            GraphCommand::SetPlan => self.pick_source(value),
            GraphCommand::FullCode => self.switch_full_code(value),
            GraphCommand::OnlyChanged => {
                let on = match value.as_deref() {
                    Some("true") => true,
                    Some("false") => false,
                    _ => !self.nav.only_changed,
                };
                self.navigate(NavChange::OnlyChanged(on));
            }
            GraphCommand::Flip => self.navigate(NavChange::Flip),
            GraphCommand::Zoom(factor) => self.zoom_key(factor),
            GraphCommand::Fit => self.fit_all(true),
            GraphCommand::FitCard => {
                if let Err(why) = self.fit_card() {
                    self.note = Some(why);
                }
            }
            GraphCommand::Depth(delta) => {
                let base = self.depth();
                self.navigate(NavChange::Depth { delta, base });
            }
            GraphCommand::GoIn => {
                if let Some(key) = self.selection.clone() {
                    self.navigate(NavChange::GoIn(key));
                }
            }
            GraphCommand::Find => {
                if let Some(value) = value {
                    self.pick_found(&value);
                }
            }
            GraphCommand::Legend => self.legend_open = !self.legend_open,
            GraphCommand::Path => match value.as_deref().map(|v| v.split_once(':').unwrap_or((v, ""))) {
                Some(("top", _)) => self.navigate(NavChange::Top),
                Some(("back", i)) => {
                    if let Ok(i) = i.parse() {
                        self.back_to(i);
                    }
                }
                Some(("into", key)) => self.navigate(NavChange::View { view: ViewKind::Contains, root: Some(key.to_owned()) }),
                _ => {}
            },
            GraphCommand::AgentReturn => {
                if let Some(agent) = self.agent.take() {
                    self.navigate(NavChange::Restore(Box::new(agent.return_to)));
                    self.pending_select = agent.return_selection;
                    self.selection = None;
                }
            }
        }
    }
}

/// The search being typed, and what came back for it.
#[derive(Default)]
pub(crate) struct Find {
    pub(super) query: String,
    /// Counts up with every query; answers to older ones are dropped.
    pub(super) asked: u64,
    /// The query hasn't gone out yet.
    pub(super) send: bool,
    found: BTreeMap<SearchKind, Found>,
}

impl GraphState {
    /// What's typed in the palette's search, as it changes.
    pub(crate) fn find_query(&mut self, query: &str) {
        let query = query.trim();
        if query == self.find.query && self.find.asked > 0 {
            return;
        }
        self.find.query = query.to_owned();
        self.find.asked += 1;
        self.find.found.clear();
        self.find.send = !query.is_empty();
    }


    pub(super) fn found(&mut self, asked: u64, kind: SearchKind, found: NetResult<Found>) {
        if asked != self.find.asked {
            return;
        }
        // A kind that failed lists nothing (the others still do).
        self.find.found.insert(kind, found.unwrap_or_default());
    }

    /// The palette's list: symbols, then files, then lines; and whether
    /// some are still on their way.
    pub(crate) fn find_choices(&self) -> (Vec<Choice>, bool) {
        let mut out = Vec::new();
        for kind in SearchKind::ALL {
            let Some(found) = self.find.found.get(&kind) else { continue };
            for (i, hit) in found.hits.iter().enumerate() {
                let what = match kind {
                    SearchKind::Symbols => "symbol",
                    SearchKind::Files => "file",
                    SearchKind::Text => "line",
                };
                let mut detail = format!("{what} · {}", hit.detail);
                // The last of a kind says what wasn't listed: more matches
                // than shown, or a search cut at its time limit.
                if i + 1 == found.hits.len() {
                    if found.total > found.hits.len() {
                        detail.push_str(&format!(" · {} of {} {what}s", found.hits.len(), found.total));
                    }
                    if found.partial {
                        detail.push_str(" · stopped at its time limit: there may be more");
                    }
                }
                out.push(Choice::new(format!("{}:{}:{i}", self.find.asked, kind.name()), hit.label.clone()).detail(Some(detail)));
            }
        }
        let waiting = !self.find.query.is_empty() && self.find.found.len() < SearchKind::ALL.len();
        (out, waiting)
    }

    /// Goes to a picked result (`value` from `find_choices`).
    pub(crate) fn pick_found(&mut self, value: &str) {
        let mut parts = value.splitn(3, ':');
        let (Some(asked), Some(kind), Some(index)) = (parts.next(), parts.next(), parts.next()) else { return };
        if asked.parse() != Ok(self.find.asked) {
            return;
        }
        let Some(hit) = SearchKind::from_name(kind).and_then(|k| self.find.found.get(&k)).and_then(|f| f.hits.get(index.parse::<usize>().ok()?)).cloned() else { return };
        self.go_to_hit(&hit);
    }

    fn go_to_hit(&mut self, hit: &Hit) {
        let Some(key) = hit.key.clone() else {
            // Nothing in the graph stands for it (a file no pack reads): show the code.
            if let Some(file) = &hit.file {
                let line = hit.line.unwrap_or(1);
                let line = line as usize;
                self.asks.push(crate::nav::Ask::Show { target: crate::nav::Target::Span { file: file.clone(), start: line, end: line }, in_: crate::app::BufferKind::Code });
            }
            return;
        };
        let here = self.nav.view;
        let (view, root) = match (here, &hit.data) {
            (ViewKind::Calls, _) if hit.callable => (here, key),
            (ViewKind::DataFlow, Some(data)) => (here, data.clone()),
            (ViewKind::Contains, _) => (here, key),
            // This view can't show it: the one that can.
            (_, _) if hit.callable => (ViewKind::Calls, key),
            (_, Some(data)) => (ViewKind::DataFlow, data.clone()),
            _ => (ViewKind::Contains, key),
        };
        self.pending_select = Some(root.clone());
        if view == here {
            self.navigate(NavChange::GoIn(root));
        } else {
            self.navigate(NavChange::View { view, root: Some(root) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{sample_tree, showing};
    use crate::nav::Navigable;

    fn hit(kind: SearchKind, key: Option<&str>, label: &str, callable: bool, data: Option<&str>) -> Hit {
        Hit { kind, key: key.map(Into::into), label: label.into(), detail: "d".into(), file: Some("src/a.rs".into()), line: Some(3), callable, data: data.map(Into::into) }
    }

    fn answer(g: &mut GraphState, kind: SearchKind, hits: Vec<Hit>) {
        let asked = g.find.asked;
        g.found(asked, kind, Ok(Found { total: hits.len(), hits, ..Found::default() }));
    }

    fn value_of(g: &GraphState, label: &str) -> String {
        g.find_choices().0.into_iter().find(|c| c.label == label).unwrap().value
    }

    #[test]
    fn finding_lists_symbols_files_and_lines_and_picking_one_makes_it_the_root() {
        let mut g = showing(sample_tree());
        assert_eq!(g.nav.view, ViewKind::Calls);
        g.find_query("step");
        assert!(g.find.send, "the query goes out on the next frame");
        assert_eq!(g.find_choices(), (Vec::new(), true), "waiting for the answers");
        answer(&mut g, SearchKind::Text, vec![hit(SearchKind::Text, None, "// step by step", false, None)]);
        answer(&mut g, SearchKind::Files, vec![hit(SearchKind::Files, Some("rust:a"), "src/a.rs", false, None)]);
        answer(&mut g, SearchKind::Symbols, vec![hit(SearchKind::Symbols, Some("rust:a::step"), "step", true, None)]);
        let (list, waiting) = g.find_choices();
        assert!(!waiting);
        assert_eq!(list.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["step", "src/a.rs", "// step by step"], "symbols, files, lines");
        // A search cut at its limit or its time says so on its last hit.
        g.found(g.find.asked, SearchKind::Text, Ok(Found { hits: vec![hit(SearchKind::Text, None, "// step by step", false, None)], total: 40, partial: true, ..Found::default() }));
        let line = g.find_choices().0.into_iter().find(|c| c.label == "// step by step").unwrap();
        let said = line.detail.unwrap_or_default();
        assert!(said.contains("1 of 40 lines") && said.contains("time limit"), "{said}");

        // A function, in the calls view: the root there.
        g.command(crate::graph::actions::GraphCommand::Find, Some(value_of(&g, "step")));
        assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::Calls, Some("rust:a::step")));

        // A file's module: the calls view can't show it, so the boxes do.
        let file = value_of(&g, "src/a.rs");
        g.command(crate::graph::actions::GraphCommand::Find, Some(file));
        assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::Contains, Some("rust:a")));

        // A line nothing in the graph stands for opens in the code.
        g.command(crate::graph::actions::GraphCommand::Find, Some(value_of(&g, "// step by step")));
        let shown = crate::nav::Ask::Show { target: crate::nav::Target::Span { file: "src/a.rs".into(), start: 3, end: 3 }, in_: crate::app::BufferKind::Code };
        assert!(g.take_asks().contains(&shown), "it asks the code to show it");

        // Answers to an older query are dropped.
        let stale = g.find.asked;
        g.find_query("run");
        g.found(stale, SearchKind::Symbols, Ok(Found { hits: vec![hit(SearchKind::Symbols, Some("rust:a::step"), "step", true, None)], ..Found::default() }));
        assert!(g.find_choices().0.is_empty());
        // So is a pick from an older list.
        g.command(crate::graph::actions::GraphCommand::Find, Some(format!("{stale}:symbols:0")));
        assert_eq!(g.nav.root.as_deref(), Some("rust:a"));
    }

    #[test]
    fn a_data_type_found_from_the_data_flow_view_is_its_root_there() {
        let mut g = showing(sample_tree());
        g.command(crate::graph::actions::GraphCommand::SetView, Some("data_flow".into()));
        g.find_query("Order");
        answer(&mut g, SearchKind::Symbols, vec![hit(SearchKind::Symbols, Some("rust:a::Order"), "Order", false, Some("type:Order"))]);
        g.command(crate::graph::actions::GraphCommand::Find, Some(value_of(&g, "Order")));
        assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::DataFlow, Some("type:Order")));
    }
}
