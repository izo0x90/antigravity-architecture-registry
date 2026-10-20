//! The user's review of a plan: marking it reviewed (later changes are
//! diffed against it) and what changed since. What's said about its parts,
//! and whether that holds an approval, is the discussions' (`Blocker`).

use arch_graph::{ArchEvent, ChangeReason, PlanDiff, PlanId, ReviewSnapshot};

use super::{Actor, ArchitectureService, now_rfc3339};
use crate::ServiceError;

impl ArchitectureService {
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
        self.emit(ArchEvent::Changed { reason: ChangeReason::PlanEdited, plans: vec![plan_id.clone()], components: Vec::new() });
        Ok(snapshot)
    }

    /// What changed in the plan since the user last reviewed it.
    pub fn review_diff(&self, plan_id: &PlanId) -> Result<PlanDiff, ServiceError> {
        let plan = self.plan(plan_id)?;
        Ok(arch_graph::review::diff(plan.last_review.as_ref(), &plan))
    }
}
