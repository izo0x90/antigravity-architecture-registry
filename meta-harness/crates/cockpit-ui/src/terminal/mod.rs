//! The terminal buffer: one of the server's terminals, its screen and
//! scrollback drawn as sent (it understands no escape codes), what's typed
//! sent as typed. It holds its own socket to the terminal (`TermLink`): the
//! server sees each panel showing it as a reader at its own size (the
//! smallest wins). Focusing it starts typing; its field takes every key
//! (`Field::Keys`: only the leave keys leave). Not typing, the cockpit's
//! keys move through the lines, as in any buffer. Its lines (`lines`) are
//! numbered as the server numbers them, for their life; its view
//! (`view`) is a whole number of lines, never pixels.

mod draw;
mod lines;
mod links;
mod view;

use std::collections::BTreeMap;

use harness_contracts::{FromTerminal, TermSize, Terminal, TerminalId, TerminalInput, TerminalStatus, ToTerminal};

use crate::actions::{Action, ActionProvider, MINOR, PRIMARY, RunCx};
use crate::hosted::{Hosted, Status, Tone};
use crate::nav::{Ask, Fold, Item, Motion, Navigable, Place, Spot, Step, Target};
use crate::shared::{Reader, Shared};
use crate::terminals::{TermAct, TermLink, Terminals};
use crate::viewport::Go;

#[derive(Clone, Debug)]
enum TermCommand {
    Type,
    Step(i32),
    Pick,
    New,
    Restart,
    Close,
    Copy,
    CopyOutput,
    RunAgain,
}

/// How often it says again that its updates were seen (while some are there).
const SEEN_EVERY: f64 = 2.0;

/// A cell: its line's number and its column.
type Cell = (u64, u16);

#[derive(Default)]
pub struct TerminalBuffer {
    /// The terminal it shows (none yet: it shows the first, or opens one).
    id: Option<TerminalId>,
    terminals: Reader<Terminals>,
    /// The desk's agents (what waits on the user: its updates, seen while drawn).
    agents: Reader<crate::agents::Agents>,
    /// When it last said its updates were seen.
    said_seen: Option<f64>,
    /// Its socket to `id` (none while it isn't drawn).
    link: Option<TermLink>,
    lines: lines::Lines,
    view: view::View,
    /// The line at the top of what was drawn last frame, and since when
    /// the lines the view wants haven't come (what was drawn stays a
    /// moment, rather than blank rows).
    drawn: Option<u64>,
    waiting: Option<f64>,
    /// Rows the window shows.
    window: u64,
    /// The scrollbar's thumb is dragged, held this far down it (a part of the track).
    thumb_held: Option<f32>,
    /// The size last said to the server.
    size_sent: Option<TermSize>,
    char_w: f32,
    /// Not typing: the line the cursor's on (its number), and the column
    /// (a hint put it on a link there).
    cursor: Option<u64>,
    cursor_col: Option<u16>,
    /// The server's matches for the search under way: each line's text.
    found: Option<(String, BTreeMap<u64, String>)>,
    /// A web link to open (Enter on it), opened when it's next drawn.
    open_url: Option<String>,
    /// Cells selected (from, to: in order as dragged).
    selection: Option<(Cell, Cell)>,
    /// A command's output to copy once its lines are here.
    copying: Option<std::ops::Range<u64>>,
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
    clock: f64,
    asks: Vec<Ask>,
    spots: Vec<Spot>,
    grid: Option<egui::Rect>,
}

/// Where it is: the terminal. (Not its cursor: a line's number is only
/// the shell's it was given in, and the cursor is there only while not
/// typing.)
#[derive(serde::Serialize, serde::Deserialize)]
struct TermPlace {
    id: Option<TerminalId>,
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

    /// The command output asked to be copied, copied once its lines are here
    /// (asked for until they are).
    fn copy_when_here(&mut self, cx: &mut RunCx<'_>) {
        let Some(lines) = self.copying.clone() else { return };
        match self.lines.text_of(lines.clone()) {
            Some(text) => {
                cx.ctx.copy_text(text);
                *cx.note = Some("Copied its output.".into());
                self.copying = None;
            }
            None => {
                if let Some((from, count)) = self.lines.wanted(lines) {
                    self.send(&ToTerminal::Lines { from, count });
                }
            }
        }
    }

    /// Its cursor's line gone (cleared, past what's kept): on the nearest
    /// line there is; none there: no cursor.
    fn keep_cursor_on_a_line(&mut self) {
        let (first, end) = (self.lines.first(), self.lines.end());
        self.cursor = self.cursor.filter(|_| end > first).map(|c| c.clamp(first, end - 1));
    }

    /// The lines there are and the window's rows.
    fn span(&self) -> view::Span {
        view::Span { first: self.lines.first(), end: self.lines.end(), window: self.window.max(1) }
    }

    /// Shows terminal `id` from now on (its socket opens when drawn).
    fn show(&mut self, id: TerminalId) {
        if self.id.as_ref() != Some(&id) {
            self.id = Some(id);
            self.drop_link();
            self.lines = lines::Lines::default();
            self.view = view::View::default();
            (self.drawn, self.cursor, self.selection) = (None, None, None);
        }
    }

    /// Its socket let go: what it has stays (a new socket's first update
    /// says if the lines are still the same), what it asked for won't come.
    fn drop_link(&mut self) {
        self.link = None;
        self.size_sent = None;
        self.lines.unasked();
    }

    fn send(&mut self, said: &ToTerminal) {
        if let Some(link) = &mut self.link {
            link.send(said);
        }
    }

    /// Typed: back to the live screen, where what's typed shows.
    fn type_in(&mut self, input: TerminalInput) {
        self.view.go_live();
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
                FromTerminal::Screen { update } => match self.lines.take(&update) {
                    lines::Took::Same => {}
                    // Laid out anew: the view and the cursor keep their
                    // distance from the live screen; a selection and the
                    // search's lines were of the old lines.
                    lines::Took::Relaid { by } => {
                        self.view.shift(by);
                        self.cursor = self.cursor.map(|c| c.saturating_add_signed(by));
                        (self.selection, self.found) = (None, None);
                    }
                    lines::Took::Restarted => {
                        self.view.go_live();
                        (self.cursor, self.selection, self.found, self.drawn) = (None, None, None, None);
                    }
                },
                FromTerminal::Lines { from, rows } => self.lines.arrived(from, rows),
                FromTerminal::ScrolledOff { lines } => self.lines.scrolled_off(lines),
                FromTerminal::Found { query, found } => {
                    if self.found.as_ref().is_some_and(|(asked, _)| *asked == query) {
                        self.found = Some((query, found.into_iter().map(|f| (f.line, f.text)).collect()));
                    }
                }
            }
        }
    }

    /// The text of the selection (or, with none, of the cursor's line).
    fn selected_text(&self) -> Option<String> {
        let line_text = |line: u64| -> String { self.lines.text(line).unwrap_or_default() };
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

    /// The link the cursor's on: at its column, else the line's first.
    fn link_here(&self) -> Option<links::Link> {
        let line = self.cursor?;
        let text = self.lines.text(line)?;
        let folder = self.info().map(|t| t.folder).unwrap_or_default();
        let found = links::links(&text, &folder);
        match self.cursor_col {
            Some(col) => found.into_iter().find(|l| col >= l.col && col < l.col + l.len),
            None => found.into_iter().next(),
        }
    }

    fn go(&mut self, go: Go) {
        self.cursor_col = None;
        let span = self.span();
        if let Some(line) = self.view.go(go, self.cursor, span) {
            self.cursor = Some(line);
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
        let keys = match (drawing.typing, self.typing) {
            (false, _) => draw::Keys::Not,
            (true, false) => draw::Keys::Starting,
            (true, true) => draw::Keys::Typing,
        };
        // Its cursor is there only while it isn't typed in: typing drops
        // it (and the view follows the live screen again); leaving typing
        // puts it where the program's cursor is.
        match (drawing.typing, self.typing) {
            (true, false) => {
                (self.cursor, self.cursor_col) = (None, None);
                self.view.go_live();
            }
            (false, true) => (self.cursor, self.cursor_col) = (Some(self.lines.cursor().0), None),
            _ => {}
        }
        self.typing = drawing.typing;
        self.keep_cursor_on_a_line();
        self.draw(ui, cx, keys);
    }

    /// While it's drawn: its socket's news taken (opened if it has none),
    /// and what it said that waited on the user seen.
    fn tick(&mut self, net: &crate::net::NetClient, ctx: &egui::Context) {
        self.take_link(net, ctx);
        let now = ctx.input(|i| i.time);
        let unseen = self.id.as_ref().is_some_and(|id| self.agents.get().store().notifications.values().any(|n| n.level == harness_contracts::Level::Update && n.terminal.as_ref() == Some(id)));
        if unseen && self.said_seen.is_none_or(|at| now - at > SEEN_EVERY) {
            self.said_seen = Some(now);
            self.asks.push(Ask::Terminals { id: self.id.clone(), act: TermAct::Seen });
        }
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
        self.agents.follow(&shared.agents);
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
        Place::of(&TermPlace { id: self.id.clone() })
    }

    fn place_in_words(&self, place: &Place) -> String {
        place.read::<TermPlace>().and_then(|p| p.id).map(|id| format!("terminal {id}")).unwrap_or_default()
    }

    fn restore(&mut self, place: &Place) {
        if let Some(p) = place.read::<TermPlace>() {
            if let Some(id) = p.id {
                self.show(id);
            }
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

    /// The file the cursor's on (a go-to takes it to the code), else the terminal.
    fn target(&self) -> Option<Target> {
        if let Some(links::Link { to: links::To::File { path, line }, .. }) = self.link_here() {
            let line = line.unwrap_or(1) as usize;
            return Some(Target::Span { file: path, start: line, end: line });
        }
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
            // By command (the shell's marks): to the next one's prompt.
            Motion::NextSibling | Motion::PrevSibling => {
                let from = self.cursor.unwrap_or(self.lines.cursor().0);
                for _ in 0..count.max(1) {
                    let Some(line) = self.lines.command_beside(self.cursor.unwrap_or(from), motion == Motion::PrevSibling) else { break };
                    self.go(Go::To((line - self.lines.first()) as usize, crate::viewport::Show::InView));
                }
            }
            Motion::Left | Motion::Right | Motion::HalfLeft | Motion::HalfRight | Motion::FewLeft | Motion::FewRight => {}
        }
    }

    /// On a link: opens it (a file in the code, a web link in the
    /// browser). Else typing (an ended shell: started again; none: a new one).
    fn enter(&mut self) -> Step {
        match self.link_here().map(|l| l.to) {
            Some(links::To::File { path, line }) => {
                let line = line.unwrap_or(1) as usize;
                self.asks.push(Ask::Show { target: Target::Span { file: path, start: line, end: line }, in_: crate::kind::BufferKind::Code });
                return Step::Done;
            }
            Some(links::To::Url(url)) => {
                self.open_url = Some(url);
                return Step::Done;
            }
            None => {}
        }
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
        let held = !self.view.live();
        if self.cursor.take().is_some() | self.selection.take().is_some() | held {
            self.view.go_live();
            Step::Done
        } else {
            Step::Nothing("It shows the live screen already.".into())
        }
    }

    fn fold(&mut self, _fold: Fold) {}

    /// Its lines: the screen's, and those the server found (through the
    /// whole scrollback) for the search under way.
    fn items(&self) -> Vec<Item> {
        let mut lines: BTreeMap<u64, String> = self.lines.screen_texts().collect();
        if let Some((_, found)) = &self.found {
            for (line, text) in found {
                lines.entry(*line).or_insert_with(|| text.clone());
            }
        }
        lines.into_iter().map(|(l, text)| Item::new(l.to_string(), "", text)).collect()
    }

    /// The server searches the whole scrollback as it's typed.
    fn on_search(&mut self, event: crate::nav::SearchEvent<'_>) {
        match event {
            crate::nav::SearchEvent::Typing(query) if !query.is_empty() => {
                if self.found.as_ref().is_none_or(|(asked, _)| asked != query) {
                    self.found = Some((query.to_owned(), BTreeMap::new()));
                    self.send(&ToTerminal::Search { query: query.to_owned() });
                }
            }
            crate::nav::SearchEvent::Typing(_) | crate::nav::SearchEvent::Cancelled => self.found = None,
            crate::nav::SearchEvent::Confirmed => {}
        }
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.cursor.map(|c| c.to_string())
    }

    /// A line ("12"), or a place on one ("12:7", a hint's link).
    fn move_to(&mut self, key: &str) {
        let (line, col) = key.split_once(':').map_or((key, None), |(l, c)| (l, c.parse::<u16>().ok()));
        if let Some(at) = line.parse::<u64>().ok().and_then(|n| n.checked_sub(self.lines.first())) {
            self.go(Go::To(at as usize, crate::viewport::Show::Middle));
            self.cursor_col = col;
        }
    }
}

impl ActionProvider for TerminalBuffer {
    fn actions(&self) -> Vec<Action> {
        use TermCommand as C;
        let mut out = crate::nav::list_moves();
        let has = self.id.is_some();
        let ended = self.ended().is_some();
        out.push(match (self.link_here().map(|l| l.to), has, ended) {
            (Some(links::To::File { path, .. }), _, _) => crate::nav::Move::Enter.offer(format!("Open {path}")),
            (Some(links::To::Url(url)), _, _) => crate::nav::Move::Enter.offer(format!("Open {url}")),
            (None, true, true) => crate::nav::Move::Enter.offer("Start its shell again"),
            (None, true, false) => crate::nav::Move::Enter.offer_if(!self.typing, "Type into it"),
            (None, false, _) => crate::nav::Move::Enter.offer("Open a terminal"),
        });
        out.push(crate::nav::Move::Leave.offer_if(self.cursor.is_some() || self.selection.is_some() || !self.view.live(), "Back to the live screen"));
        out.push(Action::new("terminal.type", "Type into the terminal", C::Type).focused().importance(PRIMARY).when(if has && !ended && !self.typing { crate::actions::When::Always } else { crate::actions::When::Unavailable }));
        out.push(Action::new("terminal.copy", if self.selection.is_some() { "Copy the selection" } else { "Copy the line" }, C::Copy).focused().importance(MINOR));
        // The command the cursor's on (the shell's marks say where).
        let commands = self.lines.command_beside(self.lines.first().saturating_sub(1), false).is_some();
        out.push(crate::nav::Move::Motion(Motion::NextSibling).offer_if(commands, "Next command"));
        out.push(crate::nav::Move::Motion(Motion::PrevSibling).offer_if(commands, "Previous command"));
        let on = self.cursor.and_then(|c| self.lines.command_at(c)).filter(|c| c.output.is_some());
        let when = |ok: bool| if ok { crate::actions::When::Always } else { crate::actions::When::Unavailable };
        out.push(Action::new("terminal.copy_output", "Copy the command's output", C::CopyOutput).focused().importance(MINOR).when(when(on.is_some())));
        out.push(Action::new("terminal.run_again", "Run the command again", C::RunAgain).focused().importance(MINOR).when(when(on.is_some() && !ended)));
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
                    self.lines = lines::Lines::default();
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
            TermCommand::CopyOutput => {
                if let Some(command) = self.cursor.and_then(|c| self.lines.command_at(c)) {
                    let lines = command.output.unwrap_or(command.input)..command.end.unwrap_or_else(|| self.lines.end());
                    // Its lines may not be here yet: copied once they are.
                    self.copying = Some(lines);
                    self.copy_when_here(cx);
                }
            }
            TermCommand::RunAgain => {
                if let Some(text) = self.cursor.and_then(|c| self.lines.command_at(c)).map(|c| c.text.clone()) {
                    self.type_in(TerminalInput::Text { text });
                    self.type_in(TerminalInput::Key { key: harness_contracts::TermKey::Enter, mods: harness_contracts::Mods::default() });
                }
            }
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
        if !self.view.live() {
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
    use harness_contracts::{Cursor, Run, ScreenUpdate, Style};

    use super::*;

    /// A whole screen showing `lines`, 80 wide, with `history` lines above it.
    pub(crate) fn screen_under(lines: &[&str], history: u32) -> FromTerminal {
        let FromTerminal::Screen { mut update } = screen(lines) else { unreachable!() };
        update.top = u64::from(history);
        FromTerminal::Screen { update }
    }

    /// A whole 24-row screen from line `top` (each row says its line's
    /// number: "line 120"), the scrollback from `oldest`.
    pub(crate) fn numbered(top: u64, oldest: u64) -> FromTerminal {
        let texts: Vec<String> = (top..top + 24).map(|n| format!("line {n}")).collect();
        let FromTerminal::Screen { mut update } = screen(&texts.iter().map(String::as_str).collect::<Vec<_>>()) else { unreachable!() };
        (update.top, update.oldest, update.full) = (top, oldest, false);
        FromTerminal::Screen { update }
    }

    /// The server's answer to lines asked for, from `from` (each says its number).
    pub(crate) fn numbered_lines(from: u64, count: u32) -> FromTerminal {
        FromTerminal::Lines { from, rows: (from..from + u64::from(count)).map(|n| vec![Run { text: format!("line {n}"), style: Style::default(), wide: false }]).collect() }
    }

    /// A whole screen showing `lines`, 80 wide.
    pub(crate) fn screen(lines: &[&str]) -> FromTerminal {
        let rows = (0..24u16).map(|r| (r, lines.get(usize::from(r)).map(|l| vec![Run { text: (*l).to_owned(), style: Style::default(), wide: false }]).unwrap_or_default())).collect();
        let cursor = Cursor { row: lines.len().saturating_sub(1) as u16, col: 2, visible: true };
        FromTerminal::Screen { update: ScreenUpdate { seq: 1, size: TermSize { cols: 80, rows: 24 }, full: true, rows, cursor, top: 0, oldest: 0, alternate: false, title: "bash".into(), shell: 1, commands: Vec::new() } }
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
