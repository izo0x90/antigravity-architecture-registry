//! The desk's discussions: every one the server has, whole, as the event
//! stream tells them (its one source). Kept on the desk
//! (`Shared::discussions`); what's on a thing is read from it.

use std::collections::BTreeMap;

use harness_contracts::{ClientEvent, Discussion, DiscussionId, Subject};

#[derive(Default)]
pub struct Discussions {
    all: BTreeMap<DiscussionId, Discussion>,
    /// Bumped whenever they change: what's made of them is made again
    /// only then.
    version: u64,
}

impl Discussions {
    /// Takes what the server says about its discussions.
    pub fn apply(&mut self, event: &ClientEvent) {
        match event {
            ClientEvent::DiscussionUpdated { discussion } => {
                self.all.insert(discussion.id.clone(), discussion.clone());
            }
            ClientEvent::DiscussionRemoved { discussion_id } => {
                self.all.remove(discussion_id);
            }
            ClientEvent::Resync => self.all.clear(),
            _ => return,
        }
        self.version += 1;
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn get(&self, id: &DiscussionId) -> Option<&Discussion> {
        self.all.get(id)
    }

    /// Every one, by id.
    pub fn all(&self) -> impl Iterator<Item = &Discussion> {
        self.all.values()
    }

    /// The discussions on `subject`, on something in it or something it's
    /// in (`Subject::touches`), oldest first.
    pub fn on(&self, subject: &Subject) -> Vec<&Discussion> {
        let mut out: Vec<&Discussion> = self.all.values().filter(|d| d.on.subject.touches(subject)).collect();
        out.sort_by_key(|d| d.said.first().map_or(0, |s| s.at_ms));
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use harness_contracts::{DiscussionKind, DiscussionStatus, Named, Said, SaidKind};

    use super::*;

    pub(crate) fn discussion(n: u64, on: Subject, status: DiscussionStatus) -> Discussion {
        Discussion {
            id: DiscussionId::new(format!("d{n}")),
            on: Named { what: on.name(&harness_contracts::Known::default()), subject: on },
            kept: None,
            kind: DiscussionKind::Question,
            by: "user".into(),
            asked: "lead".into(),
            status,
            said: vec![Said { by: "user".into(), at_ms: n, kind: SaidKind::Message, text: format!("point {n}"), changed: Vec::new() }],
        }
    }

    #[test]
    fn the_desks_discussions_are_what_the_event_stream_told() {
        let plan = arch_graph::PlanId::new("p").unwrap();
        let component = Subject::PlanComponent { plan: plan.clone(), id: arch_graph::ComponentId::new("a") };
        let section = Subject::PlanSection { plan: plan.clone(), id: arch_graph::ComponentId::new("a"), section: "logic".into() };
        let mut d = Discussions::default();
        d.apply(&ClientEvent::DiscussionUpdated { discussion: discussion(2, section, DiscussionStatus::Open) });
        d.apply(&ClientEvent::DiscussionUpdated { discussion: discussion(1, component.clone(), DiscussionStatus::Answered) });
        assert_eq!(d.on(&component).iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["d1", "d2"], "on it and its parts, oldest first");
        assert_eq!(d.on(&Subject::Plan { plan }).len(), 2, "the plan's");
        let v = d.version();
        d.apply(&ClientEvent::DiscussionRemoved { discussion_id: DiscussionId::new("d1") });
        assert_eq!((d.on(&component).len(), d.version()), (1, v + 1));
        d.apply(&ClientEvent::Resync);
        assert!(d.get(&DiscussionId::new("d2")).is_none(), "told again from scratch");
    }
}
