//! The quick look: the full card of what the cursor is on, over the panel
//! it was opened from, without taking that panel. It's a spec view of its
//! own, floating (its keys, sections and links work in it); sending it to a
//! panel makes it that panel's spec view. Or it shows the inbox (what
//! needs the user, one at a time), the same way: the view it holds has the
//! keyboard.
//!
//! Its key held is a peek (it closes when let go); tapped, it stays open
//! until Escape, the key again, a click outside it, or focus moving to a
//! panel.

use crate::kind::BufferKind;
use crate::keymap::Chord;

/// The quick look's own spec view (its buffer's number; never a panel's).
pub const PANE: u32 = u32::MAX;

pub struct QuickLook {
    /// The view it holds (and gives the keyboard to): its own spec view
    /// (a component's full card), or the inbox.
    pub kind: BufferKind,
    /// The panel it's over, and the buffer that panel showed.
    pub panel: usize,
    pub over: BufferKind,
    /// The spec view used before it opened (used again when it closes).
    pub previous_spec: u32,
    /// The key that opened it, while it's still down, and since when.
    held: Option<(Chord, f64)>,
}

impl QuickLook {
    /// Opened over panel `panel` (showing `over`) at `now`, by `key` if a
    /// key opened it (and is still down).
    pub fn open(kind: BufferKind, panel: usize, over: BufferKind, previous_spec: u32, key: Option<Chord>, now: f64) -> Self {
        Self { kind, panel, over, previous_spec, held: key.map(|k| (k, now)) }
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
        let mut peek = QuickLook::open(BufferKind::Spec, 0, BufferKind::Graph, 0, Some(key.clone()), 1.0);
        assert!(peek.held());
        assert!(peek.let_go(1.4, 0.25), "held 400 ms: a peek, it closes");
        let mut tap = QuickLook::open(BufferKind::Spec, 0, BufferKind::Graph, 0, Some(key), 1.0);
        assert!(!tap.let_go(1.1, 0.25), "tapped: it stays");
        assert!(!tap.held() && !tap.let_go(5.0, 0.25), "and only the key that opened it closes it so");
        let mut menu = QuickLook::open(BufferKind::Spec, 0, BufferKind::Graph, 0, None, 1.0);
        assert!(!menu.held() && !menu.let_go(9.0, 0.25), "opened from a menu: no key to let go");
    }
}
