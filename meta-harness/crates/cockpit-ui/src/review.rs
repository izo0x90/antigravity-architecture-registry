//! The plan's review state in the cockpit: its comments and what changed
//! since the user last reviewed it. Fetched per plan, refreshed on
//! architecture events; the graph badges changed and commented nodes, the
//! spec view marks changed sections and shows the threads.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use arch_graph::{Comment, CommentAnchor, ComponentId, PlanDiff, PlanId};

use crate::net::{NetClient, NetResult};

/// How a graph node is marked for review.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeMark {
    /// Changed since the last review.
    pub changed: bool,
    /// Unaddressed comments on it (plan-wide ones excluded).
    pub open: usize,
}

enum Reply {
    Comments(PlanId, NetResult<Vec<Comment>>),
    Diff(PlanId, NetResult<PlanDiff>),
}

#[derive(Default)]
pub struct ReviewState {
    pub plan: Option<PlanId>,
    /// Every comment on the plan, resolved ones included.
    pub comments: Vec<Comment>,
    pub diff: PlanDiff,
    /// Bumped whenever the data above changes.
    pub generation: u64,
    pub error: Option<String>,
    loaded: Option<Option<PlanId>>,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

impl ReviewState {
    /// Something changed on the server: fetch again.
    pub fn invalidate(&mut self) {
        self.loaded = None;
    }

    /// Applies replies; fetches when the plan changes or after `invalidate`.
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context, plan: Option<&PlanId>) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::Comments(p, Ok(comments)) if Some(&p) == self.plan.as_ref() => {
                    self.comments = comments;
                    self.generation += 1;
                }
                Reply::Diff(p, Ok(diff)) if Some(&p) == self.plan.as_ref() => {
                    self.diff = diff;
                    self.generation += 1;
                }
                Reply::Comments(_, Err(e)) | Reply::Diff(_, Err(e)) => self.error = Some(e.into()),
                _ => {}
            }
        }
        let wanted = plan.cloned();
        if self.loaded.as_ref() == Some(&wanted) {
            return;
        }
        self.loaded = Some(wanted.clone());
        if self.plan != wanted {
            self.comments.clear();
            self.diff = PlanDiff::default();
            self.generation += 1;
        }
        self.plan = wanted.clone();
        let Some(plan) = wanted else { return };
        let inbox = self.inbox.clone();
        let p = plan.clone();
        net.arch_comments(&plan, true, ctx.clone(), move |r| push(&inbox, Reply::Comments(p, r)));
        let inbox = self.inbox.clone();
        let p = plan.clone();
        net.arch_review_diff(&plan, ctx.clone(), move |r| push(&inbox, Reply::Diff(p, r)));
    }

    /// Badges for the graph: changed nodes and unaddressed comments per node.
    pub fn marks(&self) -> BTreeMap<ComponentId, NodeMark> {
        let mut out: BTreeMap<ComponentId, NodeMark> = BTreeMap::new();
        for id in self.diff.components() {
            out.entry(id).or_default().changed = true;
        }
        for c in self.comments.iter().filter(|c| !c.is_addressed()) {
            let on: Vec<&ComponentId> = match &c.anchor {
                CommentAnchor::Plan => Vec::new(),
                CommentAnchor::Component { id } | CommentAnchor::Section { id, .. } => vec![id],
                CommentAnchor::Relation { source, target, .. } => vec![source, target],
            };
            for id in on {
                out.entry(id.clone()).or_default().open += 1;
            }
        }
        out
    }
}

fn push(inbox: &Arc<Mutex<Vec<Reply>>>, reply: Reply) {
    if let Ok(mut i) = inbox.lock() {
        i.push(reply);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use arch_graph::{Author, Change, CommentId, CommentKind, CommentStatus, RelationKind};

    pub(crate) fn comment(n: u32, anchor: CommentAnchor, status: CommentStatus) -> Comment {
        Comment {
            id: CommentId(format!("c{n}")),
            anchor,
            kind: CommentKind::Comment,
            author: Author::User { name: "izo".into() },
            text: format!("point {n}"),
            at: "2026-10-03T10:00:00Z".into(),
            status,
            replies: vec![],
            answer: None,
        }
    }

    #[test]
    fn graph_marks_changed_and_commented_nodes() {
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let r = ReviewState {
            diff: PlanDiff {
                since: Some("2026-10-03T09:00:00Z".into()),
                changes: vec![
                    Change::ComponentChanged { id: id("a"), fields: vec!["description".into()] },
                    Change::RelationAdded { kind: RelationKind::Calls, source: id("a"), target: id("b") },
                ],
            },
            comments: vec![
                comment(1, CommentAnchor::Section { id: id("a"), section: "logic".into() }, CommentStatus::Open),
                comment(2, CommentAnchor::Component { id: id("c") }, CommentStatus::Answered),
                comment(3, CommentAnchor::Component { id: id("c") }, CommentStatus::Resolved),
                comment(4, CommentAnchor::Plan, CommentStatus::Open),
            ],
            ..ReviewState::default()
        };
        let marks = r.marks();
        assert_eq!(marks[&id("a")], NodeMark { changed: true, open: 1 });
        assert_eq!(marks[&id("b")], NodeMark { changed: true, open: 0 }, "relation ends are changed too");
        assert_eq!(marks[&id("c")], NodeMark { changed: false, open: 1 }, "answered still counts; resolved doesn't");
        assert_eq!(marks.len(), 3);
    }
}
