//! A terminal's lines as the cockpit has them: the screen as last sent,
//! and lines of the scrollback fetched by their number (a line keeps its
//! number for its life: `ScreenUpdate::top`). A line in the scrollback
//! never changes, so one fetched is kept until it's gone from the
//! server's (cleared, or past what's kept) or the lines are laid out
//! again (a new size, a new shell). Rows that scroll off the screen are
//! kept as they last showed until the server's own come (it sends them
//! as they go), so no line is ever drawn blank. No drawing here.

use std::collections::BTreeMap;
use std::ops::Range;

use harness_contracts::{Cursor, Row, ScreenUpdate, TermSize};

/// A line of the scrollback: the server's, or (until the server's
/// comes) as it last showed on the screen.
struct Back {
    row: Row,
    sure: bool,
}

#[derive(Default)]
pub(super) struct Lines {
    /// The screen's rows, its first numbered `top`.
    rows: Vec<Row>,
    size: TermSize,
    cursor: Cursor,
    top: u64,
    oldest: u64,
    alternate: bool,
    title: String,
    /// Lines of the scrollback, by number.
    back: BTreeMap<u64, Back>,
    /// Lines asked for that haven't come yet.
    asked: Option<Range<u64>>,
}

impl Lines {
    /// What the server sent: the screen, or what changed on it.
    pub(super) fn take(&mut self, update: ScreenUpdate) {
        // Laid out again (a new size reflows the scrollback), or a new
        // shell (numbered from the start again): what was fetched is stale.
        if update.size != self.size || update.top < self.top || update.oldest < self.oldest {
            self.forget();
        } else if !self.alternate && !update.alternate {
            // Scrolled off the screen: kept as they showed for now; the
            // rest move up with the screen.
            let (gone, len) = ((update.top - self.top).min(self.rows.len() as u64) as usize, self.rows.len());
            for (number, row) in (self.top..).zip(self.rows.drain(..gone)) {
                self.back.entry(number).or_insert(Back { row, sure: false });
            }
            self.rows.resize(len, Row::new());
        }
        if update.full || self.rows.len() != usize::from(update.size.rows) {
            self.rows = vec![Row::new(); usize::from(update.size.rows)];
        }
        for (row, runs) in update.rows {
            if let Some(r) = self.rows.get_mut(usize::from(row)) {
                *r = runs;
            }
        }
        // Gone from the server's scrollback.
        self.back = self.back.split_off(&update.oldest);
        (self.size, self.cursor, self.top, self.oldest, self.alternate, self.title) = (update.size, update.cursor, update.top, update.oldest, update.alternate, update.title);
    }

    /// Lines of its scrollback the server sent, from the one numbered
    /// `from` (asked for, or as they scrolled off: then before the update
    /// that moves the screen past them).
    pub(super) fn arrived(&mut self, from: u64, rows: Vec<Row>) {
        let end = from + rows.len() as u64;
        for (number, row) in (from..).zip(rows).filter(|(n, _)| *n >= self.oldest) {
            self.back.insert(number, Back { row, sure: true });
        }
        if self.asked.as_ref().is_some_and(|asked| from <= asked.start && asked.end <= end) {
            self.asked = None;
        }
    }

    /// Everything fetched forgotten (a new socket starts from the whole screen).
    pub(super) fn forget(&mut self) {
        self.back.clear();
        self.asked = None;
    }

    /// The first line there is (the scrollback's oldest; the screen's
    /// first while a program has the whole screen: it has no scrollback).
    pub(super) fn first(&self) -> u64 {
        if self.alternate { self.top } else { self.oldest }
    }

    /// One past the last line (the screen's last).
    pub(super) fn end(&self) -> u64 {
        self.top + self.rows.len() as u64
    }

    /// Line `number`: the screen's, or the scrollback's (none until fetched).
    pub(super) fn get(&self, number: u64) -> Option<&Row> {
        match number.checked_sub(self.top) {
            Some(row) => self.rows.get(row as usize),
            None if self.alternate => None,
            None => self.back.get(&number).map(|b| &b.row),
        }
    }

    /// Line `number`'s text.
    pub(super) fn text(&self, number: u64) -> Option<String> {
        self.get(number).map(|r| r.iter().map(|run| run.text.as_str()).collect())
    }

    /// Whether every line of `range` there is, is here.
    pub(super) fn has(&self, range: Range<u64>) -> bool {
        range.filter(|n| (self.first()..self.end()).contains(n)).all(|n| self.get(n).is_some())
    }

    /// What to ask the server for, for `range` to be here: the lines
    /// missing with a screenful more either side (none when they're all
    /// here, or already asked for).
    pub(super) fn wanted(&mut self, range: Range<u64>) -> Option<(u64, u32)> {
        if self.alternate {
            return None;
        }
        let mut missing = range.filter(|n| (self.oldest..self.top).contains(n) && !self.back.get(n).is_some_and(|b| b.sure));
        let first = missing.next()?;
        let last = missing.next_back().unwrap_or(first);
        if self.asked.as_ref().is_some_and(|asked| asked.contains(&first) && asked.contains(&last)) {
            return None;
        }
        let more = self.rows.len().max(10) as u64;
        let (from, to) = (first.saturating_sub(more).max(self.oldest), (last + 1 + more).min(self.top));
        self.asked = Some(from..to);
        Some((from, (to - from) as u32))
    }

    /// The screen's cursor: its line's number, its column, whether it shows.
    pub(super) fn cursor(&self) -> (u64, u16, bool) {
        (self.top + u64::from(self.cursor.row), self.cursor.col, self.cursor.visible)
    }

    pub(super) fn alternate(&self) -> bool {
        self.alternate
    }
}

#[cfg(test)]
mod tests {
    use harness_contracts::{Run, Style};

    use super::*;

    fn row(text: &str) -> Row {
        vec![Run { text: text.into(), style: Style::default(), wide: false }]
    }

    /// A 3-row screen of `lines` from line `top`, the scrollback from `oldest`.
    fn update(lines: [&str; 3], top: u64, oldest: u64) -> ScreenUpdate {
        ScreenUpdate { seq: 1, size: TermSize { cols: 80, rows: 3 }, full: true, rows: (0..3).map(|r| (r, row(lines[usize::from(r)]))).collect(), cursor: Cursor::default(), top, oldest, alternate: false, title: String::new() }
    }

    #[test]
    fn scrollback_fetched_is_kept_by_number_until_its_gone_from_the_server() {
        let mut lines = Lines::default();
        lines.take(update(["a", "b", "c"], 100, 0));
        assert_eq!(lines.wanted(95..100), Some((85, 15)), "missing, with a screenful more either side (10 at least)");
        assert_eq!(lines.wanted(96..99), None, "asked already");
        lines.arrived(85, (85..103).map(|n| row(&n.to_string())).collect());
        assert_eq!(lines.text(90).as_deref(), Some("90"));
        assert_eq!(lines.text(100).as_deref(), Some("a"), "the screen's own, not what was fetched");
        // Output: the screen moves on, the scrollback stays.
        lines.take(update(["d", "e", "f"], 120, 0));
        assert_eq!(lines.text(90).as_deref(), Some("90"));
        // Past what's kept: what went is gone.
        lines.take(update(["g", "h", "i"], 130, 91));
        assert_eq!((lines.text(90), lines.text(91).as_deref()), (None, Some("91")));
        // A new size: laid out again.
        let mut wider = update(["g", "h", "i"], 130, 91);
        wider.size.cols = 100;
        lines.take(wider);
        assert_eq!(lines.text(91), None);
    }

    #[test]
    fn rows_scrolled_off_show_as_they_were_until_the_servers_come() {
        let mut lines = Lines::default();
        lines.take(update(["a", "b", "c"], 100, 0));
        lines.take(update(["c", "d", "e"], 102, 0));
        assert_eq!((lines.text(100).as_deref(), lines.text(101).as_deref()), (Some("a"), Some("b")), "as they showed");
        assert_eq!(lines.wanted(100..102), Some((90, 12)), "still asked for: they may have changed before they went");
        lines.arrived(100, vec![row("a!"), row("b")]);
        assert_eq!(lines.text(100).as_deref(), Some("a!"), "the server's");
        assert_eq!(lines.wanted(100..102), None);
    }

    #[test]
    fn a_new_shell_numbers_from_the_start_again_and_whats_fetched_goes() {
        let mut lines = Lines::default();
        lines.take(update(["a", "b", "c"], 100, 0));
        lines.arrived(50, vec![row("old")]);
        lines.take(update(["$", "", ""], 0, 0));
        lines.take(update(["$", "", ""], 60, 0));
        assert_eq!(lines.text(50), None, "line 50 of the new shell isn't the old one's");
    }
}
