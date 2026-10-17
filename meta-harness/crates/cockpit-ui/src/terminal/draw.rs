//! Drawing a terminal: its lines as cells on a grid (one row per line,
//! one column per character, two for a wide one), in the colours the
//! program asked for as the theme shows them; the cursor, the selection;
//! and, while it's typed in, what's typed sent as typed.

use egui::{Color32, Rect, RichText, Sense, Stroke, pos2, vec2};
use harness_contracts::{Colour, Mods, Row, Style, TermKey, TermSize, TerminalInput, ToTerminal};

use super::{Cell, TerminalBuffer};

/// Whether it's typed in this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Keys {
    Not,
    /// Typing starts this frame: its keys were the cockpit's.
    Starting,
    Typing,
}

/// How long what was drawn stays while the lines the view wants come.
const WAIT_S: f64 = 0.5;

/// The grid as drawn this frame: where, from which line, how tall a row.
#[derive(Clone, Copy)]
struct Grid {
    rect: Rect,
    first: u64,
    row_h: f32,
}
use crate::actions::RunCx;
use crate::keymap::Typed;
use crate::theme::{Theme, colors};

/// A run's text and background colours (bold's named colours bright, as
/// terminals show them; inverse swapped; dim faded; hidden as its background).
fn run_colours(style: &Style, theme: &Theme) -> (Color32, Option<Color32>) {
    let fg = match style.fg {
        Colour::Named(n) if n < 8 && style.has(Style::BOLD) => Colour::Named(n + 8),
        other => other,
    };
    let (mut fg, mut bg) = (theme.term_colour(fg).unwrap_or(theme.text), theme.term_colour(style.bg));
    if style.has(Style::INVERSE) {
        (fg, bg) = (bg.unwrap_or(theme.bg), Some(fg));
    }
    if style.has(Style::DIM) {
        fg = fg.gamma_multiply(0.6);
    }
    if style.has(Style::HIDDEN) {
        fg = bg.unwrap_or(theme.bg);
    }
    (fg, bg)
}

impl TerminalBuffer {
    pub(super) fn draw(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, keys: Keys) {
        let typing = keys != Keys::Not;
        let look = crate::theme::look();
        let theme = colors();
        let font = egui::FontId::monospace(look.text_body);
        self.char_w = ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
        let row_h = ui.fonts_mut(|f| f.row_height(&font)).max(1.0);
        self.clock = ui.input(|i| i.time);
        if self.id.is_none() {
            let words = if self.awaiting.is_some() { "Opening a terminal…" } else { "No terminal here yet." };
            ui.label(RichText::new(words).size(look.text_body).color(theme.muted));
            self.grid = None;
            return;
        }
        if let Some(code) = self.ended() {
            let ended = code.map_or_else(|| "Its shell ended (the server restarted).".to_owned(), |c| format!("Its shell ended ({c})."));
            ui.label(RichText::new(ended).size(look.text_body).color(theme.warn));
        }

        // The grid: whole rows of what fits, the scrollbar beside it.
        let bar_w = ui.spacing().scroll.bar_width;
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let id = egui::Id::new(("terminal_grid", cx.at));
        let response = ui.interact(rect, id, Sense::click_and_drag());
        let grid_rect = Rect::from_min_max(rect.min, pos2((rect.right() - bar_w).max(rect.left()), rect.bottom()));
        let size = TermSize { cols: (grid_rect.width() / self.char_w).floor().clamp(10.0, 1000.0) as u16, rows: (grid_rect.height() / row_h).floor().clamp(3.0, 500.0) as u16 };
        if self.link.is_some() && self.size_sent != Some(size) {
            self.size_sent = Some(size);
            self.send(&ToTerminal::Size { size });
        }
        (self.window, self.row_h) = (u64::from(size.rows), row_h);
        let bar = Rect::from_min_max(pos2(grid_rect.right(), rect.top()), rect.max);
        self.scroll_by_hand(ui, &response, bar, row_h);

        // What shows: the view's lines; until they've come, what showed
        // before (a moment at most), so no frame draws blank rows.
        let span = self.span();
        let wanted = self.view.top(span);
        let rows = |top: u64| top..(top + span.window).min(span.end);
        let missing = !self.lines.has(rows(wanted));
        self.waiting = if missing { Some(self.waiting.unwrap_or(self.clock)) } else { None };
        // (Following the live screen, what's new shows at once.)
        let top = match self.drawn {
            Some(before) if missing && !self.view.live() && (span.first..span.end).contains(&before) && self.lines.has(rows(before)) && self.waiting.is_some_and(|since| self.clock - since < WAIT_S) => {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(WAIT_S));
                before
            }
            _ => wanted,
        };
        self.drawn = Some(top);
        if let Some((from, count)) = self.lines.wanted(wanted.saturating_sub(span.window)..(wanted + 2 * span.window).min(span.end)) {
            self.send(&ToTerminal::Lines { from, count });
        }
        let grid = Grid { rect: grid_rect, first: top, row_h };
        self.paint(ui.painter_at(rect), grid, rows(top), &font, typing, &theme);
        self.paint_bar(ui.painter(), bar, span, &theme);
        self.grid = Some(grid_rect);

        // What hints label: the files and web links on screen.
        let folder = self.info().map(|t| t.folder).unwrap_or_default();
        let char_w = self.char_w;
        let mut spots = Vec::new();
        for l in rows(top) {
            let text = self.lines.text(l).unwrap_or_default();
            let y = grid_rect.top() + (l - top) as f32 * row_h;
            for link in super::links::links(&text, &folder) {
                let at = Rect::from_min_size(pos2(grid_rect.left() + f32::from(link.col) * char_w, y), vec2(f32::from(link.len) * char_w, row_h));
                if at.intersects(grid_rect) {
                    spots.push(crate::nav::Spot { key: format!("{l}:{}", link.col), rect: at });
                }
            }
        }
        self.spots = spots;
        if let Some(said) = self.said.take() {
            *cx.note = Some(said);
        }
        if let Some(url) = self.open_url.take() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        self.pointer(ui, &response, grid, bar);
        if self.lines.alternate() {
            self.wheel_as_keys(ui, &response, row_h);
        }
        if typing {
            self.typing(ui, &response, id, grid, keys, cx);
        }
    }

    /// The wheel, a finger, the scrollbar: the view moves by whole lines
    /// (a program with the whole screen gets the wheel as keys instead).
    fn scroll_by_hand(&mut self, ui: &egui::Ui, response: &egui::Response, bar: Rect, row_h: f32) {
        let span = self.span();
        if !self.lines.alternate() && response.hovered() {
            let delta = ui.input(|i| i.raw_scroll_delta.y);
            if delta != 0.0 {
                self.view.wheel(delta, row_h, span);
            }
        }
        let touch = ui.input(|i| i.any_touches());
        let Some(pos) = response.interact_pointer_pos() else {
            self.thumb_held = None;
            return;
        };
        if response.drag_started() && bar.contains(pos) {
            // Held where it was grabbed (a click off the thumb jumps it there).
            let (at, len) = self.view.thumb(span).unwrap_or((0.0, 1.0));
            let grabbed = (pos.y - bar.top()) / bar.height();
            self.thumb_held = Some(if (at..at + len).contains(&grabbed) { grabbed - at } else { len / 2.0 });
        }
        if let Some(held) = self.thumb_held {
            if response.dragged() {
                self.view.drag_thumb((pos.y - bar.top()) / bar.height() - held, span);
            }
        } else if touch && response.dragged() && !self.lines.alternate() {
            self.view.wheel(response.drag_delta().y, row_h, span);
        }
        if !response.dragged() {
            self.thumb_held = None;
        }
    }

    /// Lines `rows`, a row each from the grid's top.
    fn paint(&self, painter: egui::Painter, grid: Grid, rows: std::ops::Range<u64>, font: &egui::FontId, typing: bool, theme: &Theme) {
        let (rect, row_h, char_w) = (grid.rect, grid.row_h, self.char_w);
        let selection = self.selection.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        let (cursor_line, cursor_col, cursor_shows) = self.lines.cursor();
        for line in rows {
            let top = rect.top() + (line - grid.first) as f32 * row_h;
            if self.cursor == Some(line) {
                painter.rect_filled(Rect::from_min_size(pos2(rect.left(), top), vec2(rect.width(), row_h)), 0.0, theme.selection);
            }
            // The selection on this line: its columns.
            if let Some(((l0, c0), (l1, c1))) = selection
                && (l0..=l1).contains(&line)
            {
                let from = if line == l0 { f32::from(c0) } else { 0.0 };
                let to = if line == l1 { f32::from(c1) + 1.0 } else { rect.width() / char_w };
                painter.rect_filled(Rect::from_min_max(pos2(rect.left() + from * char_w, top), pos2(rect.left() + to * char_w, top + row_h)), 0.0, theme.widget_selection);
            }
            if let Some(row) = self.lines.get(line) {
                paint_row(&painter, row, pos2(rect.left(), top), char_w, row_h, font, theme);
            }
            // The screen's cursor: a block while typed in, an outline else.
            if cursor_shows && line == cursor_line && self.ended().is_none() {
                let at = Rect::from_min_size(pos2(rect.left() + f32::from(cursor_col) * char_w, top), vec2(char_w, row_h));
                if typing {
                    painter.rect_filled(at, 0.0, theme.text);
                    let under = self.lines.get(line).and_then(|r| char_at(r, cursor_col)).unwrap_or(' ');
                    painter.text(at.left_top(), egui::Align2::LEFT_TOP, under, font.clone(), theme.bg);
                } else {
                    painter.rect_stroke(at, 0.0, Stroke::new(crate::theme::look().line_thin, theme.text), egui::StrokeKind::Inside);
                }
            }
        }
    }

    /// The scrollbar: where the window is in all the lines (none when they fit).
    fn paint_bar(&self, painter: &egui::Painter, bar: Rect, span: super::view::Span, theme: &Theme) {
        let Some((at, len)) = self.view.thumb(span) else { return };
        let thumb = Rect::from_min_size(pos2(bar.left() + bar.width() / 4.0, bar.top() + at * bar.height()), vec2(bar.width() / 2.0, (len * bar.height()).max(self.char_w)));
        painter.rect_filled(thumb, thumb.width() / 2.0, if self.thumb_held.is_some() { theme.text } else { theme.muted });
    }

    /// The cell under `pos`, on the grid.
    fn cell_at(&self, pos: egui::Pos2, grid: Grid) -> Cell {
        let line = grid.first + ((pos.y - grid.rect.top()) / grid.row_h).floor().max(0.0) as u64;
        let col = ((pos.x - grid.rect.left()) / self.char_w).floor().max(0.0) as u16;
        (line.min(self.lines.end().saturating_sub(1)), col)
    }

    /// The pointer on the grid: dragging selects (a finger scrolls
    /// instead), the cursor where it ends; a click puts the cursor on the
    /// line clicked and lets go of a selection.
    fn pointer(&mut self, ui: &egui::Ui, response: &egui::Response, grid: Grid, bar: Rect) {
        let by_hand = self.thumb_held.is_some() || ui.input(|i| i.any_touches()) || response.interact_pointer_pos().is_some_and(|p| bar.contains(p));
        // From where it was pressed (it's a drag only once it's moved a little).
        if response.drag_started()
            && !by_hand
            && let Some(pos) = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos())
        {
            let at = self.cell_at(pos, grid);
            self.selection = Some((at, at));
        }
        if response.dragged()
            && !by_hand
            && let (Some(pos), Some((from, _))) = (response.interact_pointer_pos(), self.selection)
        {
            let to = self.cell_at(pos, grid);
            self.selection = Some((from, to));
            self.put_cursor(to.0);
        }
        if response.clicked() {
            self.selection = None;
            if let Some(pos) = response.interact_pointer_pos() {
                self.put_cursor(self.cell_at(pos, grid).0);
            }
        }
    }

    /// The cursor on `line`, where the pointer is (not while typing: there's
    /// none then).
    fn put_cursor(&mut self, line: u64) {
        if !self.typing {
            (self.cursor, self.cursor_col) = (Some(line), None);
        }
    }

    /// A program with the whole screen (an editor, a pager) has no
    /// scrollback: the wheel is its arrow keys.
    fn wheel_as_keys(&mut self, ui: &egui::Ui, response: &egui::Response, row_h: f32) {
        if !response.hovered() {
            return;
        }
        let delta = ui.input(|i| i.raw_scroll_delta.y);
        let lines = (delta / row_h).round() as i32;
        let key = if lines > 0 { TermKey::Up } else { TermKey::Down };
        for _ in 0..lines.unsigned_abs().min(20) {
            self.send(&ToTerminal::Input { input: TerminalInput::Key { key, mods: Mods::default() } });
        }
    }

    /// Typed in: it has the keyboard (Tab, the arrows and Escape are its),
    /// a phone shows its keyboard, and what's typed goes to the program. A
    /// copy with something selected copies it instead.
    fn typing(&mut self, ui: &egui::Ui, response: &egui::Response, id: egui::Id, grid: Grid, keys: Keys, cx: &mut RunCx<'_>) {
        let (rect, first, row_h) = (grid.rect, grid.first, grid.row_h);
        response.request_focus();
        ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        // Where the program's cursor is, kept on the grid (scrolled back, it's
        // below it: the browser would scroll the page to its text field).
        let (line, col, _) = self.lines.cursor();
        let top = (rect.top() + (line.saturating_sub(first)) as f32 * row_h).clamp(rect.top(), (rect.bottom() - row_h).max(rect.top()));
        let cursor_rect = Rect::from_min_size(pos2(rect.left() + f32::from(col) * self.char_w, top), vec2(2.0, row_h));
        ui.ctx().output_mut(|o| o.ime = Some(egui::output::IMEOutput { rect, cursor_rect }));
        if keys == Keys::Starting {
            return;
        }
        let typed = ui.input(|i| crate::keymap::terminal_input(&i.events));
        let copy_c = |t: &Typed| matches!(t, Typed::Input(TerminalInput::Key { key: TermKey::Char('c'), mods }) if mods.ctrl && !mods.alt);
        let copying = self.selection.is_some() && typed.iter().any(|t| *t == Typed::Copy || copy_c(t));
        if copying && let Some(text) = self.selected_text() {
            cx.ctx.copy_text(text);
            self.selection = None;
        }
        for t in typed {
            match t {
                Typed::Input(input) if !(copying && copy_c(&Typed::Input(input.clone()))) => self.type_in(input),
                _ => {}
            }
        }
    }
}

/// The character at column `col` of a row.
fn char_at(row: &Row, col: u16) -> Option<char> {
    let mut at = 0u16;
    for run in row {
        for c in run.text.chars() {
            let width = if run.wide { 2 } else { 1 };
            if col < at + width {
                return Some(c);
            }
            at += width;
        }
    }
    None
}

/// One row's runs from `left_top`: each character in its column.
fn paint_row(painter: &egui::Painter, row: &Row, left_top: egui::Pos2, char_w: f32, row_h: f32, font: &egui::FontId, theme: &Theme) {
    let mut col = 0f32;
    for run in row {
        let (fg, bg) = run_colours(&run.style, theme);
        let width = if run.wide { 2.0 } else { run.text.chars().count() as f32 };
        let x = left_top.x + col * char_w;
        if let Some(bg) = bg {
            painter.rect_filled(Rect::from_min_size(pos2(x, left_top.y), vec2(width * char_w, row_h)), 0.0, bg);
        }
        let format = |color| egui::TextFormat {
            font_id: font.clone(),
            color,
            italics: run.style.has(Style::ITALIC),
            underline: if run.style.has(Style::UNDERLINE) { Stroke::new(crate::theme::look().line_thin, color) } else { Stroke::NONE },
            strikethrough: if run.style.has(Style::STRIKE) { Stroke::new(crate::theme::look().line_thin, color) } else { Stroke::NONE },
            ..Default::default()
        };
        if run.text.is_ascii() && !run.wide {
            // Plain characters are the font's own width: the run at once.
            let galley = painter.layout_job(egui::text::LayoutJob::single_section(run.text.clone(), format(fg)));
            painter.galley(pos2(x, left_top.y), galley, fg);
        } else {
            // Others one by one, each in its column.
            let step = if run.wide { 2.0 } else { 1.0 };
            for (i, c) in run.text.chars().enumerate() {
                let galley = painter.layout_job(egui::text::LayoutJob::single_section(c.to_string(), format(fg)));
                painter.galley(pos2(x + i as f32 * step * char_w, left_top.y), galley, fg);
            }
        }
        col += width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_the_themes_for_the_named_and_the_256_by_their_formula() {
        let theme = crate::config::UiConfig::with_overrides("").unwrap().theme;
        assert_eq!(theme.term_colour(Colour::Named(1)), Some(theme.term_red));
        assert_eq!(theme.term_colour(Colour::Indexed(9)), Some(theme.term_bright_red), "the first 16 of the 256 are the named");
        assert_eq!(theme.term_colour(Colour::Indexed(196)), Some(Color32::from_rgb(255, 0, 0)), "the cube");
        assert_eq!(theme.term_colour(Colour::Indexed(232)), Some(Color32::from_rgb(8, 8, 8)), "the greys");
        assert_eq!(theme.term_colour(Colour::Rgb(1, 2, 3)), Some(Color32::from_rgb(1, 2, 3)));
        let bold_red = Style { fg: Colour::Named(1), bg: Colour::Default, attrs: Style::BOLD };
        assert_eq!(run_colours(&bold_red, &theme), (theme.term_bright_red, None), "bold is bright");
        let inverse = Style { fg: Colour::Default, bg: Colour::Default, attrs: Style::INVERSE };
        assert_eq!(run_colours(&inverse, &theme), (theme.bg, Some(theme.text)), "inverse swaps");
    }
}
