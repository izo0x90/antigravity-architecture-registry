//! What the server sends the UI (and agents) about components and files.

use serde::{Deserialize, Serialize};

use crate::{Alignment, Component, Relation};

/// One component with its relations and source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComponentDetail {
    pub component: Component,
    pub alignment: Alignment,
    pub outgoing: Vec<Relation>,
    pub incoming: Vec<Relation>,
    /// The component's source text (from its anchor), if it's in the code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Its file changed on disk since it was indexed.
    #[serde(default)]
    pub stale: bool,
    /// Every use of it in the code, across files (from the refs that link to it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<crate::code::Reference>,
}

/// The components in one file and the relations leaving them, each with how
/// plan and code line up (for the code view's gutter).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FileView {
    pub file: String,
    pub components: Vec<(Component, Alignment)>,
    pub relations: Vec<(Relation, Alignment)>,
    /// The file changed on disk since it was indexed (what's shown is from
    /// the older version).
    #[serde(default)]
    pub stale: bool,
}
