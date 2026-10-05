//! The review loop on the service: users comment on a plan (components, spec
//! sections, relations, the plan itself) and mark it reviewed; agents answer
//! each point, linking the plan changes that address it; users accept or
//! reopen. A component can't be approved while a comment on it is
//! unaddressed, i.e. anything but resolved by the user (an answer alone
//! isn't enough: the user has to accept it).

use arch_graph::{
    Answer, ArchEvent, Author, Change, ChangeReason, Comment, CommentAnchor, CommentId, CommentKind, CommentStatus, ComponentId, Plan, PlanDiff, PlanId,
    Reply, ReviewSnapshot,
};
use serde::{Deserialize, Serialize};

use super::{Actor, ArchitectureService, now_rfc3339};
use crate::ServiceError;

/// A new comment or change request, from the cockpit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewComment {
    pub anchor: CommentAnchor,
    #[serde(default)]
    pub kind: CommentKind,
    pub text: String,
}

fn author(actor: &Actor) -> Author {
    match actor {
        Actor::User { name } => Author::User { name: name.clone() },
        Actor::Agent { session } => Author::Agent { session: session.clone() },
    }
}

fn user_only(actor: &Actor, what: &str) -> Result<(), ServiceError> {
    match actor {
        Actor::User { .. } => Ok(()),
        Actor::Agent { .. } => Err(ServiceError::Forbidden(format!("only the user {what}; answer the comment instead"))),
    }
}

fn text(raw: &str, what: &str) -> Result<String, ServiceError> {
    let t = raw.trim();
    if t.is_empty() { Err(ServiceError::InvalidOp(format!("{what} needs some text"))) } else { Ok(t.to_owned()) }
}

fn find<'a>(plan: &'a mut Plan, id: &CommentId) -> Result<&'a mut Comment, ServiceError> {
    let plan_id = plan.id.clone();
    plan.comments.iter_mut().find(|c| &c.id == id).ok_or_else(|| ServiceError::NotFound(format!("comment {id} in plan {plan_id}")))
}

fn short(text: &str) -> String {
    let first: String = text.chars().take(60).collect();
    if first.len() < text.len() { format!("{first}…") } else { first }
}

/// Comments on `id` that still block its approval.
pub fn unaddressed<'a>(plan: &'a Plan, id: &'a ComponentId) -> impl Iterator<Item = &'a Comment> + 'a {
    plan.comments.iter().filter(move |c| !c.is_addressed() && c.anchor.touches(id))
}

/// Why `id` can't be approved yet because of review comments, if it can't.
pub fn approval_blocker(plan: &Plan, id: &ComponentId) -> Option<String> {
    let points: Vec<String> = unaddressed(plan, id)
        .map(|c| match c.status {
            CommentStatus::Answered => format!("{} on {} (answered; accept or reopen it)", c.id, c.anchor.describe()),
            _ => format!("{} on {} (open: \"{}\")", c.id, c.anchor.describe(), short(&c.text)),
        })
        .collect();
    if points.is_empty() { None } else { Some(format!("{id} has unaddressed review comments: {}", points.join("; "))) }
}

impl ArchitectureService {
    fn announce_review(&self, plan: &PlanId, components: Vec<ComponentId>) {
        self.emit(ArchEvent::Changed { reason: ChangeReason::PlanEdited, plans: vec![plan.clone()], components });
    }

    fn anchor_ids(plan: &Plan, comment: &Comment) -> Vec<ComponentId> {
        match &comment.anchor {
            CommentAnchor::Plan => plan.components.iter().map(|c| c.id.clone()).collect(),
            CommentAnchor::Component { id } | CommentAnchor::Section { id, .. } => vec![id.clone()],
            CommentAnchor::Relation { source, target, .. } => vec![source.clone(), target.clone()],
        }
    }

    /// The plan's comments: the unaddressed ones, or all of them.
    pub fn comments(&self, plan_id: &PlanId, all: bool) -> Result<Vec<Comment>, ServiceError> {
        Ok(self.plan(plan_id)?.comments.into_iter().filter(|c| all || !c.is_addressed()).collect())
    }

    /// The user comments on (or asks for a change to) part of the plan.
    pub fn add_comment(&self, plan_id: &PlanId, new: NewComment, actor: &Actor) -> Result<Comment, ServiceError> {
        user_only(actor, "raises review comments")?;
        let (comment, touched) = self.edit_plan(plan_id, |plan| {
            let graph = self.graph(Some(plan_id))?;
            let exists = |id: &ComponentId| graph.component(id).is_some();
            match &new.anchor {
                CommentAnchor::Plan => {}
                CommentAnchor::Component { id } | CommentAnchor::Section { id, .. } if !exists(id) => return Err(ServiceError::NotFound(format!("{id}"))),
                CommentAnchor::Section { section, .. } if section.trim().is_empty() => return Err(ServiceError::InvalidOp("a section comment needs the section's name".into())),
                CommentAnchor::Relation { kind, source, target } if !graph.outgoing(source, Some(*kind)).any(|r| &r.target == target) => {
                    return Err(ServiceError::NotFound(format!("relation {source} -{}-> {target}", kind.name())));
                }
                _ => {}
            }
            let next = plan.comments.iter().filter_map(|c| c.id.0.strip_prefix('c')?.parse::<u32>().ok()).max().unwrap_or(0) + 1;
            let comment = Comment {
                id: CommentId(format!("c{next}")),
                anchor: new.anchor,
                kind: new.kind,
                author: author(actor),
                text: text(&new.text, "a comment")?,
                at: now_rfc3339(),
                status: CommentStatus::Open,
                replies: Vec::new(),
                answer: None,
            };
            plan.comments.push(comment.clone());
            let touched = Self::anchor_ids(plan, &comment);
            Ok((comment, touched))
        })?;
        self.announce_review(plan_id, touched);
        Ok(comment)
    }

    /// A reply in the comment's thread (user or agent); doesn't change its status.
    pub fn reply(&self, plan_id: &PlanId, comment_id: &CommentId, body: &str, actor: &Actor) -> Result<Comment, ServiceError> {
        let (comment, touched) = self.edit_plan(plan_id, |plan| {
            let comment = find(plan, comment_id)?;
            comment.replies.push(Reply { author: author(actor), text: text(body, "a reply")?, at: now_rfc3339() });
            let comment = comment.clone();
            let touched = Self::anchor_ids(plan, &comment);
            Ok((comment, touched))
        })?;
        self.announce_review(plan_id, touched);
        Ok(comment)
    }

    /// The agent answers a point, linking the plan changes (since the last
    /// review) that address it. A change request must link at least one
    /// change touching what it's about.
    pub fn answer_comment(&self, plan_id: &PlanId, comment_id: &CommentId, body: &str, changed: &[ComponentId], actor: &Actor) -> Result<Comment, ServiceError> {
        let (comment, touched) = self.edit_plan(plan_id, |plan| {
            let diff = arch_graph::review::diff(plan.last_review.as_ref(), plan);
            let since = diff.since.clone().map_or_else(|| "since the plan was started".to_owned(), |at| format!("since the review at {at}"));
            let mut changes: Vec<Change> = Vec::new();
            for id in changed {
                let found: Vec<&Change> = diff.for_component(id).collect();
                if found.is_empty() {
                    return Err(ServiceError::InvalidOp(format!("{id} hasn't changed {since}; change the plan (plan_edit) first, then link it")));
                }
                for change in found {
                    if !changes.contains(change) {
                        changes.push(change.clone());
                    }
                }
            }
            let comment = find(plan, comment_id)?;
            if comment.status == CommentStatus::Resolved {
                return Err(ServiceError::InvalidOp(format!("{comment_id} is already resolved")));
            }
            if comment.kind == CommentKind::ChangeRequest && !changes.iter().any(|ch| ch.components().iter().any(|id| comment.anchor.touches(id))) {
                return Err(ServiceError::InvalidOp(format!(
                    "{comment_id} asks for a change to {}: link the change you made to it (components changed {since}: {})",
                    comment.anchor.describe(),
                    diff.components().iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
                )));
            }
            comment.answer = Some(Answer { author: author(actor), text: text(body, "an answer")?, changes, at: now_rfc3339() });
            comment.status = CommentStatus::Answered;
            let comment = comment.clone();
            let touched = Self::anchor_ids(plan, &comment);
            Ok((comment, touched))
        })?;
        self.announce_review(plan_id, touched);
        Ok(comment)
    }

    /// The user accepts the answer: the point is addressed.
    pub fn accept_comment(&self, plan_id: &PlanId, comment_id: &CommentId, actor: &Actor) -> Result<Comment, ServiceError> {
        user_only(actor, "accepts answers")?;
        let (comment, touched) = self.edit_plan(plan_id, |plan| {
            let comment = find(plan, comment_id)?;
            if comment.status != CommentStatus::Answered {
                return Err(ServiceError::InvalidOp(format!("{comment_id} has no answer to accept yet")));
            }
            comment.status = CommentStatus::Resolved;
            let comment = comment.clone();
            let touched = Self::anchor_ids(plan, &comment);
            Ok((comment, touched))
        })?;
        self.announce_review(plan_id, touched);
        Ok(comment)
    }

    /// The user isn't satisfied: the point is open again, with the reason.
    pub fn reopen_comment(&self, plan_id: &PlanId, comment_id: &CommentId, reason: &str, actor: &Actor) -> Result<Comment, ServiceError> {
        user_only(actor, "reopens comments")?;
        let (comment, touched) = self.edit_plan(plan_id, |plan| {
            let comment = find(plan, comment_id)?;
            if comment.status == CommentStatus::Open {
                return Err(ServiceError::InvalidOp(format!("{comment_id} is already open")));
            }
            comment.replies.push(Reply { author: author(actor), text: text(reason, "reopening")?, at: now_rfc3339() });
            comment.status = CommentStatus::Open;
            let comment = comment.clone();
            let touched = Self::anchor_ids(plan, &comment);
            Ok((comment, touched))
        })?;
        self.announce_review(plan_id, touched);
        Ok(comment)
    }

    /// The user has reviewed the plan as it is now: later changes are diffed against this.
    pub fn mark_reviewed(&self, plan_id: &PlanId, actor: &Actor) -> Result<ReviewSnapshot, ServiceError> {
        let Actor::User { name } = actor else {
            return Err(ServiceError::Forbidden("only the user marks a plan reviewed".into()));
        };
        let snapshot = self.edit_plan(plan_id, |plan| {
            let snapshot = ReviewSnapshot::of(plan, name, &now_rfc3339());
            plan.last_review = Some(snapshot.clone());
            Ok(snapshot)
        })?;
        self.announce_review(plan_id, Vec::new());
        Ok(snapshot)
    }

    /// What changed in the plan since the user last reviewed it.
    pub fn review_diff(&self, plan_id: &PlanId) -> Result<PlanDiff, ServiceError> {
        let plan = self.plan(plan_id)?;
        Ok(arch_graph::review::diff(plan.last_review.as_ref(), &plan))
    }
}
