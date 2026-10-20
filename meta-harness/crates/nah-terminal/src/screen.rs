//! The screen as readers get it: rows of runs, what changed since a
//! reader last looked, the scrollback's lines, and searching them.

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::TermDamage;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use harness_contracts::{Colour, Cursor, Found, Row, Run, ScreenUpdate, Style, TermSize};

use crate::commands::Commands;
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

    /// Something besides the rows changed (a command): the count of that change.
    pub fn bump(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// What changed after `since` (all of it when `since` is none or from
    /// before the screen last changed whole).
    pub fn since<T: EventListener>(&self, term: &Term<T>, feed: &Feed, commands: &Commands, since: Option<u64>, title: &str) -> ScreenUpdate {
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
            // Which start of the shell it is: the server's to say.
            shell: 0,
            commands: commands.since(since.filter(|_| !full)),
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

/// Lines `from..to` as text, within what's kept: a line that wraps is
/// one line (as the program wrote it), wide characters once each.
pub fn text<T: EventListener>(term: &Term<T>, feed: &Feed, from: u64, to: u64) -> String {
    let (first, end) = kept(term, feed);
    let (from, to) = (from.max(first), to.min(end));
    if from >= to {
        return String::new();
    }
    let start = Point::new(grid_line(feed, from), Column(0));
    let last = Point::new(grid_line(feed, to - 1), Column(term.columns() - 1));
    term.bounds_to_string(start, last).trim_end().to_owned()
}

/// What a program wrote from line `from` to `to` (before it), as written
/// down as it goes: within what's kept (how many before it went already
/// is said), blank lines kept, each line's trailing spaces not; the lines
/// it ends with that wrap on into `to` left for later (written whole then).
pub fn written<T: EventListener>(term: &Term<T>, feed: &Feed, from: u64, to: u64) -> Written {
    let (first, end) = kept(term, feed);
    let from_kept = from.max(first);
    let mut to = to.min(end);
    while to > from_kept && wraps(term, grid_line(feed, to - 1)) {
        to -= 1;
    }
    if from_kept >= to {
        return Written { text: String::new(), lost: from_kept - from, next: from_kept.max(from) };
    }
    let start = Point::new(grid_line(feed, from_kept), Column(0));
    let last = Point::new(grid_line(feed, to - 1), Column(term.columns() - 1));
    let text: String = term.bounds_to_string(start, last).split('\n').map(|l| format!("{}\n", l.trim_end())).collect();
    Written { text, lost: from_kept - from, next: to }
}

/// What `written` gives: the text, how many lines went before they could
/// be (cleared, past what's kept), and the line to go on from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub text: String,
    pub lost: u64,
    pub next: u64,
}

/// Whether a row goes on in the next (it wrapped at the screen's edge).
fn wraps<T: EventListener>(term: &Term<T>, line: Line) -> bool {
    term.grid()[line][Column(term.columns() - 1)].flags.contains(Flags::WRAPLINE)
}

/// The rows from the oldest kept to the cursor's, as the lines the program
/// wrote (a row that wraps goes on in the next): what a new width keeps.
/// Counted up from the cursor's line (the bottom stays where it was).
pub struct Layout {
    /// The first row's number, the width.
    first: u64,
    cols: usize,
    /// Each row's line (counted up from the cursor's) and which of its rows it is.
    rows: Vec<(usize, usize)>,
    /// Each line's first row (by the line, counted up).
    starts: Vec<usize>,
}

impl Layout {
    pub fn of<T: EventListener>(term: &Term<T>, feed: &Feed) -> Self {
        let (first, _) = kept(term, feed);
        let cursor = feed.top() + term.grid().cursor.point.line.0.max(0) as u64;
        let mut from_top = Vec::new();
        let (mut line, mut part) = (0usize, 0usize);
        for number in first..=cursor {
            from_top.push((line, part));
            if wraps(term, grid_line(feed, number)) {
                part += 1;
            } else {
                (line, part) = (line + 1, 0);
            }
        }
        let last = from_top.last().map_or(0, |(l, _)| *l);
        let rows: Vec<(usize, usize)> = from_top.iter().map(|(l, p)| (last - l, *p)).collect();
        let mut starts = vec![0; last + 1];
        for (i, (up, part)) in rows.iter().enumerate() {
            if *part == 0 {
                starts[*up] = i;
            }
        }
        Self { first, cols: term.columns(), rows, starts }
    }

    /// The row after the line (as the program wrote it) that row `line` of
    /// `before` is on, in this.
    pub fn after(&self, before: &Layout, line: u64) -> Option<u64> {
        let (up, _) = *before.rows.get(usize::try_from(line.checked_sub(before.first)?).ok()?)?;
        let row = match up.checked_sub(1) {
            Some(next) => *self.starts.get(next)?,
            // The cursor's own line: its last row's next.
            None => self.rows.len(),
        };
        Some(self.first + row as u64)
    }

    /// Where a place in `before` (the same lines, another width) is in this
    /// (a column past the row's end: its last).
    pub fn place(&self, before: &Layout, line: u64, col: u16) -> Option<(u64, u16)> {
        let (up, part) = *before.rows.get(usize::try_from(line.checked_sub(before.first)?).ok()?)?;
        let offset = part * before.cols + usize::from(col).min(before.cols.saturating_sub(1));
        // Within the line it's in (a place past the line's end: its last row).
        let last_row = up.checked_sub(1).and_then(|next| self.starts.get(next)).map_or(self.rows.len(), |s| *s).checked_sub(1)?;
        let row = (*self.starts.get(up)? + offset / self.cols.max(1)).min(last_row);
        (row < self.rows.len()).then(|| (self.first + row as u64, (offset % self.cols.max(1)) as u16))
    }
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
