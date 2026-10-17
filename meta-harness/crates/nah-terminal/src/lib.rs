//! Terminals: a shell in a pseudo-terminal (`portable-pty`), its screen
//! kept by an emulator (`alacritty_terminal`). What changed on the screen
//! since a reader last looked, its scrollback, searching it, and the keys
//! typed into it (as the bytes the program expects in its modes). What
//! happens in it (its title, its bell, a command finishing, the shell
//! ending) is told to whoever opened it.
//!
//! Knows nothing of the server, its events or the cockpit: the types it
//! shares with them are `harness-contracts`'.

mod commands;
mod feed;
mod input;
mod scan;
mod screen;
mod shell;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, TermMode};
use harness_contracts::{Found, Row, ScreenUpdate, TermSize, TerminalInput};
use portable_pty::{ChildKiller, MasterPty, PtySize};

use commands::Commands;
use feed::Feed;
pub use input::{Modes, encode};
pub use shell::users_shell;

/// How to start one.
#[derive(Clone, Debug)]
pub struct Options {
    /// The shell to run (a path or a name on `PATH`).
    pub shell: String,
    /// The folder it starts in.
    pub folder: PathBuf,
    pub size: TermSize,
    /// Where the files that set up its command marks are kept.
    pub hooks: PathBuf,
    /// Lines of scrollback kept.
    pub history: usize,
}

/// What happened in a terminal, told to whoever opened it (from the
/// thread reading it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Happening {
    /// Its screen changed (ask for what's new).
    Changed,
    Title(String),
    Bell,
    /// A command finished (as the shell marked it), and how long it ran.
    CommandFinished { code: Option<i32>, ran: Duration },
    /// The shell ended, with its exit code when it had one.
    Ended(Option<i32>),
}

pub struct Terminal {
    state: Arc<Mutex<State>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// The shell's process (and its process group's) id.
    pid: Option<u32>,
}

struct State {
    term: Term<Listener>,
    feed: Feed,
    changes: screen::Changes,
    title: String,
    /// What it's called when the program gives it no title.
    named: String,
    commands: Commands,
    /// The last few times its lines were laid out anew (a new width): how
    /// a line numbered before is found after.
    relaid: std::collections::VecDeque<Relaid>,
    /// Lines numbered below this were laid out anew longer ago than is kept.
    relaid_forgotten: u64,
    /// The shell ended (and its code).
    ended: Option<Option<i32>>,
}

/// Lines laid out anew: those numbered below `end` were numbered as in
/// `before`, and are in `after` now (none: they couldn't be read then, a
/// program had the whole screen).
struct Relaid {
    end: u64,
    layouts: Option<(screen::Layout, screen::Layout)>,
}

/// How many layouts back a line can be followed.
const RELAID_KEPT: usize = 4;

/// What the emulator says while it reads output: kept, then acted on once
/// it's done with what was read.
#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<Said>>>);

enum Said {
    Title(String),
    ResetTitle,
    Bell,
    /// An answer to the program (where the cursor is, ...).
    Write(String),
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let said = match event {
            Event::Title(title) => Said::Title(title),
            Event::ResetTitle => Said::ResetTitle,
            Event::Bell => Said::Bell,
            Event::PtyWrite(text) => Said::Write(text),
            _ => return,
        };
        lock(&self.0).push(said);
    }
}

/// The emulator's size.
struct Size(TermSize);

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.0.rows.max(1))
    }

    fn columns(&self) -> usize {
        usize::from(self.0.cols.max(1))
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn io_error(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

/// A shell start's own mark (its command marks carry it): not to be
/// guessed by what a program prints.
fn new_mark() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    hasher.write_u32(std::process::id());
    let mut again = std::collections::hash_map::RandomState::new().build_hasher();
    again.write_u64(hasher.finish());
    format!("{:016x}{:016x}", hasher.finish(), again.finish())
}

fn pty_size(size: TermSize) -> PtySize {
    PtySize { rows: size.rows.max(1), cols: size.cols.max(1), pixel_width: 0, pixel_height: 0 }
}

impl Terminal {
    /// Starts `options.shell`; what happens in it is told to `on`.
    pub fn open(options: Options, on: impl Fn(Happening) + Send + Sync + 'static) -> std::io::Result<Self> {
        let pair = portable_pty::native_pty_system().openpty(pty_size(options.size)).map_err(io_error)?;
        let mark = new_mark();
        let command = shell::command(&options.shell, &options.folder, &options.hooks, &mark)?;
        let mut child = pair.slave.spawn_command(command).map_err(io_error)?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(io_error)?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pair.master.take_writer().map_err(io_error)?));
        let listener = Listener::default();
        let config = Config { scrolling_history: Feed::room(options.history), ..Config::default() };
        let named = std::path::Path::new(&options.shell).file_name().map_or_else(|| options.shell.clone(), |n| n.to_string_lossy().into_owned());
        let state = Arc::new(Mutex::new(State {
            term: Term::new(config, &Size(options.size), listener.clone()),
            feed: Feed::new(options.history, &mark),
            changes: screen::Changes::default(),
            title: named.clone(),
            named,
            commands: Commands::default(),
            relaid: std::collections::VecDeque::new(),
            relaid_forgotten: 0,
            ended: None,
        }));
        let killer = child.clone_killer();
        let pid = child.process_id();
        let reading = Arc::new(Reading { state: state.clone(), listener, writer: writer.clone(), on: Arc::new(on) });
        let waker = {
            let reading = reading.clone();
            std::thread::Builder::new().name("terminal-sync".into()).spawn(move || reading.release_held())?
        };
        std::thread::Builder::new().name("terminal".into()).spawn(move || {
            reading.read(reader, waker.thread());
            // What a program left held as it ended shows.
            reading.feed(&[], true);
            let code = child.wait().ok().map(|s| s.exit_code() as i32);
            lock(&reading.state).ended = Some(code);
            waker.thread().unpark();
            (reading.on)(Happening::Ended(code));
        })?;
        Ok(Self { state, writer, master: Mutex::new(pair.master), killer: Mutex::new(killer), pid })
    }

    /// Types `input` (as the program expects it in its modes).
    pub fn input(&self, input: &TerminalInput) -> std::io::Result<()> {
        let mode = *lock(&self.state).term.mode();
        let bytes = encode(input, Modes { app_cursor: mode.contains(TermMode::APP_CURSOR), bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE) });
        let mut writer = lock(&self.writer);
        writer.write_all(&bytes)?;
        writer.flush()
    }

    /// Shown at `size` from now on.
    pub fn resize(&self, size: TermSize) -> std::io::Result<()> {
        let mut state = lock(&self.state);
        if self.size_of(&state) == size {
            return Ok(());
        }
        lock(&self.master).resize(pty_size(size)).map_err(io_error)?;
        let State { term, feed, changes, commands, relaid, relaid_forgotten, .. } = &mut *state;
        let end = feed.top() + term.screen_lines() as u64;
        // A new width lays the lines out anew: the commands move with them,
        // and how lines moved is kept a while (unless a program has the
        // whole screen: the lines can't be read).
        let new_width = usize::from(size.cols.max(1)) != term.columns();
        let readable = |term: &Term<Listener>| !term.mode().contains(TermMode::ALT_SCREEN);
        let before = (new_width && readable(term)).then(|| screen::Layout::of(term, feed));
        term.resize(Size(size));
        feed.resized(term, end, new_width);
        changes.take(term);
        if new_width {
            let layouts = before.filter(|_| readable(term)).map(|before| (before, screen::Layout::of(term, feed)));
            match &layouts {
                Some((before, after)) => commands.moved(changes.bump(), |line, col| after.place(before, line, col), |line| after.after(before, line)),
                None => commands.lost_places(feed.top(), changes.bump()),
            }
            relaid.push_back(Relaid { end, layouts });
            if relaid.len() > RELAID_KEPT
                && let Some(gone) = relaid.pop_front()
            {
                *relaid_forgotten = gone.end;
            }
        }
        if commands.any_before(feed.oldest()) {
            commands.gone_before(feed.oldest(), changes.bump());
        }
        Ok(())
    }

    fn size_of(&self, state: &State) -> TermSize {
        TermSize { cols: state.term.columns() as u16, rows: state.term.screen_lines() as u16 }
    }

    pub fn size(&self) -> TermSize {
        self.size_of(&lock(&self.state))
    }

    /// What changed after `since` (the whole screen given none).
    pub fn screen(&self, since: Option<u64>) -> ScreenUpdate {
        let state = lock(&self.state);
        state.changes.since(&state.term, &state.feed, &state.commands, since, &state.title)
    }

    /// The scrollback's lines numbered `from` on (`ScreenUpdate::top`),
    /// up to `count`, within what's kept: where they start, and them.
    /// They never change (the screen's rows can: they come with it).
    pub fn scrollback(&self, from: u64, count: u32) -> (u64, Vec<Row>) {
        let state = lock(&self.state);
        let (from, mut rows) = screen::lines(&state.term, &state.feed, from, count);
        rows.truncate(state.feed.top().saturating_sub(from) as usize);
        (from, rows)
    }

    /// Where `query` is, newest first.
    pub fn search(&self, query: &str) -> Vec<Found> {
        let state = lock(&self.state);
        screen::search(&state.term, &state.feed, query, 500)
    }

    /// The output of command `id` as text (lines that wrap joined), and
    /// how many of its lines were left out at its start (it had more than
    /// `most`); none when there's no such command, or while a program has
    /// the whole screen (its lines can't be read then: the program's are).
    pub fn command_output(&self, id: u64, most: u64) -> Option<(String, u64)> {
        let state = lock(&self.state);
        if state.term.mode().contains(TermMode::ALT_SCREEN) {
            return None;
        }
        let command = state.commands.get(id)?;
        let Some(from) = command.output else { return Some((String::new(), 0)) };
        let end = command.end.unwrap_or_else(|| state.feed.top() + state.term.screen_lines() as u64);
        let left_out = end.saturating_sub(from).saturating_sub(most);
        Some((screen::text(&state.term, &state.feed, from + left_out, end), left_out))
    }

    /// Lines `from..to` as text (lines that wrapped joined); none when
    /// they aren't all kept (gone, or a program has the whole screen).
    pub fn lines_text(&self, from: u64, to: u64) -> Option<String> {
        let state = lock(&self.state);
        if state.term.mode().contains(TermMode::ALT_SCREEN) {
            return None;
        }
        let (first, _) = screen::lines(&state.term, &state.feed, from, 0);
        let end = state.feed.top() + state.term.screen_lines() as u64;
        (first == from && to <= end && from < to).then(|| screen::text(&state.term, &state.feed, from, to))
    }

    /// Where lines `first` to `last`, numbered so some layouts ago, are
    /// now (as they are, when no new width came since); none when that
    /// can't be told (too many layouts ago, or one that couldn't be read).
    pub fn lines_now(&self, first: u64, last: u64) -> Option<(u64, u64)> {
        let state = lock(&self.state);
        if first < state.relaid_forgotten {
            return None;
        }
        let (mut first, mut last) = (first, last);
        for relaid in &state.relaid {
            if first < relaid.end {
                let (before, after) = relaid.layouts.as_ref()?;
                (first, last) = (after.place(before, first, 0)?.0, after.place(before, last, u16::MAX)?.0);
            }
        }
        Some((first, last))
    }

    /// The screen's rows, with up to `back` rows of scrollback above them,
    /// each with its number (a row each, as shown); none while a program
    /// has the whole screen (its rows aren't numbered lines).
    pub fn numbered(&self, back: u32) -> Vec<(u64, String)> {
        let state = lock(&self.state);
        if state.term.mode().contains(TermMode::ALT_SCREEN) {
            return Vec::new();
        }
        let rows = state.term.screen_lines() as u32;
        let (from, lines) = screen::lines(&state.term, &state.feed, state.feed.top().saturating_sub(u64::from(back)), back + rows);
        let mut out: Vec<(u64, String)> = (from..).zip(lines.iter().map(|row| row.iter().map(|run| run.text.as_str()).collect())).collect();
        while out.last().is_some_and(|(_, text)| text.is_empty()) {
            out.pop();
        }
        out
    }

    /// The screen as plain text, with up to `back` lines of scrollback above it.
    pub fn text(&self, back: u32) -> String {
        let state = lock(&self.state);
        let top = state.feed.top();
        screen::text(&state.term, &state.feed, top.saturating_sub(u64::from(back)), top + state.term.screen_lines() as u64)
    }

    pub fn title(&self) -> String {
        lock(&self.state).title.clone()
    }

    /// The commands the shell ran (oldest first), as far back as kept.
    pub fn commands(&self) -> Vec<harness_contracts::TerminalCommand> {
        lock(&self.state).commands.list().cloned().collect()
    }

    /// Command `id`, while it's kept.
    pub fn command(&self, id: u64) -> Option<harness_contracts::TerminalCommand> {
        lock(&self.state).commands.get(id).cloned()
    }

    /// The shell ended (with its code when it had one).
    pub fn ended(&self) -> Option<Option<i32>> {
        lock(&self.state).ended
    }
}

impl Drop for Terminal {
    /// The shell and everything it started end with it.
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid.and_then(|p| i32::try_from(p).ok()) {
            // SAFETY: a signal to the shell's own process group (it leads
            // one: the PTY made it a session leader).
            unsafe {
                libc::killpg(pid, libc::SIGHUP);
            }
        }
        let _ = lock(&self.killer).kill();
    }
}

/// What the thread reading a shell's output and the one releasing its
/// held updates share: they feed the emulator the same way.
struct Reading {
    state: Arc<Mutex<State>>,
    listener: Listener,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    on: Arc<dyn Fn(Happening) + Send + Sync>,
}

impl Reading {
    /// Reads the shell's output until it ends; the thread releasing held
    /// updates is woken whenever one is held (it may have let one go since
    /// it was last woken, and be waiting for the next).
    fn read(&self, mut reader: Box<dyn Read + Send>, waker: &std::thread::Thread) {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            if self.feed(&buf[..n], false) {
                waker.unpark();
            }
        }
    }

    /// Feeds `bytes` of output (and a held update that's due; `all`: any
    /// held update): reads the marks, answers the program, and tells what
    /// happened. Whether an update is held after.
    fn feed(&self, bytes: &[u8], all: bool) -> bool {
        let mut happened = Vec::new();
        let mut answers = Vec::new();
        let holding = {
            let mut guard = lock(&self.state);
            let State { term, feed, changes, title, named, commands, .. } = &mut *guard;
            let before = changes.seq();
            let mut marks = feed.overdue(term, all);
            marks.extend(feed.output(term, bytes));
            for (seen, at) in marks {
                if let Some((code, ran)) = commands.mark(&seen, at, changes.bump()) {
                    happened.push(Happening::CommandFinished { code, ran });
                }
            }
            if commands.any_before(feed.oldest()) {
                commands.gone_before(feed.oldest(), changes.bump());
            }
            changes.take(term);
            for said in std::mem::take(&mut *lock(&self.listener.0)) {
                match said {
                    Said::Title(t) => {
                        *title = t.clone();
                        happened.push(Happening::Title(t));
                    }
                    Said::ResetTitle => {
                        *title = named.clone();
                        happened.push(Happening::Title(named.clone()));
                    }
                    Said::Bell => happened.push(Happening::Bell),
                    Said::Write(text) => answers.push(text),
                }
            }
            if changes.seq() != before {
                happened.insert(0, Happening::Changed);
            }
            feed.due_in().is_some()
        };
        if !answers.is_empty() {
            let mut writer = lock(&self.writer);
            for text in answers {
                let _ = writer.write_all(text.as_bytes());
            }
            let _ = writer.flush();
        }
        for happening in happened {
            (self.on)(happening);
        }
        holding
    }

    /// Feeds each synchronized update that's waited long enough (its
    /// program never said it ended), on its own thread: woken when one is
    /// held, done when the shell ends.
    fn release_held(&self) {
        loop {
            std::thread::park();
            loop {
                let (ended, due) = {
                    let state = lock(&self.state);
                    (state.ended.is_some(), state.feed.due_in())
                };
                match due {
                    _ if ended => return,
                    None => break,
                    Some(wait) if !wait.is_zero() => std::thread::park_timeout(wait),
                    Some(_) => {
                        self.feed(&[], false);
                    }
                }
            }
        }
    }
}
