//! Validation reports: one finding type for plan checks, implementation
//! checks, tools, REST and UI.

use serde::{Deserialize, Serialize};

use crate::{ComponentId, PlanId, RelationId, RepoPath, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// What a finding is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Plan { plan: PlanId },
    Component { id: ComponentId },
    Relation { id: RelationId },
}

/// Stable finding codes. Agents and the UI key off these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingCode {
    UnknownType,
    DanglingReference,
    InvalidLocation,
    LocationRule,
    NameCollision,
    MissingTouchPoint,
    MissingSymbol,
    Moved,
    MissingCallSite,
    WrongCallTarget,
    SignatureMismatch,
    PlaceholderRemains,
    UnresolvedCall,
    IllegalTransition,
    ApprovalRequired,
    /// A code rule's forbidden pattern appears in the component.
    ForbiddenPattern,
    /// A code rule's required pattern is missing from the component.
    MissingRequiredPattern,
    ToolFailed,
    UnknownTool,
    ReviewFinding,
    ReviewerUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeRef {
    pub file: RepoPath,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub code: FindingCode,
    pub severity: Severity,
    pub target: Target,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<CodeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

impl Finding {
    pub fn error(code: FindingCode, target: Target, message: impl Into<String>) -> Self {
        Self { code, severity: Severity::Error, target, message: message.into(), at: None, suggestion: None }
    }

    pub fn warning(code: FindingCode, target: Target, message: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, ..Self::error(code, target, message) }
    }

    pub fn at(mut self, file: RepoPath, span: Span) -> Self {
        self.at = Some(CodeRef { file, span });
        self
    }

    pub fn suggest(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    pub fn component(&self) -> Option<&ComponentId> {
        match &self.target {
            Target::Component { id } => Some(id),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn push(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    pub fn extend(&mut self, other: Report) {
        self.findings.extend(other.findings);
    }

    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    pub fn errors_for<'a>(&'a self, id: &'a ComponentId) -> impl Iterator<Item = &'a Finding> {
        self.findings.iter().filter(move |f| f.severity == Severity::Error && f.component() == Some(id))
    }

    pub fn codes(&self) -> Vec<FindingCode> {
        self.findings.iter().map(|f| f.code).collect()
    }

    /// Stable order: errors first, then by target and code.
    pub fn sorted(mut self) -> Self {
        self.findings.sort_by(|a, b| {
            b.severity.cmp(&a.severity).then_with(|| format!("{:?}", a.target).cmp(&format!("{:?}", b.target))).then(a.code.cmp(&b.code))
        });
        self
    }
}
