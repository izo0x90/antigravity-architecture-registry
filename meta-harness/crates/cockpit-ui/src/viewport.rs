//! One way of moving for every view that scrolls: where its cursor can
//! stop (`Stop`), where it's looking (`Viewport`), and the rules that work
//! out the next stop and where to look together (`Viewport::go`), before
//! anything is drawn. Never past the edges of the content; nothing asked
//! for when nothing would change. Views apply the result and report back
//! what was drawn (`Viewport::drawn`); none calls egui's own scrolling.

use egui::Vec2;

/// A place the cursor can stop: where it starts down the content and how
/// tall it is (the content's top is 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub top: f32,
    pub height: f32,
}

impl Stop {
    pub fn new(top: f32, height: f32) -> Self {
        Self { top, height: height.max(0.0) }
    }

    fn bottom(&self) -> f32 {
        self.top + self.height
    }

    fn center(&self) -> f32 {
        self.top + self.height / 2.0
    }
}

/// A move asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Go {
    /// `n` stops down or up.
    Down(usize),
    Up(usize),
    /// Half the window, `n` times: the view and the cursor move together.
    HalfDown(usize),
    HalfUp(usize),
    First,
    Last,
    /// To a stop: kept in view (a move), or in the middle (a jump).
    To(usize, Show),
    /// Half the window sideways, `n` times (negative: left).
    Sideways(i32),
    /// `n` columns sideways (negative: left).
    Panels(i32),
}

impl Go {
    /// The move a list motion asks for, `count` times: down and up, half
    /// a window, first and last. None for the others (a view's own).
    pub fn of(motion: crate::nav::Motion, count: usize) -> Option<Self> {
        use crate::nav::Motion;
        Some(match motion {
            Motion::Down => Self::Down(count),
            Motion::Up => Self::Up(count),
            Motion::HalfDown => Self::HalfDown(count),
            Motion::HalfUp => Self::HalfUp(count),
            Motion::First => Self::First,
            Motion::Last => Self::Last,
            _ => return None,
        })
    }
}

/// How a stop gone to is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    /// Scrolled only as far as needed, with a little room around it.
    InView,
    /// In the middle, as near as the edges allow.
    Middle,
}

/// Where a move lands: the cursor's stop and where to look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moved {
    pub cursor: usize,
    pub offset: Vec2,
}

/// Items in half a window, before any are laid out.
const UNKNOWN_HALF: usize = 10;

/// How long a key's move glides, in seconds.
pub const GLIDE_S: f64 = 1.0 / 6.0;

/// How far along a glide is at `t` (0 to 1 of its time): quick away,
/// gentle arrival.
pub fn eased(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Where a view is looking: how far it's scrolled (down and sideways), how
/// big its window and its content are, and the room kept around the
/// cursor. Kept by the view, updated after every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub offset: Vec2,
    pub window: Vec2,
    pub content: Vec2,
    /// Room kept around the cursor when it's brought into view.
    pub margin: f32,
    /// A column's width, for moving sideways by columns.
    pub column: f32,
    /// Scrolled by hand (a wheel, a finger) since the last move with keys:
    /// the next key brings the cursor onto the screen first.
    pub by_hand: bool,
    /// Where it was last drawn (a glide starts there).
    at: Vec2,
    /// A move under way: from where, to where, since when (none: there
    /// at once, next frame).
    glide: Option<(Vec2, Vec2, Option<f64>)>,
    /// Where it asked to be drawn this frame (a hand moving it while it
    /// glides takes over).
    asked: Option<Vec2>,
}

impl Default for Viewport {
    fn default() -> Self {
        Self { offset: Vec2::ZERO, window: Vec2::ZERO, content: Vec2::ZERO, margin: 0.0, column: 0.0, by_hand: false, at: Vec2::ZERO, glide: None, asked: None }
    }
}

impl Viewport {
    /// The furthest it can scroll: never past the end of the content.
    pub fn max_offset(&self) -> Vec2 {
        (self.content - self.window).max(Vec2::ZERO)
    }

    /// `offset`, kept between the top and the end of the content.
    pub fn clamp(&self, offset: Vec2) -> Vec2 {
        offset.clamp(Vec2::ZERO, self.max_offset())
    }

    /// Whether any of a stop shows in the window, scrolled to `offset`.
    fn shows_at(&self, stop: &Stop, offset: f32) -> bool {
        stop.bottom() > offset && stop.top < offset + self.window.y
    }

    /// The stop under a point down the content (the nearest, past the ends).
    fn stop_at(stops: &[Stop], y: f32) -> usize {
        let after = stops.partition_point(|s| s.top <= y);
        after.saturating_sub(1).min(stops.len().saturating_sub(1))
    }

    /// How far down to look to show `stop` as asked, from `offset`.
    fn showing(&self, stop: &Stop, show: Show, offset: f32) -> f32 {
        let (window, margin) = (self.window.y, self.margin.min((self.window.y - stop.height).max(0.0) / 2.0));
        let wanted = match show {
            Show::Middle => stop.center() - window / 2.0,
            // Taller than the window: kept while some of it shows; else
            // its top coming down to it, its end coming up.
            Show::InView if stop.height + 2.0 * margin >= window => match () {
                _ if self.shows_at(stop, offset) => offset,
                _ if stop.top >= offset => stop.top,
                _ => stop.bottom() - window,
            },
            Show::InView if stop.top - margin < offset => stop.top - margin,
            Show::InView if stop.bottom() + margin > offset + window => stop.bottom() + margin - window,
            Show::InView => offset,
        };
        wanted.clamp(0.0, self.max_offset().y)
    }

    /// The next cursor stop and where to look, for `go` from `cursor`; none
    /// when nothing would change. After a scroll by hand, the cursor is
    /// first brought onto the screen (the nearest stop that shows) when it
    /// isn't, and moves from there.
    pub fn go(&self, stops: &[Stop], cursor: Option<usize>, go: Go) -> Option<Moved> {
        if stops.is_empty() {
            return match go {
                Go::Sideways(n) => self.sideways(cursor.unwrap_or(0), self.window.x / 2.0 * n as f32),
                Go::Panels(n) => self.sideways(cursor.unwrap_or(0), self.column * n as f32),
                _ => None,
            };
        }
        let last = stops.len() - 1;
        let mut at = cursor.unwrap_or(0).min(last);
        if self.by_hand && !self.shows_at(&stops[at], self.offset.y) {
            // The nearest that shows, with the room kept around the cursor.
            let room = self.margin.min(self.window.y / 2.0);
            at = if stops[at].top < self.offset.y { Self::stop_at(stops, self.offset.y + room) } else { Self::stop_at(stops, self.offset.y + self.window.y - room - 0.01) };
        }
        let offset = self.offset;
        let to = |index: usize, show: Show| Moved { cursor: index, offset: Vec2::new(offset.x, self.showing(&stops[index], show, offset.y)) };
        let moved = match go {
            Go::Down(n) => to((at + n).min(last), Show::InView),
            Go::Up(n) => to(at.saturating_sub(n), Show::InView),
            // The ends of the content (what's above the first stop or below
            // the last shows too).
            Go::First => Moved { cursor: 0, offset: Vec2::new(offset.x, 0.0) },
            Go::Last => Moved { cursor: last, offset: Vec2::new(offset.x, self.max_offset().y) },
            Go::To(index, show) => to(index.min(last), show),
            Go::HalfDown(n) | Go::HalfUp(n) => {
                let down = matches!(go, Go::HalfDown(_));
                let step = self.window.y / 2.0 * n as f32;
                let y = self.clamp(offset + Vec2::new(0.0, if down { step } else { -step })).y;
                if y == offset.y {
                    // At the end: the cursor goes the rest of the way.
                    to(if down { last } else { 0 }, Show::InView)
                } else {
                    // The view and the cursor move together, as far as the
                    // edges let the view; the cursor stays in view.
                    let mut cursor = Self::stop_at(stops, (stops[at].top + if down { step } else { -step }).max(0.0));
                    // One starting above the room at the top would pull the
                    // view back short of half a window: the next instead
                    // (a stop taller than the window is moved through).
                    let fits = stops[cursor].height + 2.0 * self.margin < self.window.y;
                    if fits && stops[cursor].top - self.margin < y && cursor < last {
                        cursor += 1;
                    }
                    Moved { cursor, offset: Vec2::new(offset.x, self.showing(&stops[cursor], Show::InView, y)) }
                }
            }
            Go::Sideways(n) => return self.sideways(at, self.window.x / 2.0 * n as f32),
            Go::Panels(n) => return self.sideways(at, self.column * n as f32),
        };
        (moved.cursor != cursor.unwrap_or(usize::MAX) || moved.offset != self.offset).then_some(moved)
    }

    fn sideways(&self, cursor: usize, by: f32) -> Option<Moved> {
        let offset = self.clamp(self.offset + Vec2::new(by, 0.0));
        (offset != self.offset).then_some(Moved { cursor, offset })
    }

    /// Takes a move: a key's (`glide`: from `now`, from where it was last
    /// drawn) or a jump's (there at once).
    pub fn take(&mut self, moved: &Moved, now: f64, glide: bool) {
        if moved.offset != self.offset || moved.offset != self.at {
            self.glide = Some((self.at, moved.offset, glide.then_some(now)));
        }
        self.offset = moved.offset;
        self.by_hand = false;
    }

    /// Where to draw from at `now`: on the way to where it's looking while
    /// a key's move glides, else there. `None` when the view should keep
    /// what it has (nothing asked, nothing gliding).
    pub fn wanted(&mut self, now: f64) -> Option<Vec2> {
        let (from, to, since) = self.glide?;
        let Some(since) = since else {
            self.glide = None;
            return Some(to);
        };
        let t = ((now - since) / GLIDE_S).clamp(0.0, 1.0) as f32;
        if t >= 1.0 {
            self.glide = None;
        }
        let at = from + (to - from) * eased(t);
        self.asked = Some(at);
        Some(at)
    }

    /// Whether a key's move is still gliding (draw again).
    pub fn gliding(&self) -> bool {
        self.glide.is_some()
    }

    /// What was drawn: where it was scrolled to, the window and the
    /// content. Scrolled where nothing asked (a wheel, a finger) is by
    /// hand.
    pub fn drawn(&mut self, offset: Vec2, window: Vec2, content: Vec2) {
        (self.window, self.content, self.at) = (window, content, offset);
        let asked = self.asked.take();
        if self.glide.is_none() {
            // Where it was asked to look, as the content now allows.
            if (offset - self.clamp(self.offset)).length() > 0.5 {
                self.by_hand = true;
            }
            self.offset = offset;
        } else if asked.is_some_and(|asked| (offset - self.clamp(asked)).length() > 0.5) {
            // A hand moved it while it glided: the hand's.
            (self.glide, self.by_hand, self.offset) = (None, true, offset);
        }
    }
}

/// The shared rules for a view egui lays out item by item (messages, a
/// card's items, jobs): where each item was drawn last frame is its stop.
#[derive(Clone, Debug, Default)]
pub struct Measured {
    pub viewport: Viewport,
    /// The items' stops as last drawn (down the content).
    stops: Vec<Stop>,
    /// This frame's items as drawn (on screen), in order.
    drawing: Vec<egui::Rect>,
    /// An item to show as asked once laid out.
    show: Option<(usize, Show)>,
    clock: f64,
}

impl Measured {
    /// Before drawing: the area, looking where the rules say (an item asked
    /// for before it was laid out, shown now).
    pub fn area(&mut self, area: egui::ScrollArea, now: f64) -> egui::ScrollArea {
        self.clock = now;
        self.drawing.clear();
        if self.viewport.window.y > 0.0
            && let Some((at, show)) = self.show.filter(|(at, _)| *at < self.stops.len())
        {
            self.show = None;
            if let Some(moved) = self.viewport.go(&self.stops, Some(at), Go::To(at, show)) {
                self.viewport.take(&moved, now, false);
            }
        }
        match self.viewport.wanted(now) {
            Some(offset) => area.vertical_scroll_offset(offset.y),
            None => area,
        }
    }

    /// An item drawn this frame (in order), where it is on screen.
    pub fn item(&mut self, rect: egui::Rect) {
        self.drawing.push(rect);
    }

    /// After drawing: what the area showed, and where each item went.
    pub fn drawn<R>(&mut self, output: &egui::scroll_area::ScrollAreaOutput<R>) {
        let top = output.inner_rect.top() - output.state.offset.y;
        self.stops = self.drawing.iter().map(|r| Stop::new(r.top() - top, r.height())).collect();
        self.viewport.drawn(output.state.offset, output.inner_rect.size(), output.content_size);
    }

    /// A key's move from item `cursor` among `items`: the item it lands on
    /// (none: nothing changes). Not laid out as they are yet (just
    /// arrived, never drawn): by count, shown once they are.
    pub fn go(&mut self, items: usize, cursor: Option<usize>, go: Go) -> Option<usize> {
        self.show = None;
        if self.stops.len() != items {
            let (at, last) = (cursor.unwrap_or(0), items.checked_sub(1)?);
            let half = |n: usize| n * UNKNOWN_HALF;
            let to = match go {
                Go::Down(n) => (at + n).min(last),
                Go::Up(n) => at.saturating_sub(n),
                Go::HalfDown(n) => (at + half(n)).min(last),
                Go::HalfUp(n) => at.saturating_sub(half(n)),
                Go::First => 0,
                Go::Last => last,
                Go::To(i, _) => i.min(last),
                Go::Sideways(_) | Go::Panels(_) => return None,
            };
            self.show = Some((to, Show::InView));
            return (Some(to) != cursor).then_some(to);
        }
        let moved = self.viewport.go(&self.stops, cursor, go)?;
        self.viewport.take(&moved, self.clock, true);
        Some(moved.cursor)
    }

    /// Shows item `at` as asked, once it's laid out.
    pub fn show(&mut self, at: usize, show: Show) {
        self.show = Some((at, show));
    }

    /// Items as last laid out.
    pub fn len(&self) -> usize {
        self.stops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stops.is_empty()
    }

    /// Draw again: a move gliding, or an item waiting to be shown.
    pub fn busy(&self) -> bool {
        self.viewport.gliding() || self.show.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 100 stops 20 high, in a window 200 high (ten show), two stops of room.
    fn lines() -> (Vec<Stop>, Viewport) {
        let stops = (0..100).map(|i| Stop::new(i as f32 * 20.0, 20.0)).collect();
        let view = Viewport { window: Vec2::new(300.0, 200.0), content: Vec2::new(300.0, 2000.0), margin: 40.0, ..Viewport::default() };
        (stops, view)
    }

    fn y(moved: Option<Moved>) -> Option<(usize, f32)> {
        moved.map(|m| (m.cursor, m.offset.y))
    }

    #[test]
    fn moves_keep_the_cursor_in_view_with_room_and_never_past_the_edges() {
        let (stops, mut view) = lines();
        assert_eq!(y(view.go(&stops, Some(0), Go::Down(1))), Some((1, 0.0)), "in view: nothing scrolls");
        assert_eq!(y(view.go(&stops, Some(7), Go::Down(1))), Some((8, 20.0)), "near the bottom: just enough, with room");
        assert_eq!(y(view.go(&stops, Some(0), Go::Up(1))), None, "at the top: nothing changes, nothing asked");
        view.offset.y = 1800.0;
        assert_eq!(y(view.go(&stops, Some(99), Go::Down(1))), None, "at the end: nothing");
        assert_eq!(y(view.go(&stops, Some(98), Go::Down(1))), Some((99, 1800.0)), "the last line: no room past the end");
        assert_eq!(y(view.go(&stops, Some(99), Go::First)), Some((0, 0.0)));
        view.offset.y = 0.0;
        assert_eq!(y(view.go(&stops, Some(0), Go::Last)), Some((99, 1800.0)));
    }

    #[test]
    fn half_a_window_moves_the_view_and_the_cursor_together_and_stops_at_the_ends() {
        let (stops, mut view) = lines();
        assert_eq!(y(view.go(&stops, Some(3), Go::HalfDown(1))), Some((8, 100.0)), "both move 100: the cursor stays where it was on the screen");
        view.offset.y = 1750.0;
        assert_eq!(y(view.go(&stops, Some(90), Go::HalfDown(1))), Some((95, 1800.0)), "only 50 more to scroll: the cursor still moves half a window");
        view.offset.y = 1800.0;
        assert_eq!(y(view.go(&stops, Some(95), Go::HalfDown(1))), Some((99, 1800.0)), "at the end: the cursor goes to the last");
        assert_eq!(y(view.go(&stops, Some(99), Go::HalfDown(1))), None, "and then nothing: no flicker past the end");
        view.offset.y = 0.0;
        assert_eq!(y(view.go(&stops, Some(4), Go::HalfUp(1))), Some((0, 0.0)), "at the top: to the first");
        assert_eq!(y(view.go(&stops, Some(3), Go::HalfDown(2))), Some((13, 200.0)), "a count: that many halves");
    }

    #[test]
    fn half_a_window_of_uneven_items_moves_the_view_a_full_half_each_time() {
        // Messages 70 and 30 high in turn, in a window 200 high.
        let mut top = 0.0;
        let stops: Vec<Stop> = (0..40).map(|i| {
            let h = if i % 2 == 0 { 70.0 } else { 30.0 };
            top += h;
            Stop::new(top - h, h)
        }).collect();
        let mut view = Viewport { window: Vec2::new(300.0, 200.0), content: Vec2::new(300.0, 2000.0), ..Viewport::default() };
        let mut at = 0;
        let mut seen = Vec::new();
        while let Some(m) = view.go(&stops, Some(at), Go::HalfDown(1)) {
            assert!(view.shows_at(&stops[m.cursor], m.offset.y), "the cursor shows");
            seen.push(m.offset.y);
            (at, view.offset) = (m.cursor, m.offset);
        }
        assert_eq!(&seen[..5], [100.0, 200.0, 300.0, 400.0, 500.0], "a half window each time");
        assert_eq!((at, *seen.last().unwrap()), (39, 1800.0), "to the end");
        // An item a bit taller than half the window is no wall.
        let stops = vec![Stop::new(0.0, 150.0), Stop::new(150.0, 150.0), Stop::new(300.0, 150.0), Stop::new(450.0, 150.0)];
        let view = Viewport { window: Vec2::new(300.0, 200.0), content: Vec2::new(300.0, 600.0), offset: Vec2::new(0.0, 150.0), ..Viewport::default() };
        assert_eq!(y(view.go(&stops, Some(1), Go::HalfDown(1))), Some((2, 250.0)), "half a window, the next item whole");
    }

    #[test]
    fn content_smaller_than_the_window_never_scrolls() {
        let stops: Vec<Stop> = (0..5).map(|i| Stop::new(i as f32 * 20.0, 20.0)).collect();
        let view = Viewport { window: Vec2::new(300.0, 200.0), content: Vec2::new(300.0, 100.0), margin: 40.0, ..Viewport::default() };
        assert_eq!(y(view.go(&stops, Some(0), Go::Last)), Some((4, 0.0)));
        assert_eq!(y(view.go(&stops, Some(0), Go::HalfDown(1))), Some((4, 0.0)), "nothing to scroll: to the last");
        assert_eq!(y(view.go(&stops, Some(2), Go::To(2, Show::Middle))), None, "the middle as near as the edges allow: where it is");
    }

    #[test]
    fn a_stop_taller_than_the_window_is_moved_through() {
        // A short one, a 1000-high one, a short one.
        let stops = vec![Stop::new(0.0, 20.0), Stop::new(20.0, 1000.0), Stop::new(1020.0, 20.0)];
        let mut view = Viewport { window: Vec2::new(300.0, 200.0), content: Vec2::new(300.0, 1040.0), margin: 40.0, ..Viewport::default() };
        assert_eq!(y(view.go(&stops, Some(0), Go::Down(1))), Some((1, 0.0)), "its top shows: nothing scrolls");
        view.offset.y = 0.0;
        let mut at = 1;
        let mut seen = Vec::new();
        while let Some(m) = view.go(&stops, Some(at), Go::HalfDown(1)) {
            seen.push((m.cursor, m.offset.y));
            (at, view.offset) = (m.cursor, m.offset);
        }
        assert_eq!(seen, [(1, 100.0), (1, 200.0), (1, 300.0), (1, 400.0), (1, 500.0), (1, 600.0), (1, 700.0), (1, 800.0), (1, 840.0), (2, 840.0)], "half a window at a time inside it, then on");
        assert_eq!(y(view.go(&stops, Some(2), Go::Up(1))), Some((1, 840.0)), "its end shows: nothing scrolls");
        view.offset.y = 0.0;
        assert_eq!(y(view.go(&stops, Some(0), Go::To(2, Show::InView))), Some((2, 840.0)), "past it to the last");
    }

    #[test]
    fn after_a_scroll_by_hand_the_next_key_starts_from_what_shows() {
        let (stops, mut view) = lines();
        view.drawn(Vec2::new(0.0, 1000.0), view.window, view.content);
        assert!(view.by_hand, "scrolled where nothing asked");
        assert_eq!(y(view.go(&stops, Some(2), Go::Down(1))), Some((53, 1000.0)), "the cursor (above) comes to the top that shows (with room), then moves; the view stays");
        assert_eq!(y(view.go(&stops, Some(90), Go::Up(1))), Some((56, 1000.0)), "from below: the bottom that shows");
        let moved = view.go(&stops, Some(52), Go::Down(1)).unwrap();
        view.take(&moved, 0.0, true);
        assert!(!view.by_hand, "a key's move: back to following the cursor");
    }

    #[test]
    fn a_keys_move_glides_and_a_hand_doesnt() {
        let (stops, mut view) = lines();
        let moved = view.go(&stops, Some(0), Go::Last).unwrap();
        view.take(&moved, 10.0, true);
        assert_eq!(view.wanted(10.0), Some(Vec2::ZERO), "starts where it was");
        let half = view.wanted(10.0 + GLIDE_S / 2.0).unwrap().y;
        assert!(half > 900.0 && half < 1800.0, "on its way, eased: {half}");
        assert_eq!(view.wanted(10.0 + GLIDE_S), Some(Vec2::new(0.0, 1800.0)), "there");
        assert_eq!(view.wanted(11.0), None, "and done");
        view.drawn(Vec2::new(0.0, 1800.0), view.window, view.content);
        assert!(!view.by_hand, "where it was asked to be");
        // A jump: there at once.
        let moved = view.go(&stops, Some(99), Go::To(0, Show::Middle)).unwrap();
        view.take(&moved, 20.0, false);
        assert_eq!((view.wanted(20.0), view.wanted(20.0)), (Some(Vec2::ZERO), None));
    }

    /// 100 items 20 high in an area 200 high, drawn at `now`; returns the
    /// offset it showed.
    fn draw_items(ctx: &egui::Context, list: &mut Measured, cursor: usize, now: f64) -> f32 {
        let mut shown = 0.0;
        let input = egui::RawInput { time: Some(now), screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(400.0, 400.0))), ..Default::default() };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let area = list.area(egui::ScrollArea::vertical().max_height(200.0).auto_shrink([false, false]), now);
                let output = area.show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    (0..100).map(|i| ui.add_sized([300.0, 20.0], egui::Label::new(if i == cursor { "here" } else { "item" })).rect).collect::<Vec<_>>()
                });
                for rect in &output.inner {
                    list.item(*rect);
                }
                list.drawn(&output);
                shown = output.state.offset.y;
            });
        });
        shown
    }

    #[test]
    fn a_list_laid_out_by_egui_moves_by_the_same_rules() {
        let ctx = egui::Context::default();
        let mut list = Measured::default();
        assert_eq!(list.go(100, Some(0), Go::HalfDown(1)), Some(10), "before it's laid out: by count");
        draw_items(&ctx, &mut list, 10, 0.0);
        let before = draw_items(&ctx, &mut list, 10, 0.1);
        assert_eq!((list.len(), before), (100, 20.0), "the eleventh just in view");
        assert!(!list.busy(), "shown");
        let at = list.go(100, Some(10), Go::HalfDown(1)).unwrap();
        assert_eq!(at, 15, "half the window (100) is five items");
        let mut now = 1.0;
        let mut offset = draw_items(&ctx, &mut list, at, now);
        while list.busy() {
            now += 0.02;
            offset = draw_items(&ctx, &mut list, at, now);
        }
        assert_eq!(offset, 120.0, "the view moved half a window with it");
        let at = list.go(100, Some(at), Go::Last).unwrap();
        while { now += 0.02; draw_items(&ctx, &mut list, at, now); list.busy() } {}
        assert_eq!(draw_items(&ctx, &mut list, at, now + 1.0), 1800.0, "the last shows, no further");
        assert_eq!(list.go(100, Some(99), Go::HalfDown(1)), None, "and half a page more does nothing");
    }

    #[test]
    fn a_hand_moving_it_while_it_glides_takes_over() {
        let (stops, mut view) = lines();
        let moved = view.go(&stops, Some(0), Go::Last).unwrap();
        view.take(&moved, 10.0, true);
        let on_the_way = view.wanted(10.0 + GLIDE_S / 2.0).unwrap();
        view.drawn(on_the_way, view.window, view.content);
        assert!(view.gliding() && !view.by_hand, "drawn where it asked: still gliding");
        let wanted = view.wanted(10.0 + GLIDE_S * 0.75).unwrap();
        view.drawn(wanted - Vec2::new(0.0, 120.0), view.window, view.content);
        assert!(!view.gliding() && view.by_hand, "a wheel moved it: the glide stops");
        assert_eq!(view.offset, wanted - Vec2::new(0.0, 120.0), "where the hand left it");
        assert_eq!(view.wanted(11.0), None, "nothing more asked");
    }

    #[test]
    fn sideways_moves_half_a_window_and_stops_at_the_edges() {
        let (stops, mut view) = lines();
        view.content.x = 700.0;
        assert_eq!(view.go(&stops, Some(3), Go::Sideways(1)).map(|m| (m.cursor, m.offset.x)), Some((3, 150.0)));
        view.offset.x = 400.0;
        assert_eq!(view.go(&stops, Some(3), Go::Sideways(1)), None, "at the right edge");
        assert_eq!(view.go(&stops, Some(3), Go::Sideways(-3)).map(|m| m.offset.x), Some(0.0));
        view.offset.x = 0.0;
        view.column = 8.0;
        assert_eq!(view.go(&stops, Some(3), Go::Panels(4)).map(|m| m.offset.x), Some(32.0), "a few columns");
        view.content.x = 200.0;
        view.offset.x = 0.0;
        assert_eq!(view.go(&stops, Some(3), Go::Sideways(1)), None, "narrower than the window: nothing");
    }
}
