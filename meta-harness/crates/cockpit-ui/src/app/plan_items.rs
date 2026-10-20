//! A plan's items, for the shell: an item's state set on whatever item the
//! cursor's on, in any view (the plan tree's row, the item's card). A view
//! only says the item is a thing at its cursor (`Navigable::things`).

use arch_graph::{ItemId, ItemState, PlanId};
use harness_contracts::Subject;

use super::*;
use crate::actions::{Choice, Group};

impl CockpitApp {
    /// The item at the cursor where you are (none: it isn't on one).
    fn item_here(&self, here: Here) -> Option<(PlanId, ItemId)> {
        let view = self.desk.view(here.kind);
        let things = view.cursor_key().map(|k| view.things(&k)).unwrap_or_default();
        things.at.into_iter().find_map(|t| match t.subject {
            Subject::PlanItem { plan, item } => Some((plan, ItemId(item))),
            _ => None,
        })
    }

    /// Setting the state of the item you're on.
    pub(super) fn item_actions(&self, here: Here) -> Vec<Action> {
        let Some((plan, item)) = self.item_here(here) else { return Vec::new() };
        let choices = ItemState::ALL.into_iter().map(|s| Choice::new(s.name(), s.name())).collect();
        vec![Action::new("plan.item_state", format!("Set {item}'s state"), "Open, decided, done or dropped: the plan keeps it", ShellCommand::ItemState(plan, item)).param(Param::Choice(choices)).group(Group::Act).importance(MINOR)]
    }

    /// Sets an item's state (`value`: its name); what came of it is said.
    pub(super) fn set_item_state(&mut self, plan: &PlanId, item: &ItemId, value: Option<String>, ctx: &egui::Context) {
        let Some(state) = value.and_then(|v| ItemState::ALL.into_iter().find(|s| s.name() == v)) else { return };
        let notes = self.notes_later.clone();
        let said = format!("{item} is {} now.", state.name());
        let ops = serde_json::json!({ "ops": [{ "op": "set_item", "id": item, "state": state }] });
        self.desk.net.arch_plan_ops(plan, ops, ctx.clone(), move |r| {
            if let Ok(mut notes) = notes.lock() {
                notes.push(match r {
                    Ok(_) => said,
                    Err(e) => format!("Couldn't set its state: {}", e.message),
                });
            }
        });
    }
}
