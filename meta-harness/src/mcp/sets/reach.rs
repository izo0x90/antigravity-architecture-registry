//! What an agent may read or note of a thing: anything but a chat; a chat
//! its own slot's, or (with `slots.observe`) a sub-slot's or a closed
//! slot's, as `slot_read` lets it; a discussion as what it's on. The user,
//! anything. Carried into the work tools do off the async threads.

use std::sync::Arc;

use harness_contracts::Subject;
use nah_access::Capability;

pub(crate) struct Reach {
    pub(crate) slots: Arc<harness_core::SlotManager>,
    pub(crate) access: Arc<nah_access::Access>,
    pub(crate) discussions: Arc<nah_discussions::Discussions>,
    pub(crate) principal: nah_access::Principal,
    pub(crate) slot: Option<harness_contracts::SlotId>,
    /// The slots, read once for all the things one call asks about.
    listed: std::sync::OnceLock<Vec<harness_contracts::Slot>>,
}

/// Said of a thing a caller may not read.
pub(crate) const NOT_YOURS: &str = "a chat this session can't read (its own slot's, or with slots.observe a sub-slot's or a closed slot's)";

impl Reach {
    pub(crate) fn of(slots: &Arc<harness_core::SlotManager>, access: &Arc<nah_access::Access>, discussions: &Arc<nah_discussions::Discussions>, caller: &crate::mcp::McpCaller) -> Arc<Self> {
        Arc::new(Self { slots: slots.clone(), access: access.clone(), discussions: discussions.clone(), principal: caller.principal.clone(), slot: caller.slot.clone(), listed: Default::default() })
    }

    /// What slot `slot` may read (as its sessions' tools would).
    pub(crate) fn of_slot(slots: &Arc<harness_core::SlotManager>, access: &Arc<nah_access::Access>, discussions: &Arc<nah_discussions::Discussions>, slot: &harness_contracts::Slot) -> Self {
        Self { slots: slots.clone(), access: access.clone(), discussions: discussions.clone(), principal: nah_access::Principal::agent(&slot.role), slot: Some(slot.id.clone()), listed: Default::default() }
    }

    pub(crate) fn reaches(&self, subject: &Subject) -> bool {
        if matches!(self.principal, nah_access::Principal::User | nah_access::Principal::Home) {
            return true;
        }
        let session = match subject {
            Subject::ChatSession { session } | Subject::ChatTurn { session, .. } | Subject::ChatMessage { session, .. } => session,
            // A discussion as what it's on (one on a discussion, as that one).
            Subject::Discussion { id } => return self.discussions.get(id).is_ok_and(|d| d.on.subject != *subject && self.reaches(&d.on.subject)),
            _ => return true,
        };
        let observes = self.access.allows(&self.principal, Capability::SlotsObserve);
        // A session no slot holds now (its slot closed): what was kept, to one that observes.
        let listed = self.listed.get_or_init(|| self.slots.list());
        let Some(slot) = listed.iter().find(|s| s.sessions.iter().any(|x| x.native_id.as_ref() == Some(session))) else { return observes };
        self.slot.as_ref() == Some(&slot.id) || (slot.role == "sub" && observes)
    }
}
