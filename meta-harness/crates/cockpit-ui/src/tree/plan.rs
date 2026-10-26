//! A plan as a tree source: its items, nested (each with its state and
//! its discussions' state), then a change plan's (or a walkthrough's)
//! components, each with its card's sections; each part's discussions as
//! rows under it. It shows the plan the desk reads (`Shared::review`,
//! read once for every buffer). A row stands for a plan's part
//! (`Target::Plan`): going to one opens it where it is; the things at a
//! row are that part (a discussion's row: the discussion, then its part).

use std::collections::VecDeque;

use arch_graph::{Plan, PlanId};
use harness_contracts::{Discussion, DiscussionId, DiscussionStatus, Subject};

use super::source::{RowData, Rows, TreeSource};
use crate::nav::{PlanPart, Target};
use crate::net::NetClient;
use crate::shared::Shared;

#[derive(Default)]
pub struct PlanTree {
    /// The plan shown (none: the desk reads the code alone).
    plan: Option<PlanId>,
    /// It as last read, and the discussions on its parts.
    read: Option<Result<Plan, String>>,
    discussions: Vec<Discussion>,
    /// The review's changes taken.
    seen: u64,
    /// Rows asked for, answered once the plan's read.
    asked: VecDeque<u64>,
    /// Whose move it is in each of its discussions, as last said.
    whose: Whose,
}

/// A card's section's title, as the card says it (`logic`: Logic steps).
fn titled(section: &str) -> String {
    crate::card::Section::from_key(section).map_or_else(|| section.to_owned(), |s| s.title().to_owned())
}

/// Whose move it is in each discussion, in a few words (as the agents
/// say it), by its id.
pub(crate) type Whose = std::collections::BTreeMap<DiscussionId, String>;

fn whose(d: &Discussion, said: &Whose) -> String {
    said.get(&d.id).cloned().unwrap_or_default()
}

/// The discussions not settled on a part, in a few words for its row
/// (`1 waiting on you, 2 on lead`); none when there are none.
fn waiting(on: &[&Discussion], said: &Whose) -> Option<String> {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for d in on.iter().filter(|d| d.status != DiscussionStatus::Settled) {
        *counts.entry(whose(d, said)).or_default() += 1;
    }
    (!counts.is_empty()).then(|| counts.into_iter().map(|(who, n)| format!("{n} {who}")).collect::<Vec<_>>().join(", "))
}

/// A discussion's row: its id, what it is and asks, whose move it is.
fn discussion_row(d: &Discussion, parent: Option<&str>, said: &Whose) -> RowData {
    let asking: String = d.asking().lines().next().unwrap_or_default().chars().take(80).collect();
    RowData { key: format!("discussion:{}", d.id), label: format!("{} {}: {asking}", d.id, d.kind.words()), kind: whose(d, said), parent: parent.map(str::to_owned), more: 0 }
}

/// The rows of `plan`: its own discussions, items (nested), then
/// components and their sections; under each part, the discussions on it.
pub(crate) fn rows_of(plan: &Plan, discussions: &[Discussion], said: &Whose) -> Rows {
    let on = |subject: &Subject| discussions.iter().filter(|d| d.on.subject == *subject).collect::<Vec<_>>();
    // A part's row says what its discussions wait on (`also`: those of what
    // hangs off it, listed after: a component's relations').
    let part = |rows: &mut Vec<RowData>, row: RowData, subject: Subject, also: &[&Discussion]| {
        let here = on(&subject);
        let key = row.key.clone();
        let counted: Vec<&Discussion> = here.iter().chain(also).copied().collect();
        let kind = match (waiting(&counted, said), row.kind.is_empty()) {
            (Some(words), true) => words,
            (Some(words), false) => format!("{} · {words}", row.kind),
            (None, _) => row.kind,
        };
        rows.push(RowData { kind, ..row });
        rows.extend(here.into_iter().map(|d| discussion_row(d, Some(&key), said)));
    };
    let mut rows: Vec<RowData> = on(&Subject::Plan { plan: plan.id.clone() }).into_iter().map(|d| discussion_row(d, None, said)).collect();
    for i in &plan.items {
        let row = RowData { key: format!("item:{}", i.id), label: i.title.clone(), kind: i.state.name().to_owned(), parent: i.parent.as_ref().map(|p| format!("item:{p}")), more: 0 };
        part(&mut rows, row, Subject::PlanItem { plan: plan.id.clone(), item: i.id.0.clone() }, &[]);
    }
    for c in &plan.components {
        let key = format!("component:{}", c.id);
        // Its relations' discussions (those from it), under it, and counted on it.
        let related: Vec<&Discussion> = discussions.iter().filter(|d| matches!(&d.on.subject, Subject::PlanRelation { source, .. } if *source == c.id)).collect();
        part(&mut rows, RowData { key: key.clone(), label: c.name.clone(), kind: c.type_name.clone(), parent: None, more: 0 }, Subject::PlanComponent { plan: plan.id.clone(), id: c.id.clone() }, &related);
        for s in harness_contracts::CARD_SECTIONS {
            let row = RowData { key: format!("section:{}:{s}", c.id), label: titled(s), kind: String::new(), parent: Some(key.clone()), more: 0 };
            part(&mut rows, row, Subject::PlanSection { plan: plan.id.clone(), id: c.id.clone(), section: s.to_owned() }, &[]);
        }
        rows.extend(related.into_iter().map(|d| discussion_row(d, Some(&key), said)));
    }
    Rows { rows, dropped: Vec::new(), cut: None }
}

impl PlanTree {
    /// The discussion a row is.
    fn discussion(&self, key: &str) -> Option<&Discussion> {
        let id = DiscussionId::new(key.strip_prefix("discussion:")?);
        self.discussions.iter().find(|d| d.id == id)
    }

    /// The plan's part a row is (a discussion's: the part it's on).
    fn part(&self, key: &str) -> Option<PlanPart> {
        if let Some(d) = self.discussion(key) {
            return match Target::of_subject(&d.on.subject)? {
                Target::Plan { part, .. } => Some(part),
                _ => None,
            };
        }
        Some(if let Some(item) = key.strip_prefix("item:") {
            PlanPart::Item(item.to_owned())
        } else if let Some(id) = key.strip_prefix("component:") {
            PlanPart::Component(arch_graph::ComponentId::new(id))
        } else {
            let (id, section) = key.strip_prefix("section:")?.rsplit_once(':')?;
            PlanPart::Section(arch_graph::ComponentId::new(id), section.to_owned())
        })
    }
}

impl TreeSource for PlanTree {
    fn title(&self) -> String {
        match &self.plan {
            Some(plan) => format!("Plan {plan}"),
            None => "Plan".into(),
        }
    }

    /// Answered from the plan the desk read (once it's in).
    fn ask(&mut self, number: u64, _opened: &[String], _net: &NetClient, _ctx: &egui::Context) {
        self.asked.push_back(number);
    }

    fn answers(&mut self) -> Vec<(u64, Result<Rows, String>)> {
        let answer = match (&self.plan, &self.read) {
            (None, _) => Err("No plan to show: show one in the graph (its source), and it shows here.".to_owned()),
            (Some(_), None) => return Vec::new(),
            (Some(_), Some(Err(e))) => Err(e.clone()),
            (Some(_), Some(Ok(plan))) => Ok(rows_of(plan, &self.discussions, &self.whose)),
        };
        self.asked.drain(..).map(|n| (n, answer.clone())).collect()
    }

    /// The plan the desk reads, read again or discussed, or whose move it
    /// is in its discussions said otherwise (the agents' states): its rows
    /// again.
    fn share(&mut self, shared: &Shared) -> bool {
        let read = match shared.review_since(&mut self.seen) {
            Some(review) => {
                self.plan.clone_from(&review.plan);
                self.read.clone_from(&review.read);
                self.discussions.clone_from(&review.discussions);
                true
            }
            None => false,
        };
        let agents = shared.agents.get();
        let whose: Whose = self.discussions.iter().map(|d| (d.id.clone(), agents.whose(d).0)).collect();
        let said = whose != self.whose;
        self.whose = whose;
        read || said
    }

    /// A discussion's row: the discussion; else the plan's part.
    fn target(&self, key: &str) -> Option<Target> {
        if let Some(d) = self.discussion(key) {
            return Some(Target::Discussion(d.id.clone()));
        }
        Some(Target::Plan { plan: self.plan.clone()?, part: self.part(key)? })
    }

    fn key_of(&self, target: &Target) -> Option<String> {
        if let Target::Discussion(id) = target {
            return self.discussions.iter().any(|d| d.id == *id).then(|| format!("discussion:{id}"));
        }
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

    /// An item: its card; a discussion: the discussion.
    fn opens(&self, key: &str) -> Option<(Option<crate::nav::GoTo>, String)> {
        if let Some(d) = self.discussion(key) {
            return Some((None, format!("Open {}", d.id)));
        }
        let item = key.strip_prefix("item:")?;
        let title = match &self.read {
            Some(Ok(plan)) => plan.item(&arch_graph::ItemId(item.to_owned())).map_or_else(|| item.to_owned(), |i| i.title.clone()),
            _ => item.to_owned(),
        };
        Some((Some(crate::nav::GoTo::Spec), format!("Open {title}'s card")))
    }


    /// A discussion: what it's on; a section: its card; then the plan.
    fn holders(&self, key: &str) -> Vec<Subject> {
        let Some(plan) = self.plan.clone() else { return Vec::new() };
        let mut out = Vec::new();
        if let Some(d) = self.discussion(key) {
            out.push(d.on.subject.clone());
        }
        if let Some((id, _)) = key.strip_prefix("section:").and_then(|rest| rest.rsplit_once(':')) {
            out.push(Subject::PlanComponent { plan: plan.clone(), id: arch_graph::ComponentId::new(id) });
        }
        out.push(Subject::Plan { plan });
        out.dedup();
        out
    }
}
