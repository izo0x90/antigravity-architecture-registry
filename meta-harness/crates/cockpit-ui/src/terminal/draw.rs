//! Drawing a terminal: its lines as cells on a grid (one row per line,
//! one column per character, two for a wide one), in the colours the
//! program asked for as the theme shows them; the cursor, the selection;
//! and, while it's typed in, what's typed sent as typed.

use egui::{Color32, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};
use harness_contracts::{Colour, Mods, Row, Style, TermKey, TermSize, TerminalInput, ToTerminal};

use super::{Cell, TerminalBuffer};
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
    pub(super) fn draw(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, typing: bool, started: bool) {
        let look = crate::theme::look();
        let theme = colors();
        let font = egui::FontId::monospace(look.text_body);
        self.char_w = ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
        let row_h = ui.fonts_mut(|f| f.row_height(&font)).max(1.0);
        self.pitch = row_h;
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

        // Its size: what fits (the scrollbar's room kept).
        let room = ui.available_size() - vec2(ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_outer_margin, 0.0);
        let size = TermSize { cols: (room.x / self.char_w).floor().clamp(10.0, 1000.0) as u16, rows: (room.y / row_h).floor().clamp(3.0, 500.0) as u16 };
        if self.link.is_some() && self.size_sent != Some(size) {
            self.size_sent = Some(size);
            self.send(&ToTerminal::Size { size });
        }

        let total = self.total() as usize;
        self.viewport.margin = 2.0 * row_h;
        self.viewport.column = self.char_w;
        let mut area = egui::ScrollArea::vertical().id_salt(("terminal", cx.at)).auto_shrink([false, false]).stick_to_bottom(self.cursor.is_none());
        if std::mem::take(&mut self.to_live) {
            area = area.vertical_scroll_offset(total as f32 * row_h);
        } else if let Some(offset) = self.viewport.wanted(self.clock) {
            area = area.vertical_scroll_offset(offset.y);
        }
        let id = egui::Id::new(("terminal_grid", cx.at));
        let mut shown = 0..0;
        let mut grid = (Rect::NOTHING, None::<egui::Response>);
        let output = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                area.show_rows(ui, row_h, total, |ui, range| {
                    let width = ui.available_width().max(self.char_w * f32::from(size.cols));
                    let (rect, _) = ui.allocate_exact_size(vec2(width, range.len() as f32 * row_h), Sense::hover());
                    let response = ui.interact(rect, id, Sense::click_and_drag());
                    self.paint(ui.painter(), rect, range.clone(), &font, typing, &theme);
                    shown = range;
                    grid = (rect, Some(response));
                })
            })
            .inner;
        self.viewport.drawn(output.state.offset, output.inner_rect.size(), output.content_size);
        if self.viewport.gliding() {
            ui.ctx().request_repaint();
        }
        self.grid = Some(output.inner_rect);
        self.want_lines(shown.start as u64..shown.end as u64);
        let (rect, response) = grid;
        // What hints label: each line with something on it, as it shows.
        let clip = output.inner_rect;
        self.spots = (shown.start as u64..shown.end as u64)
            .filter(|l| self.line(*l).is_some_and(|r| r.iter().any(|run| !run.text.trim().is_empty())))
            .map(|l| crate::nav::Spot { key: l.to_string(), rect: Rect::from_min_size(pos2(rect.left(), rect.top() + (l - shown.start as u64) as f32 * row_h), vec2(rect.width(), row_h)) })
            .filter(|s| s.rect.intersects(clip))
            .collect();
        let Some(response) = response else { return };
        self.pointer(&response, rect, shown.start as u64, row_h);
        if self.screen.alternate {
            self.wheel_as_keys(ui, &response, row_h);
        }
        if typing {
            self.typing(ui, &response, id, rect, shown.start as u64, row_h, started, cx);
        }
    }

    /// Lines `range` on `rect`, a row each.
    fn paint(&self, painter: &egui::Painter, rect: Rect, range: std::ops::Range<usize>, font: &egui::FontId, typing: bool, theme: &Theme) {
        let (row_h, char_w) = (self.pitch, self.char_w);
        let history = u64::from(self.history());
        let selection = self.selection.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        for (i, line) in range.map(|l| l as u64).enumerate() {
            let top = rect.top() + i as f32 * row_h;
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
            if let Some(row) = self.line(line) {
                paint_row(painter, row, pos2(rect.left(), top), char_w, row_h, font, theme);
            }
            // The screen's cursor: a block while typed in, an outline else.
            let cursor = self.screen.cursor;
            if cursor.visible && line == history + u64::from(cursor.row) && self.ended().is_none() {
                let at = Rect::from_min_size(pos2(rect.left() + f32::from(cursor.col) * char_w, top), vec2(char_w, row_h));
                if typing {
                    painter.rect_filled(at, 0.0, theme.text);
                    let under = self.line(line).and_then(|r| char_at(r, cursor.col)).unwrap_or(' ');
                    painter.text(at.left_top(), egui::Align2::LEFT_TOP, under, font.clone(), theme.bg);
                } else {
                    painter.rect_stroke(at, 0.0, Stroke::new(crate::theme::look().line_thin, theme.text), egui::StrokeKind::Inside);
                }
            }
        }
    }

    /// The cell under `pos`, on the grid drawn from line `first`.
    fn cell_at(&self, pos: egui::Pos2, rect: Rect, first: u64, row_h: f32) -> Cell {
        let line = first + ((pos.y - rect.top()) / row_h).floor().max(0.0) as u64;
        let col = ((pos.x - rect.left()) / self.char_w).floor().max(0.0) as u16;
        (line.min(self.total().saturating_sub(1)), col)
    }

    /// The pointer on the grid: dragging selects; a click puts the cursor
    /// on a line (not typing) and lets go of a selection.
    fn pointer(&mut self, response: &egui::Response, rect: Rect, first: u64, row_h: f32) {
        if response.drag_started()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let at = self.cell_at(pos, rect, first, row_h);
            self.selection = Some((at, at));
        }
        if response.dragged()
            && let (Some(pos), Some((from, _))) = (response.interact_pointer_pos(), self.selection)
        {
            self.selection = Some((from, self.cell_at(pos, rect, first, row_h)));
        }
        if response.clicked() {
            self.selection = None;
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
    #[allow(clippy::too_many_arguments)]
    fn typing(&mut self, ui: &egui::Ui, response: &egui::Response, id: egui::Id, rect: Rect, first: u64, row_h: f32, started: bool, cx: &mut RunCx<'_>) {
        response.request_focus();
        ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        let cursor = self.screen.cursor;
        let line = u64::from(self.history()) + u64::from(cursor.row);
        let top = rect.top() + (line.saturating_sub(first)) as f32 * row_h;
        let cursor_rect = Rect::from_min_size(pos2(rect.left() + f32::from(cursor.col) * self.char_w, top), vec2(2.0, row_h));
        ui.ctx().output_mut(|o| o.ime = Some(egui::output::IMEOutput { rect, cursor_rect }));
        if started {
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
