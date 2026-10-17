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
/// it's cut back: under the emulator's spare rows (1000), so the rows a
/// cut frees are used again rather than freed and made anew.
const SLACK: usize = 512;
/// The emulator's room past what's kept: output fed between two cuts
/// can't fill it (it's fed in pieces short enough), so it never drops a
/// line itself. If it ever did, every line is numbered anew.
const ROOM: usize = 65_536;
/// The longest piece fed at once: a byte scrolls a quarter of the
/// screen's rows at most (`CSI n S` takes four bytes or more for `n`
/// rows), so a piece can't scroll past `ROOM`.
fn piece_len(rows: usize) -> usize {
    (4 * ROOM / rows.max(1)).clamp(64, 4096)
}
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
    /// thing seen ends (it's cut there when fed), and when it started.
    held: Vec<u8>,
    cuts: Vec<(usize, Seen)>,
    since: Option<Instant>,
}

impl Feed {
    pub fn new(kept: usize) -> Self {
        Self { processor: Processor::new(), scan: Scan::default(), kept, dropped: 0, history: 0, held: Vec::new(), cuts: Vec::new(), since: None }
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

    /// Reads `bytes` of output: fed to `term`, or held while a program
    /// draws a synchronized update; the shell's command marks fed with it.
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
                    Some(Seen::SyncEnd) => marks.extend(self.release(term)),
                    Some(seen) => self.cuts.push((self.held.len(), seen)),
                    None => {}
                }
                if self.held.len() > MOST_HELD {
                    marks.extend(self.release(term));
                }
            } else {
                match seen {
                    Some(Seen::SyncStart) => {
                        self.feed_to(term, piece, Seen::SyncStart);
                        self.since = Some(Instant::now());
                    }
                    Some(seen) => marks.extend(self.feed_to(term, piece, seen)),
                    None => self.feed(term, piece),
                }
            }
        }
        marks
    }

    /// How long until a held update is due (none held: none).
    pub fn due_in(&self) -> Option<Duration> {
        self.since.map(|at| SYNC_WAIT.saturating_sub(at.elapsed()))
    }

    /// Feeds a held update that's waited long enough (or, `now`, any held
    /// update: the shell ended): the marks in it.
    pub fn overdue<T: EventListener>(&mut self, term: &mut Term<T>, now: bool) -> Vec<Seen> {
        match self.due_in() {
            Some(wait) if now || wait.is_zero() => self.release(term),
            _ => Vec::new(),
        }
    }

    /// After the emulator was resized: lines may have moved between the
    /// screen and the scrollback (counted as ever). A new width lays every
    /// line out again: from then on lines are numbered past every number
    /// given (`before`: the end of the lines before), none given twice.
    pub fn resized<T: EventListener>(&mut self, term: &mut Term<T>, before: u64, relaid: bool) {
        self.settle(term);
        if relaid {
            self.dropped = self.dropped.max(before);
        }
    }

    /// Feeds the held update, cut where something was seen in it.
    fn release<T: EventListener>(&mut self, term: &mut Term<T>) -> Vec<Seen> {
        self.since = None;
        let held = std::mem::take(&mut self.held);
        let mut marks = Vec::new();
        let mut from = 0;
        for (end, seen) in std::mem::take(&mut self.cuts) {
            marks.extend(self.feed_to(term, &held[from..end], seen));
            from = end;
        }
        self.feed(term, &held[from..]);
        // Its room kept for the next, unless it was a big one.
        if held.capacity() <= 64 * 1024 {
            self.held = held;
            self.held.clear();
        }
        marks
    }

    fn feed<T: EventListener>(&mut self, term: &mut Term<T>, bytes: &[u8]) {
        for piece in bytes.chunks(piece_len(term.screen_lines())) {
            self.processor.advance(term, piece);
            self.settle(term);
        }
    }

    /// Feeds `bytes`, which end with what was `seen`: a clear of the
    /// scrollback or a reset count what they take (looked at just before
    /// their last byte), the alternate screen is entered with the main
    /// screen's scrollback counted. A command mark is given back.
    fn feed_to<T: EventListener>(&mut self, term: &mut Term<T>, bytes: &[u8], seen: Seen) -> Option<Seen> {
        let (before, last) = bytes.split_at(bytes.len().saturating_sub(1));
        self.feed(term, before);
        let alternate = term.mode().contains(TermMode::ALT_SCREEN);
        let gone = match seen {
            // The alternate screen has no scrollback of its own to clear.
            Seen::ClearedHistory if alternate => 0,
            Seen::ClearedHistory => self.history,
            // A reset clears the main screen and its scrollback, from either.
            Seen::Reset => self.history + term.screen_lines(),
            _ => 0,
        };
        self.processor.advance(term, last);
        self.dropped += gone as u64;
        self.history = self.history.saturating_sub(gone);
        self.settle(term);
        matches!(seen, Seen::Started | Seen::Finished(_)).then_some(seen)
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
        feed.overdue(&mut term, false);
        assert!(feed.due_in().is_some(), "not yet");
        std::thread::sleep(SYNC_WAIT);
        feed.overdue(&mut term, false);
        assert!(feed.due_in().is_none());
        assert_eq!(line(&term, &feed, 0).as_deref(), Some("half a frame"));
    }

    #[test]
    fn a_new_width_lays_lines_out_again_numbered_past_every_number_given() {
        let (mut term, mut feed) = term(100);
        feed.output(&mut term, format!("{}\r\n", "x".repeat(50)).as_bytes());
        print(&mut feed, &mut term, 0, 30);
        for cols in [10, 40, 20] {
            let end = feed.top() + term.screen_lines() as u64;
            term.resize(Size(cols, 5));
            feed.resized(&mut term, end, true);
            assert!(feed.oldest() >= end, "{cols} wide: none of the numbers before given again");
            assert_eq!(line(&term, &feed, feed.top() - 1).as_deref(), Some("line 25"), "the lines are there, laid out again (the screen: 26 to 29 and the cursor's row)");
        }
        // Only taller: lines move between the screen and the scrollback, each keeping its number.
        let numbered = |feed: &Feed, term: &Term<VoidListener>| (feed.oldest()..feed.top() + term.screen_lines() as u64).filter_map(|n| line(term, feed, n).map(|l| (n, l))).filter(|(_, l)| l.starts_with("line")).collect::<Vec<_>>();
        let before = numbered(&feed, &term);
        let end = feed.top() + term.screen_lines() as u64;
        term.resize(Size(20, 9));
        feed.resized(&mut term, end, false);
        assert_eq!(numbered(&feed, &term), before);
    }

    #[test]
    fn a_reset_numbers_the_new_screen_past_the_old_one() {
        let (mut term, mut feed) = term(100);
        print(&mut feed, &mut term, 0, 20);
        let end = feed.top() + 5;
        feed.output(&mut term, b"\x1bcprompt");
        assert_eq!((feed.oldest(), feed.top()), (end, end), "the old screen's numbers aren't the new one's");
        assert_eq!(line(&term, &feed, end).as_deref(), Some("prompt"));
        // Entered the alternate screen in the same read as more lines: they're counted before it.
        print(&mut feed, &mut term, 0, 10);
        let lines: String = (0..10).map(|i| format!("more {i}\r\n")).collect();
        feed.output(&mut term, format!("{lines}\x1b[?1049h\x1bc").as_bytes());
        assert!(feed.oldest() >= end + 20, "every line printed before the reset is past: {}", feed.oldest());
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
