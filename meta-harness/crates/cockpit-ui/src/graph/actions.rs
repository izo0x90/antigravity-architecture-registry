//! The graph's own actions. Selecting, opening and closing, going in and
//! back are the universal moves (`navigable`); these pick the view, the
//! source and the way, set the depth, and move the camera. Keys live in the
//! keymap, not here.

use arch_graph::tree::{ViewKind, Way};
use arch_graph::PlanId;

use super::{GraphState, NavChange};
use crate::actions::{Action, ActionProvider, Choice, Group, MINOR, PRIMARY, Param, RunCx, SECONDARY, ToolbarStyle};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GraphCommand {
    SetView,
    NextView,
    SetPlan,
    FullCode,
    Flip,
    Depth(i32),
    GoIn,
    Zoom(f32),
    Fit,
    Refresh,
    Legend,
    CloseInspector,
    AgentReturn,
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

    fn next_view(&self) -> ViewKind {
        let at = ViewKind::ALL.iter().position(|v| *v == self.nav.view).unwrap_or(0);
        ViewKind::ALL[(at + 1) % ViewKind::ALL.len()]
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

    /// Runs an action from a chip or the toolbar: the graph's own right here,
    /// unless it's a jump; jumps and the shell's actions go through the shell
    /// (so the jump list records them).
    pub(crate) fn use_action(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        if action.command::<GraphCommand>().is_some() && !action.jump {
            self.run(action, value, cx);
        } else {
            self.requests.run = Some((action.id.clone(), value));
        }
    }
}

impl ActionProvider for GraphState {
    fn actions(&self) -> Vec<Action> {
        use GraphCommand as G;
        use ToolbarStyle::{Button, Control};
        let mut out = Vec::new();
        let f = |id: &str, label: &str, c: GraphCommand| Action::new(id, label, c).focused();
        let views = ViewKind::ALL
            .iter()
            .map(|v| {
                let detail = self.config.as_ref().map(|c| c.view(*v).description.clone());
                Choice::new(v.name(), v.name().replace('_', " ")).detail(detail).current(*v == self.nav.view)
            })
            .collect();
        out.push(f("graph.view", "Switch view (contains, calls, data flow)", G::SetView).param(Param::Choice(views)).toolbar(Control, "View", Some(egui_phosphor::regular::GRAPH)).group(Group::View).jump());
        out.push(f("graph.next_view", &format!("Next view ({})", self.next_view().name().replace('_', " ")), G::NextView).icon(egui_phosphor::regular::SWAP).group(Group::View).importance(SECONDARY).jump());
        let plan = self.nav.plan.clone();
        let mut plans = vec![Choice::new("", "code").current(plan.is_none())];
        plans.extend(self.plans.iter().map(|p| Choice::new(&p.0, &p.0).current(plan.as_ref() == Some(p))));
        out.push(f("graph.plan", "Show the code, or one plan", G::SetPlan).param(Param::Choice(plans)).toolbar(Control, "Source", None).group(Group::View).jump());
        if plan.is_some() {
            out.push(f("graph.full_code", "Show the plan in the full code", G::FullCode).param(Param::Toggle { on: self.nav.full_code }).toolbar(Control, "Full code", None).group(Group::View).jump());
        }
        out.push(f("graph.flip", self.flip_label(), G::Flip).toolbar(Button, "", Some(egui_phosphor::regular::ARROWS_DOWN_UP)).group(Group::View).importance(SECONDARY).jump());
        out.push(f("graph.depth_less", "Show fewer levels", G::Depth(-1)).toolbar(Button, "", Some(egui_phosphor::regular::MINUS)).group(Group::View).importance(MINOR).jump());
        out.push(f("graph.depth_more", "Show more levels", G::Depth(1)).toolbar(Button, "", Some(egui_phosphor::regular::PLUS)).group(Group::View).importance(MINOR).jump());
        if let Some(card) = self.selected_card()
            && self.nav.root.as_deref() != Some(card.key.as_str())
        {
            out.push(f("graph.go_in", &format!("Go into {}", card.label), G::GoIn).icon(egui_phosphor::regular::ARROW_SQUARE_IN).group(Group::Navigate).importance(SECONDARY).jump());
        }
        out.push(f("graph.fit", "Fit to screen", G::Fit).toolbar(Button, "", Some(egui_phosphor::regular::CORNERS_OUT)).group(Group::View).importance(MINOR));
        out.push(f("graph.zoom_in", "Zoom in", G::Zoom(1.2)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_PLUS).group(Group::View).importance(MINOR));
        out.push(f("graph.zoom_out", "Zoom out", G::Zoom(1.0 / 1.2)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_MINUS).group(Group::View).importance(MINOR));
        out.push(f("graph.refresh", "Refresh the view", G::Refresh).group(Group::View).importance(MINOR));
        out.push(f("graph.legend", if self.legend_open { "Hide the legend" } else { "Show the legend" }, G::Legend).toolbar(Button, "", Some(egui_phosphor::regular::INFO)).group(Group::View).importance(MINOR));
        if self.selection.is_some() {
            out.push(f("graph.close_inspector", "Close the details", G::CloseInspector).group(Group::View).importance(MINOR));
        }
        if self.agent.is_some() {
            out.push(f("graph.agent_return", "Return to where you were before the agent moved the view", G::AgentReturn).group(Group::Navigate).importance(PRIMARY).jump());
        }
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, _cx: &mut RunCx<'_>) {
        if let Some(command) = action.command::<GraphCommand>().cloned() {
            self.command(command, value);
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
            GraphCommand::NextView => {
                let view = self.next_view();
                let root = self.root_for(view);
                self.navigate(NavChange::View { view, root });
            }
            GraphCommand::SetPlan => {
                let plan = value.filter(|v| !v.is_empty()).and_then(|v| PlanId::new(&v).ok());
                self.navigate(NavChange::Plan(plan));
            }
            GraphCommand::FullCode => {
                let on = match value.as_deref() {
                    Some("true") => true,
                    Some("false") => false,
                    _ => !self.nav.full_code,
                };
                self.navigate(NavChange::FullCode(on));
            }
            GraphCommand::Flip => self.navigate(NavChange::Flip),
            GraphCommand::Depth(delta) => {
                let base = self.depth();
                self.navigate(NavChange::Depth { delta, base });
            }
            GraphCommand::GoIn => {
                if let Some(key) = self.selection.clone() {
                    self.navigate(NavChange::GoIn(key));
                }
            }
            GraphCommand::Zoom(factor) => {
                let center = self.selected_card().map_or(self.canvas.center(), |c| self.to_screen(&c.rect).center());
                self.zoom_by(factor, center);
            }
            GraphCommand::Fit => self.fit_view(),
            GraphCommand::Refresh => self.invalidate(),
            GraphCommand::Legend => {
                self.legend_open = match value.as_deref() {
                    Some("true") => true,
                    Some("false") => false,
                    _ => !self.legend_open,
                };
            }
            GraphCommand::CloseInspector => self.drawer_open = false,
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
