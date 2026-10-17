//! Feeding a terminal's output to the emulator, and numbering its lines.
//!
//! A line's number is how many lines went into the scrollback before it,
//! so it keeps its number for its life (the screen's first row is
//! `top`). The emulator counts nothing past its scrollback's cap (it
//! just drops the oldest line), so it's given room past what's kept here
//! and the scrollback is cut back to what's kept here, counting what goes.
//! What a clear of the scrollback takes is counted too: the output is cut
//! just before the clear's last byte, to see what was there.
//!
//! A synchronized update (a program drawing a whole frame between two
//! marks) is held here until it ends, or has waited long enough, or grew
//! too big, then fed at once: the screen is never read half drawn, and
//! the emulator never holds output of its own, which would hide clears in
//! it from the count.

use std::time::{Duration, Instant};

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Processor, Timeout};

use crate::scan::{Scan, Seen};

/// How far past what's kept the emulator's scrollback may grow before
/// it's cut back (cutting is cheapest done in batches).
const SLACK: usize = 4096;
/// The emulator's room past what's kept: output fed between two cuts
/// can't fill it (it's fed in pieces of `PIECE`, a line at most a byte),
/// so it never drops a line itself. If it ever did (a flood of scrolls),
/// every line is numbered anew.
const ROOM: usize = 65_536;
const PIECE: usize = 4096;
/// How long a synchronized update is held at most, and how much of it.
pub const SYNC_WAIT: Duration = Duration::from_millis(150);
const MOST_HELD: usize = 2 * 1024 * 1024;

/// The emulator's own holding of synchronized updates, turned off: they're
/// held here.
#[derive(Default)]
pub struct Unheld;

impl Timeout for Unheld {
    fn set_timeout(&mut self, _: Duration) {}

    fn clear_timeout(&mut self) {}

    fn pending_timeout(&self) -> bool {
        false
    }
}

pub struct Feed {
    processor: Processor<Unheld>,
    scan: Scan,
    /// Lines kept in the scrollback (at least).
    kept: usize,
    /// Lines gone off the top of the scrollback: the oldest kept line's number.
    dropped: u64,
    /// The main screen's scrollback when last seen (while the alternate
    /// screen shows, it can't be read, and nothing goes into it).
    history: usize,
    /// A synchronized update not yet fed: its output, where in it each
    /// clear of the scrollback ends, and when it started.
    held: Vec<u8>,
    clears: Vec<(usize, Seen)>,
    since: Option<Instant>,
}

impl Feed {
    pub fn new(kept: usize) -> Self {
        Self { processor: Processor::new(), scan: Scan::default(), kept, dropped: 0, history: 0, held: Vec::new(), clears: Vec::new(), since: None }
    }

    /// The emulator's scrollback for `kept` lines kept here.
    pub fn room(kept: usize) -> usize {
        kept + ROOM
    }

    /// The number of the screen's first row.
    pub fn top(&self) -> u64 {
        self.dropped + self.history as u64
    }

    /// The oldest line kept's number.
    pub fn oldest(&self) -> u64 {
        self.dropped
    }

    /// Reads `bytes` of output: fed to `term` (or held, in a synchronized
    /// update); the shell's command marks in them.
    pub fn output<T: EventListener>(&mut self, term: &mut Term<T>, bytes: &[u8]) -> Vec<Seen> {
        let mut marks = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            let (n, seen) = self.scan.next(rest);
            let (piece, after) = rest.split_at(n);
            rest = after;
            if self.since.is_some() {
                self.held.extend_from_slice(piece);
                match seen {
                    Some(clear @ (Seen::ClearedHistory | Seen::Reset)) => self.clears.push((self.held.len(), clear)),
                    Some(Seen::SyncEnd) => self.release(term),
                    _ => {}
                }
                if self.held.len() > MOST_HELD {
                    self.release(term);
                }
            } else {
                match seen {
                    Some(clear @ (Seen::ClearedHistory | Seen::Reset)) => self.feed_clear(term, piece, clear),
                    Some(Seen::SyncStart) => {
                        self.feed(term, piece);
                        self.since = Some(Instant::now());
                    }
                    _ => self.feed(term, piece),
                }
            }
            if let Some(mark @ (Seen::Started | Seen::Finished(_))) = seen {
                marks.push(mark);
            }
        }
        marks
    }

    /// How long until a held update is due (none held: none).
    pub fn due_in(&self) -> Option<Duration> {
        self.since.map(|at| SYNC_WAIT.saturating_sub(at.elapsed()))
    }

    /// Feeds a held update that's waited long enough; whether it did.
    pub fn overdue<T: EventListener>(&mut self, term: &mut Term<T>) -> bool {
        if self.due_in().is_some_and(|d| d.is_zero()) {
            self.release(term);
            return true;
        }
        false
    }

    /// After the emulator was resized (it may have moved lines between
    /// the screen and the scrollback).
    pub fn resized<T: EventListener>(&mut self, term: &mut Term<T>) {
        self.settle(term);
    }

    /// Feeds the held update, cut before each clear in it.
    fn release<T: EventListener>(&mut self, term: &mut Term<T>) {
        self.since = None;
        let held = std::mem::take(&mut self.held);
        let mut from = 0;
        for (end, clear) in std::mem::take(&mut self.clears) {
            self.feed_clear(term, &held[from..end], clear);
            from = end;
        }
        self.feed(term, &held[from..]);
        self.held = held;
        self.held.clear();
    }

    fn feed<T: EventListener>(&mut self, term: &mut Term<T>, bytes: &[u8]) {
        for piece in bytes.chunks(PIECE) {
            self.processor.advance(term, piece);
            self.settle(term);
        }
    }

    /// Feeds `bytes`, which end with `clear`'s last byte: what's in the
    /// scrollback just before it is counted as gone.
    fn feed_clear<T: EventListener>(&mut self, term: &mut Term<T>, bytes: &[u8], clear: Seen) {
        let (before, last) = bytes.split_at(bytes.len().saturating_sub(1));
        self.feed(term, before);
        let alternate = term.mode().contains(TermMode::ALT_SCREEN);
        // The alternate screen has no scrollback of its own to clear; a
        // reset clears the main screen's from either.
        let gone = match clear {
            Seen::ClearedHistory if alternate => 0,
            _ => self.history,
        };
        self.processor.advance(term, last);
        self.dropped += gone as u64;
        self.history -= gone;
        self.settle(term);
    }

    /// Cuts the scrollback back to what's kept, counting what goes.
    fn settle<T: EventListener>(&mut self, term: &mut Term<T>) {
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        let history = term.history_size();
        if history >= self.kept + ROOM {
            // The emulator may have dropped lines itself: past every
            // number given so far, so none is given twice.
            self.dropped += (self.history + term.screen_lines() + ROOM) as u64;
            self.history = 0;
        }
        if history > self.kept + SLACK {
            let grid = term.grid_mut();
            grid.update_history(self.kept);
            grid.update_history(Self::room(self.kept));
            self.dropped += (history - self.kept) as u64;
            self.history = self.kept;
        } else {
            self.history = history;
        }
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::Config;

    use super::*;

    struct Size(usize, usize);

    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            self.1
        }

        fn screen_lines(&self) -> usize {
            self.1
        }

        fn columns(&self) -> usize {
            self.0
        }
    }

    fn term(kept: usize) -> (Term<VoidListener>, Feed) {
        let config = Config { scrolling_history: Feed::room(kept), ..Config::default() };
        (Term::new(config, &Size(20, 5), VoidListener), Feed::new(kept))
    }

    /// What line `number` says now (none: not kept).
    fn line(term: &Term<VoidListener>, feed: &Feed, number: u64) -> Option<String> {
        let at = i64::try_from(number).ok()? - i64::try_from(feed.top()).ok()?;
        let lowest = -(term.history_size() as i64);
        (lowest..term.screen_lines() as i64).contains(&at).then(|| {
            let row = &term.grid()[Line(at as i32)];
            (0..term.columns()).map(|c| row[Column(c)].c).collect::<String>().trim_end().to_owned()
        })
    }

    /// Prints lines `0..n` (`line 0`, `line 1`, ...).
    fn print(feed: &mut Feed, term: &mut Term<VoidListener>, from: usize, to: usize) {
        let text: String = (from..to).map(|i| format!("line {i}\r\n")).collect();
        feed.output(term, text.as_bytes());
    }

    #[test]
    fn a_line_keeps_its_number_past_the_scrollbacks_cap() {
        let (mut term, mut feed) = term(100);
        // Line i is number i; past what's kept, in one read and in many.
        print(&mut feed, &mut term, 0, 30_000);
        for i in 0..30_000 {
            print(&mut feed, &mut term, 30_000 + i, 30_001 + i);
        }
        assert_eq!(line(&term, &feed, 59_999).as_deref(), Some("line 59999"));
        assert_eq!(line(&term, &feed, 59_950).as_deref(), Some("line 59950"));
        assert_eq!(feed.top(), 59_996, "the screen: the last four lines, then the empty row the cursor is on");
        assert!(feed.oldest() <= 59_900, "at least what's kept stays");
        assert_eq!(line(&term, &feed, feed.oldest()), Some(format!("line {}", feed.oldest())));
        assert_eq!(line(&term, &feed, feed.oldest() - 1), None, "what went is gone");
    }

    #[test]
    fn clearing_the_scrollback_keeps_the_numbers_of_whats_left() {
        let (mut term, mut feed) = term(100);
        print(&mut feed, &mut term, 0, 50);
        // `clear` in bash: home, clear the screen (it goes into the
        // scrollback), clear the scrollback.
        feed.output(&mut term, b"\x1b[H\x1b[2J\x1b[3Jline 50\r\n");
        assert_eq!(feed.oldest(), feed.top(), "nothing above the screen");
        assert_eq!(line(&term, &feed, feed.top()).as_deref(), Some("line 50"));
        assert!(feed.top() >= 50, "no number given twice");
        // Lines after it count on from there, and a clear split across reads too.
        let top = feed.top();
        print(&mut feed, &mut term, 51, 60);
        feed.output(&mut term, b"\x1b[");
        feed.output(&mut term, b"3J");
        assert_eq!(feed.oldest(), top + 6, "the six lines pushed up went with the clear");
        assert_eq!(feed.oldest(), feed.top());
        assert_eq!(line(&term, &feed, top + 6).as_deref(), Some("line 56"));
    }

    #[test]
    fn a_synchronized_update_is_held_until_it_ends_and_its_clears_counted() {
        let (mut term, mut feed) = term(100);
        print(&mut feed, &mut term, 0, 20);
        feed.output(&mut term, b"\x1b[?2026h\x1b[H\x1b[2J\x1b[3Jframe\r\n");
        assert_eq!(line(&term, &feed, 16).as_deref(), Some("line 16"), "nothing of it shows yet");
        assert!(feed.due_in().is_some());
        feed.output(&mut term, b"more\x1b[?2026l");
        assert!(feed.due_in().is_none());
        assert_eq!(line(&term, &feed, feed.top()).as_deref(), Some("frame"));
        assert_eq!(feed.oldest(), feed.top(), "the clear in it was counted");
        assert!(feed.top() >= 20);
    }

    #[test]
    fn a_synchronized_update_that_never_ends_is_fed_once_its_wait_is_over() {
        let (mut term, mut feed) = term(100);
        feed.output(&mut term, b"\x1b[?2026hhalf a frame");
        assert!(!feed.overdue(&mut term));
        std::thread::sleep(SYNC_WAIT);
        assert!(feed.overdue(&mut term));
        assert_eq!(line(&term, &feed, 0).as_deref(), Some("half a frame"));
    }

    #[test]
    fn the_alternate_screen_leaves_the_numbering_alone() {
        let (mut term, mut feed) = term(100);
        print(&mut feed, &mut term, 0, 20);
        let top = feed.top();
        feed.output(&mut term, b"\x1b[?1049h\x1b[3J");
        print(&mut feed, &mut term, 100, 140);
        assert_eq!(feed.top(), top, "nothing went into the scrollback, and no clear of it");
        feed.output(&mut term, b"\x1b[?1049l");
        assert_eq!(line(&term, &feed, 19).as_deref(), Some("line 19"));
        // A reset from the alternate screen clears the main one's scrollback.
        feed.output(&mut term, b"\x1b[?1049h\x1bc");
        assert_eq!(feed.oldest(), feed.top());
        assert!(feed.top() >= top);
    }
}
