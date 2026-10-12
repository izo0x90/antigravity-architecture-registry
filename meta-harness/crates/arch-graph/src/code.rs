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

/// One file as the analysis has it: its text, and what the analysis says
/// of that same text (by its content hash).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CodeAnalysis {
    pub file: String,
    /// The text, as read for this answer.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub content_hash: String,
    /// The text isn't what was analysed (changed since, or never read):
    /// nothing below describes it.
    pub stale: bool,
    pub highlights: Highlights,
    pub occurrences: Vec<Occurrence>,
    /// Every component the occurrences name.
    pub targets: Vec<CodeTarget>,
    /// The file's components and the relations leaving them, for the
    /// gutter (kept even when stale: they say where things were).
    #[serde(default)]
    pub view: crate::FileView,
}

/// A use of a component in the code: where, and how (a call, a type named,
/// an import, a path), with how it's known.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub file: String,
    /// 1-based.
    pub line: u32,
    pub start: u32,
    pub end: u32,
    pub kind: String,
    /// The component it's in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<crate::ComponentId>,
    /// `name` (matched by name) or `server` (confirmed by the language's server).
    pub evidence: String,
    /// The file changed since it was read: where the use is now isn't known
    /// (its line is 0) until it's read again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}
