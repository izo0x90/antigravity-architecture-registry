//! What sits over the panels, as one stack: the shell's. Each kind says
//! only how it behaves (`Rules`); each, as it's drawn, says where. A tap
//! outside the top one closes it (and goes no further, unless it lets taps
//! through); the way back out (Escape) closes the top one, unless it steps
//! back through its own first; what's under a modal one takes no keys.
//! Which are open is each one's own state; the top is the one opened last
//! (still drawn). Nothing is allocated per frame past the places drawn (a
//! few; the hints, one per label).

use egui::{Pos2, RawInput, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Overlay {
    /// A buffer (a card, the inbox, a chat, a sub-agent's work, the pop-up
    /// terminal) over the panel it's about.
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
    /// The key bar's keys act on it (a tap there is inside it).
    pub keys_on_the_bar: bool,
}

impl Overlay {
    pub fn rules(self) -> Rules {
        let (outside, modal, own_back, keys_on_the_bar) = match self {
            Self::QuickLook => (Outside::Closes, false, true, true),
            Self::Inspector => (Outside::Stays, true, false, false),
            Self::Feedback | Self::Issues => (Outside::Closes, true, false, false),
            Self::Hints | Self::Keys => (Outside::ClosesAndPasses, false, true, false),
            Self::Menu | Self::Palette => (Outside::Closes, true, true, false),
        };
        Rules { outside, modal, own_back, keys_on_the_bar }
    }
}

/// A tap taken by closing what it was outside of: the rest of it is taken
/// too (its release; its fingers' moves and lifts).
#[derive(Clone, Debug, Default, PartialEq)]
struct Taking {
    button: bool,
    touches: Vec<egui::TouchId>,
}

#[derive(Debug, Default)]
pub struct Overlays {
    /// Where each was drawn this frame, in the order drawn.
    drawing: Vec<(Overlay, Rect)>,
    /// Last frame's: what a tap is outside of.
    drawn: Vec<(Overlay, Rect)>,
    /// Those drawn, in the order they opened (the last on top).
    order: Vec<Overlay>,
    /// The key bar: inside one whose keys it shows (`keys_on_the_bar`),
    /// this frame and last.
    across: Option<Rect>,
    across_drawn: Option<Rect>,
    taking: Taking,
}

impl Overlays {
    /// `overlay` was drawn at `rect` (one drawn in parts says each).
    pub fn drawn_at(&mut self, overlay: Overlay, rect: Rect) {
        self.drawing.push((overlay, rect));
    }

    /// `rect` is the key bar: a tap in it is inside one whose keys it
    /// shows.
    pub fn drawn_across(&mut self, rect: Rect) {
        self.across = Some(rect);
    }

    /// The frame's drawn: what the next frame's taps are measured by; one
    /// drawn first now opened last.
    pub fn frame_done(&mut self) {
        self.across_drawn = self.across.take();
        std::mem::swap(&mut self.drawn, &mut self.drawing);
        self.drawing.clear();
        let drawn = &self.drawn;
        self.order.retain(|o| drawn.iter().any(|(d, _)| d == o));
        for (o, _) in drawn {
            if !self.order.contains(o) {
                self.order.push(*o);
            }
        }
    }

    /// The one on top of those `open`.
    pub fn top(&self, open: impl Fn(Overlay) -> bool) -> Option<Overlay> {
        self.order.iter().rev().copied().find(|o| open(*o))
    }

    fn inside(&self, overlay: Overlay, at: Pos2) -> bool {
        (overlay.rules().keys_on_the_bar && self.across_drawn.is_some_and(|r| r.contains(at)))
            || self.drawn.iter().any(|(o, r)| *o == overlay && r.contains(at))
    }

    /// A tap in `raw` outside `top` (the top one open): it, to close.
    /// Unless it lets taps through, the tap is taken out of `raw` (and the
    /// rest of it, as it comes).
    pub fn outside_tap(&mut self, raw: &mut RawInput, top: Option<Overlay>) -> Option<Overlay> {
        self.take_the_rest(raw, 0);
        let top = top?;
        let rules = top.rules();
        if rules.outside == Outside::Stays {
            return None;
        }
        let (at, pressed) = raw.events.iter().enumerate().find_map(|(n, e)| match e {
            egui::Event::PointerButton { pos, pressed: true, .. } => Some((n, *pos)),
            egui::Event::Touch { phase: egui::TouchPhase::Start, pos, .. } => Some((n, *pos)),
            _ => None,
        })?;
        if self.inside(top, pressed) {
            return None;
        }
        if rules.outside == Outside::Closes {
            // Every finger put down with it, and its press.
            let touches = raw.events.iter().filter_map(|e| match e {
                egui::Event::Touch { id, phase: egui::TouchPhase::Start, .. } => Some(*id),
                _ => None,
            });
            self.taking = Taking { button: true, touches: touches.collect() };
            let press =
                raw.events.iter().skip(at).position(|e| matches!(e, egui::Event::PointerButton { pressed: true, .. })).map(|n| n + at);
            self.take_the_rest(raw, press.map_or(at, |p| p + 1));
            if let Some(p) = press {
                raw.events.remove(p);
            }
            raw.events.retain(|e| !matches!(e, egui::Event::Touch { phase: egui::TouchPhase::Start, .. }));
        }
        Some(top)
    }

    /// The tap being taken, out of `raw`'s events from `from` on: its
    /// release, its fingers' moves and lifts. A press before its release
    /// means the release was lost (let go off the page): the tap is over,
    /// and the press is a tap of its own.
    fn take_the_rest(&mut self, raw: &mut RawInput, from: usize) {
        if self.taking == Taking::default() {
            return;
        }
        let mut taking = std::mem::take(&mut self.taking);
        let mut n = 0;
        raw.events.retain(|e| {
            n += 1;
            if n <= from {
                return true;
            }
            match e {
                egui::Event::PointerButton { pressed: true, .. } | egui::Event::PointerGone if taking.button => {
                    taking.button = false;
                    true
                }
                egui::Event::PointerButton { pressed: false, .. } if taking.button => {
                    taking.button = false;
                    false
                }
                egui::Event::Touch { id, phase, .. } if taking.touches.contains(id) => {
                    if matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel) {
                        taking.touches.retain(|t| t != id);
                    }
                    false
                }
                _ => true,
            }
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
        let top = o.top(|_| true);
        assert_eq!(top, Some(Overlay::Palette));
        // Inside the top one: its own.
        assert_eq!(o.outside_tap(&mut raw(vec![press(Pos2::new(200.0, 170.0), true)]), top), None);
        // Inside the card but outside the list on it: the list closes; the
        // tap, press and release, is taken.
        let mut input = raw(vec![press(Pos2::new(120.0, 280.0), true)]);
        assert_eq!(o.outside_tap(&mut input, top), Some(Overlay::Palette));
        assert!(input.events.is_empty());
        let card = o.top(|k| k != Overlay::Palette);
        let mut release = raw(vec![press(Pos2::new(120.0, 280.0), false), egui::Event::Text("x".into())]);
        assert_eq!(o.outside_tap(&mut release, card), None, "inside the card: it stays");
        assert_eq!(release.events, vec![egui::Event::Text("x".into())], "the release taken, the rest kept");
        // The next tap is a tap.
        let mut next = raw(vec![press(Pos2::new(120.0, 280.0), true)]);
        o.outside_tap(&mut next, card);
        assert_eq!(next.events.len(), 1);
    }

    #[test]
    fn a_hint_closes_on_a_tap_outside_and_lets_it_through_and_one_over_everything_stays() {
        let mut o = drawn(&[(Overlay::Keys, LIST)]);
        let mut input = raw(vec![press(Pos2::new(10.0, 10.0), true)]);
        assert_eq!(o.outside_tap(&mut input, Some(Overlay::Keys)), Some(Overlay::Keys));
        assert_eq!(input.events.len(), 1, "it goes on to what's under it");
        let mut o = drawn(&[(Overlay::Inspector, CARD)]);
        assert_eq!(o.outside_tap(&mut raw(vec![press(Pos2::new(10.0, 10.0), true)]), Some(Overlay::Inspector)), None);
    }

    #[test]
    fn a_fingers_tap_outside_is_taken_to_its_lift() {
        let mut o = drawn(&[(Overlay::Menu, LIST)]);
        let touch = |id, phase| egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: egui::TouchId(id),
            phase,
            pos: Pos2::new(10.0, 10.0),
            force: None,
        };
        // Two fingers down at once: both taken.
        let mut input = raw(vec![touch(7, egui::TouchPhase::Start), touch(8, egui::TouchPhase::Start), press(Pos2::new(10.0, 10.0), true)]);
        assert_eq!(o.outside_tap(&mut input, Some(Overlay::Menu)), Some(Overlay::Menu));
        assert!(input.events.is_empty(), "{:?}", input.events);
        let mut lift = raw(vec![
            touch(7, egui::TouchPhase::Move),
            touch(8, egui::TouchPhase::End),
            touch(7, egui::TouchPhase::End),
            press(Pos2::new(10.0, 10.0), false),
        ]);
        o.outside_tap(&mut lift, None);
        assert!(lift.events.is_empty());
    }

    #[test]
    fn nothing_open_takes_nothing() {
        let mut o = drawn(&[(Overlay::Menu, LIST)]);
        let mut input = raw(vec![press(Pos2::new(10.0, 10.0), true)]);
        assert_eq!(o.outside_tap(&mut input, None), None);
        assert_eq!(input.events.len(), 1);
    }

    /// A tap outside whose release never comes (let go off the page): the
    /// next press is a tap of its own, not eaten.
    #[test]
    fn a_lost_release_doesnt_eat_the_next_tap() {
        let mut o = drawn(&[(Overlay::Palette, LIST)]);
        let mut input = raw(vec![press(Pos2::new(10.0, 10.0), true)]);
        assert_eq!(o.outside_tap(&mut input, Some(Overlay::Palette)), Some(Overlay::Palette));
        let mut next = raw(vec![press(Pos2::new(20.0, 20.0), true), press(Pos2::new(20.0, 20.0), false)]);
        o.outside_tap(&mut next, None);
        assert_eq!(next.events.len(), 2, "{:?}", next.events);
    }

    /// A tap on the key bar is inside the quick look (its keys are the
    /// bar's), not inside a menu over the panels (the bar's are theirs).
    #[test]
    fn a_tap_on_the_key_bar_is_inside_the_one_whose_keys_it_shows() {
        let bar = Rect::from_min_max(Pos2::new(0.0, 900.0), Pos2::new(1000.0, 940.0));
        let mut o = Overlays::default();
        o.drawn_at(Overlay::QuickLook, CARD);
        o.drawn_across(bar);
        o.frame_done();
        assert_eq!(o.outside_tap(&mut raw(vec![press(Pos2::new(10.0, 920.0), true)]), Some(Overlay::QuickLook)), None);
        assert_eq!(o.outside_tap(&mut raw(vec![press(Pos2::new(10.0, 10.0), true)]), Some(Overlay::QuickLook)), Some(Overlay::QuickLook));
        let mut o = Overlays::default();
        o.drawn_at(Overlay::Menu, LIST);
        o.drawn_across(bar);
        o.frame_done();
        assert_eq!(o.outside_tap(&mut raw(vec![press(Pos2::new(10.0, 920.0), true)]), Some(Overlay::Menu)), Some(Overlay::Menu));
    }

    /// The top is the one opened last, whatever the order they're drawn
    /// in.
    #[test]
    fn the_top_is_the_one_opened_last() {
        let mut o = drawn(&[(Overlay::Menu, LIST)]);
        o.drawn_at(Overlay::QuickLook, CARD);
        o.drawn_at(Overlay::Menu, LIST);
        o.frame_done();
        assert_eq!(o.top(|_| true), Some(Overlay::QuickLook), "drawn before it, opened after");
        o.drawn_at(Overlay::QuickLook, CARD);
        o.frame_done();
        assert_eq!(o.top(|_| true), Some(Overlay::QuickLook));
    }
}
