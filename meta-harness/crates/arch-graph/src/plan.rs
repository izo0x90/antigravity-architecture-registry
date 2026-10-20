//! Plans: the only persisted part of the architecture. A plan lays new or
//! changed components over the code, references existing components it builds
//! on, and records who approved what. A walkthrough is the same document the
//! other way round: it changes nothing, and explains existing code.

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

/// What a plan is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanKind {
    /// Changes to the code: new components and planned changes, approved
    /// and built through the lifecycle.
    #[default]
    Change,
    /// An explanation of code that exists: its components as they are in
    /// the code, described and specified from it. Nothing is added, changed,
    /// approved or built.
    Walkthrough,
    /// A list to go through: items only (findings, decisions, questions to
    /// settle), each with its state; no components.
    Review,
}

impl PlanKind {
    pub const ALL: [PlanKind; 3] = [PlanKind::Change, PlanKind::Walkthrough, PlanKind::Review];

    pub fn is_change(&self) -> bool {
        *self == PlanKind::Change
    }

    /// The word for it, as written in JSON and arguments.
    pub fn name(&self) -> &'static str {
        match self {
            PlanKind::Change => "change",
            PlanKind::Walkthrough => "walkthrough",
            PlanKind::Review => "review",
        }
    }
}

impl std::str::FromStr for PlanKind {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|k| k.name() == text).ok_or_else(|| format!("plan kind '{text}': use \"change\", \"walkthrough\" or \"review\""))
    }
}

/// An item's id in its plan (`i1`, `i2`...).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemId(pub String);

impl std::fmt::Display for ItemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where an item stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    /// Still to go through.
    #[default]
    Open,
    /// Gone through: what's to be done about it is decided.
    Decided,
    /// Done.
    Done,
    /// Not to be done.
    Dropped,
}

impl ItemState {
    pub const ALL: [ItemState; 4] = [ItemState::Open, ItemState::Decided, ItemState::Done, ItemState::Dropped];

    /// The word for it, as written in JSON and arguments.
    pub fn name(&self) -> &'static str {
        match self {
            ItemState::Open => "open",
            ItemState::Decided => "decided",
            ItemState::Done => "done",
            ItemState::Dropped => "dropped",
        }
    }
}

/// An item of a plan: a finding, a decision, a to-do, a question to
/// settle; under another item, or at the top.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ItemId>,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default)]
    pub state: ItemState,
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
    /// A change to the code, or a walkthrough of it.
    #[serde(default, skip_serializing_if = "PlanKind::is_change")]
    pub kind: PlanKind,
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
    /// Its items, nested by `parent`, in order (findings, decisions,
    /// to-dos): a review plan's whole content; any plan may have some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Item>,
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
            kind: PlanKind::Change,
            description: String::new(),
            greenfield: false,
            components: Vec::new(),
            references: Vec::new(),
            relations: Vec::new(),
            approvals: Vec::new(),
            workflows: Vec::new(),
            items: Vec::new(),
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

    /// Item `id`.
    pub fn item(&self, id: &ItemId) -> Option<&Item> {
        self.items.iter().find(|i| i.id == *id)
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
