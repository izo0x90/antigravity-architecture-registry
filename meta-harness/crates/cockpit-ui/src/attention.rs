//! What needs the user and what happened: the open notifications (kept in
//! the session store as the server sends them), in the order they're gone
//! through, with the words the cockpit shows for each.

use std::collections::HashMap;

use harness_contracts::{Level, Notification, NotificationKind, SlotId};

use crate::store::Store;

/// The open ones of `level` (both, given none), in order.
pub fn list(store: &Store, level: Option<Level>) -> Vec<&Notification> {
    let mut all: Vec<&Notification> = store.notifications.values().filter(|n| level.is_none_or(|l| n.level == l)).collect();
    all.sort_by(|a, b| harness_contracts::order(a, b));
    all
}

/// How many wait on the user, and how many are updates.
pub fn counts(store: &Store) -> (usize, usize) {
    store.notifications.values().fold((0, 0), |(w, u), n| if n.level == Level::Waiting { (w + 1, u) } else { (w, u + 1) })
}

pub fn kind_word(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Approval => "approval",
        NotificationKind::Question => "question",
        NotificationKind::Link => "link to open",
        NotificationKind::SignIn => "sign-in",
        NotificationKind::SessionCrashed => "session stopped",
        NotificationKind::TurnFailed => "turn failed",
        NotificationKind::TurnFinished => "turn finished",
        NotificationKind::Message => "message",
        NotificationKind::ServerLost => "server lost",
        NotificationKind::TerminalBell => "terminal bell",
        NotificationKind::ShellEnded => "shell ended",
        NotificationKind::CommandFinished => "command finished",
        NotificationKind::CommandFailed => "command failed",
    }
}

/// How long ago, in a word or two.
pub fn age(at_ms: u64, now_ms: u64) -> String {
    let secs = now_ms.saturating_sub(at_ms) / 1000;
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

/// Its line in a list: `slot · kind · age`.
pub fn detail(n: &Notification, store: &Store, now_ms: u64) -> String {
    let slot = n.slot.as_ref().map(|s| store.slots.get(s).map_or_else(|| s.to_string(), |slot| slot.name.clone()));
    [slot, Some(kind_word(n.kind).to_owned()), Some(age(n.at_ms, now_ms))].into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

pub fn now_ms() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// When each slot's chat was last said to be seen: said again only after
/// a while (and only when it has updates).
#[derive(Default)]
pub struct Seen {
    said: HashMap<SlotId, f64>,
}

impl Seen {
    /// Whether to say `slot` was seen at `now` (app time), at most once per `every` seconds.
    pub fn due(&mut self, slot: &SlotId, now: f64, every: f64) -> bool {
        match self.said.get(slot) {
            Some(at) if now - at < every => false,
            _ => {
                self.said.insert(slot.clone(), now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::{ClientEvent, NotificationId, NotificationState};

    pub(crate) fn note(id: &str, kind: NotificationKind, slot: &str, at_ms: u64) -> Notification {
        Notification { id: NotificationId::new(id), server: None, level: kind.level(), kind, slot: Some(SlotId::new(slot)), thread: None, request: None, terminal: None, title: id.into(), detail: None, at_ms, state: NotificationState::Open }
    }

    #[test]
    fn they_are_gone_through_waiting_first() {
        let mut store = Store::default();
        for n in [note("turn:1", NotificationKind::TurnFinished, "lead", 1), note("req:2", NotificationKind::Approval, "sub-1", 5), note("req:1", NotificationKind::Question, "sub-1", 2)] {
            store.apply(ClientEvent::NotificationUpdated { notification: n });
        }
        assert_eq!(list(&store, None).iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["req:1", "req:2", "turn:1"]);
        assert_eq!(counts(&store), (2, 1));
        let id = |s: &str| NotificationId::new(s);
        assert_eq!(detail(&store.notifications[&id("req:2")], &store, 125_000), "sub-1 · approval · 2 min ago");
    }

    #[test]
    fn a_slot_seen_is_said_once_in_a_while() {
        let mut seen = Seen::default();
        let slot = SlotId::new("sub-1");
        assert!(seen.due(&slot, 1.0, 2.0));
        assert!(!seen.due(&slot, 2.5, 2.0));
        assert!(seen.due(&slot, 3.5, 2.0));
    }
}
