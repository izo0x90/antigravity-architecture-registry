//! Plans: the only persisted part of the architecture. A plan lays new or
//! changed components over the code, references existing components it builds
//! on, and records who approved what.

use serde::{Deserialize, Serialize};

use crate::{Component, ComponentId, ModelError, Relation, RelationKind, Stage};

/// Major version of persisted documents. Readers reject other majors.
pub const SCHEMA_VERSION: u32 = 1;

pub fn check_schema_version(found: u32, what: &str) -> Result<(), ModelError> {
    if found == SCHEMA_VERSION { Ok(()) } else { Err(ModelError::SchemaVersion { what: what.to_owned(), found, supported: SCHEMA_VERSION }) }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlanId(pub String);

impl PlanId {
    /// Plan ids become file names: letters, digits, `-` and `_` only.
    pub fn new(id: &str) -> Result<Self, ModelError> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(ModelError::InvalidId(id.to_owned()));
        }
        Ok(Self(id.to_owned()))
    }
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An existing component the plan builds on, optionally with its neighborhood.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub id: ComponentId,
    #[serde(default)]
    pub kinds: Vec<RelationKind>,
    #[serde(default)]
    pub depth: u32,
}

/// What an entry in the approval log records.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    /// The user approved the move.
    #[default]
    Approved,
    /// NAH took a stage back (the design or what it builds on changed, or its
    /// code stopped holding up): earlier approvals past `to` no longer count.
    Revoked,
}

impl ApprovalKind {
    pub fn is_approved(&self) -> bool {
        *self == ApprovalKind::Approved
    }
}

/// Who records revocations in the approval log.
pub const REVOKED_BY: &str = "nah";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub component: ComponentId,
    pub from: Stage,
    pub to: Stage,
    /// Who approved (a user, never an agent), or NAH for a revocation.
    pub by: String,
    /// RFC 3339 timestamp.
    pub at: String,
    #[serde(default, skip_serializing_if = "ApprovalKind::is_approved")]
    pub kind: ApprovalKind,
}

/// A named end-to-end behavior: the call chain from `entry`, derived from
/// ordered Calls relations (planned and observed), never stored as a tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    pub entry: ComponentId,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub schema_version: u32,
    pub id: PlanId,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// A plan with nothing to integrate with in the existing code.
    #[serde(default)]
    pub greenfield: bool,
    /// New components and planned changes to existing ones.
    #[serde(default)]
    pub components: Vec<Component>,
    /// Existing components this plan builds on (kept as they are in code).
    #[serde(default)]
    pub references: Vec<Reference>,
    #[serde(default)]
    pub relations: Vec<Relation>,
    #[serde(default)]
    pub approvals: Vec<Approval>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<Workflow>,
    /// Review comments and change requests, with their answers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<crate::Comment>,
    /// The plan as the user last reviewed it (what the review diff is against).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_review: Option<crate::ReviewSnapshot>,
}

impl Plan {
    pub fn new(id: &str, title: &str) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: PlanId(id.to_owned()),
            title: title.to_owned(),
            description: String::new(),
            greenfield: false,
            components: Vec::new(),
            references: Vec::new(),
            relations: Vec::new(),
            approvals: Vec::new(),
            workflows: Vec::new(),
            comments: Vec::new(),
            last_review: None,
        }
    }

    /// Parses a persisted plan, rejecting other schema majors.
    pub fn from_json(text: &str) -> Result<Self, ModelError> {
        #[derive(Deserialize)]
        struct Header {
            schema_version: u32,
        }
        let header: Header = serde_json::from_str(text).map_err(|e| ModelError::Format(e.to_string()))?;
        check_schema_version(header.schema_version, "plan")?;
        serde_json::from_str(text).map_err(|e| ModelError::Format(e.to_string()))
    }

    pub fn to_json(&self) -> Result<String, ModelError> {
        serde_json::to_string_pretty(self).map_err(|e| ModelError::Format(e.to_string()))
    }

    /// Records that `id` was taken back from `from` to `to`.
    pub fn revoke(&mut self, id: &ComponentId, from: Stage, to: Stage, at: &str) {
        self.approvals.push(Approval { component: id.clone(), from, to, by: REVOKED_BY.to_owned(), at: at.to_owned(), kind: ApprovalKind::Revoked });
    }

    /// The user's plan approval is the latest word on `id`: nothing took it
    /// back since.
    pub fn plan_approved(&self, id: &ComponentId) -> bool {
        self.approvals.iter().rev().find(|a| &a.component == id).is_some_and(|a| a.kind == ApprovalKind::Approved && a.to == Stage::PlanApproved)
    }

    pub fn component(&self, id: &ComponentId) -> Option<&Component> {
        self.components.iter().find(|c| &c.id == id)
    }

    pub fn component_mut(&mut self, id: &ComponentId) -> Option<&mut Component> {
        self.components.iter_mut().find(|c| &c.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/golden/plan.json");

    // req: P2.10
    #[test]
    fn plans_round_trip_through_the_golden_file() {
        let plan = Plan::from_json(GOLDEN).unwrap();
        assert_eq!(plan.schema_version, SCHEMA_VERSION);
        assert_eq!(plan.components.len(), 2);
        assert_eq!(plan.relations.len(), 1);
        assert_eq!(plan.approvals[0].by, "user");
        let again = Plan::from_json(&plan.to_json().unwrap()).unwrap();
        assert_eq!(again, plan);
        let golden: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
        let written: serde_json::Value = serde_json::from_str(&plan.to_json().unwrap()).unwrap();
        assert_eq!(written, golden, "the written form is exactly the golden file");
    }

    // req: P2.10
    #[test]
    fn other_schema_majors_and_bad_ids_are_rejected() {
        let future = GOLDEN.replacen("\"schema_version\": 1", "\"schema_version\": 2", 1);
        assert!(matches!(Plan::from_json(&future), Err(ModelError::SchemaVersion { found: 2, .. })));
        assert!(matches!(Plan::from_json("{}"), Err(ModelError::Format(_))));
        assert!(PlanId::new("add-validation_2").is_ok());
        for bad in ["", "../x", "a b", "a/b"] {
            assert!(PlanId::new(bad).is_err(), "{bad}");
        }
    }
}
