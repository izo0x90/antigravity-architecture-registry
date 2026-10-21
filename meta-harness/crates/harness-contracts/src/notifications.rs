//! Notifications: what needs the user (waiting: an approval, a question, a
//! sign-in) and what happened that they'd want to know (updates: a crash, a
//! failed or finished turn, a message). Each is about one subject and keyed
//! by it, so a repeat changes nothing; its state only moves forward.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::ids::{InstanceId, NotificationId, RequestId, SlotId, ThreadId, TurnId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: NotificationId,
    /// The server it's from (set by a home server passing it on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    pub level: Level,
    pub kind: NotificationKind,
    pub slot: Option<SlotId>,
    pub thread: Option<ThreadId>,
    pub request: Option<RequestId>,
    /// The terminal it's about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<crate::ids::TerminalId>,
    /// The discussion it's about (one waiting on the user).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discussion: Option<crate::ids::DiscussionId>,
    pub title: String,
    pub detail: Option<String>,
    pub at_ms: u64,
    pub state: NotificationState,
}

/// Waiting: the user must act. Update: they'd want to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Waiting,
    Update,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// A tool call, command or file change to allow or not.
    Approval,
    /// The agent asks the user something.
    Question,
    /// A tool server asks the user to open a link (or fill something in).
    Link,
    /// A harness a slot uses needs signing in.
    SignIn,
    SessionCrashed,
    TurnFailed,
    TurnFinished,
    /// A sub-slot's report, or an agent telling the user something.
    Message,
    /// A server the home server links to stopped answering.
    ServerLost,
    /// A terminal's program rang its bell.
    TerminalBell,
    /// A terminal's shell ended.
    ShellEnded,
    /// A long command in a terminal finished: well, or not.
    CommandFinished,
    CommandFailed,
    /// A discussion waits on the user: answered (settle or reopen it), or
    /// an agent asks them.
    Discussion,
}

impl NotificationKind {
    pub fn level(self) -> Level {
        match self {
            Self::Approval | Self::Question | Self::Link | Self::SignIn | Self::Discussion => Level::Waiting,
            Self::SessionCrashed | Self::TurnFailed | Self::TurnFinished | Self::Message | Self::ServerLost | Self::TerminalBell | Self::ShellEnded | Self::CommandFinished | Self::CommandFailed => Level::Update,
        }
    }

    /// Something went wrong (shown before the other updates).
    pub fn is_failure(self) -> bool {
        matches!(self, Self::SessionCrashed | Self::TurnFailed | Self::CommandFailed)
    }
}

/// Open until it's done (what it waited on happened), expired (the server
/// restarted while it waited) or dismissed (seen, or put away).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationState {
    Open,
    Done,
    Expired,
    Dismissed,
}

/// A change a notification's state can't make (one that ended, opening again).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionError {
    pub id: NotificationId,
    pub from: NotificationState,
    pub to: NotificationState,
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "notification {} can't go from {:?} to {:?}", self.id, self.from, self.to)
    }
}

impl std::error::Error for TransitionError {}

impl NotificationState {
    /// Open goes to any end; an end is final. Staying as it is is no change.
    pub fn transition(self, to: Self, id: &NotificationId) -> Result<Self, TransitionError> {
        match (self, to) {
            (from, to) if from == to => Ok(to),
            (Self::Open, to) => Ok(to),
            (from, to) => Err(TransitionError { id: id.clone(), from, to }),
        }
    }
}

impl Notification {
    /// This one, moved to `to` (checked).
    pub fn moved(&self, to: NotificationState) -> Result<Self, TransitionError> {
        Ok(Self { state: self.state.transition(to, &self.id)?, ..self.clone() })
    }
}

/// Each subject's id. Sign-ins and lost servers can happen again: each time
/// is a new one, numbered.
impl NotificationId {
    pub fn request(request: &RequestId) -> Self {
        Self::new(format!("req:{request}"))
    }

    pub fn turn(turn: &TurnId) -> Self {
        Self::new(format!("turn:{turn}"))
    }

    pub fn crash(thread: &ThreadId) -> Self {
        Self::new(format!("crash:{thread}"))
    }

    pub fn sign_in(instance: &InstanceId, time: u32) -> Self {
        Self::new(format!("signin:{instance}:{time}"))
    }

    /// A discussion waiting on the user: one's, from when it was `said` long (each time it's the
    /// user's move anew is a notification of its own).
    pub fn discussion(discussion: &crate::ids::DiscussionId, said: usize) -> Self {
        Self::new(format!("disc:{discussion}:{said}"))
    }

    pub fn message(unique: &str) -> Self {
        Self::new(format!("msg:{unique}"))
    }

    pub fn lost(server: &str, time: u32) -> Self {
        Self::new(format!("lost:{server}:{time}"))
    }

    /// Something a terminal's program or shell did, at `at_ms` (each time
    /// is a new one).
    pub fn terminal(terminal: &crate::ids::TerminalId, what: &str, at_ms: u64) -> Self {
        Self::new(format!("term:{terminal}:{what}:{at_ms}"))
    }
}

/// The order they're listed and gone through: waiting first; then updates,
/// failures first; oldest first within each.
pub fn order(a: &Notification, b: &Notification) -> Ordering {
    let rank = |n: &Notification| (n.level, !n.kind.is_failure());
    rank(a).cmp(&rank(b)).then(a.at_ms.cmp(&b.at_ms)).then_with(|| a.id.cmp(&b.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, kind: NotificationKind, at_ms: u64) -> Notification {
        Notification { id: NotificationId::new(id), server: None, level: kind.level(), kind, slot: None, thread: None, request: None, terminal: None, discussion: None, title: id.into(), detail: None, at_ms, state: NotificationState::Open }
    }

    #[test]
    fn waiting_comes_first_then_failures_then_the_rest_oldest_first() {
        use NotificationKind as K;
        let mut all = vec![note("done", K::TurnFinished, 1), note("crash", K::SessionCrashed, 5), note("ask", K::Question, 9), note("allow", K::Approval, 3), note("failed", K::TurnFailed, 2)];
        all.sort_by(order);
        let ids: Vec<&str> = all.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["allow", "ask", "failed", "crash", "done"]);
    }

    #[test]
    fn an_ended_notification_stays_ended() {
        let open = note("req:1", NotificationKind::Approval, 0);
        let done = open.moved(NotificationState::Done).unwrap();
        assert_eq!(done.moved(NotificationState::Done).unwrap().state, NotificationState::Done, "a repeat is no change");
        let err = done.moved(NotificationState::Open).unwrap_err();
        assert_eq!(err.to_string(), "notification req:1 can't go from Done to Open");
        assert!(done.moved(NotificationState::Dismissed).is_err());
    }

    #[test]
    fn each_subject_has_its_own_id() {
        assert_eq!(NotificationId::request(&RequestId::new("r1")).as_str(), "req:r1");
        assert_eq!(NotificationId::sign_in(&InstanceId::new("claude-code"), 2).as_str(), "signin:claude-code:2");
    }
}
