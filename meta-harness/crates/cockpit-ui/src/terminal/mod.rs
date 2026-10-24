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

use harness_contracts::{FromTerminal, Subject, TermSize, Terminal, TerminalId, TerminalInput, TerminalStatus, ToTerminal};

use crate::actions::{Action, ActionProvider, MINOR, PRIMARY, RunCx};
use crate::hosted::{Hosted, Status, Tone};
use crate::nav::{Ask, Fold, Item, Motion, Navigable, Place, Spot, Step, Target, Thing, Things};
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

/// How long a new size holds still before it's said (seconds).
const SIZE_STILL_S: f64 = 0.15;

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
    /// Another size, and since when it's been the size (said once it's
    /// held still `SIZE_STILL_S`).
    size_seen: Option<(TermSize, f64)>,
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
    /// A command whose output was asked for, to copy when it comes (its
    /// number, in this start of the shell).
    copying: Option<u64>,
    /// Said to the user when it's next drawn (what came of asking).
    said: Option<String>,
    /// Lines or a command a go-to asked for, shown once its lines come.
    going_to: Option<Target>,
    /// Where the lines asked for are now: asked of the server (the lines
    /// as numbered then), and whether it has said.
    asked_now: Option<(u64, u64, u64)>,
    known_now: bool,
    /// A row's height, as last drawn.
    row_h: f32,
    /// Its panel had focus last frame, with a shell running (gaining it
    /// starts typing).
    was_focused: bool,
    /// A go-to is landing on its lines: gaining focus for that doesn't
    /// start typing (you came to look).
    landing: bool,
    /// It was typed in last frame.
    typing: bool,
    /// The pointer is a finger (the host says): a drag moves the lines
    /// rather than select them.
    fingers: bool,
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

/// Where it is: the terminal, and the line its cursor's on while it isn't
/// typed in (with the start of its shell it's in: a line's number is only
/// that start's). Kept across a reload without the line: the cursor comes
/// back only when it's gone back to (a jump, a mark).
#[derive(serde::Serialize, serde::Deserialize)]
struct TermPlace {
    id: Option<TerminalId>,
    /// Its line: the start of the shell, and the line's number then (the
    /// terminal's durable name for it, as a tag's; where it is now, after
    /// new layouts, the server says).
    #[serde(default)]
    at: Option<(u64, u64)>,
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

    /// The command the cursor's on, once it ran; else why there's none.
    fn run_on(&self) -> Result<&harness_contracts::TerminalCommand, String> {
        let command = self.cursor.and_then(|c| self.lines.command_at(c)).ok_or("The cursor isn't on a command the shell marked (bash and zsh mark theirs).")?;
        if command.output.is_none() { Err("That command hasn't run yet.".into()) } else { Ok(command) }
    }

    /// Where a go-to asked it to go (lines, a command), once its lines are
    /// here: the cursor there; in another start of its shell, said.
    fn go_where_asked(&mut self) {
        let Some(target) = &self.going_to else { return };
        let (shell, line) = match target {
            Target::TerminalLines { shell, first, .. } => (*shell, Some(*first)),
            Target::TerminalCommand { shell, command, .. } => (*shell, self.lines.command(*command).map(|c| c.prompt)),
            _ => (0, None),
        };
        // No lines yet: they'll come; unless its shell didn't outlive the
        // server (none will).
        if self.lines.shell() == 0 && self.ended() != Some(None) {
            return;
        }
        // A program has the whole screen: its lines show once it leaves.
        if self.lines.alternate() {
            return;
        }
        // Lines are numbered anew with each new width: where they are now,
        // the server says (exactly, as it moves a tag's); asked once, waited for.
        if let Target::TerminalLines { shell, first, last, .. } = target
            && shell == &self.lines.shell()
            && !self.known_now
        {
            if self.asked_now != Some((*shell, *first, *last)) {
                self.asked_now = Some((*shell, *first, *last));
                self.send(&ToTerminal::LineNow { shell: *shell, first: *first, last: *last });
            }
            return;
        }
        (self.going_to, self.asked_now, self.known_now) = (None, None, false);
        let (first, end) = (self.lines.first(), self.lines.end());
        match line.filter(|_| shell == self.lines.shell() && end > first) {
            // The cursor there (a move to where it already is moves nothing),
            // in view; its line gone (cleared, past what's kept), the
            // nearest there is, said.
            Some(line) => {
                let near = line.clamp(first, end - 1);
                if near != line {
                    self.said = Some("It's gone from the terminal (its lines went): the nearest line there is.".into());
                }
                self.cursor = Some(near);
                self.go(Go::To((near - first) as usize, crate::viewport::Show::Middle));
            }
            None => self.said = Some("It's gone from the terminal (an earlier start of its shell, or its lines went).".into()),
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
            (self.drawn, self.cursor, self.selection, self.copying, self.going_to) = (None, None, None, None, None);
            (self.asked_now, self.known_now) = (None, false);
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
            // Closed, or fell behind: start again from the whole screen (a
            // question in flight is asked again on the new socket).
            self.drop_link();
            self.asked_now = None;
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
                        (self.selection, self.found) = (None, None);
                        // The cursor's line, where it is now: the server says.
                        if let (Some(line), Some(id)) = (self.cursor.take(), self.id.clone()) {
                            self.going_to = Some(Target::TerminalLines { terminal: id, shell: self.lines.shell(), first: line, last: line });
                            (self.asked_now, self.known_now) = (None, false);
                        }
                    }
                    lines::Took::Restarted => {
                        self.view.go_live();
                        (self.cursor, self.selection, self.found, self.drawn, self.copying) = (None, None, None, None, None);
                        (self.asked_now, self.known_now) = (None, false);
                    }
                },
                FromTerminal::Lines { from, rows } => self.lines.arrived(from, rows),
                FromTerminal::ScrolledOff { lines } => self.lines.scrolled_off(lines),
                FromTerminal::CommandOutput { command, text } => {
                    if self.copying.take_if(|asked| *asked == command).is_some() {
                        self.said = Some(match text {
                            Some(text) if text.is_empty() => "It printed nothing to copy.".into(),
                            Some(text) => {
                                ctx.copy_text(text);
                                "Copied its output.".into()
                            }
                            None => "That command is gone (its lines went).".into(),
                        });
                    }
                }
                FromTerminal::Found { query, found } => {
                    if self.found.as_ref().is_some_and(|(asked, _)| *asked == query) {
                        self.found = Some((query, found.into_iter().map(|f| (f.line, f.text)).collect()));
                    }
                }
                // Where the lines asked for are now (none: as numbered then,
                // and said to be gone if they are).
                FromTerminal::LineNow { shell, first, last, now } => {
                    if self.asked_now == Some((shell, first, last)) {
                        self.known_now = true;
                        match (now, &mut self.going_to) {
                            (Some((now_first, now_last)), Some(Target::TerminalLines { first, last, .. })) => (*first, *last) = (now_first, now_last),
                            // Laid out anew too long ago to follow: not landed on
                            // a line that may be another's.
                            (None, _) => {
                                self.going_to = None;
                                self.said = Some("Its lines were laid out anew too long ago to follow: where they are now isn't known.".into());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        self.go_where_asked();
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
        if drawing.focused && !self.was_focused && runs && !self.landing {
            *cx.input_mode = crate::keymap::InputMode::Insert { at: cx.at };
        }
        self.was_focused = drawing.focused && (self.was_focused || runs);
        self.landing &= !drawing.focused;
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
        (self.typing, self.fingers) = (drawing.typing, drawing.fingers);
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


    fn describe(&self) -> String {
        match self.info() {
            Some(t) => format!("terminal {} · {}", t.id, if t.title.is_empty() { t.shell.as_str() } else { t.title.as_str() }),
            None => "a terminal".to_owned(),
        }
    }
}

impl crate::nav::DrawsThings for TerminalBuffer {
    /// Lines, or a command (its prompt to its last line; running, to the
    /// shell's line), as much of them as shows (not while a program has
    /// the whole screen).
    fn drawn_at(&self, subject: &Subject) -> Option<egui::Rect> {
        let (grid, top) = (self.grid?, self.drawn?);
        if self.lines.alternate() || self.window == 0 {
            return None;
        }
        let ours = |terminal: &TerminalId, shell: u64| Some(terminal) == self.id.as_ref() && shell == self.lines.shell();
        let (first, last) = match subject {
            Subject::TerminalLines { terminal, shell, first, last, .. } if ours(terminal, *shell) => (*first, *last),
            Subject::TerminalCommand { terminal, shell, command, .. } if ours(terminal, *shell) => {
                let c = self.lines.command(*command)?;
                (c.prompt, c.end.map_or_else(|| self.lines.cursor().0, |end| end.saturating_sub(1)).max(c.prompt))
            }
            _ => return None,
        };
        let (from, to) = (first.max(top), last.min(top + self.window.saturating_sub(1)));
        (from <= to).then(|| egui::Rect::from_min_max(grid.min + egui::vec2(0.0, (from - top) as f32 * self.row_h), egui::pos2(grid.max.x, grid.min.y + (to - top + 1) as f32 * self.row_h)))
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

    /// The terminal, and the cursor's line (or the line a go-to under way
    /// is going to).
    fn place(&self) -> Place {
        let shell = self.lines.shell();
        let at = match (&self.going_to, self.cursor) {
            (Some(Target::TerminalLines { shell, first, .. }), _) => Some((*shell, *first)),
            (_, Some(line)) if shell != 0 => Some((shell, line)),
            _ => None,
        };
        Place::of(&TermPlace { id: self.id.clone(), at })
    }

    fn kept_place(&self) -> Place {
        Place::of(&TermPlace { id: self.id.clone(), at: None })
    }

    fn place_in_words(&self, place: &Place) -> String {
        match place.read::<TermPlace>() {
            Some(TermPlace { id: Some(id), at: Some((_, line)), .. }) => format!("terminal {id} line {line}"),
            Some(TermPlace { id: Some(id), at: None, .. }) => format!("terminal {id}"),
            _ => String::new(),
        }
    }

    /// The terminal; its line, when it had one (as a go-to lands there:
    /// where the server says the line is now); without one, its live
    /// screen. A terminal closed since: said.
    fn restore(&mut self, place: &Place) {
        let Some(TermPlace { id: Some(id), at }) = place.read::<TermPlace>() else { return };
        let closed = {
            let terminals = self.terminals.get();
            terminals.known() && terminals.get(&id).is_none()
        };
        if closed {
            self.said = Some(format!("Terminal {id} is closed."));
            return;
        }
        match at {
            Some((shell, line)) => {
                if let Err(why) = self.show_target(&Target::TerminalLines { terminal: id, shell, first: line, last: line }) {
                    self.said = Some(why);
                }
            }
            None if self.id.as_ref() == Some(&id) => {
                (self.cursor, self.selection) = (None, None);
                self.view.go_live();
            }
            None => self.show(id),
        }
    }

    fn can_show(&self, target: &Target) -> bool {
        match target {
            Target::Terminal(id) => self.id.as_ref() != Some(id),
            Target::TerminalLines { .. } | Target::TerminalCommand { .. } => true,
            _ => false,
        }
    }

    /// A terminal; or lines or a command of one (shown once its lines come).
    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        let id = match target {
            Target::Terminal(id) | Target::TerminalLines { terminal: id, .. } | Target::TerminalCommand { terminal: id, .. } => id,
            _ => return Err("Not a terminal.".into()),
        };
        let terminals = self.terminals.get();
        if terminals.known() && terminals.get(id).is_none() {
            return Err(format!("Terminal {id} is closed."));
        }
        drop(terminals);
        self.show(id.clone());
        if !matches!(target, Target::Terminal(_)) {
            self.going_to = Some(target.clone());
            (self.asked_now, self.known_now) = (None, false);
            self.landing = true;
        }
        self.go_where_asked();
        Ok(())
    }

    /// What a tag can be on at a line: the selection's lines, when it's
    /// in it (exactly those); else the line, then the command it's in (the
    /// unit: its line and all its output). Nothing while a program has the
    /// whole screen (its lines aren't the terminal's to keep).
    fn things(&self, key: &str) -> Things {
        let Some(line) = key.split(':').next().and_then(|l| l.parse::<u64>().ok()) else { return Things::default() };
        let Some(terminal) = self.id.clone().filter(|_| !self.lines.alternate()) else { return Things::default() };
        let shell = self.lines.shell();
        let known = harness_contracts::Known::default();
        // The command lines are all in (named with them, as they're kept).
        let of = |first: u64, last: u64| self.lines.command_at(first).filter(|c| c.output.is_some() && self.lines.command_at(last).is_some_and(|l| l.id == c.id));
        if let Some((a, b)) = self.selection.filter(|(a, b)| (a.0.min(b.0)..=a.0.max(b.0)).contains(&line)) {
            let (first, last) = (a.0.min(b.0), a.0.max(b.0));
            return Things::one(Subject::TerminalLines { terminal, shell, first, last, command: of(first, last).map(|c| c.id) }, &known);
        }
        let command = of(line, line);
        let mut things = Things::one(Subject::TerminalLines { terminal: terminal.clone(), shell, first: line, last: line, command: command.map(|c| c.id) }, &known);
        if let Some(c) = command {
            things.at.push(Thing::named(Subject::TerminalCommand { terminal, shell, command: c.id, typed: harness_contracts::typed_line(&c.text) }, &known));
            things.unit = 1;
        }
        things
    }

    fn draws_things(&self) -> Option<&dyn crate::nav::DrawsThings> {
        Some(self)
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
        if let Some(go) = Go::of(motion, count) {
            self.go(go);
            return;
        }
        match motion {
            // Line number `n`, as lines are numbered for their life (the
            // nearest there is).
            Motion::Line(n) => {
                let (first, end) = (self.lines.first(), self.lines.end());
                let at = (n as u64).clamp(first, end.saturating_sub(1).max(first)) - first;
                self.go(Go::To(at as usize, crate::viewport::Show::Middle));
            }
            // By command (the shell's marks): to the next one's prompt.
            Motion::NextSibling | Motion::PrevSibling => {
                let from = self.cursor.unwrap_or(self.lines.cursor().0);
                for _ in 0..count.max(1) {
                    let Some(at) = self.lines.command_beside(self.cursor.unwrap_or(from), motion == Motion::PrevSibling).and_then(|line| line.checked_sub(self.lines.first())) else { break };
                    self.go(Go::To(at as usize, crate::viewport::Show::InView));
                }
            }
            _ => {}
        }
    }

    /// On a link: opens it (a file in the code, a web link in the
    /// browser). Else typing (an ended shell: started again; none: a new one).
    fn enter(&mut self) -> Step {
        match self.link_here().map(|l| l.to) {
            Some(links::To::File { path, line }) => {
                let line = line.unwrap_or(1) as usize;
                self.asks.push(Ask::Show { target: Target::Span { file: path, start: line, end: line }, in_: Some(crate::kind::BufferKind::Code) });
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

    /// A long press on a line (a finger's click): the cursor there, so what
    /// can be done with it is offered.
    fn gesture(&mut self, gesture: crate::touch::Gesture) {
        let crate::touch::Gesture::LongPress(pos) = gesture else { return };
        let (Some(grid), Some(top)) = (self.grid, self.drawn) else { return };
        if grid.contains(pos) && self.row_h > 0.0 {
            let line = top + ((pos.y - grid.top()) / self.row_h) as u64;
            self.put_cursor(line.min(self.lines.end().saturating_sub(1)));
        }
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
            (Some(links::To::File { path, .. }), _, _) => crate::nav::Move::Enter.offer(format!("Open {path}")).detail("The file the cursor's on, in the code"),
            (Some(links::To::Url(url)), _, _) => crate::nav::Move::Enter.offer(format!("Open {url}")).detail("The web link the cursor's on, in the browser"),
            (None, true, true) => crate::nav::Move::Enter.offer("Start its shell again").detail("A new start of your shell, in the same folder"),
            (None, true, false) => crate::nav::Move::Enter.offer_if(!self.typing, "Type into it").detail("Every key goes to its program until you stop typing"),
            (None, false, _) => crate::nav::Move::Enter.offer("Open a terminal").detail("Your shell, in the project's folder"),
        });
        out.push(crate::nav::Move::Leave.offer_if(self.cursor.is_some() || self.selection.is_some() || !self.view.live(), "Back to the live screen").detail("Drops the cursor and the selection; the view follows the output again"));
        out.push(Action::new("terminal.type", "Type into the terminal", "Every key goes to its program until you stop typing", C::Type).focused().importance(PRIMARY).when(if has && !ended && !self.typing { crate::actions::When::Always } else { crate::actions::When::Unavailable }));
        out.push(Action::new("terminal.copy", if self.selection.is_some() { "Copy the selection" } else { "Copy the line" }, "To the clipboard, as text (drag to select)", C::Copy).focused().importance(MINOR));
        let many = self.terminals.get().list().count() > 1;
        out.push(Action::new("terminal.next", "Next terminal", "This panel shows the next of the server's terminals", C::Step(1)).focused().importance(MINOR).when(if many { crate::actions::When::Always } else { crate::actions::When::Unavailable }));
        out.push(Action::new("terminal.prev", "Previous terminal", "This panel shows the terminal before", C::Step(-1)).focused().importance(MINOR).when(if many { crate::actions::When::Always } else { crate::actions::When::Unavailable }));
        let choices = self.terminals.get().list().map(|t| crate::actions::Choice::new(t.id.as_str(), if t.title.is_empty() { t.shell.clone() } else { t.title.clone() }).detail(Some(t.id.to_string())).current(Some(&t.id) == self.id.as_ref())).collect();
        out.push(Action::new("terminal.pick", "Show another terminal here", "Every terminal the server runs: pick one for this panel", C::Pick).param(crate::actions::Param::Choice(choices)).focused().importance(MINOR));
        out.push(Action::new("terminal.new", "Open a new terminal here", "Your shell, in the project's folder, shown in this panel", C::New).focused().importance(MINOR));
        if ended {
            out.push(Action::new("terminal.restart", "Start its shell again", "A new start of your shell, in the same folder (what the last one showed goes)", C::Restart).focused().importance(PRIMARY).on_item(crate::actions::Style::Button, "Start again", Some(egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE)));
        }
        if has {
            out.push(Action::new("terminal.close", "Close the terminal (its shell ends)", "Its shell and what runs in it end; the terminal is forgotten", C::Close).focused().importance(MINOR));
        }
        // The command the cursor's on (the shell's marks say where).
        let commands = !self.lines.alternate() && self.lines.command_beside(self.lines.first().saturating_sub(1), false).is_some();
        out.push(crate::nav::Move::Motion(Motion::NextSibling).offer_if(commands, "Next command"));
        out.push(crate::nav::Move::Motion(Motion::PrevSibling).offer_if(commands, "Previous command"));
        let when = |ok: bool| if ok { crate::actions::When::Always } else { crate::actions::When::Unavailable };
        out.push(Action::new("terminal.copy_output", "Copy the command's output", "All of what the command the cursor's on printed, to the clipboard", C::CopyOutput).focused().importance(MINOR).when(when(self.run_on().is_ok())));
        out.push(Action::new("terminal.run_again", "Run the command again", "Types the command the cursor's on at the empty prompt and runs it", C::RunAgain).focused().importance(MINOR).when(when(self.run_on().is_ok_and(|c| !c.text.is_empty()) && self.lines.at_empty_prompt().is_some() && !ended)));
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
            // Its output as the server has it (lines that wrapped joined, all
            // of it), copied when it comes.
            TermCommand::CopyOutput => match self.run_on().map(|c| c.id) {
                Ok(id) => {
                    self.copying = Some(id);
                    self.send(&ToTerminal::CommandOutput { command: id });
                }
                Err(why) => *cx.note = Some(why),
            },
            // Typed in as the user would, at an empty prompt.
            TermCommand::RunAgain => {
                let text = match (self.run_on().map(|c| c.text.clone()), self.lines.at_empty_prompt()) {
                    (Err(why), _) => Err(why),
                    (Ok(text), _) if text.is_empty() => Err("The shell didn't say what that command was (bash doesn't for one its history doesn't keep).".to_owned()),
                    (Ok(_), None) => Err("Not at an empty prompt: wait for what runs to end, and clear what's typed.".to_owned()),
                    (Ok(text), Some(_)) => Ok(text),
                };
                match text {
                    Ok(text) => {
                        self.type_in(TerminalInput::Text { text });
                        self.type_in(TerminalInput::Key { key: harness_contracts::TermKey::Enter, mods: harness_contracts::Mods::default() });
                    }
                    Err(why) => *cx.note = Some(why),
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

    #[test]
    fn a_tag_goes_on_a_command_or_its_line_on_the_selected_lines_and_on_nothing_while_a_program_has_the_whole_screen() {
        let mut t = TerminalBuffer { id: Some(TerminalId::new("t1")), ..TerminalBuffer::default() };
        let FromTerminal::Screen { mut update } = numbered(1000, 0) else { unreachable!() };
        update.commands = vec![harness_contracts::TerminalCommand { id: 3, prompt: 40, input: 40, input_col: 2, output: Some(41), end: Some(60), text: "make".into(), code: Some(0), ran_ms: Some(5) }];
        t.lines.take(&update);
        let lines = |first, last, command| Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first, last, command };
        let said = |things: Things| things.at.iter().map(|t| (t.subject.clone(), t.words.clone())).collect::<Vec<_>>();
        // In a command's output: the line, then the command (the unit),
        // each by its one name (what's kept says it too).
        let in_output = t.things("50");
        assert_eq!(in_output.unit, 1);
        assert_eq!(said(in_output), [(lines(50, 50, Some(3)), "terminal t1 line 50 (of command 3)".to_owned()), (Subject::TerminalCommand { terminal: TerminalId::new("t1"), shell: 1, command: 3, typed: "make".into() }, "terminal t1 command 3 (make)".to_owned())]);
        assert_eq!(said(t.things("70")), [(lines(70, 70, None), "terminal t1 line 70".to_owned())], "in no command: the line");
        t.selection = Some(((1023, 3), (100, 0)));
        assert_eq!(said(t.things("500")), [(lines(100, 1023, None), "terminal t1 lines 100-1023".to_owned())], "the selection's, alone (the server says if it's too many)");
        (t.grid, t.drawn, t.window, t.row_h) = (Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 240.0))), Some(1000), 24, 10.0);
        let FromTerminal::Screen { mut update } = numbered(1000, 0) else { unreachable!() };
        update.alternate = true;
        t.lines.take(&update);
        assert_eq!((t.things("1010"), crate::nav::DrawsThings::drawn_at(&t, &lines(1005, 1007, None))), (Things::default(), None));
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
