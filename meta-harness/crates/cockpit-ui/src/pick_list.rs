//! One list to pick from, for every list of choices: the action list, its
//! choices and searches, the key sheet. Rows of one height (a label, its
//! keys, a line of description), only those in sight drawn, laid out in one
//! pass (nothing measured a frame before it's shown, so nothing blinks); one
//! highlight, moved by the keys every list has and by the pointer, scrolled
//! into sight when the keys move it; a row picked by its key or a tap.

use egui::{Color32, Sense};

use crate::actions::Mark;
use crate::theme::colors;

/// A move through the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Next,
    Prev,
    HalfDown,
    HalfUp,
    First,
    Last,
}

/// One row as it's drawn.
#[derive(Clone, Debug, Default)]
pub struct Row<'a> {
    pub label: &'a str,
    pub mark: Option<(Color32, Mark)>,
    /// What it is now (a tick before it).
    pub current: bool,
    pub detail: Option<&'a str>,
    /// Its keys, on the right.
    pub keys: std::borrow::Cow<'a, str>,
    /// Shown but not to be picked here (an action that doesn't apply now).
    pub faded: bool,
}

/// What's listed: its id (what keeps where it's scrolled), its height
/// whatever the rows' number, how many rows, and whether any has a line
/// of description (all are then that tall).
#[derive(Clone, Copy, Debug)]
pub struct Listing<'s> {
    pub id: &'s str,
    pub height: f32,
    pub len: usize,
    pub details: bool,
}

#[derive(Default)]
pub struct PickList {
    highlight: usize,
    /// The keys moved it: it's brought into sight.
    bring: bool,
    /// Rows in sight last frame (half a page is half of these).
    in_sight: usize,
    /// Which list was drawn last frame and how far it was scrolled, as
    /// its scroll area said.
    scrolled: Option<(egui::Id, f32)>,
}

impl PickList {
    pub fn highlight(&self) -> usize {
        self.highlight
    }

    /// Back to the first row (the list changed).
    pub fn reset(&mut self) {
        self.highlight = 0;
    }

    /// A move over `len` rows.
    pub fn moved(&mut self, by: Move, len: usize) {
        if len == 0 {
            self.highlight = 0;
            return;
        }
        let half = (self.in_sight / 2).max(1);
        self.highlight = match by {
            Move::Next => (self.highlight + 1) % len,
            Move::Prev => (self.highlight + len - 1) % len,
            Move::HalfDown => (self.highlight + half).min(len - 1),
            Move::HalfUp => self.highlight.saturating_sub(half),
            Move::First => 0,
            Move::Last => len - 1,
        };
        self.bring = true;
    }

    /// The height of a row: its label line, and a line of description when
    /// any row has one.
    fn row_height(ui: &egui::Ui, details: bool) -> f32 {
        let look = crate::theme::look();
        let line = |size: f32| ui.fonts_mut(|f| f.row_height(&egui::FontId::proportional(size)));
        let text = line(look.text_label) + if details { line(look.text_small) } else { 0.0 };
        text + 2.0 * look.space_sm
    }

    /// Draws the rows listed (`row` gives each); returns the one picked
    /// (tapped, or the highlight on `enter`).
    pub fn show<'a>(&mut self, ui: &mut egui::Ui, listing: Listing<'_>, row: impl Fn(usize) -> Row<'a>, enter: bool) -> Option<usize> {
        let Listing { id, height, len, details } = listing;
        self.highlight = self.highlight.min(len.saturating_sub(1));
        let row_h = Self::row_height(ui, details);
        let spacing = ui.spacing().item_spacing.y;
        let stride = row_h + spacing;
        let mut area = egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).max_height(height).min_scrolled_height(height);
        // The keys moved it: into sight, as little as it takes.
        let list_id = egui::Id::new(id);
        if std::mem::take(&mut self.bring) {
            let offset = self.scrolled.filter(|(was, _)| *was == list_id).map_or(0.0, |(_, y)| y);
            let (top, bottom) = (self.highlight as f32 * stride, self.highlight as f32 * stride + row_h);
            if top < offset {
                area = area.vertical_scroll_offset(top);
            } else if bottom > offset + height {
                area = area.vertical_scroll_offset(bottom - height);
            }
        }
        let mut picked = None;
        let moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
        let output = area.show_rows(ui, row_h, len, |ui, range| {
            for i in range {
                let row = row(i);
                let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click());
                // The pointer moving over a row moves the highlight: hand and
                // keys always agree on it.
                if moved && response.hovered() {
                    self.highlight = i;
                }
                if response.clicked() && !row.faded {
                    picked = Some(i);
                }
                Self::paint(ui, rect, &row, i == self.highlight);
            }
        });
        self.in_sight = ((output.inner_rect.height() / stride).floor() as usize).max(1);
        self.scrolled = Some((list_id, output.state.offset.y));
        if enter && len > 0 && picked.is_none() && !row(self.highlight).faded {
            picked = Some(self.highlight);
        }
        picked
    }

    /// A row, painted: its fill when highlighted, its label (cut to fit
    /// beside its keys), its keys, its description under them.
    fn paint(ui: &egui::Ui, rect: egui::Rect, row: &Row<'_>, highlighted: bool) {
        let (look, theme) = (crate::theme::look(), colors());
        if highlighted {
            ui.painter().rect_filled(rect, look.radius_m, theme.selection);
        }
        let inner = rect.shrink2(egui::vec2(look.space_m, look.space_sm));
        let keys = ui.painter().layout_no_wrap(row.keys.to_string(), egui::FontId::monospace(look.text_small), theme.key);
        let label_width = (inner.width() - keys.size().x - look.space_m).max(0.0);
        let strong = if row.faded { theme.faint } else { theme.text_strong };
        let label = if row.current { format!("{} {}", egui_phosphor::regular::CHECK, row.label) } else { row.label.to_owned() };
        let galley = crate::actions::choice_text(ui, &label, row.mark, look.text_label, strong).into_galley(ui, Some(egui::TextWrapMode::Truncate), label_width, egui::TextStyle::Body);
        ui.painter().galley(inner.left_top(), galley.clone(), strong);
        ui.painter().galley(egui::pos2(inner.right() - keys.size().x, inner.top() + (galley.size().y - keys.size().y) / 2.0), keys, theme.key);
        if let Some(detail) = row.detail.filter(|d| !d.is_empty()) {
            let mut job = egui::text::LayoutJob::simple_singleline(detail.to_owned(), egui::FontId::proportional(look.text_small), theme.muted);
            job.wrap = egui::text::TextWrapping::truncate_at_width(inner.width());
            let detail = ui.painter().layout_job(job);
            ui.painter().galley(egui::pos2(inner.left(), inner.top() + galley.size().y), detail, theme.muted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame of a list of `len` rows ("row 0" …), 120 tall; what it
    /// picked and where each row's label was drawn.
    fn frame(ctx: &egui::Context, list: &mut PickList, len: usize, faded: Option<usize>, events: Vec<egui::Event>, enter: bool) -> (Option<usize>, Vec<(String, egui::Pos2)>) {
        let labels: Vec<String> = (0..len).map(|i| format!("row {i}")).collect();
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))), events, ..Default::default() };
        let mut picked = None;
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let row = |i: usize| Row { label: &labels[i], faded: faded == Some(i), ..Row::default() };
                picked = list.show(ui, Listing { id: "test_list", height: 120.0, len, details: false }, row, enter);
            });
        });
        let mut drawn = Vec::new();
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                drawn.push((text.galley.text().to_owned(), text.pos));
            }
        }
        (picked, drawn)
    }

    fn at(drawn: &[(String, egui::Pos2)], label: &str) -> Option<egui::Pos2> {
        drawn.iter().find(|(t, _)| t == label).map(|(_, p)| *p + egui::vec2(4.0, 4.0))
    }

    #[test]
    fn keys_move_the_highlight_round_by_one_and_to_the_ends_by_half_a_page() {
        let mut list = PickList { in_sight: 6, ..PickList::default() };
        let mut went = Vec::new();
        for by in [Move::Prev, Move::Next, Move::HalfDown, Move::HalfDown, Move::HalfDown, Move::HalfDown, Move::HalfUp, Move::First, Move::Last] {
            list.moved(by, 10);
            went.push(list.highlight());
        }
        assert_eq!(went, [9, 0, 3, 6, 9, 9, 6, 0, 9], "one wraps round; half a page (3 of 6 in sight) stops at the ends");
        list.moved(Move::Next, 0);
        assert_eq!(list.highlight(), 0, "an empty list has the first");
    }

    #[test]
    fn the_row_the_keys_move_to_is_brought_into_sight_and_the_pointer_and_a_tap_pick_as_the_keys_do() {
        let ctx = egui::Context::default();
        let mut list = PickList::default();
        let (_, drawn) = frame(&ctx, &mut list, 50, None, vec![], false);
        assert!(at(&drawn, "row 0").is_some() && at(&drawn, "row 49").is_none(), "only the rows in sight are drawn");
        list.moved(Move::Last, 50);
        frame(&ctx, &mut list, 50, None, vec![], false);
        let (_, drawn) = frame(&ctx, &mut list, 50, None, vec![], false);
        assert!(at(&drawn, "row 49").is_some() && at(&drawn, "row 0").is_none(), "the last, brought into sight");
        list.moved(Move::Next, 50);
        frame(&ctx, &mut list, 50, None, vec![], false);
        let (_, drawn) = frame(&ctx, &mut list, 50, None, vec![], false);
        assert!(at(&drawn, "row 0").is_some() && at(&drawn, "row 49").is_none(), "round to the first, back up to it");

        // A new list: the pointer moving over a row moves the highlight there.
        let (ctx, mut list) = (egui::Context::default(), PickList::default());
        let (_, drawn) = frame(&ctx, &mut list, 5, Some(3), vec![], false);
        let (two, three) = (at(&drawn, "row 2").unwrap(), at(&drawn, "row 3").unwrap());
        // (Its movement is between frames: it comes in, then moves.)
        let over = |list: &mut PickList, pos: egui::Pos2| {
            frame(&ctx, list, 5, Some(3), vec![egui::Event::PointerMoved(pos - egui::vec2(0.0, 2.0))], false);
            frame(&ctx, list, 5, Some(3), vec![egui::Event::PointerMoved(pos)], false);
        };
        over(&mut list, two);
        assert_eq!(list.highlight(), 2, "the hand and the keys agree on the highlight");
        let (picked, _) = frame(&ctx, &mut list, 5, Some(3), vec![], true);
        assert_eq!(picked, Some(2), "the key picks the highlight");
        let tap = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let (picked, _) = frame(&ctx, &mut list, 5, Some(3), vec![tap(two, true), tap(two, false)], false);
        assert_eq!(picked, Some(2), "a tap picks its row");
        over(&mut list, three);
        let (tapped, _) = frame(&ctx, &mut list, 5, Some(3), vec![tap(three, true), tap(three, false)], false);
        let (entered, _) = frame(&ctx, &mut list, 5, Some(3), vec![], true);
        assert_eq!((list.highlight(), tapped, entered), (3, None, None), "a faded row is shown, highlighted, not picked");
    }
}
