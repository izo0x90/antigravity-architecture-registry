//! The Code/plan picker and the "show in full code" switch: what the graph
//! shows the code with, the plans to pick from, and saying so when the plan
//! shown is gone.

use arch_graph::{PlanId, Source};

use super::{GraphState, NavChange};
use crate::actions::Choice;

impl GraphState {
    /// The sources to pick from: the code alone, and each plan.
    pub(crate) fn source_choices(&self) -> Vec<Choice> {
        let choice = |s: Source| {
            let current = s == self.nav.source;
            let label = s.plan().map_or_else(|| "code".to_owned(), |p| p.0.clone());
            Choice::new(String::from(s), &label).current(current)
        };
        let mut out = vec![choice(Source::Code)];
        out.extend(self.plans.iter().map(|p| choice(Source::Plan(p.clone()))));
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
        match (&self.nav.source, self.nav.full_code) {
            (Source::Plan(p), true) => format!("plan {p} in the full code"),
            (source, _) => source.to_string(),
        }
    }

    /// The plans there are: a plan whose tree wasn't found and isn't among
    /// them is gone, so the code shows instead.
    pub(crate) fn plans_arrived(&mut self, plans: Vec<PlanId>) {
        if let Some(plan) = self.plan_unsure.take()
            && self.nav.source.plan() == Some(&plan)
            && !plans.contains(&plan)
        {
            self.navigate(NavChange::Source(Source::Code));
            self.note = Some(format!("Plan {plan} isn't there any more: showing the code."));
        }
        self.plans = plans;
    }
}
