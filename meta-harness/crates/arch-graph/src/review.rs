//! The review loop's data: comments and change requests on a plan (on a
//! component, one of its spec sections, a relation, or the plan as a whole),
//! the snapshot taken when the user last reviewed the plan, and the
//! structured diff of what changed since.
//!
//! A comment is *addressed* only once the user resolves it: an agent's answer
//! moves it to `answered`, and the user either accepts (`resolved`) or
//! reopens it. Unaddressed comments on a component block its approval.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Component, ComponentId, Plan, Reference, Relation, RelationKind, Workflow};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommentId(pub String);

impl std::fmt::Display for CommentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a comment is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum CommentAnchor {
    /// The plan as a whole (blocks every approval in it until resolved).
    Plan,
    Component { id: ComponentId },
    /// One section of a component's spec (`overview`, `signature`, `logic`, `invariants`, ...).
    Section { id: ComponentId, section: String },
    Relation { kind: RelationKind, source: ComponentId, target: ComponentId },
}

impl CommentAnchor {
    /// The component it's on, if any (relations count for both ends).
    pub fn touches(&self, id: &ComponentId) -> bool {
        match self {
            Self::Plan => true,
            Self::Component { id: on } | Self::Section { id: on, .. } => on == id,
            Self::Relation { source, target, .. } => source == id || target == id,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Plan => "the plan".to_owned(),
            Self::Component { id } => id.to_string(),
            Self::Section { id, section } => format!("{id} § {section}"),
            Self::Relation { kind, source, target } => format!("{source} -{}-> {target}", kind.name()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Author {
    User { name: String },
    Agent { session: String },
}

impl Author {
    pub fn is_user(&self) -> bool {
        matches!(self, Self::User { .. })
    }
}

impl std::fmt::Display for Author {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User { name } => f.write_str(name),
            Self::Agent { session } if session.is_empty() => f.write_str("agent"),
            Self::Agent { session } => write!(f, "agent ({session})"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentKind {
    /// A question or remark: may be answered without changing the plan.
    #[default]
    Comment,
    /// The plan must change: the answer has to link the changes made.
    ChangeRequest,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentStatus {
    /// Waiting for the agent.
    #[default]
    Open,
    /// The agent answered; waiting for the user to accept or reopen.
    Answered,
    /// The user accepted the answer.
    Resolved,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub author: Author,
    pub text: String,
    /// RFC 3339.
    pub at: String,
}

/// The agent's answer to a comment, with the plan changes that address it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub author: Author,
    pub text: String,
    #[serde(default)]
    pub changes: Vec<Change>,
    pub at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: CommentId,
    pub anchor: CommentAnchor,
    #[serde(default)]
    pub kind: CommentKind,
    pub author: Author,
    pub text: String,
    pub at: String,
    #[serde(default)]
    pub status: CommentStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
}

impl Comment {
    pub fn is_addressed(&self) -> bool {
        self.status == CommentStatus::Resolved
    }
}

/// The plan as the user last reviewed it (approvals and comments aside).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewSnapshot {
    pub at: String,
    pub by: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub greenfield: bool,
    #[serde(default)]
    pub components: Vec<Component>,
    #[serde(default)]
    pub references: Vec<Reference>,
    #[serde(default)]
    pub relations: Vec<Relation>,
    #[serde(default)]
    pub workflows: Vec<Workflow>,
}

impl ReviewSnapshot {
    pub fn of(plan: &Plan, by: &str, at: &str) -> Self {
        Self {
            at: at.to_owned(),
            by: by.to_owned(),
            title: plan.title.clone(),
            description: plan.description.clone(),
            greenfield: plan.greenfield,
            components: plan.components.clone(),
            references: plan.references.clone(),
            relations: plan.relations.clone(),
            workflows: plan.workflows.clone(),
        }
    }
}

/// One difference between the reviewed plan and the current one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    PlanChanged { fields: Vec<String> },
    ComponentAdded { id: ComponentId },
    ComponentRemoved { id: ComponentId },
    /// `fields` are dotted paths, one level into `spec` and `schemas`
    /// (e.g. `description`, `spec.logic_steps`, `schemas.inputs`).
    ComponentChanged { id: ComponentId, fields: Vec<String> },
    RelationAdded { kind: RelationKind, source: ComponentId, target: ComponentId },
    RelationRemoved { kind: RelationKind, source: ComponentId, target: ComponentId },
    RelationChanged { kind: RelationKind, source: ComponentId, target: ComponentId, fields: Vec<String> },
    ReferenceAdded { id: ComponentId },
    ReferenceRemoved { id: ComponentId },
    WorkflowAdded { name: String },
    WorkflowRemoved { name: String },
    WorkflowChanged { name: String },
}

impl Change {
    /// The components this change is about.
    pub fn components(&self) -> Vec<&ComponentId> {
        match self {
            Self::ComponentAdded { id } | Self::ComponentRemoved { id } | Self::ComponentChanged { id, .. } | Self::ReferenceAdded { id } | Self::ReferenceRemoved { id } => vec![id],
            Self::RelationAdded { source, target, .. } | Self::RelationRemoved { source, target, .. } | Self::RelationChanged { source, target, .. } => vec![source, target],
            Self::PlanChanged { .. } | Self::WorkflowAdded { .. } | Self::WorkflowRemoved { .. } | Self::WorkflowChanged { .. } => Vec::new(),
        }
    }

    pub fn describe(&self) -> String {
        let rel = |kind: &RelationKind, source: &ComponentId, target: &ComponentId| format!("{source} -{}-> {target}", kind.name());
        match self {
            Self::PlanChanged { fields } => format!("plan changed: {}", fields.join(", ")),
            Self::ComponentAdded { id } => format!("added {id}"),
            Self::ComponentRemoved { id } => format!("removed {id}"),
            Self::ComponentChanged { id, fields } => format!("changed {id}: {}", fields.join(", ")),
            Self::RelationAdded { kind, source, target } => format!("related {}", rel(kind, source, target)),
            Self::RelationRemoved { kind, source, target } => format!("unrelated {}", rel(kind, source, target)),
            Self::RelationChanged { kind, source, target, fields } => format!("changed {}: {}", rel(kind, source, target), fields.join(", ")),
            Self::ReferenceAdded { id } => format!("now references {id}"),
            Self::ReferenceRemoved { id } => format!("no longer references {id}"),
            Self::WorkflowAdded { name } => format!("added workflow {name}"),
            Self::WorkflowRemoved { name } => format!("removed workflow {name}"),
            Self::WorkflowChanged { name } => format!("changed workflow {name}"),
        }
    }
}

/// What changed in a plan since its last review (everything, if never reviewed).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDiff {
    /// When the snapshot was taken; none when the plan was never reviewed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    pub changes: Vec<Change>,
}

impl PlanDiff {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Every component some change is about.
    pub fn components(&self) -> BTreeSet<ComponentId> {
        self.changes.iter().flat_map(|c| c.components()).cloned().collect()
    }

    /// The changes about `id`.
    pub fn for_component<'a>(&'a self, id: &'a ComponentId) -> impl Iterator<Item = &'a Change> + 'a {
        self.changes.iter().filter(move |c| c.components().contains(&id))
    }

    /// The changed fields of `id` (`added` when it is new; `relations` when one
    /// of its relations changed).
    pub fn fields(&self, id: &ComponentId) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for change in self.for_component(id) {
            match change {
                Change::ComponentAdded { .. } => {
                    out.insert("added".to_owned());
                }
                Change::ComponentRemoved { .. } => {
                    out.insert("removed".to_owned());
                }
                Change::ComponentChanged { fields, .. } => out.extend(fields.iter().cloned()),
                Change::RelationAdded { .. } | Change::RelationRemoved { .. } | Change::RelationChanged { .. } => {
                    out.insert("relations".to_owned());
                }
                Change::ReferenceAdded { .. } | Change::ReferenceRemoved { .. } => {
                    out.insert("reference".to_owned());
                }
                _ => {}
            }
        }
        out
    }
}

/// Fields compared one level deeper, so a change says which part moved.
const NESTED: [&str; 2] = ["spec", "schemas"];
/// Facts from the code, not intent: not part of what the user reviews.
const IGNORED: [&str; 2] = ["anchor", "placeholders"];

fn field_changes(before: &Value, after: &Value) -> Vec<String> {
    let empty = serde_json::Map::new();
    let (b, a) = (before.as_object().unwrap_or(&empty), after.as_object().unwrap_or(&empty));
    let keys: BTreeSet<&String> = b.keys().chain(a.keys()).collect();
    let mut out = Vec::new();
    for key in keys {
        if IGNORED.contains(&key.as_str()) {
            continue;
        }
        let (old, new) = (b.get(key).unwrap_or(&Value::Null), a.get(key).unwrap_or(&Value::Null));
        if old == new || (is_blank(old) && is_blank(new)) {
            continue;
        }
        let objects = (old.is_object() || is_blank(old)) && (new.is_object() || is_blank(new));
        if NESTED.contains(&key.as_str()) && objects {
            let inner = field_changes(old, new);
            out.extend(inner.into_iter().map(|f| format!("{key}.{f}")));
        } else {
            out.push(key.clone());
        }
    }
    out
}

/// Absent, null, empty list or empty map: all "nothing" (serialization skips them).
fn is_blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

fn to_value<T: Serialize>(item: &T) -> Value {
    serde_json::to_value(item).unwrap_or(Value::Null)
}

/// What changed between the reviewed snapshot (or nothing) and `plan`.
pub fn diff(reviewed: Option<&ReviewSnapshot>, plan: &Plan) -> PlanDiff {
    let empty = ReviewSnapshot::of(&Plan::new(plan.id.0.as_str(), &plan.title), "", "");
    let base = reviewed.unwrap_or(&empty);
    let mut changes = Vec::new();

    let mut plan_fields = Vec::new();
    if reviewed.is_some() {
        if base.title != plan.title {
            plan_fields.push("title".to_owned());
        }
        if base.description != plan.description {
            plan_fields.push("description".to_owned());
        }
        if base.greenfield != plan.greenfield {
            plan_fields.push("greenfield".to_owned());
        }
    }
    if !plan_fields.is_empty() {
        changes.push(Change::PlanChanged { fields: plan_fields });
    }

    let before: BTreeMap<&ComponentId, &Component> = base.components.iter().map(|c| (&c.id, c)).collect();
    let after: BTreeMap<&ComponentId, &Component> = plan.components.iter().map(|c| (&c.id, c)).collect();
    for (id, now) in &after {
        match before.get(id) {
            None => changes.push(Change::ComponentAdded { id: (*id).clone() }),
            Some(was) => {
                let fields = field_changes(&to_value(was), &to_value(now));
                if !fields.is_empty() {
                    changes.push(Change::ComponentChanged { id: (*id).clone(), fields });
                }
            }
        }
    }
    for id in before.keys().filter(|id| !after.contains_key(*id)) {
        changes.push(Change::ComponentRemoved { id: (*id).clone() });
    }

    let rel_before: BTreeMap<_, &Relation> = base.relations.iter().map(|r| (r.id(), r)).collect();
    let rel_after: BTreeMap<_, &Relation> = plan.relations.iter().map(|r| (r.id(), r)).collect();
    for (rid, now) in &rel_after {
        let (kind, source, target) = (now.kind, now.source.clone(), now.target.clone());
        match rel_before.get(rid) {
            None => changes.push(Change::RelationAdded { kind, source, target }),
            Some(was) => {
                let fields: Vec<String> = field_changes(&to_value(was), &to_value(now)).into_iter().filter(|f| f != "sites" && f != "provenance").collect();
                if !fields.is_empty() {
                    changes.push(Change::RelationChanged { kind, source, target, fields });
                }
            }
        }
    }
    for (rid, was) in &rel_before {
        if !rel_after.contains_key(rid) {
            changes.push(Change::RelationRemoved { kind: was.kind, source: was.source.clone(), target: was.target.clone() });
        }
    }

    let refs_before: BTreeSet<&ComponentId> = base.references.iter().map(|r| &r.id).collect();
    let refs_after: BTreeSet<&ComponentId> = plan.references.iter().map(|r| &r.id).collect();
    changes.extend(refs_after.difference(&refs_before).map(|id| Change::ReferenceAdded { id: (*id).clone() }));
    changes.extend(refs_before.difference(&refs_after).map(|id| Change::ReferenceRemoved { id: (*id).clone() }));

    let wf_before: BTreeMap<&str, &Workflow> = base.workflows.iter().map(|w| (w.name.as_str(), w)).collect();
    let wf_after: BTreeMap<&str, &Workflow> = plan.workflows.iter().map(|w| (w.name.as_str(), w)).collect();
    for (name, now) in &wf_after {
        match wf_before.get(name) {
            None => changes.push(Change::WorkflowAdded { name: (*name).to_owned() }),
            Some(was) if was != now => changes.push(Change::WorkflowChanged { name: (*name).to_owned() }),
            Some(_) => {}
        }
    }
    for name in wf_before.keys().filter(|n| !wf_after.contains_key(*n)) {
        changes.push(Change::WorkflowRemoved { name: (*name).to_owned() });
    }

    PlanDiff { since: reviewed.map(|r| r.at.clone()), changes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::tests::planned;
    use crate::{ImplementationSpec, Invariant, InvariantKind, Provenance, Stage};

    fn id(s: &str) -> ComponentId {
        ComponentId::new(format!("rust:{s}"))
    }

    // req: R.2
    #[test]
    fn diff_since_review_lists_component_relation_reference_and_workflow_changes() {
        let mut plan = Plan::new("p", "Add validation");
        plan.components.push(planned("rust:app::validate", Stage::Declared));
        plan.components.push(planned("rust:app::old", Stage::Declared));
        plan.relations.push(Relation::new(RelationKind::Calls, id("app::run"), id("app::validate"), Provenance::PLANNED));
        plan.references.push(Reference { id: id("app::run"), kinds: vec![], depth: 0 });
        plan.workflows.push(Workflow { name: "checkout".into(), entry: id("app::run"), description: String::new() });

        // Never reviewed: everything is new.
        let first = diff(None, &plan);
        assert_eq!(first.since, None);
        assert!(first.changes.contains(&Change::ComponentAdded { id: id("app::validate") }));
        assert!(first.changes.contains(&Change::WorkflowAdded { name: "checkout".into() }));

        let snapshot = ReviewSnapshot::of(&plan, "ana", "2026-10-03T10:00:00Z");
        assert!(diff(Some(&snapshot), &plan).is_empty(), "nothing changed since the review");

        // Spec and schema edits are reported by the part that moved.
        let v = plan.component_mut(&id("app::validate")).unwrap();
        v.description = "Rejects empty input.".into();
        v.spec = Some(ImplementationSpec {
            invariants: vec![Invariant { name: "pure".into(), kind: InvariantKind::PostCondition, description: String::new() }],
            ..ImplementationSpec::default()
        });
        v.schemas.inputs = Some(serde_json::json!({"type": "object"}));
        // Code facts aren't review changes.
        v.placeholders.push(crate::Span { start_line: 1, end_line: 1, start_byte: 0, end_byte: 1 });
        plan.components.retain(|c| c.id != id("app::old"));
        plan.components.push(planned("rust:app::new", Stage::Declared));
        plan.relations[0].order = Some(1);
        plan.relations.push(Relation::new(RelationKind::Calls, id("app::validate"), id("app::new"), Provenance::PLANNED));
        plan.references.clear();
        plan.workflows[0].description = "Pay for the cart.".into();
        plan.title = "Add input validation".into();

        let d = diff(Some(&snapshot), &plan);
        assert_eq!(d.since.as_deref(), Some("2026-10-03T10:00:00Z"));
        assert_eq!(
            d.changes,
            vec![
                Change::PlanChanged { fields: vec!["title".into()] },
                Change::ComponentAdded { id: id("app::new") },
                Change::ComponentChanged { id: id("app::validate"), fields: vec!["description".into(), "schemas.inputs".into(), "spec.invariants".into()] },
                Change::ComponentRemoved { id: id("app::old") },
                Change::RelationChanged { kind: RelationKind::Calls, source: id("app::run"), target: id("app::validate"), fields: vec!["order".into()] },
                Change::RelationAdded { kind: RelationKind::Calls, source: id("app::validate"), target: id("app::new") },
                Change::ReferenceRemoved { id: id("app::run") },
                Change::WorkflowChanged { name: "checkout".into() },
            ]
        );
        assert_eq!(d.fields(&id("app::validate")), ["description", "relations", "schemas.inputs", "spec.invariants"].into_iter().map(String::from).collect());
        assert!(d.components().contains(&id("app::run")), "relation ends count as changed");

        // A spec that existed before reports which part changed.
        let snapshot2 = ReviewSnapshot::of(&plan, "ana", "2026-10-03T11:00:00Z");
        plan.component_mut(&id("app::validate")).unwrap().spec.as_mut().unwrap().invariants.clear();
        assert_eq!(diff(Some(&snapshot2), &plan).changes, vec![Change::ComponentChanged { id: id("app::validate"), fields: vec!["spec.invariants".into()] }]);
        // Round-trips as data (it goes over REST and MCP).
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<PlanDiff>(&json).unwrap(), d);
    }

    #[test]
    fn anchors_touch_their_components() {
        let section = CommentAnchor::Section { id: id("a"), section: "logic".into() };
        assert!(section.touches(&id("a")) && !section.touches(&id("b")));
        let rel = CommentAnchor::Relation { kind: RelationKind::Calls, source: id("a"), target: id("b") };
        assert!(rel.touches(&id("b")));
        assert!(CommentAnchor::Plan.touches(&id("z")));
        assert_eq!(serde_json::to_value(&section).unwrap(), serde_json::json!({"on": "section", "id": "rust:a", "section": "logic"}));
    }
}
