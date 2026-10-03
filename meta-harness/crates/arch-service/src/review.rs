//! Reviewers: per-category checks NAH runs on a component before it counts as
//! implemented. Defined as data (`config/reviewers.toml`, extended by
//! `.nah/reviewers.toml`). Rule reviewers apply rules over the graph; session
//! reviewers hand a brief, the contract and the code to a reviewing agent
//! through a [`ReviewProvider`] and report what it finds.

use std::collections::{BTreeMap, BTreeSet};

use arch_graph::{Component, ComponentId, Finding, FindingCode, Graph, Plan, PlanId, RelationKind, RepoPath, Report, Severity, Span, Status, Target, TypeSystem};
use serde::{Deserialize, Serialize};

use crate::ServiceError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerKind {
    Rule,
    Session,
}

/// The checks a rule reviewer can apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
pub enum Rule {
    /// The component's code is longer than `limit` lines.
    MaxLines { limit: u32 },
    /// The component calls more than `limit` other components.
    MaxCalls { limit: usize },
    /// A new component calls code the plan doesn't mention.
    UnplannedCalls,
    /// Placeholder bodies are left.
    NoPlaceholders,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewerDef {
    pub category: String,
    #[serde(default)]
    pub description: String,
    /// Types reviewed (by lineage). Empty: every type.
    #[serde(default)]
    pub applies_to: Vec<String>,
    pub kind: ReviewerKind,
    /// Rule reviewers: their findings block. Session reviewers: whether
    /// failing to run blocks (their own findings say if they block).
    #[serde(default)]
    pub blocking: bool,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Session reviewers: what the reviewing agent is asked to look for.
    #[serde(default)]
    pub brief: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewerCatalog {
    #[serde(default)]
    pub reviewers: BTreeMap<String, ReviewerDef>,
}

const DEFAULTS: &str = include_str!("../config/reviewers.toml");

impl ReviewerCatalog {
    pub fn defaults() -> Result<Self, ServiceError> {
        Self::from_toml(DEFAULTS)
    }

    pub fn from_toml(text: &str) -> Result<Self, ServiceError> {
        let catalog: Self = toml::from_str(text).map_err(|e| ServiceError::InvalidOp(format!("reviewers: {e}")))?;
        for (name, r) in &catalog.reviewers {
            match r.kind {
                ReviewerKind::Rule if r.rules.is_empty() => return Err(ServiceError::InvalidOp(format!("rule reviewer '{name}' has no rules"))),
                ReviewerKind::Session if r.brief.trim().is_empty() => return Err(ServiceError::InvalidOp(format!("session reviewer '{name}' has no brief"))),
                _ => {}
            }
        }
        Ok(catalog)
    }

    /// Adds or replaces reviewers by name.
    pub fn extend_with(&mut self, other: ReviewerCatalog) {
        self.reviewers.extend(other.reviewers);
    }
}

/// What a session reviewer gets to look at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRequest {
    pub reviewer: String,
    pub category: String,
    pub brief: String,
    pub plan: PlanId,
    pub component: ComponentId,
    /// The component's contract (markdown).
    pub contract: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<RepoPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// One point a reviewing agent raises.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewNote {
    pub message: String,
    #[serde(default)]
    pub blocking: bool,
    /// Line in the component's file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// Runs session reviews (a reviewing agent on some harness).
pub trait ReviewProvider: Send + Sync {
    fn review(&self, request: &ReviewRequest) -> Result<Vec<ReviewNote>, String>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRun {
    pub reviewer: String,
    pub category: String,
    pub kind: ReviewerKind,
    pub ran: bool,
    pub findings: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewsReport {
    pub runs: Vec<ReviewRun>,
    pub report: Report,
}

/// The component under review: as merged with its plan, with the code.
pub struct Subject<'a> {
    pub plan: &'a Plan,
    pub component: &'a Component,
    pub observed: &'a Graph,
    pub contract: String,
    pub code: Option<String>,
}

fn applies(def: &ReviewerDef, lineage: &[String]) -> bool {
    def.applies_to.is_empty() || def.applies_to.iter().any(|t| lineage.contains(t))
}

fn rule_findings(rule: &Rule, s: &Subject) -> Vec<String> {
    let c = s.component;
    let calls: Vec<&ComponentId> = s.observed.outgoing(&c.id, Some(RelationKind::Calls)).map(|r| &r.target).collect();
    match rule {
        Rule::MaxLines { limit } => {
            let lines = c.anchor.as_ref().map(|a| a.span.end_line.saturating_sub(a.span.start_line) + 1).unwrap_or(0);
            if lines > *limit { vec![format!("{lines} lines long (limit {limit}); split it")] } else { Vec::new() }
        }
        Rule::MaxCalls { limit } => {
            let distinct: BTreeSet<&ComponentId> = calls.iter().copied().collect();
            if distinct.len() > *limit { vec![format!("calls {} components (limit {limit}); it does too much", distinct.len())] } else { Vec::new() }
        }
        Rule::UnplannedCalls if c.status == Status::New => {
            let planned: BTreeSet<&ComponentId> = s.plan.relations.iter().filter(|r| r.kind == RelationKind::Calls && r.source == c.id).map(|r| &r.target).collect();
            let extra: BTreeSet<&ComponentId> = calls.into_iter().filter(|t| !planned.contains(t)).collect();
            extra.into_iter().map(|t| format!("calls {t}, which the plan doesn't mention; plan it or remove it")).collect()
        }
        Rule::UnplannedCalls => Vec::new(),
        Rule::NoPlaceholders if !c.placeholders.is_empty() => vec![format!("{} placeholder bodies left", c.placeholders.len())],
        Rule::NoPlaceholders => Vec::new(),
    }
}

/// Runs every reviewer that applies to the subject's type.
pub fn review(catalog: &ReviewerCatalog, types: &TypeSystem, provider: Option<&dyn ReviewProvider>, s: &Subject) -> ReviewsReport {
    let mut out = ReviewsReport::default();
    let c = s.component;
    let target = || Target::Component { id: c.id.clone() };
    let lineage = types.resolve(&c.type_name).map(|t| t.lineage).unwrap_or_default();
    let file = c.anchor.as_ref().map(|a| a.file.clone());
    for (name, def) in catalog.reviewers.iter().filter(|(_, d)| applies(d, &lineage)) {
        let mut findings: Vec<Finding> = Vec::new();
        let mut ran = true;
        let tag = format!("[{}/{name}]", def.category);
        match def.kind {
            ReviewerKind::Rule => {
                let severity = if def.blocking { Severity::Error } else { Severity::Warning };
                for message in def.rules.iter().flat_map(|r| rule_findings(r, s)) {
                    let mut f = Finding { severity, ..Finding::error(FindingCode::ReviewFinding, target(), format!("{tag} {message}")) };
                    if let Some(a) = &c.anchor {
                        f = f.at(a.file.clone(), a.span);
                    }
                    findings.push(f);
                }
            }
            ReviewerKind::Session => {
                let request = ReviewRequest {
                    reviewer: name.clone(),
                    category: def.category.clone(),
                    brief: def.brief.clone(),
                    plan: s.plan.id.clone(),
                    component: c.id.clone(),
                    contract: s.contract.clone(),
                    file: file.clone(),
                    code: s.code.clone(),
                };
                let unavailable = |why: String| {
                    let severity = if def.blocking { Severity::Error } else { Severity::Warning };
                    Finding { severity, ..Finding::error(FindingCode::ReviewerUnavailable, target(), format!("{tag} didn't run: {why}")) }
                };
                match provider.map(|p| p.review(&request)) {
                    None => {
                        ran = false;
                        findings.push(unavailable("no review session provider is configured".to_owned()));
                    }
                    Some(Err(why)) => {
                        ran = false;
                        findings.push(unavailable(why));
                    }
                    Some(Ok(notes)) => {
                        for note in notes {
                            let severity = if note.blocking { Severity::Error } else { Severity::Warning };
                            let mut f = Finding { severity, ..Finding::error(FindingCode::ReviewFinding, target(), format!("{tag} {}", note.message)) };
                            if let (Some(line), Some(file)) = (note.line, &file) {
                                f = f.at(file.clone(), Span { start_line: line, end_line: line, start_byte: 0, end_byte: 0 });
                            }
                            findings.push(f);
                        }
                    }
                }
            }
        }
        out.runs.push(ReviewRun { reviewer: name.clone(), category: def.category.clone(), kind: def.kind, ran, findings: findings.len() });
        out.report.findings.extend(findings);
    }
    out
}
