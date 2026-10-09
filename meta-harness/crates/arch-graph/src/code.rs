//! What the analysis says about one file of code, for the code viewer:
//! its colours (highlight runs) and what each named place refers to
//! (occurrences), as kept with the analysis.

use serde::{Deserialize, Serialize};

use crate::ComponentId;

/// Coloured runs over a file: `[start byte, end byte, index into names]`,
/// `names` being highlight capture names (`keyword`, `function.method`...).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Highlights {
    pub names: Vec<String>,
    pub runs: Vec<[u32; 3]>,
}

impl Highlights {
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

/// One place in a file that names a component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Occurrence {
    pub start: u32,
    pub end: u32,
    /// The component it names, when it's one of ours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ComponentId>,
    /// The indexer's symbol, when an indexer said what it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// It's where the component is defined.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub definition: bool,
}

/// What an occurrence's target is, to show without asking again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeTarget {
    pub id: ComponentId,
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    /// Where it's defined (file, first line), when it's in the code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<(String, u32)>,
    /// Its signature, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// One file as the analysis has it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAnalysis {
    pub file: String,
    /// The file changed since it was analysed: nothing below describes it.
    pub stale: bool,
    pub highlights: Highlights,
    pub occurrences: Vec<Occurrence>,
    /// Every component the occurrences name.
    pub targets: Vec<CodeTarget>,
}
