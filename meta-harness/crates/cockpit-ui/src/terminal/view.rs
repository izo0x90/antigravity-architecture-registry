//! Where a terminal's view is: a whole number of lines, never pixels (as
//! terminals do: Alacritty's `display_offset`), so every frame draws whole
//! rows of what's there. It follows the live screen, or is held on a line
//! (by its number: output arriving below doesn't move what's read). A
//! wheel's pixels are kept until they make a line. Moves by keys go
//! through the shared moving rules (`viewport`), in lines. No drawing here.

use crate::viewport::{Go, Stop, Viewport};

/// The lines there are, and how many the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Span {
    /// The first line there is and one past the last.
    pub first: u64,
    pub end: u64,
    pub window: u64,
}

impl Span {
    /// The window's top on the live screen: its last lines.
    fn live(&self) -> u64 {
        self.end.saturating_sub(self.window).max(self.first)
    }
}

#[derive(Default)]
pub(super) struct View {
    /// Held with this line at the window's top (none: on the live screen).
    held: Option<u64>,
    /// A wheel's or a finger's pixels not yet a line (down: more).
    pixels: f32,
    /// Moved by hand (a wheel, a finger, the scrollbar) since the last
    /// move by keys: the next key brings the cursor into view first.
    by_hand: bool,
}

/// Room kept around the cursor, in lines.
const MARGIN: f32 = 2.0;

impl View {
    /// The line at the window's top.
    pub(super) fn top(&self, span: Span) -> u64 {
        let live = span.live();
        self.held.map_or(live, |at| at.clamp(span.first, live))
    }

    /// It shows the live screen (and follows it).
    pub(super) fn live(&self) -> bool {
        self.held.is_none()
    }

    /// Back to the live screen.
    pub(super) fn go_live(&mut self) {
        (self.held, self.pixels) = (None, 0.0);
    }

    /// The lines were laid out anew, their end `by` lines on: held, it
    /// keeps its distance from the live screen.
    pub(super) fn shift(&mut self, by: i64) {
        self.held = self.held.map(|at| at.saturating_add_signed(by));
    }

    /// The window's top on `top` (on the live screen's, or past it: live).
    fn hold(&mut self, top: i64, span: Span) {
        let live = span.live() as i64;
        self.held = (top < live).then(|| top.max(span.first as i64) as u64);
    }

    /// Scrolled by hand, `lines` down (up when negative).
    pub(super) fn scroll(&mut self, lines: i64, span: Span) {
        if lines != 0 {
            self.hold(self.top(span) as i64 + lines, span);
            self.by_hand = true;
        }
    }

    /// A wheel or a finger moved the content `pixels` down (rows `pitch`
    /// tall): whole lines of it scroll, the rest is kept for the next.
    pub(super) fn wheel(&mut self, pixels: f32, pitch: f32, span: Span) {
        self.pixels += pixels;
        let lines = (self.pixels / pitch).trunc();
        self.pixels -= lines * pitch;
        self.scroll(-(lines as i64), span);
    }

    /// The scrollbar's thumb: where it starts and how long it is, as parts
    /// of the track (none when everything fits).
    pub(super) fn thumb(&self, span: Span) -> Option<(f32, f32)> {
        let total = span.end - span.first;
        if total <= span.window {
            return None;
        }
        let len = span.window as f32 / total as f32;
        let at = (self.top(span) - span.first) as f32 / (total - span.window) as f32;
        Some((at * (1.0 - len), len))
    }

    /// The scrollbar dragged: the thumb's start at `at` (a part of the track).
    pub(super) fn drag_thumb(&mut self, at: f32, span: Span) {
        let Some((_, len)) = self.thumb(span) else { return };
        let room = (span.end - span.first - span.window) as f32;
        let top = span.first as f32 + (at / (1.0 - len).max(f32::EPSILON)).clamp(0.0, 1.0) * room;
        self.hold(top.round() as i64, span);
        self.by_hand = true;
    }

    /// A move by keys, the cursor on `cursor` (none: the last line): by the
    /// shared moving rules, where the cursor lands (none: nowhere new).
    pub(super) fn go(&mut self, go: Go, cursor: Option<u64>, span: Span) -> Option<u64> {
        let stops: Vec<Stop> = (0..span.end - span.first).map(|i| Stop::new(i as f32, 1.0)).collect();
        let mut rules = Viewport::default();
        rules.offset.y = (self.top(span) - span.first) as f32;
        rules.window.y = span.window as f32;
        rules.content.y = stops.len() as f32;
        (rules.margin, rules.by_hand) = (MARGIN, self.by_hand);
        let at = cursor.map(|c| c.clamp(span.first, span.end.saturating_sub(1)) - span.first);
        let moved = rules.go(&stops, at.map(|a| a as usize).or(stops.len().checked_sub(1)), go)?;
        self.hold(span.first as i64 + moved.offset.y.round() as i64, span);
        self.by_hand = false;
        Some(span.first + moved.cursor as u64)
    }
}

#[cfg(test)]
mod tests {
    use crate::viewport::Show;

    use super::*;

    /// Lines 100..1100 (1000 of them), 20 in the window.
    const SPAN: Span = Span { first: 100, end: 1100, window: 20 };

    #[test]
    fn it_follows_the_live_screen_until_scrolled_and_then_stays_on_what_is_read() {
        let mut view = View::default();
        assert_eq!(view.top(SPAN), 1080, "the live screen: the last 20");
        let more = Span { end: 1150, ..SPAN };
        assert_eq!(view.top(more), 1130, "output arrives: it follows");
        view.scroll(-30, more);
        assert_eq!(view.top(more), 1100);
        assert_eq!(view.top(Span { end: 1300, ..more }), 1100, "held on its line while output arrives");
        view.scroll(1000, more);
        assert!(view.live(), "scrolled to the end: live again");
        view.scroll(-5000, more);
        assert_eq!(view.top(more), 100, "not past the first line");
        assert_eq!(view.top(Span { first: 500, ..more }), 500, "its line gone: the first there is");
    }

    #[test]
    fn a_wheels_pixels_make_whole_lines_and_the_rest_waits() {
        let mut view = View::default();
        for _ in 0..5 {
            view.wheel(7.0, 16.0, SPAN);
        }
        assert_eq!(view.top(SPAN), 1078, "35 pixels: two lines up, 3 pixels kept");
        view.wheel(13.0, 16.0, SPAN);
        assert_eq!(view.top(SPAN), 1077, "with the 3 kept, one more");
        view.wheel(-100.0, 16.0, SPAN);
        assert!(view.live());
    }

    #[test]
    fn keys_move_the_cursor_by_the_shared_rules_in_whole_lines() {
        let mut view = View::default();
        // From the last line, up 30: the view keeps it two lines from the top.
        assert_eq!(view.go(Go::Up(30), None, SPAN), Some(1069));
        assert_eq!(view.top(SPAN), 1067);
        assert_eq!(view.go(Go::HalfUp(1), Some(1069), SPAN), Some(1059));
        assert_eq!(view.top(SPAN), 1057);
        assert_eq!(view.go(Go::First, Some(1059), SPAN), Some(100));
        assert_eq!(view.top(SPAN), 100);
        assert_eq!(view.go(Go::To(500, Show::Middle), Some(100), SPAN), Some(600));
        assert_eq!(view.top(SPAN), 591, "in the middle (the rows below it one more)");
        assert_eq!(view.go(Go::Last, Some(600), SPAN), Some(1099));
        assert!(view.live());
    }

    #[test]
    fn after_a_scroll_by_hand_a_key_starts_from_what_shows() {
        let mut view = View::default();
        view.scroll(-500, SPAN);
        assert_eq!(
            view.go(Go::Down(1), Some(1099), SPAN),
            Some(598),
            "the cursor was below: the nearest line that shows, kept from the bottom, then one down"
        );
    }

    #[test]
    fn the_thumb_shows_where_the_window_is_and_dragging_it_moves_there() {
        let mut view = View::default();
        let (at, len) = view.thumb(SPAN).unwrap();
        assert!((at + len - 1.0).abs() < 1e-4 && (len - 0.02).abs() < 1e-4, "at the end, a fiftieth long");
        view.drag_thumb(0.0, SPAN);
        assert_eq!(view.top(SPAN), 100);
        view.drag_thumb(0.49, SPAN);
        assert_eq!(view.top(SPAN), 590);
        assert_eq!(View::default().thumb(Span { first: 0, end: 10, window: 20 }), None, "it all fits");
    }
}
