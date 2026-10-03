//! The outline: the structure view as a collapsible tree (what contains
//! what), with a filter. Picking an entry selects it in the graph.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use arch_graph::{ComponentId, PlanId, RelationKind, ViewResult};
use egui::RichText;

use crate::actions::{Action, ActionProvider, RunCx};
use crate::feedback::FeedbackInspectorState;
use crate::net::{NetClient, NetResult};
use crate::theme::colors;

#[derive(Default)]
pub struct ArchitectureTreeState {
    pub search_filter: String,
    plan: Option<PlanId>,
    result: Option<ViewResult>,
    stale: bool,
    loaded_once: bool,
    error: Option<String>,
    /// Select this in the graph.
    pub select: Option<ComponentId>,
    inbox: Arc<Mutex<Vec<NetResult<ViewResult>>>>,
}

impl ArchitectureTreeState {
    pub fn set_plan(&mut self, plan: Option<&PlanId>) {
        if self.plan.as_ref() != plan {
            self.plan = plan.cloned();
            self.stale = true;
        }
    }

    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Ok(result) => {
                    self.result = Some(result);
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if self.stale || !self.loaded_once {
            self.stale = false;
            self.loaded_once = true;
            let inbox = self.inbox.clone();
            net.arch_view("structure", self.plan.as_ref(), &[], Some(12), ctx.clone(), move |r| {
                if let Ok(mut i) = inbox.lock() {
                    i.push(r);
                }
            });
        }
    }

    pub fn render(&mut self, ui: &mut egui::Ui, net: &NetClient, feedback: &mut FeedbackInspectorState, slot_idx: usize) {
        self.tick(net, ui.ctx());
        let header = ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} Outline", egui_phosphor::regular::TREE_STRUCTURE)).strong().color(colors().info));
            ui.add(egui::TextEdit::singleline(&mut self.search_filter).hint_text("filter…").desired_width(160.0));
        });
        feedback.register_inspectable(ui, &format!("slot_{}:tree:header", slot_idx + 1), header.response.rect);
        ui.separator();
        let Some(result) = &self.result else {
            ui.label(RichText::new(self.error.clone().unwrap_or_else(|| "Loading…".into())).color(colors().muted));
            return;
        };
        let mut children: BTreeMap<&ComponentId, Vec<&ComponentId>> = BTreeMap::new();
        let mut has_parent: BTreeSet<&ComponentId> = BTreeSet::new();
        for e in result.view.edges.iter().filter(|e| e.kind == RelationKind::Contains) {
            if has_parent.insert(&e.target) {
                children.entry(&e.source).or_default().push(&e.target);
            }
        }
        let filter = self.search_filter.to_lowercase();
        let matches = |id: &ComponentId| filter.is_empty() || id.as_str().to_lowercase().contains(&filter);
        let mut picked = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let roots: Vec<&ComponentId> = result.view.nodes.iter().map(|n| &n.id).filter(|id| !has_parent.contains(id)).collect();
            for root in roots {
                outline(ui, result, root, &children, &matches, &mut picked, &mut BTreeSet::new());
            }
        });
        if picked.is_some() {
            self.select = picked;
        }
    }
}

fn outline<'a>(
    ui: &mut egui::Ui,
    result: &'a ViewResult,
    id: &'a ComponentId,
    children: &BTreeMap<&'a ComponentId, Vec<&'a ComponentId>>,
    matches: &dyn Fn(&ComponentId) -> bool,
    picked: &mut Option<ComponentId>,
    seen: &mut BTreeSet<&'a ComponentId>,
) {
    if !seen.insert(id) {
        return;
    }
    let Some(c) = result.components.get(id) else { return };
    let label = RichText::new(format!("{} {}", c.name, c.type_name)).size(12.0).color(if matches(id) { colors().text } else { colors().faint });
    match children.get(id) {
        Some(kids) => {
            let response = egui::CollapsingHeader::new(label).id_salt(id.as_str()).default_open(true).show(ui, |ui| {
                for kid in kids {
                    outline(ui, result, kid, children, matches, picked, seen);
                }
            });
            if response.header_response.secondary_clicked() || response.header_response.double_clicked() {
                *picked = Some(id.clone());
            }
        }
        None => {
            if ui.selectable_label(false, label).clicked() {
                *picked = Some(id.clone());
            }
        }
    }
}

impl ActionProvider for ArchitectureTreeState {
    fn actions(&self) -> Vec<Action> {
        Vec::new()
    }

    fn run(&mut self, _action: &Action, _value: Option<String>, _cx: &mut RunCx<'_>) {}
}
