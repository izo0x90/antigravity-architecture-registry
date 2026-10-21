//! The quick look: a buffer over the panel it was opened from, without
//! taking that panel; the buffer it holds has the keys (its sections, links
//! and typing work in it). It holds the full card of what the cursor is on
//! (a spec buffer of its own), the inbox (what needs the user, one at a
//! time), or the lead's chat (talking to the agent from anywhere). Sent to
//! a panel, a buffer of its own becomes that panel's.
//!
//! Its key held is a peek (it closes when let go); tapped, it stays open
//! until Escape, the key again, a click outside it, or focus moving to a
//! panel.

use crate::kind::BufferKind;
use crate::panels::BufferId;
use crate::keymap::Chord;

/// The number of the quick look's own buffer of a kind (never a panel's).
pub const PANE: u32 = u32::MAX;

pub struct QuickLook {
    /// The buffer it holds (and gives the keys to).
    pub buffer: BufferId,
    /// The buffer of that kind used before it opened (used again when it
    /// closes).
    pub before: BufferId,
    /// The panel it's over, and the buffer that panel showed.
    pub panel: usize,
    pub over: BufferKind,
    /// The key that opened it, while it's still down, and since when.
    held: Option<(Chord, f64)>,
    /// When it opened (a click that opened it doesn't close it as a click
    /// outside).
    pub opened: f64,
}

impl QuickLook {
    /// `buffer` over panel `panel` (showing `over`) at `now`, `before`
    /// used before it; by `key` if a key opened it (and is still down).
    pub fn open(buffer: BufferId, before: BufferId, panel: usize, over: BufferKind, key: Option<Chord>, now: f64) -> Self {
        Self { buffer, before, panel, over, held: key.map(|k| (k, now)), opened: now }
    }

    /// The kind of buffer it holds.
    pub fn kind(&self) -> BufferKind {
        self.buffer.kind
    }

    /// It holds a buffer of its own (not one a panel has).
    pub fn own(&self) -> bool {
        self.buffer.id == PANE
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
        let card = BufferId::new(BufferKind::Spec, PANE);
        let open = |key| QuickLook::open(card, BufferId::new(BufferKind::Spec, 0), 0, BufferKind::Graph, key, 1.0);
        let mut peek = open(Some(key.clone()));
        assert!(peek.held());
        assert!(peek.let_go(1.4, 0.25), "held 400 ms: a peek, it closes");
        let mut tap = open(Some(key));
        assert!(!tap.let_go(1.1, 0.25), "tapped: it stays");
        assert!(!tap.held() && !tap.let_go(5.0, 0.25), "and only the key that opened it closes it so");
        let mut menu = open(None);
        assert!(!menu.held() && !menu.let_go(9.0, 0.25), "opened from a menu: no key to let go");
    }
}
