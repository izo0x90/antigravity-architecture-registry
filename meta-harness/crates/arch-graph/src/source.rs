//! What a read shows: the code alone, or the code with one plan laid over
//! it. There is no "every plan at once": plans are seen one at a time.

use serde::{Deserialize, Serialize};

use crate::PlanId;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// What's in the code, and nothing planned.
    #[default]
    Code,
    /// The code with this plan laid over it.
    Plan(PlanId),
}

impl Source {
    /// A plan when one is named, else the code.
    pub fn from_plan(plan: Option<PlanId>) -> Self {
        plan.map_or(Source::Code, Source::Plan)
    }

    pub fn plan(&self) -> Option<&PlanId> {
        match self {
            Source::Code => None,
            Source::Plan(id) => Some(id),
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Code => f.write_str("code"),
            Source::Plan(id) => write!(f, "plan {id}"),
        }
    }
}
