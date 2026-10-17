//! A terminal's lines as the cockpit has them: the screen as last sent,
//! and lines of the scrollback by their number (a line keeps its number
//! for its life: `ScreenUpdate::top`). A line in the scrollback never
//! changes: rows that scroll off the screen are kept as they showed (the
//! server sends again any that changed before they went), lines further
//! back are asked for, one ask at a time. They're kept until they're gone
//! from the server's (cleared, or past what's kept) or laid out anew (a new
//! width, a new start of the shell). No drawing here.

use std::collections::BTreeMap;
use std::ops::Range;

use harness_contracts::{Cursor, ReaderScreen, Row, ScreenUpdate, TermSize, TerminalCommand};

/// The most lines asked for at once (the server's own limit).
const MOST_ASKED: u64 = 1000;

/// What an update did to the lines besides changing them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Took {
    /// The same lines (the screen may have moved on).
    Same,
    /// A new width laid every line out anew: they're numbered past the
    /// old ones; `by` is how far the end moved.
    Relaid { by: i64 },
    /// A new start of the shell: other lines altogether.
    Restarted,
}

#[derive(Default)]
pub(super) struct Lines {
    screen: ReaderScreen,
    size: TermSize,
    cursor: Cursor,
    oldest: u64,
    alternate: bool,
    title: String,
    shell: u64,
    /// Lines of the scrollback, by number.
    back: BTreeMap<u64, Row>,
    /// Lines asked for whose answer hasn't come.
    asked: Option<Range<u64>>,
    /// Lines asked for that the answer didn't have: not asked again until
    /// the next update.
    refused: Option<Range<u64>>,
    /// The commands the shell ran, in order (by number: by their lines too).
    commands: Vec<TerminalCommand>,
}

impl Lines {
    /// What the server sent: the screen, or what changed on it.
    pub(super) fn take(&mut self, update: &ScreenUpdate) -> Took {
        let end = self.end();
        let took = if self.shell != 0 && update.shell != self.shell {
            Took::Restarted
        } else if self.size.cols != 0 && update.size.cols != self.size.cols {
            Took::Relaid { by: (update.top + u64::from(update.size.rows)) as i64 - end as i64 }
        } else {
            Took::Same
        };
        if took != Took::Same {
            self.forget();
        }
        let (from, gone) = self.screen.take(update);
        for (number, row) in (from..).zip(gone) {
            self.back.entry(number).or_insert(row);
        }
        self.back = self.back.split_off(&update.oldest);
        self.refused = None;
        for command in &update.commands {
            match self.commands.binary_search_by_key(&command.id, |c| c.id) {
                Ok(i) => self.commands[i].clone_from(command),
                Err(i) => self.commands.insert(i, command.clone()),
            }
        }
        // A finished one goes with its lines (as the server's do).
        self.commands.retain(|c| c.end.is_none_or(|end| end > update.oldest));
        (self.size, self.cursor, self.oldest, self.alternate, self.shell) = (update.size, update.cursor, update.oldest, update.alternate, update.shell);
        self.title.clone_from(&update.title);
        took
    }

    /// The lines asked for, from the one numbered `from` (a short answer:
    /// the rest aren't there).
    pub(super) fn arrived(&mut self, from: u64, rows: Vec<Row>) {
        let end = from + rows.len() as u64;
        for (number, row) in (from..).zip(rows).filter(|(n, _)| *n >= self.oldest) {
            self.back.insert(number, row);
        }
        self.refused = self.asked.take().filter(|asked| asked.start < from || end < asked.end);
    }

    /// Rows that scrolled off and changed before they went: as they are.
    pub(super) fn scrolled_off(&mut self, lines: Vec<(u64, Row)>) {
        self.back.extend(lines.into_iter().filter(|(n, _)| *n >= self.oldest));
    }

    /// Everything of the scrollback forgotten.
    fn forget(&mut self) {
        self.back.clear();
        self.commands.clear();
        self.unasked();
    }

    /// What was asked for won't come (its socket went).
    pub(super) fn unasked(&mut self) {
        (self.asked, self.refused) = (None, None);
    }

    /// The first line there is (the scrollback's oldest; the screen's
    /// first while a program has the whole screen: it has no scrollback).
    pub(super) fn first(&self) -> u64 {
        if self.alternate { self.screen.top() } else { self.oldest }
    }

    /// One past the last line (the screen's last).
    pub(super) fn end(&self) -> u64 {
        self.screen.top() + self.screen.rows().len() as u64
    }

    /// Line `number`: the screen's, or the scrollback's.
    pub(super) fn get(&self, number: u64) -> Option<&Row> {
        match self.screen.get(number) {
            Some(row) => Some(row),
            None if self.alternate || number >= self.screen.top() => None,
            None => self.back.get(&number),
        }
    }

    /// Line `number`'s text.
    pub(super) fn text(&self, number: u64) -> Option<String> {
        self.get(number).map(|r| r.iter().map(|run| run.text.as_str()).collect())
    }

    /// The screen's lines: their numbers and text.
    pub(super) fn screen_texts(&self) -> impl Iterator<Item = (u64, String)> + '_ {
        (self.screen.top()..).zip(self.screen.rows().iter().map(|r| r.iter().map(|run| run.text.as_str()).collect()))
    }

    /// Whether every line of `range` there is, is here.
    pub(super) fn has(&self, range: Range<u64>) -> bool {
        range.filter(|n| (self.first()..self.end()).contains(n)).all(|n| self.get(n).is_some())
    }

    /// What to ask the server for, for `range` to be here: the lines
    /// missing with a screenful more either side (none when they're all
    /// here, or an ask is out: one at a time).
    pub(super) fn wanted(&mut self, range: Range<u64>) -> Option<(u64, u32)> {
        if self.alternate || self.asked.is_some() {
            return None;
        }
        let top = self.screen.top();
        let refused = self.refused.clone().unwrap_or_default();
        let mut missing = range.filter(|n| (self.oldest..top).contains(n) && !self.back.contains_key(n) && !refused.contains(n));
        let first = missing.next()?;
        let last = missing.next_back().unwrap_or(first);
        let more = self.screen.rows().len().max(10) as u64;
        let from = first.saturating_sub(more).max(self.oldest);
        let to = (last + 1 + more).min(top).min(from + MOST_ASKED);
        self.asked = Some(from..to);
        Some((from, (to - from) as u32))
    }

    /// The command line `number` is in (its prompt, what was typed, its output).
    pub(super) fn command_at(&self, number: u64) -> Option<&TerminalCommand> {
        let after = self.commands.partition_point(|c| c.prompt <= number);
        self.commands.get(after.checked_sub(1)?).filter(|c| c.end.is_none_or(|end| number < end))
    }

    /// The prompt's line of the next command after line `number` (back:
    /// the one before), within the lines there are.
    pub(super) fn command_beside(&self, number: u64, back: bool) -> Option<u64> {
        let at = self.command_at(number).map_or(number, |c| c.prompt);
        let prompts = self.commands.iter().map(|c| c.prompt).filter(|p| (self.first()..self.end()).contains(p));
        if back { prompts.filter(|p| *p < at).next_back() } else { prompts.into_iter().find(|p| *p > at) }
    }

    /// The command waiting at an empty prompt (nothing running, nothing
    /// typed yet): where a command can be typed in whole.
    pub(super) fn at_empty_prompt(&self) -> Option<&TerminalCommand> {
        let (line, col, _) = self.cursor();
        self.commands.last().filter(|c| !self.alternate && c.output.is_none() && (c.input, c.input_col) == (line, col))
    }

    /// The screen's cursor: its line's number, its column, whether it shows.
    pub(super) fn cursor(&self) -> (u64, u16, bool) {
        (self.screen.top() + u64::from(self.cursor.row), self.cursor.col, self.cursor.visible)
    }

    pub(super) fn alternate(&self) -> bool {
        self.alternate
    }

    /// Which start of its shell these lines are.
    pub(super) fn shell(&self) -> u64 {
        self.shell
    }

    /// Command `id` (in this start of the shell).
    pub(super) fn command(&self, id: u64) -> Option<&TerminalCommand> {
        self.commands.binary_search_by_key(&id, |c| c.id).ok().map(|i| &self.commands[i])
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
        ScreenUpdate { seq: 1, size: TermSize { cols: 80, rows: 3 }, full: false, rows: (0..3).map(|r| (r, row(lines[usize::from(r)]))).collect(), cursor: Cursor::default(), top, oldest, alternate: false, title: String::new(), shell: 7, commands: Vec::new() }
    }

    #[test]
    fn scrollback_is_kept_by_number_asked_for_one_ask_at_a_time_until_its_gone_from_the_server() {
        let mut lines = Lines::default();
        lines.take(&update(["a", "b", "c"], 100, 0));
        assert_eq!(lines.wanted(95..100), Some((85, 15)), "missing, with a screenful more either side (10 at least)");
        assert_eq!(lines.wanted(50..60), None, "one ask at a time");
        lines.arrived(85, (85..100).map(|n| row(&n.to_string())).collect());
        assert_eq!(lines.text(90).as_deref(), Some("90"));
        // Output: what scrolled off is kept as it showed; the server's own when it changed.
        lines.scrolled_off(vec![(101, row("b!"))]);
        lines.take(&update(["d", "e", "f"], 103, 0));
        assert_eq!([100, 101, 102].map(|n| lines.text(n)), [Some("a".into()), Some("b!".into()), Some("c".into())]);
        assert_eq!(lines.wanted(85..103), None, "all here");
        // Past what's kept: what went is gone.
        lines.take(&update(["g", "h", "i"], 130, 91));
        assert_eq!((lines.text(90), lines.text(91).as_deref()), (None, Some("91")));
        // Taller: the same lines.
        let mut taller = update(["g", "h", "i"], 130, 91);
        taller.size.rows = 5;
        assert_eq!(lines.take(&taller), Took::Same);
        assert_eq!(lines.text(91).as_deref(), Some("91"));
        // Wider: laid out anew.
        let mut wider = update(["g", "h", "i"], 140, 135);
        wider.size.cols = 100;
        assert_eq!(lines.take(&wider), Took::Relaid { by: 8 });
        assert_eq!(lines.text(91), None);
    }

    #[test]
    fn commands_are_found_by_their_lines_and_go_with_them() {
        let mut lines = Lines::default();
        let command = |id, prompt, end: Option<u64>| TerminalCommand { id, prompt, input: prompt, input_col: 2, output: Some(prompt + 1), end, text: format!("c{id}"), code: Some(0), ran_ms: Some(1) };
        let mut first = update(["a", "b", "c"], 100, 0);
        first.commands = vec![command(1, 10, Some(14)), command(2, 14, Some(20)), command(3, 20, None)];
        // (Lines 10 on are here.)
        first.oldest = 5;
        lines.take(&first);
        assert_eq!(lines.command_at(12).map(|c| c.id), Some(1));
        assert_eq!(lines.command_at(14).map(|c| c.id), Some(2));
        assert_eq!(lines.command_at(5).map(|c| c.id), None, "before any");
        assert_eq!(lines.command_beside(12, false), Some(14));
        assert_eq!(lines.command_beside(12, true), None);
        assert_eq!(lines.command_beside(21, true), Some(14), "from the running one, the one before it");
        // The third ended; the first's lines went.
        let mut next = update(["a", "b", "c"], 100, 14);
        next.commands = vec![command(3, 20, Some(25))];
        lines.take(&next);
        assert_eq!(lines.command_at(13).map(|c| c.id), None, "its lines went, so did it");
        assert_eq!(lines.command_at(30).map(|c| c.id), None, "past its end");
        assert_eq!(lines.command_at(15).map(|c| c.id), Some(2), "the second's last line is still here");
    }

    #[test]
    fn a_short_answer_isnt_asked_again_until_the_next_update() {
        let mut lines = Lines::default();
        lines.take(&update(["a", "b", "c"], 100, 0));
        assert_eq!(lines.wanted(95..100), Some((85, 15)));
        lines.arrived(85, Vec::new());
        assert_eq!(lines.wanted(95..100), None, "the server hadn't them");
        lines.take(&update(["a", "b", "c"], 100, 0));
        assert_eq!(lines.wanted(95..100), Some((85, 15)), "asked again after an update");
    }

    #[test]
    fn a_new_start_of_the_shell_is_other_lines() {
        let mut lines = Lines::default();
        lines.take(&update(["a", "b", "c"], 100, 0));
        lines.arrived(50, vec![row("old")]);
        let mut again = update(["$", "", ""], 120, 0);
        again.shell = 8;
        assert_eq!(lines.take(&again), Took::Restarted);
        assert_eq!(lines.text(50), None, "line 50 of the new start isn't the old one's");
    }
}
