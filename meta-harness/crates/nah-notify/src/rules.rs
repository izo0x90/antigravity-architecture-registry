//! What each event means for notifications. Pure: what it needs to know
//! about the world comes through `World`.

use std::collections::BTreeMap;

use harness_contracts::{
    AdapterEvent, AuthStatus, ClientEvent, InstanceId, Notification, NotificationId, NotificationKind, NotificationState, RequestKind, RequestStatus, SlotId, Terminal, TerminalNews, ThreadId,
    ThreadStatus, TurnId, TurnStatus,
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
    Open(Box<Notification>),
    /// An open one ends.
    End { id: NotificationId, to: NotificationState },
}

/// A new one, as a change.
fn opened(notification: Notification) -> Change {
    Change::Open(Box::new(notification))
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
        terminal: None,
        discussion: None,
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
                RequestKind::Approval { .. } => NotificationKind::Approval,
                RequestKind::Question => NotificationKind::Question,
                RequestKind::Link => NotificationKind::Link,
            };
            let slot = world.slot_of(&request.thread_id);
            let mut n = new(id, kind, slot.as_ref(), Some(&request.thread_id), request.title.clone(), request.detail.clone(), world);
            n.request = Some(request.id.clone());
            vec![opened(n)]
        }
        ClientEvent::InstanceUpdated { snapshot } => {
            let prefix = sign_in_prefix(&snapshot.instance_id);
            let mine: Vec<&NotificationId> = open.keys().filter(|id| id.as_str().starts_with(&prefix)).collect();
            match snapshot.auth.status {
                AuthStatus::SignedOut if mine.is_empty() && world.in_use(&snapshot.instance_id) => {
                    let id = NotificationId::sign_in(&snapshot.instance_id, world.sign_ins(&snapshot.instance_id) + 1);
                    let title = format!("{} needs signing in", snapshot.display_name);
                    vec![opened(new(id, NotificationKind::SignIn, None, None, title, None, world))]
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
                    vec![opened(new(id, NotificationKind::TurnFailed, Some(&slot), Some(&turn.thread_id), format!("{}'s turn failed", slot.name), detail, world))]
                }
                TurnStatus::Completed => {
                    let title = if slot.lead && !world.queued(&turn.thread_id) { "The lead is idle".to_owned() } else { format!("{} finished a turn", slot.name) };
                    let detail = clip(&world.reply(&turn.thread_id, &turn.id));
                    vec![opened(new(id, NotificationKind::TurnFinished, Some(&slot), Some(&turn.thread_id), title, detail, world))]
                }
                _ => Vec::new(),
            }
        }
        // A bell rung while one is open is the same news.
        ClientEvent::TerminalNews { news: TerminalNews::Bell, terminal } if open.values().any(|n| n.kind == NotificationKind::TerminalBell && n.terminal.as_ref() == Some(&terminal.id)) => Vec::new(),
        ClientEvent::TerminalNews { terminal, news } => vec![opened(terminal_news(terminal, *news, world))],
        ClientEvent::DiscussionUpdated { discussion } => discussion_news(discussion, world, open),
        ClientEvent::DiscussionRemoved { discussion_id } => open.values().filter(|n| n.discussion.as_ref() == Some(discussion_id)).map(|n| Change::End { id: n.id.clone(), to: NotificationState::Done }).collect(),
        _ => Vec::new(),
    }
}

/// A discussion waiting on the user (answered: settle or reopen it; an
/// agent asking them), a notification each time it is anew (the one
/// before done); done once it isn't.
fn discussion_news(d: &harness_contracts::Discussion, world: &dyn World, open: &BTreeMap<NotificationId, Notification>) -> Vec<Change> {
    let id = NotificationId::discussion(&d.id, d.said.len());
    let now: Vec<&NotificationId> = open.values().filter(|n| n.discussion.as_ref() == Some(&d.id)).map(|n| &n.id).collect();
    let mut changes: Vec<Change> = now.iter().filter(|n| ***n != id || d.whose_move() != harness_contracts::Move::User).map(|n| Change::End { id: (*n).clone(), to: NotificationState::Done }).collect();
    if d.whose_move() != harness_contracts::Move::User || now.contains(&&id) {
        return changes;
    }
    let last = d.said.last();
    let who = |by: &str| d.to_user(by).to_owned();
    let title = match (d.status, last) {
        (harness_contracts::DiscussionStatus::Answered, Some(said)) => format!("{} answered {} on {}: settle or reopen it", who(&said.by), d.id, d.on.what),
        (_, Some(said)) if d.said.len() > 1 => format!("{} said something back in {} on {}", who(&said.by), d.id, d.on.what),
        _ => format!("{} asks you on {} ({})", who(&d.by), d.on.what, d.id),
    };
    let mut n = new(id, NotificationKind::Discussion, None, None, title, last.and_then(|s| clip(&s.text)), world);
    n.discussion = Some(d.id.clone());
    changes.push(opened(n));
    changes
}

/// What a terminal's program or shell did, as a notification about it
/// (seen once the user looks at the terminal).
fn terminal_news(terminal: &Terminal, news: TerminalNews, world: &dyn World) -> Notification {
    let name = if terminal.title.is_empty() { terminal.id.to_string() } else { terminal.title.clone() };
    let (what, kind, title, detail) = match news {
        TerminalNews::Bell => ("bell", NotificationKind::TerminalBell, format!("{name} rang its bell"), None),
        TerminalNews::Ended { code } => ("end", NotificationKind::ShellEnded, format!("{name}: its shell ended"), code.map(|c| format!("exit code {c}"))),
        TerminalNews::Finished { code, ran_ms } => {
            let took = took(ran_ms);
            match code {
                Some(0) | None => ("done", NotificationKind::CommandFinished, format!("{name}: a command finished"), Some(format!("after {took}"))),
                Some(c) => ("done", NotificationKind::CommandFailed, format!("{name}: a command failed"), Some(format!("exit code {c}, after {took}"))),
            }
        }
    };
    let mut n = new(NotificationId::terminal(&terminal.id, what, world.now_ms()), kind, None, None, title, detail, world);
    n.terminal = Some(terminal.id.clone());
    n
}

/// How long something took, in words ("12 s", "3 min 4 s").
pub fn took(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min {} s", secs / 60, secs % 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

fn crashed(thread: &ThreadId, why: &str, world: &dyn World) -> Vec<Change> {
    let Some(slot) = world.slot_of(thread) else { return Vec::new() };
    let title = format!("{}'s session stopped", slot.name);
    vec![opened(new(NotificationId::crash(thread), NotificationKind::SessionCrashed, Some(&slot), Some(thread), title, Some(format!("{why}; its next prompt resumes it")), world))]
}

/// A server home links to stopped answering (the `time`th time).
pub fn server_lost(server: &str, reason: &str, time: u32, now_ms: u64) -> Notification {
    Notification {
        id: NotificationId::lost(server, time),
        server: None,
        level: NotificationKind::ServerLost.level(),
        kind: NotificationKind::ServerLost,
        slot: None,
        thread: None,
        request: None,
        terminal: None,
        discussion: None,
        title: format!("{server} stopped answering"),
        detail: Some(format!("{reason}; home keeps trying")),
        at_ms: now_ms,
        state: NotificationState::Open,
    }
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
