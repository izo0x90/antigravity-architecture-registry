//! The order the chat goes through its slots: the lead, then sub-slots,
//! then queue workers, each by name. The slot picker lists them so, and
//! the next/previous slot keys step through them so.

use harness_contracts::{Slot, SlotId};

use crate::store::Store;

/// Where a role goes: the lead first, workers last, unknown roles after.
fn rank(role: &str) -> u8 {
    match role {
        "lead" => 0,
        "sub" => 1,
        "worker" => 2,
        _ => 3,
    }
}

/// Every slot, in the chat's order.
pub(crate) fn ordered(store: &Store) -> Vec<&Slot> {
    let mut slots: Vec<&Slot> = store.slots.values().collect();
    slots.sort_by(|a, b| (rank(&a.role), &a.name, &a.id).cmp(&(rank(&b.role), &b.name, &b.id)));
    slots
}

/// The slot `by` places from `from` (none past either end).
pub(crate) fn beside(store: &Store, from: &SlotId, by: i32) -> Option<SlotId> {
    let slots = ordered(store);
    let at = slots.iter().position(|s| &s.id == from)?;
    let to = at.checked_add_signed(isize::try_from(by).ok()?)?;
    slots.get(to).map(|s| s.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(id: &str, role: &str, name: &str) -> Slot {
        Slot { id: SlotId::new(id), role: role.into(), name: name.into(), ..crate::chat::tests::lead(None) }
    }

    #[test]
    fn the_lead_then_sub_slots_then_workers_each_by_name() {
        let mut store = Store::default();
        for s in [slot("w1", "worker", "alpha"), slot("s2", "sub", "zed"), slot("s1", "sub", "beta"), slot("lead", "lead", "main")] {
            store.slots.insert(s.id.clone(), s);
        }
        let names: Vec<&str> = ordered(&store).iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["main", "beta", "zed", "alpha"]);
        assert_eq!(beside(&store, &SlotId::new("lead"), 1), Some(SlotId::new("s1")));
        assert_eq!(beside(&store, &SlotId::new("s2"), -1), Some(SlotId::new("s1")));
        assert_eq!(beside(&store, &SlotId::new("w1"), 1), None, "the last");
        assert_eq!(beside(&store, &SlotId::new("lead"), -1), None, "the first");
    }
}
