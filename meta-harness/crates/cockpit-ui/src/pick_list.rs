//! One list to pick from, for every list of choices: the action list, its
//! choices and searches, a panel's setting picker. Rows of one height (a
//! label, its keys, a line of description), only those in sight drawn,
//! laid out in one pass (nothing measured a frame before it's shown, so
//! nothing blinks); with no rows, a note in their place, the list keeping
//! its height. One highlight: moved by the moves every view has
//! (`nav::Motion`: down and up, going round at the ends; half a window;
//! first and last), the list scrolled by the views' own rules
//! (`viewport::Viewport`: room around the highlight, a glide, a scroll by
//! hand respected), and moved by the pointer too. A row is picked by
//! Enter or a tap.

use std::borrow::Cow;

use egui::{Color32, Sense};

use crate::actions::{Choice, Mark};
use crate::nav::Motion;
use crate::theme::colors;
use crate::viewport::{Go, Moved, Stop, Viewport};

/// One row as it's drawn.
#[derive(Clone, Debug, Default)]
pub struct Row<'a> {
    pub label: &'a str,
    /// Its colour, shown before its label (as a choice's is).
    pub colour: Option<(Color32, Mark)>,
    /// What it is now (a tick before it).
    pub current: bool,
    pub detail: Option<&'a str>,
    /// Its keys, on the right.
    pub keys: Cow<'a, str>,
    /// Shown but not picked here (an action that doesn't apply now).
    pub unavailable: bool,
}

impl<'a> Row<'a> {
    /// A choice, as it's listed wherever choices are.
    pub fn of_choice(choice: &'a Choice) -> Self {
        Self { label: &choice.label, colour: choice.colour, current: choice.current, detail: choice.detail.as_deref(), ..Self::default() }
    }
}

/// What's listed: its id (what keeps where it's scrolled), its height
/// whatever the rows' number, how many rows, whether any has a line of
/// description (all are then that tall), and what's said when there are
/// none.
#[derive(Clone, Copy, Debug)]
pub struct Listing<'s> {
    pub id: &'s str,
    pub height: f32,
    pub len: usize,
    pub details: bool,
    pub empty: &'s str,
}

#[derive(Clone, Debug, Default)]
pub struct PickList {
    highlight: usize,
    viewport: Viewport,
    /// The rows changed: back to the top when next drawn.
    to_top: bool,
}

impl PickList {
    /// A list whose highlight starts on row `index` (what's chosen now).
    pub fn at(index: usize) -> Self {
        Self { highlight: index, ..Self::default() }
    }

    pub fn highlight(&self) -> usize {
        self.highlight
    }

    /// Back to the first row (the rows changed).
    pub fn reset(&mut self) {
        (self.highlight, self.to_top) = (0, true);
    }

    /// The height of a row: its label line, and a line of description when
    /// any row has one.
    fn row_height(ui: &egui::Ui, details: bool) -> f32 {
        let look = crate::theme::look();
        let line = |size: f32| ui.fonts_mut(|f| f.row_height(&egui::FontId::proportional(size)));
        let text = line(look.text_label) + if details { line(look.text_small) } else { 0.0 };
        text + 2.0 * look.space_sm
    }

    /// How tall `len` rows are, `most` of them at most (a list sized to
    /// what it holds).
    pub fn fitting(ui: &egui::Ui, len: usize, details: bool, most: usize) -> f32 {
        let stride = Self::row_height(ui, details) + ui.spacing().item_spacing.y;
        len.clamp(1, most.max(1)) as f32 * stride
    }

    /// Draws the rows listed (`row` gives each), after the keys' move
    /// (`moved`); returns the one picked (tapped, or the highlight on
    /// `enter`).
    pub fn show<'a>(&mut self, ui: &mut egui::Ui, listing: Listing<'_>, moved: Option<Motion>, enter: bool, row: impl Fn(usize) -> Row<'a>) -> Option<usize> {
        let Listing { id, height, len, details, empty } = listing;
        let now = ui.input(|i| i.time);
        let row_h = Self::row_height(ui, details);
        let stride = row_h + ui.spacing().item_spacing.y;
        self.viewport.margin = stride;
        self.highlight = self.highlight.min(len.saturating_sub(1));
        if std::mem::take(&mut self.to_top) {
            self.viewport.take(&Moved { cursor: 0, offset: egui::Vec2::ZERO }, now, false);
        }
        if let Some(motion) = moved.filter(|_| len > 0) {
            self.go(motion, len, stride, now);
        }
        let mut area = egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).max_height(height).min_scrolled_height(height);
        if let Some(offset) = self.viewport.wanted(now) {
            area = area.vertical_scroll_offset(offset.y);
        }
        if len == 0 {
            let output = area.show(ui, |ui| ui.label(egui::RichText::new(empty).size(crate::theme::look().text_body).color(colors().muted)));
            self.viewport.drawn(output.state.offset, output.inner_rect.size(), output.content_size);
            return None;
        }
        let mut picked = None;
        let pointer_moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
        let output = area.show_rows(ui, row_h, len, |ui, range| {
            // Laid out, then painted: the row the pointer moved onto is
            // the highlight before any row is painted (never two lit).
            let laid: Vec<(usize, egui::Rect, egui::Response)> = range
                .map(|i| {
                    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click());
                    (i, rect, response)
                })
                .collect();
            if pointer_moved && let Some((i, ..)) = laid.iter().find(|(_, _, r)| r.hovered()) {
                self.highlight = *i;
            }
            for (i, rect, response) in laid {
                let row = row(i);
                if response.clicked() && !row.unavailable {
                    picked = Some(i);
                }
                Self::paint(ui, rect, row, i == self.highlight);
            }
        });
        self.viewport.drawn(output.state.offset, output.inner_rect.size(), output.content_size);
        if self.viewport.gliding() {
            ui.ctx().request_repaint();
        }
        if enter && picked.is_none() && !row(self.highlight).unavailable {
            picked = Some(self.highlight);
        }
        picked
    }

    /// A key's move over `len` rows, by the views' rules; the list's own:
    /// one down from the last goes round to the first, one up from the
    /// first to the last.
    fn go(&mut self, motion: Motion, len: usize, stride: f32, now: f64) {
        let go = match motion {
            Motion::Down if self.highlight + 1 == len => Go::First,
            Motion::Up if self.highlight == 0 => Go::Last,
            _ => match Go::of(motion, 1) {
                Some(go) => go,
                None => return,
            },
        };
        let stops: Vec<Stop> = (0..len).map(|i| Stop::new(i as f32 * stride, stride)).collect();
        if let Some(moved) = self.viewport.go(&stops, Some(self.highlight), go) {
            self.highlight = moved.cursor;
            self.viewport.take(&moved, now, true);
        }
    }

    /// A row, painted: its fill when highlighted, its label (cut to fit
    /// beside its keys), its keys, its description under them.
    fn paint(ui: &egui::Ui, rect: egui::Rect, row: Row<'_>, highlighted: bool) {
        let (look, theme) = (crate::theme::look(), colors());
        if highlighted {
            ui.painter().rect_filled(rect, look.radius_m, theme.selection);
        }
        let inner = rect.shrink2(egui::vec2(look.space_m, look.space_sm));
        let keys = ui.painter().layout_no_wrap(row.keys.into_owned(), egui::FontId::monospace(look.text_small), theme.key);
        let label_width = (inner.width() - keys.size().x - look.space_m).max(0.0);
        let strong = if row.unavailable { theme.faint } else { theme.text_strong };
        let label: Cow<'_, str> = if row.current { format!("{} {}", egui_phosphor::regular::CHECK, row.label).into() } else { row.label.into() };
        let galley = crate::actions::choice_text(ui, &label, row.colour, look.text_label, strong).into_galley(ui, Some(egui::TextWrapMode::Truncate), label_width, egui::TextStyle::Body);
        ui.painter().galley(egui::pos2(inner.right() - keys.size().x, inner.top() + (galley.size().y - keys.size().y) / 2.0), keys, theme.key);
        let label_height = galley.size().y;
        ui.painter().galley(inner.left_top(), galley, strong);
        if let Some(detail) = row.detail.filter(|d| !d.is_empty()) {
            let mut job = egui::text::LayoutJob::simple_singleline(detail.to_owned(), egui::FontId::proportional(look.text_small), theme.muted);
            job.wrap = egui::text::TextWrapping::truncate_at_width(inner.width());
            ui.painter().galley(egui::pos2(inner.left(), inner.top() + label_height), ui.painter().layout_job(job), theme.muted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list of `len` rows ("row 0" …), 120 tall, in a window of its own.
    struct Shown {
        ctx: egui::Context,
        list: PickList,
        len: usize,
        unavailable: Option<usize>,
        time: f64,
    }

    /// What one frame gave: the row picked, where each row's label was
    /// drawn, and how tall the list was.
    struct Frame {
        picked: Option<usize>,
        drawn: Vec<(String, egui::Pos2)>,
        height: f32,
    }

    impl Shown {
        fn new(len: usize, unavailable: Option<usize>) -> Self {
            Self { ctx: egui::Context::default(), list: PickList::default(), len, unavailable, time: 0.0 }
        }

        /// A frame a second after the last (a glide is over by then).
        fn frame(&mut self, events: Vec<egui::Event>, moved: Option<Motion>, enter: bool) -> Frame {
            self.time += 1.0;
            let labels: Vec<String> = (0..self.len).map(|i| format!("row {i}")).collect();
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))), time: Some(self.time), events, ..Default::default() };
            let (mut picked, mut height) = (None, 0.0);
            let (list, len, unavailable) = (&mut self.list, self.len, self.unavailable);
            let output = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let row = |i: usize| Row { label: &labels[i], unavailable: unavailable == Some(i), ..Row::default() };
                    let top = ui.cursor().top();
                    picked = list.show(ui, Listing { id: "test_list", height: 120.0, len, details: false, empty: "Nothing here" }, moved, enter, row);
                    height = ui.cursor().top() - top;
                });
            });
            let mut drawn = Vec::new();
            for clipped in &output.shapes {
                if let egui::Shape::Text(text) = &clipped.shape {
                    drawn.push((text.galley.text().to_owned(), text.pos));
                }
            }
            Frame { picked, drawn, height }
        }

        /// The pointer, moved onto `pos` (its movement is between frames:
        /// it comes in, then moves).
        fn point(&mut self, pos: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(pos - egui::vec2(0.0, 2.0))], None, false);
            self.frame(vec![egui::Event::PointerMoved(pos)], None, false);
        }
    }

    fn at(frame: &Frame, label: &str) -> Option<egui::Pos2> {
        frame.drawn.iter().find(|(t, _)| t == label).map(|(_, p)| *p + egui::vec2(4.0, 4.0))
    }

    /// The moves every view has, by the views' rules: one up from the first
    /// goes round to the last and back; half a window moves the list and
    /// the highlight together; first and last; the highlight is always in
    /// sight, and only the rows in sight are drawn.
    #[test]
    fn the_keys_move_the_highlight_as_in_every_view_and_round_at_the_ends() {
        let mut shown = Shown::new(50, None);
        let first = shown.frame(vec![], None, false);
        assert!(at(&first, "row 0").is_some() && at(&first, "row 49").is_none(), "only the rows in sight are drawn");
        let mut went = Vec::new();
        for moved in [Motion::Up, Motion::Down, Motion::HalfDown, Motion::HalfDown, Motion::Last, Motion::HalfUp, Motion::First] {
            shown.frame(vec![], Some(moved), false);
            let frame = shown.frame(vec![], None, false);
            let highlight = shown.list.highlight();
            assert!(at(&frame, &format!("row {highlight}")).is_some(), "{moved:?}: row {highlight} is in sight");
            went.push(highlight);
        }
        assert_eq!((went[0], went[1], went[4], went[6]), (49, 0, 49, 0), "round at the ends; last and first: {went:?}");
        assert!(went[2] > 0 && went[3] > went[2] && went[5] < 49, "half a window down twice, then up: {went:?}");
    }

    /// The pointer moving over a row moves the highlight; Enter and a tap
    /// pick it; a row that doesn't apply is shown and highlighted but not
    /// picked; with no rows, a note in their place and the same height.
    #[test]
    fn the_pointer_and_a_tap_pick_as_the_keys_do_and_an_empty_list_keeps_its_height() {
        let mut shown = Shown::new(5, Some(3));
        let frame = shown.frame(vec![], None, false);
        let (two, three, full) = (at(&frame, "row 2").unwrap(), at(&frame, "row 3").unwrap(), frame.height);
        shown.point(two);
        assert_eq!(shown.list.highlight(), 2, "the hand and the keys agree on the highlight");
        assert_eq!(shown.frame(vec![], None, true).picked, Some(2), "Enter picks the highlight");
        let tap = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        assert_eq!(shown.frame(vec![tap(two, true), tap(two, false)], None, false).picked, Some(2), "a tap picks its row");
        shown.point(three);
        let tapped = shown.frame(vec![tap(three, true), tap(three, false)], None, false).picked;
        let entered = shown.frame(vec![], None, true).picked;
        assert_eq!((shown.list.highlight(), tapped, entered), (3, None, None), "a row that doesn't apply: highlighted, not picked");

        let mut empty = Shown::new(0, None);
        let frame = empty.frame(vec![], Some(Motion::Down), true);
        assert!(frame.drawn.iter().any(|(t, _)| t == "Nothing here") && frame.picked.is_none());
        assert_eq!(frame.height, full, "no rows: the same height");
    }
}
