//! Slots: seats for agents. One lead slot drives the IDE; sub-slots work
//! under it; the work queue's jobs run in worker slots. A slot keeps its
//! sessions (one current), each resumable after a restart by the harness's
//! own id; whether one is running is live state, never stored.

use serde::{Deserialize, Serialize};

use crate::ids::{InstanceId, NativeId, SessionKey, SlotId, ThreadId};
use crate::policy::{ModelSelection, RuntimePolicy};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Slot {
    pub id: SlotId,
    /// Its role (what its agent may do): "lead", "sub", "worker".
    pub role: String,
    pub name: String,
    /// Its harness; the lead's is chosen before its first session.
    pub instance_id: Option<InstanceId>,
    pub policy: RuntimePolicy,
    pub model: Option<ModelSelection>,
    pub sessions: Vec<SlotSession>,
    pub current: Option<SessionKey>,
    pub started_by: Starter,
    /// Whether its harness can run (live, from the harness).
    pub status: SlotStatus,
}

impl Slot {
    pub fn session(&self, key: &SessionKey) -> Option<&SlotSession> {
        self.sessions.iter().find(|s| &s.key == key)
    }

    pub fn current_session(&self) -> Option<&SlotSession> {
        self.current.as_ref().and_then(|k| self.session(k))
    }

    /// The thread of its current session, if that session is running.
    pub fn current_thread(&self) -> Option<&ThreadId> {
        self.current_session().and_then(|s| s.thread.as_ref())
    }

    pub fn is_lead(&self) -> bool {
        self.role == LEAD
    }
}

/// The lead slot's role (and its id).
pub const LEAD: &str = "lead";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotSession {
    pub key: SessionKey,
    /// The harness's id for it, to resume it.
    pub native_id: Option<NativeId>,
    pub title: Option<String>,
    /// Its thread while it runs; none when it isn't running.
    pub thread: Option<ThreadId>,
}

/// Who started a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Starter {
    User,
    Lead,
    Queue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum SlotStatus {
    Ready,
    /// Its harness can't run now: why, and what fixes it.
    Unavailable { reason: String, fix: Option<String> },
}
