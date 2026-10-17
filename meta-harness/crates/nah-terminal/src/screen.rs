//! The screen as readers get it: rows of runs, what changed since a
//! reader last looked, the scrollback's lines, and searching them.

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermDamage;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use harness_contracts::{Colour, Cursor, Found, Row, Run, ScreenUpdate, Style, TermSize};

use crate::feed::Feed;

/// Which rows changed when, so each reader gets what's new to it.
#[derive(Default)]
pub struct Changes {
    /// Grows with every change.
    seq: u64,
    /// When each screen row last changed.
    rows: Vec<u64>,
    /// When the whole screen last changed (a resize): a reader from
    /// before then gets all of it.
    whole: u64,
}

impl Changes {
    /// Takes what the emulator says changed since it was last asked.
    pub fn take<T: EventListener>(&mut self, term: &mut Term<T>) {
        let lines = term.screen_lines();
        if self.rows.len() != lines {
            self.rows.resize(lines, 0);
            self.whole = self.seq + 1;
        }
        let damaged: Vec<usize> = match term.damage() {
            TermDamage::Full => (0..lines).collect(),
            TermDamage::Partial(lines) => lines.filter(|l| l.is_damaged()).map(|l| l.line).collect(),
        };
        term.reset_damage();
        if damaged.is_empty() && self.whole <= self.seq {
            return;
        }
        self.seq += 1;
        for line in damaged {
            if let Some(row) = self.rows.get_mut(line) {
                *row = self.seq;
            }
        }
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// What changed after `since` (all of it when `since` is none or from
    /// before the screen last changed whole).
    pub fn since<T: EventListener>(&self, term: &Term<T>, feed: &Feed, since: Option<u64>, title: &str) -> ScreenUpdate {
        let full = since.is_none_or(|s| s < self.whole);
        let rows = (0..term.screen_lines())
            .filter(|&r| full || since.is_some_and(|s| self.rows.get(r).copied().unwrap_or(0) > s))
            .map(|r| (r as u16, row(term, Line(r as i32))))
            .collect();
        let point = term.grid().cursor.point;
        ScreenUpdate {
            seq: self.seq,
            size: TermSize { cols: term.columns() as u16, rows: term.screen_lines() as u16 },
            full,
            rows,
            cursor: Cursor { row: point.line.0.max(0) as u16, col: point.column.0 as u16, visible: term.mode().contains(TermMode::SHOW_CURSOR) },
            top: feed.top(),
            oldest: feed.oldest(),
            alternate: term.mode().contains(TermMode::ALT_SCREEN),
            title: title.to_owned(),
        }
    }
}

/// The lines numbered `from` on, up to `count` of them, within what's
/// kept (from the oldest, when it's older).
pub fn lines<T: EventListener>(term: &Term<T>, feed: &Feed, from: u64, count: u32) -> (u64, Vec<Row>) {
    let (first, end) = kept(term, feed);
    let from = from.clamp(first, end);
    let to = from.saturating_add(u64::from(count)).min(end);
    (from, (from..to).map(|n| row(term, grid_line(feed, n))).collect())
}

/// The numbers of the lines kept: the scrollback's (none while the
/// alternate screen shows) and the screen's.
fn kept<T: EventListener>(term: &Term<T>, feed: &Feed) -> (u64, u64) {
    let top = feed.top();
    (top - term.history_size() as u64, top + term.screen_lines() as u64)
}

/// Where line `number` is in the emulator's grid.
fn grid_line(feed: &Feed, number: u64) -> Line {
    Line((number as i64 - feed.top() as i64) as i32)
}

/// Where `query` is in the scrollback and on the screen, newest first (at
/// most `most`); case doesn't matter unless the query has a capital.
pub fn search<T: EventListener>(term: &Term<T>, feed: &Feed, query: &str, most: usize) -> Vec<Found> {
    if query.is_empty() {
        return Vec::new();
    }
    let exact = query.chars().any(char::is_uppercase);
    let fold = |s: &str| if exact { s.to_owned() } else { s.to_lowercase() };
    let wanted = fold(query);
    let (first, end) = kept(term, feed);
    let mut found = Vec::new();
    for number in (first..end).rev() {
        let line = grid_line(feed, number);
        // The line's text, and the column each char is in.
        let mut text = String::new();
        let mut cols = Vec::new();
        let grid_line = &term.grid()[line];
        for col in 0..term.columns() {
            let cell = &grid_line[Column(col)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            for c in fold(&cell.c.to_string()).chars() {
                text.push(c);
                cols.push(col);
            }
        }
        let mut at = 0;
        let shown = || row(term, line).iter().map(|r| r.text.as_str()).collect::<String>();
        while let Some(i) = text[at..].find(&wanted) {
            let start = at + i;
            let first = text[..start].chars().count();
            let last = first + wanted.chars().count() - 1;
            if let (Some(&c0), Some(&c1)) = (cols.get(first), cols.get(last)) {
                found.push(Found { line: number, col: c0 as u16, len: (c1 - c0 + 1) as u16, text: shown() });
            }
            at = start + wanted.len().max(1);
        }
        if found.len() >= most {
            found.truncate(most);
            break;
        }
    }
    found
}

/// One line as runs (trailing blanks left out).
pub fn row<T: EventListener>(term: &Term<T>, line: Line) -> Row {
    let grid_line = &term.grid()[line];
    let mut runs: Vec<Run> = Vec::new();
    for col in 0..term.columns() {
        let cell = &grid_line[Column(col)];
        if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let style = style(cell);
        let wide = cell.flags.contains(Flags::WIDE_CHAR);
        let mut text = cell.c.to_string();
        if let Some(extra) = cell.zerowidth() {
            text.extend(extra);
        }
        match runs.last_mut() {
            Some(last) if !wide && !last.wide && last.style == style => last.text.push_str(&text),
            _ => runs.push(Run { text, style, wide }),
        }
    }
    // Trailing blanks in the plain style aren't sent.
    while let Some(last) = runs.last_mut() {
        if last.wide || last.style.bg != Colour::Default || last.style.has(Style::INVERSE) || last.style.has(Style::UNDERLINE) || last.style.has(Style::STRIKE) {
            break;
        }
        let kept = last.text.trim_end_matches(' ').len();
        if kept > 0 {
            last.text.truncate(kept);
            break;
        }
        runs.pop();
    }
    runs
}

fn style(cell: &Cell) -> Style {
    let f = cell.flags;
    let (fg, fg_dim) = colour(cell.fg);
    let (bg, _) = colour(cell.bg);
    let mut attrs = 0;
    for (flag, attr) in [
        (Flags::BOLD, Style::BOLD),
        (Flags::ITALIC, Style::ITALIC),
        (Flags::ALL_UNDERLINES, Style::UNDERLINE),
        (Flags::INVERSE, Style::INVERSE),
        (Flags::DIM, Style::DIM),
        (Flags::STRIKEOUT, Style::STRIKE),
        (Flags::HIDDEN, Style::HIDDEN),
    ] {
        if f.intersects(flag) {
            attrs |= attr;
        }
    }
    if fg_dim {
        attrs |= Style::DIM;
    }
    Style { fg, bg, attrs }
}

/// A colour as asked for; true when it's a dim one (sent as its colour, dim).
fn colour(c: Color) -> (Colour, bool) {
    match c {
        Color::Spec(rgb) => (Colour::Rgb(rgb.r, rgb.g, rgb.b), false),
        Color::Indexed(i) if i < 16 => (Colour::Named(i), false),
        Color::Indexed(i) => (Colour::Indexed(i), false),
        Color::Named(n) => {
            let i = n as usize;
            if i < 16 {
                (Colour::Named(i as u8), false)
            } else if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) {
                (Colour::Named((i - NamedColor::DimBlack as usize) as u8), true)
            } else {
                (Colour::Default, n == NamedColor::DimForeground)
            }
        }
    }
}
