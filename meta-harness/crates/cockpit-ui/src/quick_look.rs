//! The quick look: the full card of what the cursor is on, over the column
//! it was opened from, without taking that column. It's a spec view of its
//! own, floating (its keys, sections and links work in it); sending it to a
//! column makes it that column's spec view. It also shows one notification
//! (what waits on the user, or what happened): answered in place, the next
//! and previous a key away.
//!
//! Its key held is a peek (it closes when let go); tapped, it stays open
//! until Escape, the key again, a click outside it, or focus moving to a
//! column.

use harness_contracts::NotificationId;

use crate::app::BufferKind;
use crate::keymap::Chord;

/// What the quick look shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Look {
    /// The full card of a component (its own spec view).
    Card,
    /// One notification.
    Attention(NotificationId),
}

/// The quick look's own spec view (its pane's number; never a column's).
pub const PANE: u32 = u32::MAX;

pub struct QuickLook {
    pub look: Look,
    /// The column it's over, and the view that column showed.
    pub slot: usize,
    pub over: BufferKind,
    /// The spec view used before it opened (used again when it closes).
    pub previous_spec: u32,
    /// The key that opened it, while it's still down, and since when.
    held: Option<(Chord, f64)>,
}

impl QuickLook {
    /// Opened over column `slot` (showing `over`) at `now`, by `key` if a
    /// key opened it (and is still down).
    pub fn open(look: Look, slot: usize, over: BufferKind, previous_spec: u32, key: Option<Chord>, now: f64) -> Self {
        Self { look, slot, over, previous_spec, held: key.map(|k| (k, now)) }
    }

    /// The notification it shows, if that's what it shows.
    pub fn attention(&self) -> Option<&NotificationId> {
        match &self.look {
            Look::Attention(id) => Some(id),
            Look::Card => None,
        }
    }

    /// Its key is still down (a press now is the key repeating, not a
    /// second press).
    pub fn held(&self) -> bool {
        self.held.is_some()
    }

    /// Its key went up at `now`: true when it was held long enough to be
    /// a peek (so it closes); a tap leaves it open.
    pub fn let_go(&mut self, now: f64, hold_secs: f64) -> bool {
        self.held.take().is_some_and(|(_, since)| now - since >= hold_secs)
    }

    /// The key that opened it, while it's down.
    pub fn key(&self) -> Option<&Chord> {
        self.held.as_ref().map(|(k, _)| k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_key_is_a_peek_and_a_tap_keeps_it_open() {
        let key = Chord::try_from("q".to_owned()).unwrap();
        let mut peek = QuickLook::open(Look::Card, 0, BufferKind::Graph, 0, Some(key.clone()), 1.0);
        assert!(peek.held());
        assert!(peek.let_go(1.4, 0.25), "held 400 ms: a peek, it closes");
        let mut tap = QuickLook::open(Look::Card, 0, BufferKind::Graph, 0, Some(key), 1.0);
        assert!(!tap.let_go(1.1, 0.25), "tapped: it stays");
        assert!(!tap.held() && !tap.let_go(5.0, 0.25), "and only the key that opened it closes it so");
        let mut menu = QuickLook::open(Look::Card, 0, BufferKind::Graph, 0, None, 1.0);
        assert!(!menu.held() && !menu.let_go(9.0, 0.25), "opened from a menu: no key to let go");
    }
}
