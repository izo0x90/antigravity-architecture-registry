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
}

/// What a terminal's socket sends its reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum FromTerminal {
    Screen { update: ScreenUpdate },
    /// Lines of the scrollback (they never change), from the one numbered
    /// `from` (the oldest kept, when an older one was asked for): when
    /// asked for, and as they scroll off the screen (before the update
    /// that moves it).
    Lines { from: u64, rows: Vec<Row> },
    Found { query: String, found: Vec<Found> },
}
