//! What the server sends the UI (and agents) about views and components.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::view::Layout;
use crate::{Alignment, Component, ComponentId, Relation, View};

/// A view ready to draw: the subgraph, its layout, the shown components, and
/// the same view as text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewResult {
    pub view: View,
    pub layout: Layout,
    pub text: String,
    pub components: BTreeMap<ComponentId, Component>,
}

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
}

/// The components in one file and the relations leaving them, each with how
/// plan and code line up (for the code view's gutter).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileView {
    pub file: String,
    pub components: Vec<(Component, Alignment)>,
    pub relations: Vec<(Relation, Alignment)>,
}
