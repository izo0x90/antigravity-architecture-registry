//! The plan the desk reads, kept once for every buffer: the plan itself
//! and what changed since the user last reviewed it (read per plan, again
//! on architecture events, the last asked for winning), and the
//! discussions on its parts (the desk's, from the event stream). The
//! graph badges changed and discussed nodes; the spec view marks changed
//! sections and shows the discussions and an item's card; the plan tree
//! lists its items, cards and discussions.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use arch_graph::{ComponentId, PlanDiff, PlanId};
use harness_contracts::{Discussion, DiscussionStatus, Subject};

use crate::net::{NetClient, NetResult};

/// How a graph node is marked for review.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeMark {
    /// Changed since the last review.
    pub changed: bool,
    /// Discussions on it not settled (plan-wide ones excluded).
    pub open: usize,
}

/// An answer, with the number of the read it answers.
enum Reply {
    Plan(u64, Box<NetResult<arch_graph::Plan>>),
    Diff(u64, NetResult<PlanDiff>),
}

#[derive(Default)]
pub struct ReviewState {
    pub plan: Option<PlanId>,
    /// The plan as last read (or why it couldn't be).
    pub read: Option<Result<arch_graph::Plan, String>>,
    /// Every discussion on the plan's parts, settled ones included.
    pub discussions: Vec<Discussion>,
    pub diff: PlanDiff,
    /// Bumped whenever the data above changes.
    pub generation: u64,
    pub error: Option<String>,
    loaded: Option<Option<PlanId>>,
    /// The last read asked for: an older one's answer is dropped.
    asked: u64,
    /// The desk's discussions' version they were taken at.
    discussed: Option<(Option<PlanId>, u64)>,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

impl ReviewState {
    /// Something changed on the server: fetch again.
    pub fn invalidate(&mut self) {
        self.loaded = None;
    }

    /// Applies replies; fetches when the plan changes or after `invalidate`;
    /// takes the plan's discussions from the desk's when they changed.
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context, plan: Option<&PlanId>, discussions: &crate::discussions::Discussions) {
        let now = (plan.cloned(), discussions.version());
        if self.discussed.as_ref() != Some(&now) {
            // Told again only when the plan's own changed (not one on a terminal).
            let on: Vec<Discussion> = plan.map(|p| discussions.on(&Subject::Plan { plan: p.clone() }).into_iter().cloned().collect()).unwrap_or_default();
            if on != self.discussions {
                self.discussions = on;
                self.generation += 1;
            }
            self.discussed = Some(now);
        }
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::Plan(n, read) if n == self.asked => {
                    self.read = Some((*read).map_err(String::from));
                    self.generation += 1;
                }
                Reply::Diff(n, Ok(diff)) if n == self.asked => {
                    self.diff = diff;
                    self.generation += 1;
                }
                Reply::Diff(n, Err(e)) if n == self.asked => self.error = Some(e.into()),
                _ => {}
            }
        }
        let wanted = plan.cloned();
        if self.loaded.as_ref() == Some(&wanted) {
            return;
        }
        self.loaded = Some(wanted.clone());
        if self.plan != wanted {
            self.diff = PlanDiff::default();
            self.read = None;
            self.generation += 1;
        }
        self.plan = wanted.clone();
        self.asked += 1;
        let Some(plan) = wanted else { return };
        let (inbox, n) = (self.inbox.clone(), self.asked);
        net.arch_plan(&plan, ctx.clone(), move |r| push(&inbox, Reply::Plan(n, Box::new(r))));
        let inbox = self.inbox.clone();
        net.arch_review_diff(&plan, ctx.clone(), move |r| push(&inbox, Reply::Diff(n, r)));
    }

    /// Badges for the graph: changed nodes and discussions not settled per node.
    pub fn marks(&self) -> BTreeMap<ComponentId, NodeMark> {
        let mut out: BTreeMap<ComponentId, NodeMark> = BTreeMap::new();
        for id in self.diff.components() {
            out.entry(id).or_default().changed = true;
        }
        for d in self.discussions.iter().filter(|d| d.status != DiscussionStatus::Settled) {
            let on: Vec<&ComponentId> = match &d.on.subject {
                Subject::PlanComponent { id, .. } | Subject::PlanSection { id, .. } => vec![id],
                Subject::PlanRelation { source, target, .. } => vec![source, target],
                _ => Vec::new(),
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
    use arch_graph::{Change, RelationKind};
    pub(crate) use crate::discussions::tests::discussion;

    fn plan() -> PlanId {
        PlanId::new("p").unwrap()
    }

    /// A component of plan p, or one of its sections.
    pub(crate) fn on(id: &ComponentId, section: Option<&str>) -> Subject {
        match section {
            Some(s) => Subject::PlanSection { plan: plan(), id: id.clone(), section: s.into() },
            None => Subject::PlanComponent { plan: plan(), id: id.clone() },
        }
    }

    #[test]
    fn graph_marks_changed_and_discussed_nodes() {
        let id = |s: &str| ComponentId::new(format!("rust:{s}"));
        let r = ReviewState {
            diff: PlanDiff {
                since: Some("2026-10-03T09:00:00Z".into()),
                changes: vec![
                    Change::ComponentChanged { id: id("a"), fields: vec!["description".into()] },
                    Change::RelationAdded { kind: RelationKind::Calls, source: id("a"), target: id("b") },
                ],
            },
            discussions: vec![
                discussion(1, on(&id("a"), Some("logic")), DiscussionStatus::Open),
                discussion(2, on(&id("c"), None), DiscussionStatus::Answered),
                discussion(3, on(&id("c"), None), DiscussionStatus::Settled),
                discussion(4, Subject::Plan { plan: plan() }, DiscussionStatus::Open),
            ],
            ..ReviewState::default()
        };
        let marks = r.marks();
        assert_eq!(marks[&id("a")], NodeMark { changed: true, open: 1 });
        assert_eq!(marks[&id("b")], NodeMark { changed: true, open: 0 }, "relation ends are changed too");
        assert_eq!(marks[&id("c")], NodeMark { changed: false, open: 1 }, "answered still counts; settled doesn't");
        assert_eq!(marks.len(), 3);
    }
}
