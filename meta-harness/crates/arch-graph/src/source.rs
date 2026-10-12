//! What a read shows: the code alone, or the code with one plan laid over
//! it. There is no "every plan at once": plans are seen one at a time.

use serde::{Deserialize, Serialize};

use crate::PlanId;

/// Written `code` or `plan:<id>`, wherever it's written (JSON, a query
/// string, an MCP argument).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Source {
    /// What's in the code, and nothing planned.
    #[default]
    Code,
    /// The code with this plan laid over it.
    Plan(PlanId),
}

impl Source {
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

/// The prefix that names a plan in a source's text.
const PLAN: &str = "plan:";

impl std::str::FromStr for Source {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.strip_prefix(PLAN) {
            Some(id) => PlanId::new(id).map(Source::Plan).map_err(|e| format!("source '{text}': {e}")),
            None if text == "code" => Ok(Source::Code),
            None => Err(format!("source '{text}': use \"code\" or \"{PLAN}<id>\"")),
        }
    }
}

impl TryFrom<String> for Source {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<Source> for String {
    fn from(source: Source) -> Self {
        match source {
            Source::Code => "code".to_owned(),
            Source::Plan(id) => format!("{PLAN}{id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_is_written_code_or_plan_colon_id_everywhere() {
        let plan = Source::Plan(PlanId::new("checkout").unwrap());
        assert_eq!(serde_json::to_string(&plan).unwrap(), "\"plan:checkout\"");
        assert_eq!(serde_json::to_string(&Source::Code).unwrap(), "\"code\"");
        assert_eq!(serde_json::from_str::<Source>("\"plan:checkout\"").unwrap(), plan);
        assert_eq!("code".parse::<Source>().unwrap(), Source::Code);
        assert!("checkout".parse::<Source>().unwrap_err().contains("use \"code\" or \"plan:<id>\""));
        assert!("plan:no way".parse::<Source>().is_err());
    }
}
