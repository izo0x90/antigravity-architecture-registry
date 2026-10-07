//! Go-to: where a go-to key takes the cursor's target. A component's
//! relations come from the server (its detail, the same in every view),
//! cached until the architecture changes; a go-to waits for what it needs
//! and asks which one when there are several.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use arch_graph::{ComponentDetail, ComponentId, PlanId, RelationKind};

use crate::app::BufferKind;
use crate::nav::{GoTo, Jump};
use crate::net::{NetClient, NetResult};

/// Where a go-to can land: a component, and for a caller the line of the call.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub id: ComponentId,
    pub line: Option<u32>,
}

/// The components a relational go-to reaches from `detail`, in order (calls
/// in call order), each once.
pub fn candidates(op: GoTo, detail: &ComponentDetail) -> Vec<Candidate> {
    let Some((kind, outgoing)) = op.relation() else { return Vec::new() };
    let list = if outgoing { &detail.outgoing } else { &detail.incoming };
    let mut relations: Vec<&arch_graph::Relation> = list.iter().filter(|r| r.kind == kind).collect();
    if kind == RelationKind::Calls && outgoing {
        relations.sort_by_key(|r| r.order.unwrap_or(u32::MAX));
    }
    let mut seen = BTreeSet::new();
    relations
        .into_iter()
        .filter_map(|r| {
            let id = if outgoing { r.target.clone() } else { r.source.clone() };
            // A caller opens at the call.
            let line = (kind == RelationKind::Calls && !outgoing).then(|| r.sites.first().map(|s| s.start_line)).flatten();
            seen.insert(id.clone()).then_some(Candidate { id, line })
        })
        .collect()
}

/// What a go-to is waiting for.
#[derive(Clone, Debug, PartialEq)]
pub enum Wait {
    /// The detail of `component`, to follow its relations.
    Relations { op: GoTo, component: ComponentId },
    /// The detail of `component`, to open its code (at `line` in its file,
    /// or its span).
    Code { component: ComponentId, line: Option<u32> },
    /// The component at a line of code (the index hasn't said), to run `op`
    /// on it.
    At { op: GoTo },
}

/// A go-to under way: what it waits for, where it started, and the view it
/// started in (relational go-tos land in the same kind of view).
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub wait: Wait,
    pub from: Jump,
    pub origin: BufferKind,
}

/// A go-to with several places to land: the user picks one.
#[derive(Clone, Debug, PartialEq)]
pub struct Choosing {
    pub op: GoTo,
    pub from: Jump,
    pub origin: BufferKind,
    pub candidates: Vec<Candidate>,
}

type DetailReply = (ComponentId, Option<PlanId>, NetResult<ComponentDetail>);

/// Component details by (component, plan), fetched once each.
#[derive(Default)]
pub struct Details {
    cache: BTreeMap<(ComponentId, Option<PlanId>), ComponentDetail>,
    asked: BTreeSet<(ComponentId, Option<PlanId>)>,
    inbox: Arc<Mutex<Vec<DetailReply>>>,
    /// Lookups that failed, by component (said once, then dropped).
    pub failed: Vec<(ComponentId, String)>,
}

impl Details {
    pub fn get(&self, id: &ComponentId, plan: Option<&PlanId>) -> Option<&ComponentDetail> {
        self.cache.get(&(id.clone(), plan.cloned()))
    }

    pub fn insert(&mut self, detail: ComponentDetail, plan: Option<&PlanId>) {
        self.cache.insert((detail.component.id.clone(), plan.cloned()), detail);
    }

    /// Fetches a detail unless it's here or on its way.
    pub fn want(&mut self, id: &ComponentId, plan: Option<&PlanId>, net: &NetClient, ctx: &egui::Context) {
        let key = (id.clone(), plan.cloned());
        if self.cache.contains_key(&key) || !self.asked.insert(key) {
            return;
        }
        let inbox = self.inbox.clone();
        let (for_id, for_plan) = (id.clone(), plan.cloned());
        net.arch_component(id, plan, ctx.clone(), move |r| {
            if let Ok(mut i) = inbox.lock() {
                i.push((for_id, for_plan, r));
            }
        });
    }

    /// Takes the replies in.
    pub fn tick(&mut self) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for (id, plan, reply) in replies {
            match reply {
                Ok(detail) => {
                    self.asked.remove(&(id.clone(), plan.clone()));
                    self.cache.insert((id, plan), detail);
                }
                // A failed one isn't asked again until the architecture changes.
                // A late failure for something already here changes nothing.
                Err(e) if !self.cache.contains_key(&(id.clone(), plan)) => self.failed.push((id, e)),
                Err(_) => {}
            }
        }
    }

    /// The architecture changed: relations may have too.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.asked.clear();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use arch_graph::{Alignment, Component, Provenance, Relation, Span, Stage, Status};

    pub(crate) fn detail(id: &str, outgoing: Vec<Relation>, incoming: Vec<Relation>) -> ComponentDetail {
        ComponentDetail {
            component: Component {
                id: ComponentId::new(id),
                name: id.rsplit("::").next().unwrap().into(),
                type_name: "function".into(),
                description: String::new(),
                status: Status::Existing,
                stage: Stage::Implemented,
                anchor: None,
                proposed: None,
                signature: None,
                schemas: Default::default(),
                spec: None,
                placeholders: vec![],
            },
            alignment: Alignment::Observed,
            outgoing,
            incoming,
            code: None,
        }
    }

    // req: N.14
    #[test]
    fn relational_go_tos_follow_the_components_relations_in_order() {
        let call = |a: &str, b: &str, order: u32, line: u32| {
            let mut r = Relation::new(RelationKind::Calls, ComponentId::new(a), ComponentId::new(b), Provenance::OBSERVED);
            r.order = Some(order);
            r.sites = vec![Span { start_line: line, end_line: line, start_byte: 0, end_byte: 1 }];
            r
        };
        let d = detail(
            "rust:a::run",
            vec![call("rust:a::run", "rust:a::save", 1, 5), call("rust:a::run", "rust:a::step", 0, 4), call("rust:a::run", "rust:a::step", 2, 6)],
            vec![call("rust:a::main", "rust:a::run", 0, 12), Relation::new(RelationKind::Contains, ComponentId::new("rust:a"), ComponentId::new("rust:a::run"), Provenance::OBSERVED)],
        );
        let ids = |op| candidates(op, &d).into_iter().map(|c| c.id.to_string()).collect::<Vec<_>>();
        assert_eq!(ids(GoTo::Callees), ["rust:a::step", "rust:a::save"], "call order, each once");
        assert_eq!(candidates(GoTo::Callers, &d), vec![Candidate { id: ComponentId::new("rust:a::main"), line: Some(12) }], "a caller opens at the call");
        assert_eq!(ids(GoTo::Parent), ["rust:a"]);
        assert!(ids(GoTo::Implements).is_empty());
        assert!(candidates(GoTo::Code, &d).is_empty(), "not relational");
    }
}
