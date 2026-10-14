//! The Code/plan picker and the "show in full code" switch: what the graph
//! shows the code with, the plans and walkthroughs to pick from, and saying
//! so when the one shown is gone.

use arch_graph::{PlanId, PlanKind, Source};

use super::{GraphState, NavChange};
use crate::actions::Choice;

impl GraphState {
    /// The sources to pick from: the code alone, then each plan, then each
    /// walkthrough (named as one).
    pub(crate) fn source_choices(&self) -> Vec<Choice> {
        let choice = |s: Source, label: &str| {
            let current = s == self.nav.source;
            Choice::new(String::from(s), label).current(current)
        };
        let mut out = vec![choice(Source::Code, "code")];
        for kind in PlanKind::ALL {
            for (plan, _) in self.plans.iter().filter(|(_, k)| *k == kind) {
                let label = match kind {
                    PlanKind::Change => plan.0.clone(),
                    PlanKind::Walkthrough => format!("{} {plan}", kind.name()),
                };
                out.push(choice(Source::Plan(plan.clone()), &label));
            }
        }
        out
    }

    /// Shows the source `value` names (`code` or `plan:<id>`).
    pub(crate) fn pick_source(&mut self, value: Option<String>) {
        if let Some(source) = value.and_then(|v| v.parse::<Source>().ok()) {
            self.navigate(NavChange::Source(source));
        }
    }

    /// With a plan: in the whole codebase (`true`), only the plan and what
    /// it plugs into (`false`), or the other of the two.
    pub(crate) fn switch_full_code(&mut self, value: Option<String>) {
        let on = match value.as_deref() {
            Some("true") => true,
            Some("false") => false,
            _ => !self.nav.full_code,
        };
        self.navigate(NavChange::FullCode(on));
    }

    /// What the graph shows the code with, in words.
    pub(crate) fn source_label(&self) -> String {
        let kind = |p: &PlanId| self.plans.iter().find(|(id, _)| id == p).map_or(PlanKind::Change, |(_, k)| *k);
        match (&self.nav.source, self.nav.full_code) {
            (Source::Plan(p), true) => format!("{} {p} in the full code", word(kind(p))),
            (Source::Plan(p), false) => format!("{} {p}", word(kind(p))),
            (Source::Code, _) => Source::Code.to_string(),
        }
    }

    /// The plans there are: a plan whose tree wasn't found and isn't among
    /// them is gone, so the code shows instead.
    pub(crate) fn plans_arrived(&mut self, plans: Vec<(PlanId, PlanKind)>) {
        if let Some(plan) = self.plan_unsure.take()
            && self.nav.source.plan() == Some(&plan)
            && !plans.iter().any(|(id, _)| *id == plan)
        {
            self.navigate(NavChange::Source(Source::Code));
            self.note = Some(format!("Plan {plan} isn't there any more: showing the code."));
        }
        self.plans = plans;
    }
}

/// What a plan of this kind is called.
fn word(kind: PlanKind) -> &'static str {
    match kind {
        PlanKind::Change => "plan",
        PlanKind::Walkthrough => kind.name(),
    }
}
