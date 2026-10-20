//! A plan as a tree source: its items, nested (each with its state), then
//! a change plan's (or a walkthrough's) components, each with its card's
//! sections. It shows the plan the desk reads from (`Shared::source`). A
//! row stands for a plan's part (`Target::Plan`): going to one opens it
//! where it is; the things at a row are that part.

use std::sync::{Arc, Mutex};

use arch_graph::{ArchEvent, Plan, PlanId, Source};

use super::source::{RowData, Rows, TreeSource};
use crate::nav::{PlanPart, Target};
use crate::net::{NetClient, NetResult};
use crate::shared::Shared;

/// An answer, with the number of the request it answers.
type Answer = (u64, NetResult<Plan>);

#[derive(Default)]
pub struct PlanTree {
    /// The plan shown (none: the desk reads the code alone).
    plan: Option<PlanId>,
    inbox: Arc<Mutex<Vec<Answer>>>,
}

/// A card's section's title, as the card says it (`logic`: Logic steps).
fn titled(section: &str) -> String {
    crate::card::Section::from_key(section).map_or_else(|| section.to_owned(), |s| s.title().to_owned())
}

/// The rows of `plan`: items, nested, then components and their sections.
pub(crate) fn rows_of(plan: &Plan) -> Rows {
    let mut rows: Vec<RowData> = plan.items.iter().map(|i| RowData { key: format!("item:{}", i.id), label: i.title.clone(), kind: i.state.name().to_owned(), parent: i.parent.as_ref().map(|p| format!("item:{p}")), more: 0 }).collect();
    for c in &plan.components {
        let key = format!("component:{}", c.id);
        rows.push(RowData { key: key.clone(), label: c.name.clone(), kind: c.type_name.clone(), parent: None, more: 0 });
        rows.extend(harness_contracts::CARD_SECTIONS.iter().map(|s| RowData { key: format!("section:{}:{s}", c.id), label: titled(s), kind: "section".into(), parent: Some(key.clone()), more: 0 }));
    }
    Rows { rows, dropped: Vec::new(), cut: None }
}

impl TreeSource for PlanTree {
    fn title(&self) -> String {
        match &self.plan {
            Some(plan) => format!("Plan {plan}"),
            None => "Plan".into(),
        }
    }

    fn ask(&mut self, number: u64, _opened: &[String], net: &NetClient, ctx: &egui::Context) {
        let inbox = self.inbox.clone();
        let Some(plan) = &self.plan else {
            if let Ok(mut i) = inbox.lock() {
                i.push((number, Err(crate::net::NetError { status: None, message: "No plan to show: show one in the graph (its source), and it shows here.".into() })));
            }
            return;
        };
        net.arch_plan(plan, ctx.clone(), move |r| {
            if let Ok(mut i) = inbox.lock() {
                i.push((number, r));
            }
        });
    }

    fn answers(&mut self) -> Vec<(u64, Result<Rows, String>)> {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        replies.into_iter().map(|(n, r)| (n, r.map(|p| rows_of(&p)).map_err(String::from))).collect()
    }

    /// Another plan read from: its rows.
    fn share(&mut self, shared: &Shared) -> bool {
        let plan = match shared.source.get() {
            Source::Plan(plan) => Some(plan.clone()),
            Source::Code => None,
        };
        let changed = plan != self.plan;
        self.plan = plan;
        changed
    }

    /// Its plan changed.
    fn on_event(&mut self, event: &ArchEvent) -> bool {
        matches!(event, ArchEvent::Changed { plans, .. } if self.plan.as_ref().is_some_and(|p| plans.contains(p)))
    }

    fn target(&self, key: &str) -> Option<Target> {
        let plan = self.plan.clone()?;
        let part = if let Some(item) = key.strip_prefix("item:") {
            PlanPart::Item(item.to_owned())
        } else if let Some(id) = key.strip_prefix("component:") {
            PlanPart::Component(arch_graph::ComponentId::new(id))
        } else {
            let (id, section) = key.strip_prefix("section:")?.rsplit_once(':')?;
            PlanPart::Section(arch_graph::ComponentId::new(id), section.to_owned())
        };
        Some(Target::Plan { plan, part })
    }

    /// An item: its card.
    fn opens(&self, key: &str) -> Option<crate::nav::GoTo> {
        key.starts_with("item:").then_some(crate::nav::GoTo::Spec)
    }

    /// A section: its card, then the plan; a card or an item: the plan.
    fn holders(&self, key: &str) -> Vec<harness_contracts::Subject> {
        use harness_contracts::Subject;
        let Some(plan) = self.plan.clone() else { return Vec::new() };
        let mut out = Vec::new();
        if let Some((id, _)) = key.strip_prefix("section:").and_then(|rest| rest.rsplit_once(':')) {
            out.push(Subject::PlanComponent { plan: plan.clone(), id: arch_graph::ComponentId::new(id) });
        }
        out.push(Subject::Plan { plan });
        out
    }

    fn key_of(&self, target: &Target) -> Option<String> {
        let Target::Plan { plan, part } = target else { return None };
        if Some(plan) != self.plan.as_ref() {
            return None;
        }
        match part {
            PlanPart::Item(item) => Some(format!("item:{item}")),
            PlanPart::Component(id) => Some(format!("component:{id}")),
            PlanPart::Section(id, section) => Some(format!("section:{id}:{section}")),
            PlanPart::Relation { .. } | PlanPart::Whole => None,
        }
    }
}
