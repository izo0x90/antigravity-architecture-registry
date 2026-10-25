//! What sits over the panels, as one stack: the shell's. Each kind says
//! only how it behaves (`Rules`); each, as it's drawn, says where. A tap
//! outside the top one closes it (and goes no further, unless it lets taps
//! through); the way back out (Escape) closes the top one, unless it steps
//! back through its own first; what's under a modal one takes no keys.
//! Which are open is each one's own state; the stack is in the order they
//! were drawn (the last on top). Nothing here is allocated per frame past
//! the few places drawn.

use egui::{Pos2, RawInput, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Overlay {
    /// A card (or the pop-up terminal) over the panel it's about.
    QuickLook,
    /// The developer inspector's pick (over everything).
    Inspector,
    /// A report on a picked element, being written.
    Feedback,
    /// The issues found, listed.
    Issues,
    /// Labels to jump to, over what's on screen.
    Hints,
    /// The keys typed so far of a longer sequence, and what follows them.
    Keys,
    /// The menus (the leader's).
    Menu,
    /// The action list, or a value's choices.
    Palette,
}

/// What a tap outside it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outside {
    /// Closes it, and goes no further.
    Closes,
    /// Closes it, and goes on to what's under it (it was only a hint).
    ClosesAndPasses,
    /// Nothing (it's over everything: every tap is its own).
    Stays,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    pub outside: Outside,
    /// What's under it takes no keys while it's up.
    pub modal: bool,
    /// It reads the way back out itself (it steps back through its own
    /// first: a menu's path, a picker to its list).
    pub own_back: bool,
}

/// Every kind.
pub const ALL: [Overlay; 8] = [Overlay::QuickLook, Overlay::Inspector, Overlay::Feedback, Overlay::Issues, Overlay::Hints, Overlay::Keys, Overlay::Menu, Overlay::Palette];

impl Overlay {
    pub fn rules(self) -> Rules {
        let (outside, modal, own_back) = match self {
            Self::QuickLook => (Outside::Closes, false, true),
            Self::Inspector => (Outside::Stays, false, false),
            Self::Feedback | Self::Issues => (Outside::Closes, true, false),
            Self::Hints | Self::Keys => (Outside::ClosesAndPasses, false, true),
            Self::Menu | Self::Palette => (Outside::Closes, true, true),
        };
        Rules { outside, modal, own_back }
    }
}

/// A tap taken by closing what it was outside of: the rest of it is taken
/// too (its release; a finger's moves and lift).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Taking {
    button: bool,
    touch: Option<egui::TouchId>,
}

#[derive(Debug, Default)]
pub struct Overlays {
    /// Where each was drawn this frame, in the order drawn.
    drawing: Vec<(Overlay, Rect)>,
    /// Last frame's: what a tap is outside of.
    drawn: Vec<(Overlay, Rect)>,
    taking: Taking,
}

impl Overlays {
    /// `overlay` was drawn at `rect` (one drawn in parts says each).
    pub fn drawn_at(&mut self, overlay: Overlay, rect: Rect) {
        self.drawing.push((overlay, rect));
    }

    /// The frame's drawn: what the next frame's taps are measured by.
    pub fn frame_done(&mut self) {
        std::mem::swap(&mut self.drawn, &mut self.drawing);
        self.drawing.clear();
    }

    /// The one on top of those `open`, as last drawn.
    pub fn top(&self, open: impl Fn(Overlay) -> bool) -> Option<Overlay> {
        self.drawn.iter().rev().map(|(o, _)| *o).find(|o| open(*o))
    }

    fn inside(&self, overlay: Overlay, at: Pos2) -> bool {
        self.drawn.iter().any(|(o, r)| *o == overlay && r.contains(at))
    }

    /// A tap in `raw` outside the top of those `open`: that one, to close.
    /// Unless it lets taps through, the tap is taken out of `raw` (and the
    /// rest of it, as it comes).
    pub fn outside_tap(&mut self, raw: &mut RawInput, open: impl Fn(Overlay) -> bool) -> Option<Overlay> {
        self.take_the_rest(raw);
        let top = self.top(&open)?;
        let rules = top.rules();
        if rules.outside == Outside::Stays {
            return None;
        }
        let pressed = raw.events.iter().find_map(|e| match e {
            egui::Event::PointerButton { pos, pressed: true, .. } => Some(*pos),
            egui::Event::Touch { phase: egui::TouchPhase::Start, pos, .. } => Some(*pos),
            _ => None,
        })?;
        if self.inside(top, pressed) {
            return None;
        }
        if rules.outside == Outside::Closes {
            let touch = raw.events.iter().find_map(|e| match e {
                egui::Event::Touch { id, phase: egui::TouchPhase::Start, .. } => Some(*id),
                _ => None,
            });
            self.taking = Taking { button: true, touch };
            self.take_the_rest(raw);
        }
        Some(top)
    }

    /// The tap being taken, out of `raw`: its press and release, its
    /// finger's start, moves and lift.
    fn take_the_rest(&mut self, raw: &mut RawInput) {
        if self.taking == Taking::default() {
            return;
        }
        let mut taking = self.taking;
        raw.events.retain(|e| match e {
            egui::Event::PointerButton { pressed, .. } if taking.button => {
                if !pressed {
                    taking.button = false;
                }
                false
            }
            egui::Event::Touch { id, phase, .. } if taking.touch == Some(*id) => {
                if matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel) {
                    taking.touch = None;
                }
                false
            }
            _ => true,
        });
        self.taking = taking;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(at: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    fn raw(events: Vec<egui::Event>) -> RawInput {
        RawInput { events, ..Default::default() }
    }

    fn drawn(overlays: &[(Overlay, Rect)]) -> Overlays {
        let mut o = Overlays::default();
        overlays.iter().for_each(|(k, r)| o.drawn_at(*k, *r));
        o.frame_done();
        o
    }

    const CARD: Rect = Rect { min: Pos2::new(100.0, 100.0), max: Pos2::new(300.0, 300.0) };
    const LIST: Rect = Rect { min: Pos2::new(150.0, 150.0), max: Pos2::new(250.0, 200.0) };

    #[test]
    fn a_tap_outside_the_top_one_closes_it_and_goes_no_further() {
        let mut o = drawn(&[(Overlay::QuickLook, CARD), (Overlay::Palette, LIST)]);
        let open = |_| true;
        // Inside the top one: its own.
        assert_eq!(o.outside_tap(&mut raw(vec![press(Pos2::new(200.0, 170.0), true)]), open), None);
        // Inside the card but outside the list on it: the list closes; the
        // tap, press and release, is taken.
        let mut input = raw(vec![press(Pos2::new(120.0, 280.0), true)]);
        assert_eq!(o.outside_tap(&mut input, open), Some(Overlay::Palette));
        assert!(input.events.is_empty());
        let mut release = raw(vec![press(Pos2::new(120.0, 280.0), false), egui::Event::Text("x".into())]);
        assert_eq!(o.outside_tap(&mut release, |k| k != Overlay::Palette), None, "inside the card: it stays");
        assert_eq!(release.events, vec![egui::Event::Text("x".into())], "the release taken, the rest kept");
        // The next tap is a tap.
        let mut next = raw(vec![press(Pos2::new(120.0, 280.0), true)]);
        o.outside_tap(&mut next, |k| k != Overlay::Palette);
        assert_eq!(next.events.len(), 1);
    }

    #[test]
    fn a_hint_closes_on_a_tap_outside_and_lets_it_through_and_one_over_everything_stays() {
        let mut o = drawn(&[(Overlay::Keys, LIST)]);
        let mut input = raw(vec![press(Pos2::new(10.0, 10.0), true)]);
        assert_eq!(o.outside_tap(&mut input, |_| true), Some(Overlay::Keys));
        assert_eq!(input.events.len(), 1, "it goes on to what's under it");
        let mut o = drawn(&[(Overlay::Inspector, CARD)]);
        assert_eq!(o.outside_tap(&mut raw(vec![press(Pos2::new(10.0, 10.0), true)]), |_| true), None);
    }

    #[test]
    fn a_fingers_tap_outside_is_taken_to_its_lift() {
        let mut o = drawn(&[(Overlay::Menu, LIST)]);
        let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(7), phase, pos: Pos2::new(10.0, 10.0), force: None };
        let mut input = raw(vec![touch(egui::TouchPhase::Start), press(Pos2::new(10.0, 10.0), true)]);
        assert_eq!(o.outside_tap(&mut input, |_| true), Some(Overlay::Menu));
        assert!(input.events.is_empty());
        let mut lift = raw(vec![touch(egui::TouchPhase::Move), touch(egui::TouchPhase::End), press(Pos2::new(10.0, 10.0), false)]);
        o.outside_tap(&mut lift, |_| false);
        assert!(lift.events.is_empty());
    }

    #[test]
    fn nothing_open_takes_nothing() {
        let mut o = drawn(&[(Overlay::Menu, LIST)]);
        let mut input = raw(vec![press(Pos2::new(10.0, 10.0), true)]);
        assert_eq!(o.outside_tap(&mut input, |_| false), None);
        assert_eq!(input.events.len(), 1);
    }
}
