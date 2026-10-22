//! Terminals: shells a project server runs for the user. The shared event
//! stream says which exist and how they are (`Terminal`); each one's screen
//! and the keys typed into it go on its own socket (`ToTerminal`,
//! `FromTerminal`), never on the shared stream.
//!
//! A cell's colour is sent as the program asked for it (one of the 16 named
//! ones, an index into the 256, or RGB): the client's theme decides what
//! the named and indexed ones look like.

use serde::{Deserialize, Serialize};

use crate::ids::TerminalId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Terminal {
    pub id: TerminalId,
    /// What the shell (or the program in it) calls itself.
    pub title: String,
    pub status: TerminalStatus,
    pub size: TermSize,
    /// The shell run, and the folder it started in.
    pub shell: String,
    pub folder: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum TerminalStatus {
    Running,
    /// The shell exited (its code, when it had one), or the server
    /// restarted under it.
    Ended { code: Option<i32> },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

/// A colour as the program asked for it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Colour {
    /// The terminal's own foreground or background.
    #[default]
    Default,
    /// One of the 16 named colours (0–7 normal, 8–15 bright).
    Named(u8),
    /// One of the 256 (16–255 past the named ones).
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// How a run of text is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    #[serde(default, skip_serializing_if = "is_default_colour")]
    pub fg: Colour,
    #[serde(default, skip_serializing_if = "is_default_colour")]
    pub bg: Colour,
    /// `BOLD`, `ITALIC`, ... (bits).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attrs: u8,
}

impl Style {
    pub const BOLD: u8 = 1;
    pub const ITALIC: u8 = 1 << 1;
    pub const UNDERLINE: u8 = 1 << 2;
    /// Foreground and background swapped.
    pub const INVERSE: u8 = 1 << 3;
    pub const DIM: u8 = 1 << 4;
    pub const STRIKE: u8 = 1 << 5;
    pub const HIDDEN: u8 = 1 << 6;

    pub fn has(&self, attr: u8) -> bool {
        self.attrs & attr != 0
    }
}

fn is_default_colour(c: &Colour) -> bool {
    *c == Colour::Default
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

/// Cells side by side in one style: each char one column, or two when the
/// run is `wide` (one char then).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    #[serde(default, skip_serializing_if = "is_plain")]
    pub style: Style,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wide: bool,
}

fn is_plain(s: &Style) -> bool {
    *s == Style::default()
}

/// One line of the screen or the scrollback, its runs left to right
/// (trailing blanks left out).
pub type Row = Vec<Run>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    /// Its row on the screen and its column.
    pub row: u16,
    pub col: u16,
    pub visible: bool,
}

/// What changed on a screen since the reader's last update (the whole
/// screen when `full`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenUpdate {
    /// Grows with every change; a reader asks for what's after the last it saw.
    pub seq: u64,
    pub size: TermSize,
    pub full: bool,
    /// Changed rows: their number on the screen, and what they show now.
    pub rows: Vec<(u16, Row)>,
    pub cursor: Cursor,
    /// The number of the screen's first row. Lines are numbered by how
    /// many went into the scrollback before them, so a line keeps its
    /// number for its life (the row below it is `top + 1`, and so on).
    pub top: u64,
    /// The oldest line kept: lines from it up to `top` are the scrollback,
    /// to ask for. Below it they're gone (past what's kept, or cleared).
    pub oldest: u64,
    /// The program shows its own full screen (an editor, a pager): there's
    /// no scrollback to go through while it does, and its rows aren't
    /// numbered lines (they're gone when it's done).
    pub alternate: bool,
    pub title: String,
    /// Which start of its shell this is: a new start numbers its lines
    /// from the beginning again (they're other lines).
    #[serde(default)]
    pub shell: u64,
    /// Commands that are new or changed (all of them when `full`); a
    /// finished one is gone once its lines are (its `end` at or below
    /// `oldest`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<TerminalCommand>,
}

/// A command the shell ran, as its marks say where (bash's and zsh's,
/// set up when they start): kept while any of its lines is (a running one
/// always).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCommand {
    /// Its number in this start of the shell (a new start counts from 1 again).
    pub id: u64,
    /// The line its prompt starts on, and where what's typed starts.
    pub prompt: u64,
    pub input: u64,
    pub input_col: u16,
    /// The line its output starts on (none: still being typed), and the
    /// line after its last (none: still running).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
    /// The command line, as the shell said it (empty: it didn't say, as
    /// bash doesn't for a command its history doesn't keep).
    pub text: String,
    /// How it ended (the shell said no code: none), and how long it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ran_ms: Option<u64>,
}

/// A screen as a reader has it, from the updates it was sent: the one
/// way updates are applied, by the cockpit and by the server's own copy
/// of what each reader has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReaderScreen {
    rows: Vec<Row>,
    size: TermSize,
    top: u64,
    alternate: bool,
}

impl ReaderScreen {
    /// `update` applied. What scrolled off the top as the reader had it:
    /// the first one's number, and the rows (none when the screen was
    /// laid out anew or a program has the whole screen).
    pub fn take(&mut self, update: &ScreenUpdate) -> (u64, Vec<Row>) {
        let rows = usize::from(update.size.rows);
        let mut gone = Vec::new();
        let from = self.top;
        if update.full || update.size != self.size || update.top < self.top || self.alternate || update.alternate {
            self.rows = vec![Row::new(); rows];
        } else {
            let moved = ((update.top - self.top) as usize).min(self.rows.len());
            gone = self.rows.drain(..moved).collect();
            self.rows.resize(rows, Row::new());
        }
        for (row, runs) in &update.rows {
            if let Some(r) = self.rows.get_mut(usize::from(*row)) {
                r.clone_from(runs);
            }
        }
        (self.size, self.top, self.alternate) = (update.size, update.top, update.alternate);
        (from, gone)
    }

    /// The screen's rows, the first numbered `top`.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn top(&self) -> u64 {
        self.top
    }

    /// Line `number`, when it's on the screen.
    pub fn get(&self, number: u64) -> Option<&Row> {
        number.checked_sub(self.top).and_then(|i| self.rows.get(usize::try_from(i).ok()?))
    }
}

/// A key that isn't text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TermKey {
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1–F12.
    F(u8),
    /// A letter, digit or symbol pressed with Ctrl or Alt.
    Char(char),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mods {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alt: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shift: bool,
}

/// What's typed into a terminal; the server turns it into the bytes the
/// program expects in the modes it's in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalInput {
    Text { text: String },
    Key { key: TermKey, #[serde(default)] mods: Mods },
    /// Pasted text (bracketed, when the program asked for that).
    Paste { text: String },
}

/// What a terminal's program or shell did that the user may want to hear
/// of (the event stream carries it; notifications are made of it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum TerminalNews {
    Bell,
    /// The shell ended (its code when it had one).
    Ended { code: Option<i32> },
    /// A command ran this long and finished (with its code, when the shell
    /// said it).
    Finished { code: Option<i32>, ran_ms: u64 },
}

/// A match of a search through a terminal's lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    /// Its line's number (`ScreenUpdate::top`) and column.
    pub line: u64,
    pub col: u16,
    pub len: u16,
    /// The line's text.
    pub text: String,
}

/// What a reader sends on a terminal's socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ToTerminal {
    /// Typed (the user's only: the socket is).
    Input { input: TerminalInput },
    /// The size this reader shows it at (the smallest of all readers wins).
    Size { size: TermSize },
    /// Lines of the scrollback from the one numbered `from`
    /// (`ScreenUpdate::top`), `count` of them.
    Lines { from: u64, count: u32 },
    /// Where `query` is, newest first.
    Search { query: String },
    /// Where lines `first..=last` of start `shell` of the shell are now
    /// (laid out anew since, with a new width, they're numbered anew).
    LineNow { shell: u64, first: u64, last: u64 },
    /// The output of a command (`TerminalCommand::id`) as text.
    CommandOutput { command: u64 },
}

/// What a terminal's socket sends its reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum FromTerminal {
    Screen { update: ScreenUpdate },
    /// Lines of the scrollback asked for (they never change), from the one
    /// numbered `from` (the oldest kept, when an older one was asked for).
    /// Every `Lines` asked for is answered by one, maybe short.
    Lines { from: u64, rows: Vec<Row> },
    /// Rows that scrolled off the reader's screen and changed after it was
    /// last sent them, sent before the update that moves the screen: the
    /// rest are as the reader had them (`ReaderScreen::take`).
    ScrolledOff { lines: Vec<(u64, Row)> },
    Found { query: String, found: Vec<Found> },
    /// Where the lines asked about are now (none: another start of the
    /// shell, or laid out anew longer ago than is followed).
    LineNow { shell: u64, first: u64, last: u64, now: Option<(u64, u64)> },
    /// A command's output as text (lines that wrapped joined; none: there's
    /// no such command now).
    CommandOutput { command: u64, text: Option<String> },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(top: u64, rows: &[(u16, &str)], full: bool) -> ScreenUpdate {
        let rows = rows.iter().map(|(r, t)| (*r, vec![Run { text: (*t).into(), style: Style::default(), wide: false }])).collect();
        ScreenUpdate { seq: 1, size: TermSize { cols: 80, rows: 3 }, full, rows, cursor: Cursor::default(), top, oldest: 0, alternate: false, title: String::new(), shell: 1, commands: Vec::new() }
    }

    fn text(row: &Row) -> String {
        row.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn a_reader_screen_moves_with_its_top_and_gives_back_what_scrolled_off() {
        let mut screen = ReaderScreen::default();
        assert_eq!(screen.take(&update(10, &[(0, "a"), (1, "b"), (2, "c")], true)).1, Vec::<Row>::new());
        // Two lines on: the first two went, as they were; the rest moved up.
        let (from, gone) = screen.take(&update(12, &[(1, "d"), (2, "e")], false));
        assert_eq!((from, gone.iter().map(text).collect::<Vec<_>>()), (10, vec!["a".to_owned(), "b".to_owned()]));
        assert_eq!(screen.rows().iter().map(text).collect::<Vec<_>>(), ["c", "d", "e"]);
        assert_eq!(screen.get(12).map(text).as_deref(), Some("c"));
        // Laid out anew: nothing scrolled off.
        assert!(screen.take(&update(20, &[(0, "x")], true)).1.is_empty());
    }
}
