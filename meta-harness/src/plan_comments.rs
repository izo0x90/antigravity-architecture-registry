//! Comments plans' files held before discussions did, moved into the
//! discussion engine once (`ArchitectureService::move_comments_to`): each
//! anchor becomes the plan's part it was on, each comment a discussion with
//! its replies and answer, as it stood. The only reader of that shape.

use arch_graph::{Change, ComponentId, PlanId, RelationKind};
use harness_contracts::{DiscussionKind, DiscussionStatus, Named, Said, SaidKind, Subject};
use nah_discussions::{Brought, Discussions, LEAD, USER};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
enum Anchor {
    Plan,
    Component { id: ComponentId },
    Section { id: ComponentId, section: String },
    Relation { kind: RelationKind, source: ComponentId, target: ComponentId },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Author {
    User {},
    Agent { session: String },
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    #[default]
    Comment,
    ChangeRequest,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Status {
    #[default]
    Open,
    Answered,
    Resolved,
}

#[derive(Deserialize)]
struct Reply {
    author: Author,
    text: String,
    at: String,
}

#[derive(Deserialize)]
struct Answer {
    author: Author,
    text: String,
    #[serde(default)]
    changes: Vec<Change>,
    at: String,
}

#[derive(Deserialize)]
struct Comment {
    id: String,
    anchor: Anchor,
    #[serde(default)]
    kind: Kind,
    author: Author,
    text: String,
    at: String,
    #[serde(default)]
    status: Status,
    #[serde(default)]
    replies: Vec<Reply>,
    #[serde(default)]
    answer: Option<Answer>,
}

/// Who said it, as discussions name them: the user, or an agent by its
/// slot's name (what comments called its session).
fn by(author: &Author) -> String {
    match author {
        Author::User {} => USER.to_owned(),
        Author::Agent { session } if session.is_empty() => "agent".to_owned(),
        Author::Agent { session } => session.clone(),
    }
}

/// What an answer's change was to, in the plan.
fn changed(plan: &PlanId, change: &Change) -> Subject {
    let component = |id: &ComponentId| Subject::PlanComponent { plan: plan.clone(), id: id.clone() };
    match change {
        Change::ComponentAdded { id }
        | Change::ComponentRemoved { id }
        | Change::ComponentChanged { id, .. }
        | Change::ReferenceAdded { id }
        | Change::ReferenceRemoved { id } => component(id),
        Change::RelationAdded { kind, source, target }
        | Change::RelationRemoved { kind, source, target }
        | Change::RelationChanged { kind, source, target, .. } => {
            Subject::PlanRelation { plan: plan.clone(), relation: *kind, source: source.clone(), target: target.clone() }
        }
        Change::PlanChanged { .. } | Change::WorkflowAdded { .. } | Change::WorkflowRemoved { .. } | Change::WorkflowChanged { .. } => {
            Subject::Plan { plan: plan.clone() }
        }
    }
}

/// One comment, as the discussion it becomes: its first message, its
/// replies and its answer in the order said.
fn brought(plan: &PlanId, c: Comment) -> Brought {
    // Its id and when it was written: one of another branch with the same id is another.
    let from = format!("plan {plan} comment {} of {}", c.id, c.at);
    let on = match c.anchor {
        Anchor::Plan => Subject::Plan { plan: plan.clone() },
        Anchor::Component { id } => Subject::PlanComponent { plan: plan.clone(), id },
        Anchor::Section { id, section } => Subject::PlanSection { plan: plan.clone(), id, section },
        Anchor::Relation { kind, source, target } => Subject::PlanRelation { plan: plan.clone(), relation: kind, source, target },
    };
    let first = arch_graph::when::read(&c.at).unwrap_or(0);
    let mut said =
        vec![Said { by: by(&c.author), at_ms: first, kind: SaidKind::Message, text: c.text, links: Vec::new(), changed: Vec::new() }];
    let mut later: Vec<Said> = c
        .replies
        .into_iter()
        .map(|r| Said {
            by: by(&r.author),
            at_ms: arch_graph::when::read(&r.at).unwrap_or(first),
            kind: SaidKind::Message,
            text: r.text,
            links: Vec::new(),
            changed: Vec::new(),
        })
        .collect();
    if let Some(a) = c.answer {
        let changed = a
            .changes
            .iter()
            .map(|ch| changed(plan, ch))
            .map(|subject| Named { what: subject.name(&harness_contracts::Known::default()), subject })
            .collect();
        later.push(Said {
            by: by(&a.author),
            at_ms: arch_graph::when::read(&a.at).unwrap_or(first),
            kind: SaidKind::Answer,
            text: a.text,
            links: Vec::new(),
            changed,
        });
    }
    later.sort_by_key(|s| s.at_ms);
    said.extend(later);
    let status = match c.status {
        Status::Open => DiscussionStatus::Open,
        Status::Answered => DiscussionStatus::Answered,
        Status::Resolved => DiscussionStatus::Settled,
    };
    let kind = if c.kind == Kind::ChangeRequest { DiscussionKind::ChangeRequest } else { DiscussionKind::Question };
    Brought { from, on, kind, by: said[0].by.clone(), asked: LEAD.to_owned(), status, said }
}

/// Plan `plan`'s comments, as its file held them, moved into `discussions`:
/// all of them, or none.
pub fn move_into(discussions: &Discussions, plan: &PlanId, comments: &[serde_json::Value]) -> Result<(), String> {
    let read: Vec<Comment> = comments
        .iter()
        .map(|c| serde_json::from_value(c.clone()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("a comment in plan {plan} couldn't be read: {e}"))?;
    let brought = read.into_iter().map(|c| brought(plan, c)).collect();
    discussions.bring(brought, &format!("plan {plan}'s comments")).map(|_| ()).map_err(|e| e.to_string())
}
