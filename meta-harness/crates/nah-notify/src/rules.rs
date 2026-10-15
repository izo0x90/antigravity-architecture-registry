//! What each event means for notifications. Pure: what it needs to know
//! about the world comes through `World`.

use std::collections::BTreeMap;

use harness_contracts::{
    AdapterEvent, AuthStatus, ClientEvent, InstanceId, Notification, NotificationId, NotificationKind, NotificationState, RequestKind, RequestStatus, SlotId, ThreadId, ThreadStatus,
    TurnId, TurnStatus,
};

/// A slot as notifications name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotRef {
    pub id: SlotId,
    pub name: String,
    pub lead: bool,
}

pub trait World {
    /// The slot running `thread`.
    fn slot_of(&self, thread: &ThreadId) -> Option<SlotRef>;
    /// Whether a slot uses this harness.
    fn in_use(&self, instance: &InstanceId) -> bool;
    /// Whether the thread's end was asked for.
    fn closed_on_purpose(&self, thread: &ThreadId) -> bool;
    /// Whether a turn waits behind the running one in `thread`.
    fn queued(&self, thread: &ThreadId) -> bool;
    /// Whether `turn` was seen running (or queued). A turn first seen
    /// already ended is history a resumed session replays, not news.
    fn was_running(&self, turn: &TurnId) -> bool;
    /// What the turn said (its assistant messages).
    fn reply(&self, thread: &ThreadId, turn: &TurnId) -> String;
    /// How many times this harness needed signing in before.
    fn sign_ins(&self, instance: &InstanceId) -> u32;
    fn now_ms(&self) -> u64;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// A new one (a repeat of one that exists, in any state, changes nothing).
    Open(Notification),
    /// An open one ends.
    End { id: NotificationId, to: NotificationState },
}

/// The longest detail kept (a turn's reply, say): the rest is in the chat.
const DETAIL_CHARS: usize = 400;

fn clip(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut clipped: String = text.chars().take(DETAIL_CHARS).collect();
    if clipped.len() < text.len() {
        clipped.push('…');
    }
    Some(clipped)
}

fn new(id: NotificationId, kind: NotificationKind, slot: Option<&SlotRef>, thread: Option<&ThreadId>, title: String, detail: Option<String>, world: &dyn World) -> Notification {
    Notification {
        id,
        server: None,
        level: kind.level(),
        kind,
        slot: slot.map(|s| s.id.clone()),
        thread: thread.cloned(),
        request: None,
        title,
        detail,
        at_ms: world.now_ms(),
        state: NotificationState::Open,
    }
}

fn sign_in_prefix(instance: &InstanceId) -> String {
    format!("signin:{instance}:")
}

/// What `event` changes, given what's open.
pub fn derive(event: &ClientEvent, world: &dyn World, open: &BTreeMap<NotificationId, Notification>) -> Vec<Change> {
    match event {
        ClientEvent::Session { event: AdapterEvent::RequestUpdated { request } } => {
            let id = NotificationId::request(&request.id);
            if request.status != RequestStatus::Pending {
                return open.contains_key(&id).then_some(Change::End { id, to: NotificationState::Done }).into_iter().collect();
            }
            let kind = match request.kind {
                RequestKind::Permission | RequestKind::Command | RequestKind::FileRead | RequestKind::FileChange => NotificationKind::Approval,
                RequestKind::UserInput => NotificationKind::Question,
                RequestKind::McpElicitation => NotificationKind::Link,
            };
            let slot = world.slot_of(&request.thread_id);
            let mut n = new(id, kind, slot.as_ref(), Some(&request.thread_id), request.title.clone(), request.detail.clone(), world);
            n.request = Some(request.id.clone());
            vec![Change::Open(n)]
        }
        ClientEvent::InstanceUpdated { snapshot } => {
            let prefix = sign_in_prefix(&snapshot.instance_id);
            let mine: Vec<&NotificationId> = open.keys().filter(|id| id.as_str().starts_with(&prefix)).collect();
            match snapshot.auth.status {
                AuthStatus::SignedOut if mine.is_empty() && world.in_use(&snapshot.instance_id) => {
                    let id = NotificationId::sign_in(&snapshot.instance_id, world.sign_ins(&snapshot.instance_id) + 1);
                    let title = format!("{} needs signing in", snapshot.display_name);
                    vec![Change::Open(new(id, NotificationKind::SignIn, None, None, title, None, world))]
                }
                AuthStatus::SignedIn => mine.into_iter().map(|id| Change::End { id: id.clone(), to: NotificationState::Done }).collect(),
                _ => Vec::new(),
            }
        }
        ClientEvent::Session { event: AdapterEvent::ThreadUpdated { thread } } if thread.status == ThreadStatus::Error => crashed(&thread.id, "its harness reported an error", world),
        ClientEvent::ThreadClosed { thread_id } if !world.closed_on_purpose(thread_id) => crashed(thread_id, "it ended without being asked to", world),
        ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn } } => {
            if !world.was_running(&turn.id) {
                return Vec::new();
            }
            let Some(slot) = world.slot_of(&turn.thread_id) else { return Vec::new() };
            let id = NotificationId::turn(&turn.id);
            match turn.status {
                TurnStatus::Failed => {
                    let detail = turn.failure.as_ref().map(|f| f.message.clone());
                    vec![Change::Open(new(id, NotificationKind::TurnFailed, Some(&slot), Some(&turn.thread_id), format!("{}'s turn failed", slot.name), detail, world))]
                }
                TurnStatus::Completed => {
                    let title = if slot.lead && !world.queued(&turn.thread_id) { "The lead is idle".to_owned() } else { format!("{} finished a turn", slot.name) };
                    let detail = clip(&world.reply(&turn.thread_id, &turn.id));
                    vec![Change::Open(new(id, NotificationKind::TurnFinished, Some(&slot), Some(&turn.thread_id), title, detail, world))]
                }
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

fn crashed(thread: &ThreadId, why: &str, world: &dyn World) -> Vec<Change> {
    let Some(slot) = world.slot_of(thread) else { return Vec::new() };
    let title = format!("{}'s session stopped", slot.name);
    vec![Change::Open(new(NotificationId::crash(thread), NotificationKind::SessionCrashed, Some(&slot), Some(thread), title, Some(format!("{why}; its next prompt resumes it")), world))]
}

/// Something an agent says to the user (or a sub-slot's report, which the
/// user sees too).
pub fn message(from: Option<&SlotRef>, text: &str, world: &dyn World) -> Notification {
    let title = match from {
        Some(slot) => format!("From {}", slot.name),
        None => "From an agent".to_owned(),
    };
    new(NotificationId::message(&uuid::Uuid::new_v4().simple().to_string()), NotificationKind::Message, from, None, title, Some(text.trim().to_owned()), world)
}

#[cfg(test)]
mod tests;
