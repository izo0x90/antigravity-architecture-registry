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


use arch_search::{Hit, SearchKind};

use super::{GraphState, NavChange};
use crate::actions::{Action, ActionProvider, Choice, Group, MINOR, PRIMARY, Param, RunCx, SECONDARY};

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
        let mut out = self.moves();
        let f = |id: &str, label: &str, detail: &str, c: GraphCommand| Action::new(id, label, detail, c).focused();
        let views = ViewKind::ALL
            .iter()
            .map(|v| {
                let detail = self.config.as_ref().map(|c| c.view(*v).description.clone());
                Choice::new(v.name(), v.name().replace('_', " ")).detail(detail).current(*v == self.nav.view)
            })
            .collect();
        out.push(f("graph.view", "Switch view (contains, calls, data flow)", "What the graph draws: what holds what, what calls what, or where data goes", G::SetView).param(Param::Choice(views)).setting("View", Some(egui_phosphor::regular::GRAPH)).group(Group::View).jump());
        out.push(f("graph.plan", "Show the code, or one plan", "The code as it is, or a plan's components (built or not); every view reads from it", G::SetPlan).param(Param::Choice(self.source_choices())).setting("Source", None).group(Group::View).jump());
        if self.nav.source != Source::Code {
            out.push(f("graph.full_code", "Show the plan in the full code", "The plan's components among all the code's, not on their own", G::FullCode).param(Param::Toggle { on: self.nav.full_code }).setting("Full code", None).group(Group::View).jump());
        }
        if self.nav.only_changed || !self.changed.is_empty() {
            out.push(f("graph.only_changed", "Show only what the changes touch", "Only the components the comparison changed (and what holds them)", G::OnlyChanged).param(Param::Toggle { on: self.nav.only_changed }).setting("Only changed", None).group(Group::View).importance(MINOR).jump());
        }
        out.push(f("graph.flip", self.flip_label(), "The other way round: what leads to it instead of what it leads to", G::Flip).group(Group::View).importance(SECONDARY).jump());
        out.push(f("graph.depth_less", "Show fewer levels", "One level less below the root", G::Depth(-1)).group(Group::View).importance(MINOR).jump());
        out.push(f("graph.depth_more", "Show more levels", "One level more below the root", G::Depth(1)).group(Group::View).importance(MINOR).jump());
        if let Some(card) = self.selected_card()
            && self.nav.root.as_deref() != Some(card.key.as_str())
        {
            out.push(f("graph.go_in", &format!("Make {} the root", card.label), "The graph starts from it (the path back is kept)", G::GoIn).icon(egui_phosphor::regular::ARROW_SQUARE_IN).group(Group::Navigate).importance(SECONDARY).jump());
        }
        out.push(f("graph.fit", "Fit the whole tree to the screen", "Zooms and pans so all of it shows", G::Fit).group(Group::View).importance(MINOR));
        let fit_card = f("graph.fit_card", "Fit the selected card to the screen", "Zooms and pans so the card fills the panel", G::FitCard).group(Group::View).importance(MINOR);
        out.push(if self.selection.is_some() { fit_card } else { fit_card.when(crate::actions::When::Unavailable) });
        out.push(f("graph.zoom_in", "Zoom in", "Closer, around the middle of the panel", G::Zoom(self.ui.zoom_step)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_PLUS).group(Group::View).importance(MINOR));
        out.push(f("graph.zoom_out", "Zoom out", "Further, around the middle of the panel", G::Zoom(1.0 / self.ui.zoom_step)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_MINUS).group(Group::View).importance(MINOR));
        let (found, of) = self.find_choices();
        let find = Param::Search { hint: "a name, a file, or words in a line".into(), found, of };
        out.push(f("graph.find", "Find in the code (symbols, files, lines)", "As you type; picking one makes it the root", G::Find).param(find).group(Group::Navigate).importance(SECONDARY).jump());
        // Too big to show whole: only the levels that fit.
        if let Some(cut) = self.tree.as_ref().and_then(|t| t.cut.as_ref()).filter(|c| c.shown < c.asked && c.shown > 0) {
            let delta = cut.shown as i32 - self.depth() as i32;
            let fit = f("graph.fit_levels", &format!("Show only the {} levels that fit", cut.shown), "Too big to show whole: as many levels as the server lays out", G::Depth(delta)).group(Group::View).importance(SECONDARY).jump();
            // Under the size note while you're on it.
            let on_note = crate::hosted::Hosted::here(self).is_some();
            out.push(if on_note { fit.on_item(crate::actions::Style::Button, format!("Show {} levels", cut.shown), None) } else { fit });
        }
        if self.agent.is_some() {
            out.push(f("graph.agent_return", "Return to where you were before the agent moved the view", "The view as it was before the agent's navigate", G::AgentReturn).on_item(crate::actions::Style::Button, "Return", Some(egui_phosphor::regular::ARROW_U_UP_LEFT)).group(Group::Navigate).importance(PRIMARY).jump());
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
            out.push(f("graph.path", "Go back along the path", "The roots you went in through: pick one to go back to it", G::Path).param(Param::Choice(steps)).setting("Path", None).group(Group::Navigate).importance(MINOR).jump());
        }
        out.push(f("graph.legend", if self.legend_open { "Hide the legend" } else { "Show the legend" }, "What the graph's colours, shapes and badges mean", G::Legend).group(Group::View).importance(MINOR));
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, _cx: &mut RunCx<'_>) {
        if let Some(command) = action.command::<GraphCommand>().cloned() {
            self.command(command, value);
        }
    }


    fn search(&mut self, action: &str, query: &str) {
        if action == "graph.find" {
            query.clone_into(&mut self.find.query);
        }
    }

    fn search_done(&mut self, action: &str) {
        if action == "graph.find" {
            self.find = Find::default();
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

/// The search being typed, and what came back for it (as every code
/// search: `CodeSearch`); gone once its picker closes.
pub(crate) struct Find {
    pub(super) search: crate::code_search::CodeSearch,
    pub(super) query: String,
}

impl Default for Find {
    fn default() -> Self {
        Self { search: crate::code_search::CodeSearch::new(&SearchKind::ALL), query: String::new() }
    }
}

impl GraphState {
    /// The palette's list (symbols, then files, then lines), each row the
    /// hit it stands for, and the query it's of.
    pub(crate) fn find_choices(&self) -> (Vec<Choice>, Option<String>) {
        let rows = self.find.search.rows(|_, hit| serde_json::to_string(hit).ok());
        (rows, self.find.search.rows_of().map(str::to_owned))
    }

    /// Goes to a picked result (`value` from `find_choices`: its hit).
    pub(crate) fn pick_found(&mut self, value: &str) {
        if let Ok(hit) = serde_json::from_str::<Hit>(value) {
            self.go_to_hit(&hit);
        }
    }

    fn go_to_hit(&mut self, hit: &Hit) {
        let Some(key) = hit.key.clone() else {
            // Nothing in the graph stands for it (a file no pack reads): show the code.
            if let Some(file) = &hit.file {
                let line = hit.line.unwrap_or(1);
                let line = line as usize;
                self.asks.push(crate::nav::Ask::Show { target: crate::nav::Target::Span { file: file.clone(), start: line, end: line }, in_: Some(crate::kind::BufferKind::Code) });
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
    use crate::net::NetClient;
    use arch_search::Found;

    fn hit(kind: SearchKind, key: Option<&str>, label: &str, callable: bool, data: Option<&str>) -> Hit {
        Hit { kind, key: key.map(Into::into), label: label.into(), detail: "d".into(), file: Some("src/a.rs".into()), line: Some(3), callable, data: data.map(Into::into) }
    }

    /// The graph's find, typed and driven as the palette does, answered
    /// as the server would.
    struct Finding {
        server: NetClient,
        ctx: egui::Context,
    }

    impl Finding {
        fn new() -> Self {
            Self { server: NetClient::new("http://test"), ctx: egui::Context::default() }
        }

        /// `query` typed in the find (its round sent, what came taken).
        fn typed(&self, g: &mut GraphState, query: &str) {
            g.search("graph.find", query);
            g.tend_find(&self.server, &self.ctx);
        }

        /// The server's answer for `kind` to the latest round of `query`.
        fn answer(&self, query: &str, kind: SearchKind, found: Found) {
            let asked = format!("GET /api/search?q={}&kind={}&", urlencoding::encode(query), kind.name());
            let call = self.server.sent.lock().unwrap().iter().rev().find(|c| c.starts_with(&asked)).cloned().unwrap_or_else(|| panic!("{asked}"));
            assert!(self.server.answer(&call, 200, &serde_json::to_value(found).unwrap()), "{call} waits");
        }

        fn hits(&self, query: &str, kind: SearchKind, hits: Vec<Hit>) {
            self.answer(query, kind, Found { total: hits.len(), hits, ..Found::default() });
        }
    }

    /// "step" searched and answered (a symbol, a file, a line), taken.
    fn search_step(g: &mut GraphState, f: &Finding) {
        f.typed(g, "step");
        f.answer("step", SearchKind::Text, Found { hits: vec![hit(SearchKind::Text, None, "// step by step", false, None)], total: 40, partial: true, ..Found::default() });
        f.hits("step", SearchKind::Files, vec![hit(SearchKind::Files, Some("rust:a"), "src/a.rs", false, None)]);
        f.hits("step", SearchKind::Symbols, vec![hit(SearchKind::Symbols, Some("rust:a::step"), "step", true, None)]);
        g.tend_find(&f.server, &f.ctx);
    }

    fn value_of(g: &GraphState, label: &str) -> String {
        g.find_choices().0.into_iter().find(|c| c.label == label).unwrap().value
    }

    #[test]
    fn finding_lists_symbols_files_and_lines_and_picking_one_makes_it_the_root() {
        let (mut g, f) = (showing(sample_tree()), Finding::new());
        assert_eq!(g.nav.view, ViewKind::Calls);
        f.typed(&mut g, "step");
        assert_eq!(g.find_choices(), (Vec::new(), None), "nothing answered yet");
        search_step(&mut g, &f);
        let (list, of) = g.find_choices();
        assert_eq!(of.as_deref(), Some("step"));
        assert_eq!(list.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["step", "src/a.rs", "// step by step"], "symbols, files, lines");
        // A search cut at its limit or its time says so on its last hit.
        let said = list[2].detail.clone().unwrap_or_default();
        assert!(said.contains("1 of 40 lines") && said.contains("time limit"), "{said}");

        // A function, in the calls view: the root there.
        g.command(crate::graph::actions::GraphCommand::Find, Some(value_of(&g, "step")));
        assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::Calls, Some("rust:a::step")));

        // A file's module: the calls view can't show it, so the boxes do.
        g.command(crate::graph::actions::GraphCommand::Find, Some(value_of(&g, "src/a.rs")));
        assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::Contains, Some("rust:a")));

        // A line nothing in the graph stands for opens in the code.
        g.command(crate::graph::actions::GraphCommand::Find, Some(value_of(&g, "// step by step")));
        let shown = crate::nav::Ask::Show { target: crate::nav::Target::Span { file: "src/a.rs".into(), start: 3, end: 3 }, in_: Some(crate::kind::BufferKind::Code) };
        assert!(g.take_asks().contains(&shown), "it asks the code to show it");

        // A row picked from rows since replaced goes where it stands for.
        let step = value_of(&g, "step");
        f.typed(&mut g, "run");
        for kind in SearchKind::ALL {
            f.hits("run", kind, Vec::new());
        }
        g.tend_find(&f.server, &f.ctx);
        assert_eq!(g.find_choices(), (Vec::new(), Some("run".to_owned())));
        g.command(crate::graph::actions::GraphCommand::Find, Some(step));
        assert_eq!(g.nav.root.as_deref(), Some("rust:a::step"), "a row carries what it stands for");

        // Its picker left: nothing kept for the next find; what comes for
        // a round out then is dropped.
        f.typed(&mut g, "save");
        g.search_done("graph.find");
        f.hits("save", SearchKind::Symbols, vec![hit(SearchKind::Symbols, Some("rust:a::save"), "save", true, None)]);
        f.hits("save", SearchKind::Files, Vec::new());
        f.hits("save", SearchKind::Text, Vec::new());
        g.tend_find(&f.server, &f.ctx);
        assert!(g.find_choices().0.is_empty(), "the find starts afresh");
    }

    #[test]
    fn a_data_type_found_from_the_data_flow_view_is_its_root_there() {
        let (mut g, f) = (showing(sample_tree()), Finding::new());
        g.command(crate::graph::actions::GraphCommand::SetView, Some("data_flow".into()));
        f.typed(&mut g, "Order");
        f.hits("Order", SearchKind::Symbols, vec![hit(SearchKind::Symbols, Some("rust:a::Order"), "Order", false, Some("type:Order"))]);
        f.hits("Order", SearchKind::Files, Vec::new());
        f.hits("Order", SearchKind::Text, Vec::new());
        g.tend_find(&f.server, &f.ctx);
        g.command(crate::graph::actions::GraphCommand::Find, Some(value_of(&g, "Order")));
        assert_eq!((g.nav.view, g.nav.root.as_deref()), (ViewKind::DataFlow, Some("type:Order")));
    }
}
