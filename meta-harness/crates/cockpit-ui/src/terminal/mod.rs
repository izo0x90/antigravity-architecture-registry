//! The terminal buffer: one of the server's terminals, its screen and
//! scrollback drawn as sent (it understands no escape codes), what's typed
//! sent as typed. It holds its own socket to the terminal (`TermLink`): the
//! server sees each panel showing it as a reader at its own size (the
//! smallest wins). Focusing it starts typing; its field takes every key
//! (`Field::Keys`: only the leave keys leave). Not typing, the cockpit's
//! keys move through the lines, as in any buffer.

mod draw;

use std::collections::BTreeMap;

use harness_contracts::{Cursor, FromTerminal, Row, ScreenUpdate, TermSize, Terminal, TerminalId, TerminalInput, TerminalStatus, ToTerminal};

use crate::actions::{Action, ActionProvider, MINOR, PRIMARY, RunCx};
use crate::hosted::{Hosted, Status, Tone};
use crate::nav::{Ask, Fold, Item, Motion, Navigable, Place, Spot, Step, Target};
use crate::shared::{Reader, Shared};
use crate::terminals::{TermAct, TermLink, Terminals};
use crate::viewport::{Go, Stop, Viewport};

/// The screen as last sent.
#[derive(Default)]
struct Screen {
    rows: Vec<Row>,
    size: TermSize,
    cursor: Cursor,
    /// Lines in the scrollback above it.
    history: u32,
    alternate: bool,
    title: String,
}

impl Screen {
    fn take(&mut self, update: ScreenUpdate) {
        if update.full || self.rows.len() != usize::from(update.size.rows) {
            self.rows = vec![Row::new(); usize::from(update.size.rows)];
        }
        for (row, runs) in update.rows {
            if let Some(r) = self.rows.get_mut(usize::from(row)) {
                *r = runs;
            }
        }
        (self.size, self.cursor, self.history, self.alternate, self.title) = (update.size, update.cursor, update.history, update.alternate, update.title);
    }
}

#[derive(Clone, Debug)]
enum TermCommand {
    Type,
    Step(i32),
    Pick,
    New,
    Restart,
    Close,
    Copy,
}

/// A cell: its line (from the scrollback's top) and column.
type Cell = (u64, u16);

#[derive(Default)]
pub struct TerminalBuffer {
    /// The terminal it shows (none yet: it shows the first, or opens one).
    id: Option<TerminalId>,
    terminals: Reader<Terminals>,
    /// Its socket to `id` (none while it isn't drawn).
    link: Option<TermLink>,
    screen: Screen,
    /// Scrollback lines as fetched, by their line from the scrollback's top.
    back: BTreeMap<u64, Row>,
    /// Lines asked for and not come yet (from, count).
    asked: Option<(u64, u32)>,
    /// The size last said to the server.
    size_sent: Option<TermSize>,
    viewport: Viewport,
    pitch: f32,
    char_w: f32,
    /// Not typing: the line the cursor's on (from the scrollback's top).
    cursor: Option<u64>,
    /// Cells selected (from, to: in order as dragged).
    selection: Option<(Cell, Cell)>,
    /// Its panel had focus last frame, with a shell running (gaining it
    /// starts typing).
    was_focused: bool,
    /// It was typed in last frame.
    typing: bool,
    /// Asked for a new terminal: shows it once the desk has made more than this.
    awaiting: Option<u64>,
    /// Asked once for a terminal, having none to show.
    opened: bool,
    /// Popped up over everything (the quick look's own): it has a terminal
    /// of its own (sharing one would size a panel's to it).
    popped: bool,
    /// Typing went on: the view goes back to the live screen.
    to_live: bool,
    clock: f64,
    asks: Vec<Ask>,
    spots: Vec<Spot>,
    grid: Option<egui::Rect>,
}

/// Where it is: the terminal, and the line its cursor is on.
#[derive(serde::Serialize, serde::Deserialize)]
struct TermPlace {
    id: Option<TerminalId>,
    cursor: Option<u64>,
}

impl TerminalBuffer {
    fn info(&self) -> Option<Terminal> {
        let id = self.id.as_ref()?;
        self.terminals.get().get(id).cloned()
    }

    fn ended(&self) -> Option<Option<i32>> {
        match self.info()?.status {
            TerminalStatus::Ended { code } => Some(code),
            TerminalStatus::Running => None,
        }
    }

    /// Lines in all: the scrollback (none while a program has the whole
    /// screen) and the screen.
    fn total(&self) -> u64 {
        u64::from(self.history()) + self.screen.rows.len() as u64
    }

    fn history(&self) -> u32 {
        if self.screen.alternate { 0 } else { self.screen.history }
    }

    /// Line `line`'s runs: the screen's, or the scrollback's (none until fetched).
    fn line(&self, line: u64) -> Option<&Row> {
        let history = u64::from(self.history());
        match line.checked_sub(history) {
            Some(row) => self.screen.rows.get(row as usize),
            None => self.back.get(&line),
        }
    }

    /// Shows terminal `id` from now on (its socket opens when drawn).
    fn show(&mut self, id: TerminalId) {
        if self.id.as_ref() != Some(&id) {
            self.id = Some(id);
            self.drop_link();
            self.screen = Screen::default();
            self.cursor = None;
            self.selection = None;
        }
    }

    fn drop_link(&mut self) {
        self.link = None;
        self.size_sent = None;
        self.back.clear();
        self.asked = None;
    }

    fn send(&mut self, said: &ToTerminal) {
        if let Some(link) = &mut self.link {
            link.send(said);
        }
    }

    fn type_in(&mut self, input: TerminalInput) {
        self.to_live = true;
        self.selection = None;
        self.send(&ToTerminal::Input { input });
    }

    /// What it was sent, applied; the socket opened if it's drawn and has none.
    fn take_link(&mut self, net: &crate::net::NetClient, ctx: &egui::Context) {
        let Some(id) = self.id.clone() else { return };
        let link = self.link.get_or_insert_with(|| TermLink::open(net, &id, ctx));
        let polled = link.poll(ctx);
        if polled.lost {
            // Closed, or fell behind: start again from the whole screen.
            self.drop_link();
            ctx.request_repaint();
            return;
        }
        for said in polled.said {
            match said {
                FromTerminal::Screen { update } => {
                    let before = self.screen.history;
                    if update.history < before {
                        self.back.clear();
                    }
                    self.screen.take(update);
                }
                FromTerminal::Lines { from, rows } => {
                    let top = i64::from(self.screen.history) + from;
                    for (i, row) in rows.into_iter().enumerate() {
                        if let Ok(line) = u64::try_from(top + i as i64) {
                            self.back.insert(line, row);
                        }
                    }
                    self.asked = None;
                }
                FromTerminal::Found { .. } => {}
            }
        }
    }

    /// The scrollback lines `range` shows and doesn't have yet, asked for.
    fn want_lines(&mut self, range: std::ops::Range<u64>) {
        let history = u64::from(self.history());
        let missing = range.filter(|l| *l < history && !self.back.contains_key(l)).collect::<Vec<_>>();
        let (Some(&first), Some(&last)) = (missing.first(), missing.last()) else { return };
        if self.asked.is_some_and(|(from, count)| first >= from && last < from + u64::from(count)) {
            return;
        }
        // A screenful either side, so scrolling finds them there.
        let rows = self.screen.rows.len().max(10) as u64;
        let from = first.saturating_sub(rows);
        let count = ((last + rows).min(history) - from) as u32;
        self.asked = Some((from, count));
        self.send(&ToTerminal::Lines { from: from as i64 - history as i64, count });
    }

    /// The text of the selection (or, with none, of the cursor's line).
    fn selected_text(&self) -> Option<String> {
        let line_text = |line: u64| -> String { self.line(line).map(|r| r.iter().map(|run| run.text.as_str()).collect()).unwrap_or_default() };
        let Some((a, b)) = self.selection else { return self.cursor.map(line_text) };
        let ((l0, c0), (l1, c1)) = if a <= b { (a, b) } else { (b, a) };
        let cut = |text: String, from: u16, to: Option<u16>| -> String { text.chars().skip(usize::from(from)).take(to.map_or(usize::MAX, |t| usize::from(t.saturating_sub(from)) + 1)).collect() };
        let lines: Vec<String> = (l0..=l1)
            .map(|l| {
                let text = line_text(l);
                match (l == l0, l == l1) {
                    (true, true) => cut(text, c0, Some(c1)),
                    (true, false) => cut(text, c0, None),
                    (false, true) => cut(text, 0, Some(c1)),
                    (false, false) => text,
                }
            })
            .collect();
        Some(lines.join("\n").trim_end().to_owned())
    }

    /// Where the cursor can stop: every line, one row each.
    fn stops(&self) -> Vec<Stop> {
        (0..self.total()).map(|l| Stop::new(l as f32 * self.pitch, self.pitch)).collect()
    }

    fn go(&mut self, go: Go) {
        let stops = self.stops();
        let cursor = self.cursor.map(|c| c as usize).or(stops.len().checked_sub(1));
        if let Some(moved) = self.viewport.go(&stops, cursor, go) {
            self.cursor = Some(moved.cursor as u64);
            self.viewport.take(&moved, self.clock, true);
        }
    }
}

impl crate::buffer::Buffer for TerminalBuffer {
    fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, drawing: &mut crate::buffer::Drawing<'_>) {
        // Its panel gaining focus starts typing (in a terminal, that's what
        // you came to do), once there's a shell running to type into.
        let runs = self.info().is_some() && self.ended().is_none();
        if drawing.focused && !self.was_focused && runs {
            *cx.input_mode = crate::keymap::InputMode::Insert { at: cx.at };
        }
        self.was_focused = drawing.focused && (self.was_focused || runs);
        // The frame typing starts in, its keys were the cockpit's (the key
        // that started it): the program gets keys from the next.
        let started = drawing.typing && !self.typing;
        self.typing = drawing.typing;
        self.draw(ui, cx, drawing.typing, started);
    }

    /// While it's drawn: its socket's news taken (opened if it has none).
    fn tick(&mut self, net: &crate::net::NetClient, ctx: &egui::Context) {
        self.take_link(net, ctx);
    }

    /// Not drawn any more: its socket closes (it opens again when it is),
    /// and drawn again, focused, it starts typing again.
    fn hidden(&mut self) {
        self.drop_link();
        (self.was_focused, self.typing) = (false, false);
    }

    fn as_card(&mut self, card: bool) {
        self.popped = card;
    }

    /// The terminal it shows (the pop-up's is its own across reloads).
    fn keep(&self) -> Option<serde_json::Value> {
        self.id.as_ref().map(|id| serde_json::Value::String(id.to_string()))
    }

    fn restore_kept(&mut self, kept: &serde_json::Value) {
        if let Some(id) = kept.as_str() {
            self.show(TerminalId::new(id));
        }
    }

    fn describe(&self) -> String {
        match self.info() {
            Some(t) => format!("terminal {} · {}", t.id, if t.title.is_empty() { t.shell.as_str() } else { t.title.as_str() }),
            None => "a terminal".to_owned(),
        }
    }
}

impl Navigable for TerminalBuffer {
    fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    /// The desk's terminals; a terminal asked for, once made.
    fn share(&mut self, shared: &Shared) {
        self.terminals.follow(&shared.terminals);
        let (made, id) = {
            let terminals = self.terminals.get();
            let (count, id) = terminals.made();
            (count, id.cloned())
        };
        if let (Some(before), Some(id)) = (self.awaiting, id)
            && made > before
        {
            self.awaiting = None;
            self.show(id);
        }
        // None yet: the first there is, else (the server said it has none)
        // a new one, asked once.
        if self.id.is_none() {
            let (first, known) = {
                let terminals = self.terminals.get();
                // Popped up: one of its own, not the first there is.
                (terminals.list().next().map(|t| t.id.clone()).filter(|_| !self.popped), terminals.known())
            };
            match first {
                None if !known => {}
                Some(id) => self.show(id),
                None if !self.opened && self.awaiting.is_none() => {
                    self.opened = true;
                    self.awaiting = Some(made);
                    self.asks.push(Ask::Terminals { id: None, act: TermAct::Open });
                }
                None => {}
            }
        }
    }

    fn place(&self) -> Place {
        Place::of(&TermPlace { id: self.id.clone(), cursor: self.cursor })
    }

    fn place_in_words(&self, place: &Place) -> String {
        place.read::<TermPlace>().and_then(|p| p.id).map(|id| format!("terminal {id}")).unwrap_or_default()
    }

    fn restore(&mut self, place: &Place) {
        if let Some(p) = place.read::<TermPlace>() {
            if let Some(id) = p.id {
                self.show(id);
            }
            self.cursor = p.cursor;
        }
    }

    fn can_show(&self, target: &Target) -> bool {
        matches!(target, Target::Terminal(id) if self.id.as_ref() != Some(id))
    }

    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        let Target::Terminal(id) = target else { return Err("Not a terminal.".into()) };
        self.show(id.clone());
        Ok(())
    }

    fn target(&self) -> Option<Target> {
        self.id.clone().map(Target::Terminal)
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        match motion {
            Motion::Down => self.go(Go::Down(count)),
            Motion::Up => self.go(Go::Up(count)),
            Motion::HalfDown => self.go(Go::HalfDown(count)),
            Motion::HalfUp => self.go(Go::HalfUp(count)),
            Motion::First => self.go(Go::First),
            Motion::Last => self.go(Go::Last),
            Motion::Line(n) => self.go(Go::To(n.saturating_sub(1), crate::viewport::Show::Middle)),
            Motion::Left | Motion::Right | Motion::HalfLeft | Motion::HalfRight | Motion::FewLeft | Motion::FewRight | Motion::NextSibling | Motion::PrevSibling => {}
        }
    }

    /// Typing (an ended shell: started again; none: a new one).
    fn enter(&mut self) -> Step {
        match (self.id.is_some(), self.ended()) {
            (true, Some(_)) => self.asks.push(Ask::Terminals { id: self.id.clone(), act: TermAct::Restart }),
            (true, None) => self.asks.push(Ask::Type),
            (false, _) => {
                self.awaiting = Some(self.terminals.get().made().0);
                self.asks.push(Ask::Terminals { id: None, act: TermAct::Open });
            }
        }
        Step::Done
    }

    /// Back to the live screen.
    fn leave(&mut self) -> Step {
        if self.cursor.take().is_some() || self.selection.take().is_some() {
            self.to_live = true;
            Step::Done
        } else {
            Step::Nothing("It shows the live screen already.".into())
        }
    }

    fn fold(&mut self, _fold: Fold) {}

    /// Its lines (what's fetched of the scrollback, and the screen).
    fn items(&self) -> Vec<Item> {
        (0..self.total()).filter_map(|l| self.line(l).map(|r| Item::new(l.to_string(), "", r.iter().map(|run| run.text.as_str()).collect::<String>()))).collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.cursor.map(|c| c.to_string())
    }

    fn move_to(&mut self, key: &str) {
        if let Ok(line) = key.parse::<usize>() {
            self.go(Go::To(line, crate::viewport::Show::Middle));
        }
    }
}

impl ActionProvider for TerminalBuffer {
    fn actions(&self) -> Vec<Action> {
        use TermCommand as C;
        let mut out = crate::nav::list_moves();
        let has = self.id.is_some();
        let ended = self.ended().is_some();
        out.push(match (has, ended) {
            (true, true) => crate::nav::Move::Enter.offer("Start its shell again"),
            (true, false) => crate::nav::Move::Enter.offer_if(!self.typing, "Type into it"),
            (false, _) => crate::nav::Move::Enter.offer("Open a terminal"),
        });
        out.push(crate::nav::Move::Leave.offer_if(self.cursor.is_some() || self.selection.is_some(), "Back to the live screen"));
        out.push(Action::new("terminal.type", "Type into the terminal", C::Type).focused().importance(PRIMARY).when(if has && !ended && !self.typing { crate::actions::When::Always } else { crate::actions::When::Unavailable }));
        out.push(Action::new("terminal.copy", if self.selection.is_some() { "Copy the selection" } else { "Copy the line" }, C::Copy).focused().importance(MINOR));
        let many = self.terminals.get().list().count() > 1;
        out.push(Action::new("terminal.next", "Next terminal", C::Step(1)).focused().importance(MINOR).when(if many { crate::actions::When::Always } else { crate::actions::When::Unavailable }));
        out.push(Action::new("terminal.prev", "Previous terminal", C::Step(-1)).focused().importance(MINOR).when(if many { crate::actions::When::Always } else { crate::actions::When::Unavailable }));
        let choices = self.terminals.get().list().map(|t| crate::actions::Choice::new(t.id.as_str(), if t.title.is_empty() { t.shell.clone() } else { t.title.clone() }).detail(Some(t.id.to_string())).current(Some(&t.id) == self.id.as_ref())).collect();
        out.push(Action::new("terminal.pick", "Show another terminal here", C::Pick).param(crate::actions::Param::Choice(choices)).focused().importance(MINOR));
        out.push(Action::new("terminal.new", "Open a new terminal here", C::New).focused().importance(MINOR));
        if ended {
            out.push(Action::new("terminal.restart", "Start its shell again", C::Restart).focused().importance(PRIMARY).header(crate::actions::Style::Button, "Start again", Some(egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE)));
        }
        if has {
            out.push(Action::new("terminal.close", "Close the terminal (its shell ends)", C::Close).focused().importance(MINOR));
        }
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        let Some(command) = action.command::<TermCommand>() else { return };
        match command {
            TermCommand::Type => *cx.input_mode = crate::keymap::InputMode::Insert { at: cx.at },
            TermCommand::Step(by) => {
                let next = self.terminals.get().beside(self.id.as_ref(), *by);
                if let Some(id) = next {
                    self.show(id);
                }
            }
            TermCommand::Pick => {
                if let Some(id) = value {
                    self.show(TerminalId::new(id));
                }
            }
            TermCommand::New => {
                self.awaiting = Some(self.terminals.get().made().0);
                self.asks.push(Ask::Terminals { id: None, act: TermAct::Open });
            }
            TermCommand::Restart => self.asks.push(Ask::Terminals { id: self.id.clone(), act: TermAct::Restart }),
            TermCommand::Close => {
                if let Some(id) = self.id.take() {
                    self.drop_link();
                    self.screen = Screen::default();
                    self.opened = true;
                    self.asks.push(Ask::Terminals { id: Some(id), act: TermAct::Close });
                }
            }
            TermCommand::Copy => match self.selected_text() {
                Some(text) => {
                    cx.ctx.copy_text(text);
                    *cx.note = Some("Copied.".into());
                }
                None => *cx.note = Some("Nothing to copy: select some text, or put the cursor on a line.".into()),
            },
        }
    }
}

impl Hosted for TerminalBuffer {
    fn title(&self) -> Status {
        let name = match self.info() {
            Some(t) if !t.title.is_empty() => t.title,
            Some(t) => t.shell,
            None => "Terminal".to_owned(),
        };
        Status::new(format!("{} {name}", egui_phosphor::regular::TERMINAL_WINDOW), Tone::Muted)
    }

    fn status(&self) -> Vec<Status> {
        let mut out = Vec::new();
        if let Some(code) = self.ended() {
            out.push(Status::new(code.map_or_else(|| "ended".to_owned(), |c| format!("ended ({c})")), Tone::Warn));
        }
        if self.cursor.is_some() {
            out.push(Status::new("scrolled back", Tone::Muted));
        }
        out
    }

    fn busy(&self) -> Option<String> {
        let terminals = self.terminals.get();
        if self.awaiting.is_some() && terminals.opening() {
            return Some("Opening a terminal…".into());
        }
        match &self.link {
            Some(link) if link.state() == crate::terminals::LinkState::Connecting => Some("Connecting…".into()),
            _ => None,
        }
    }

    fn alert(&self) -> Option<Status> {
        let terminals = self.terminals.get();
        terminals.error(self.id.as_ref()).or_else(|| terminals.error(None)).map(|e| Status::new(e.clone(), Tone::Error))
    }

    fn own_pointer(&self) -> bool {
        true
    }

    fn typing_rect(&self) -> Option<egui::Rect> {
        self.grid
    }

    fn field(&self) -> crate::keymap::Field {
        crate::keymap::Field::Keys
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use harness_contracts::{Run, Style};

    use super::*;

    /// A whole screen showing `lines`, 80 wide.
    pub(crate) fn screen(lines: &[&str]) -> FromTerminal {
        let rows = (0..24u16).map(|r| (r, lines.get(usize::from(r)).map(|l| vec![Run { text: (*l).to_owned(), style: Style::default(), wide: false }]).unwrap_or_default())).collect();
        let cursor = Cursor { row: lines.len().saturating_sub(1) as u16, col: 2, visible: true };
        FromTerminal::Screen { update: ScreenUpdate { seq: 1, size: TermSize { cols: 80, rows: 24 }, full: true, rows, cursor, history: 0, alternate: false, title: "bash".into() } }
    }

    impl TerminalBuffer {
        /// The server said `said` on its socket (taken next tick).
        pub(crate) fn arrive(&mut self, said: Vec<FromTerminal>) {
            self.link.as_mut().expect("its socket, opened by a tick").arrive(said);
        }

        /// It holds a socket.
        pub(crate) fn linked(&self) -> bool {
            self.link.is_some()
        }

        /// What it sent on its socket.
        pub(crate) fn sent(&self) -> Vec<ToTerminal> {
            self.link.as_ref().map(|l| l.sent.clone()).unwrap_or_default()
        }
    }
}
