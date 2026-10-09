//! Components: the nodes of the graph, planned or found in code.

use serde::{Deserialize, Serialize};

use crate::{CodeAnchor, ModelError, ProposedLocation, Span, Stage, Status};

/// Stable component identity: `<language>:<symbol>` for anything that is (or
/// will be) in code, so a planned component and the code that implements it
/// share an id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ComponentId(pub String);

impl ComponentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn for_symbol(language: &str, symbol: &str) -> Self {
        Self(format!("{language}:{symbol}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ComponentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    /// The type as written in the language, if declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<String>,
    /// Callers may leave it out (a default value, an optional marker).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
    /// Takes any number of arguments (`*args`, `...rest`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub variadic: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    #[serde(default)]
    pub params: Vec<Param>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    /// Qualifiers that change how it's called or what it returns (e.g.
    /// async, unsafe, const), as written.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub qualifiers: Vec<String>,
}

impl Signature {
    /// As one line: `async name(a: A, b?: B) -> R` (the same shape for
    /// every language; types as written).
    pub fn written(&self, name: &str) -> String {
        let params: Vec<String> = self
            .params
            .iter()
            .map(|p| {
                let rest = if p.variadic { "..." } else { "" };
                let opt = if p.optional { "?" } else { "" };
                match &p.ty {
                    Some(ty) => format!("{rest}{}{opt}: {ty}", p.name),
                    None => format!("{rest}{}{opt}", p.name),
                }
            })
            .collect();
        let mut out = self.qualifiers.iter().map(|q| format!("{q} ")).collect::<String>();
        out.push_str(&format!("{name}({})", params.join(", ")));
        if let Some(r) = &self.returns {
            out.push_str(&format!(" -> {r}"));
        }
        out
    }
}

/// The shape of the data a component holds and takes/returns, as JSON
/// Schema (written in the shorthand DSL or as full schema), and what it
/// touches outside itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schemas {
    /// Fields/attributes (data types).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<serde_json::Value>,
    /// What it takes (an object schema, one property per parameter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<serde_json::Value>,
    /// What it returns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub side_effects: Vec<SideEffect>,
}

impl Schemas {
    pub fn is_empty(&self) -> bool {
        self == &Schemas::default()
    }
}

/// Something outside the component it changes (a tag like `db`, `fs`, `network`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideEffect {
    pub target: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicStep {
    pub sequence: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    /// A shell snippet that checks this step on its own (run from the repo
    /// root during validation; exit 0 = pass).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvariantKind {
    PreCondition,
    PostCondition,
    #[default]
    StateInvariant,
    Complexity,
}

impl InvariantKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::PreCondition => "pre_condition",
            Self::PostCondition => "post_condition",
            Self::StateInvariant => "state_invariant",
            Self::Complexity => "complexity",
        }
    }
}

impl std::fmt::Display for InvariantKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invariant {
    pub name: String,
    #[serde(default)]
    pub kind: InvariantKind,
    #[serde(default)]
    pub description: String,
}

/// A small piece of work attached to a spec (specs and invariants come first).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

/// A project validation tool bound to a component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRef {
    pub tool: String,
    #[serde(default)]
    pub targets: Vec<String>,
    /// Replaces the tool's default arguments entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}

/// How a component should work: the primary planning artifact.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplementationSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default)]
    pub logic_steps: Vec<LogicStep>,
    #[serde(default)]
    pub invariants: Vec<Invariant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<Task>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validation: Vec<ToolRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub id: ComponentId,
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default)]
    pub description: String,
    pub status: Status,
    pub stage: Stage,
    /// Where it is in the code (set by ingestion/verification only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<CodeAnchor>,
    /// Where it is meant to go (set by the plan).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<ProposedLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
    #[serde(default, skip_serializing_if = "Schemas::is_empty")]
    pub schemas: Schemas,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<ImplementationSpec>,
    /// Placeholder bodies found in its code (skeleton stage).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placeholders: Vec<Span>,
}

impl Component {
    /// The location rules:
    /// - anything in the code (existing, modifying, deprecated, implemented)
    ///   has a verified anchor;
    /// - plan-approved and skeleton components have a proposed location or
    ///   an anchor;
    /// - an implemented component has no placeholders left.
    pub fn check_location(&self) -> Result<(), ModelError> {
        let in_code = matches!(self.status, Status::Existing | Status::Modifying | Status::Deprecated) || self.stage == Stage::Implemented;
        if in_code && self.anchor.is_none() {
            return Err(ModelError::Location { id: self.id.clone(), reason: format!("{:?}/{:?} needs a verified code anchor", self.status, self.stage) });
        }
        if self.stage.needs_location() && self.anchor.is_none() && self.proposed.is_none() {
            return Err(ModelError::Location { id: self.id.clone(), reason: format!("{:?} needs a proposed location", self.stage) });
        }
        if self.stage == Stage::Implemented && !self.placeholders.is_empty() {
            return Err(ModelError::Location {
                id: self.id.clone(),
                reason: format!("implemented but {} placeholder bodies remain", self.placeholders.len()),
            });
        }
        Ok(())
    }

    /// The file it lives in, or will.
    pub fn file(&self) -> Option<&crate::RepoPath> {
        self.anchor.as_ref().map(|a| &a.file).or(self.proposed.as_ref().map(|p| &p.file))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::RepoPath;

    pub fn planned(id: &str, stage: Stage) -> Component {
        Component {
            id: ComponentId::new(id),
            name: id.split_once(':').map_or(id, |(_, symbol)| symbol).rsplit("::").next().unwrap_or(id).to_owned(),
            type_name: "function".into(),
            description: String::new(),
            status: Status::New,
            stage,
            anchor: None,
            proposed: None,
            signature: None,
            schemas: Schemas::default(),
            spec: None,
            placeholders: Vec::new(),
        }
    }

    pub fn anchor_for(symbol: &str) -> CodeAnchor {
        CodeAnchor {
            language: "rust".into(),
            file: RepoPath::new("src/lib.rs").unwrap(),
            symbol: symbol.into(),
            syntax_kind: "function_item".into(),
            span: Span { start_line: 1, end_line: 5, start_byte: 0, end_byte: 50 },
            content_hash: "h".into(),
        }
    }

    fn proposed() -> ProposedLocation {
        ProposedLocation { language: "rust".into(), file: RepoPath::new("src/lib.rs").unwrap(), parent: None, symbol: "app::run".into() }
    }

    // req: P2.4
    #[test]
    fn implemented_needs_a_verified_anchor() {
        let mut c = planned("rust:app::run", Stage::Implemented);
        c.proposed = Some(proposed());
        assert!(c.check_location().is_err(), "a proposed location isn't enough");
        c.anchor = Some(anchor_for("app::run"));
        assert!(c.check_location().is_ok());
        let mut existing = planned("rust:app::old", Stage::Implemented);
        existing.status = Status::Existing;
        existing.anchor = None;
        assert!(existing.check_location().is_err());
    }

    // req: P2.4
    #[test]
    fn approved_plans_and_skeletons_need_a_location() {
        for stage in [Stage::PlanApproved, Stage::Skeleton] {
            let mut c = planned("rust:app::run", stage);
            assert!(c.check_location().is_err(), "{stage:?} without location");
            c.proposed = Some(proposed());
            assert!(c.check_location().is_ok(), "{stage:?} with proposed location");
        }
        assert!(planned("rust:app::run", Stage::Declared).check_location().is_ok(), "declared may be unplaced");
    }

    // req: P2.4
    #[test]
    fn implemented_with_placeholders_is_rejected() {
        let mut c = planned("rust:app::run", Stage::Implemented);
        c.anchor = Some(anchor_for("app::run"));
        c.placeholders.push(Span { start_line: 2, end_line: 2, start_byte: 10, end_byte: 17 });
        assert!(c.check_location().is_err());
        c.stage = Stage::Skeleton;
        assert!(c.check_location().is_ok(), "placeholders are expected in a skeleton");
    }
}
