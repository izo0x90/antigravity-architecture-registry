//! Terminals: a shell in a pseudo-terminal (`portable-pty`), its screen
//! kept by an emulator (`alacritty_terminal`). What changed on the screen
//! since a reader last looked, its scrollback, searching it, and the keys
//! typed into it (as the bytes the program expects in its modes). What
//! happens in it (its title, its bell, a command finishing, the shell
//! ending) is told to whoever opened it.
//!
//! Knows nothing of the server, its events or the cockpit: the types it
//! shares with them are `harness-contracts`'.

mod input;
mod marks;
mod screen;
mod shell;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use harness_contracts::{Found, Row, ScreenUpdate, TermSize, TerminalInput};
use portable_pty::{ChildKiller, MasterPty, PtySize};

pub use input::{Modes, encode};
pub use marks::{Mark, Marks};
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
    parser: Processor,
    changes: screen::Changes,
    title: String,
    /// What it's called when the program gives it no title.
    named: String,
    marks: Marks,
    /// When the running command started.
    started: Option<Instant>,
    /// The shell ended (and its code).
    ended: Option<Option<i32>>,
}

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

fn pty_size(size: TermSize) -> PtySize {
    PtySize { rows: size.rows.max(1), cols: size.cols.max(1), pixel_width: 0, pixel_height: 0 }
}

impl Terminal {
    /// Starts `options.shell`; what happens in it is told to `on`.
    pub fn open(options: Options, on: impl Fn(Happening) + Send + Sync + 'static) -> std::io::Result<Self> {
        let pair = portable_pty::native_pty_system().openpty(pty_size(options.size)).map_err(io_error)?;
        let command = shell::command(&options.shell, &options.folder, &options.hooks)?;
        let mut child = pair.slave.spawn_command(command).map_err(io_error)?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(io_error)?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pair.master.take_writer().map_err(io_error)?));
        let listener = Listener::default();
        let config = Config { scrolling_history: options.history, ..Config::default() };
        let named = std::path::Path::new(&options.shell).file_name().map_or_else(|| options.shell.clone(), |n| n.to_string_lossy().into_owned());
        let state = Arc::new(Mutex::new(State {
            term: Term::new(config, &Size(options.size), listener.clone()),
            parser: Processor::new(),
            changes: screen::Changes::default(),
            title: named.clone(),
            named,
            marks: Marks::default(),
            started: None,
            ended: None,
        }));
        let killer = child.clone_killer();
        let pid = child.process_id();
        {
            let (state, writer) = (state.clone(), writer.clone());
            std::thread::Builder::new().name("terminal".into()).spawn(move || {
                read(reader, &state, &listener, &writer, &on);
                let code = child.wait().ok().map(|s| s.exit_code() as i32);
                lock(&state).ended = Some(code);
                on(Happening::Ended(code));
            })?;
        }
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
        let State { term, changes, .. } = &mut *state;
        term.resize(Size(size));
        changes.take(term);
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
        state.changes.since(&state.term, since, &state.title)
    }

    /// Lines `from` (0: the screen's first, below: the scrollback's), up to `count`.
    pub fn lines(&self, from: i64, count: u32) -> (i64, Vec<Row>) {
        screen::lines(&lock(&self.state).term, from, count)
    }

    /// Where `query` is, newest first.
    pub fn search(&self, query: &str) -> Vec<Found> {
        screen::search(&lock(&self.state).term, query, 500)
    }

    /// The screen as plain text, with up to `back` lines of scrollback above it.
    pub fn text(&self, back: u32) -> String {
        let state = lock(&self.state);
        let rows = state.term.screen_lines() as u32;
        let (_, lines) = screen::lines(&state.term, -i64::from(back), back + rows);
        let mut text: Vec<String> = lines.iter().map(|row| row.iter().map(|run| run.text.as_str()).collect()).collect();
        while text.last().is_some_and(String::is_empty) {
            text.pop();
        }
        text.join("\n")
    }

    pub fn title(&self) -> String {
        lock(&self.state).title.clone()
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

/// Reads the shell's output until it ends: feeds the emulator, reads the
/// marks, answers the program, and tells `on` what happened.
fn read(mut reader: Box<dyn Read + Send>, state: &Mutex<State>, listener: &Listener, writer: &Mutex<Box<dyn Write + Send>>, on: &(impl Fn(Happening) + Send + Sync)) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let bytes = &buf[..n];
        let mut happened = Vec::new();
        let mut answers = Vec::new();
        {
            let mut guard = lock(state);
            let State { term, parser, changes, title, named, marks, started, .. } = &mut *guard;
            let before = changes.seq();
            for mark in marks.read(bytes) {
                match mark {
                    Mark::Started => *started = Some(Instant::now()),
                    Mark::Finished(code) => {
                        if let Some(at) = started.take() {
                            happened.push(Happening::CommandFinished { code, ran: at.elapsed() });
                        }
                    }
                }
            }
            parser.advance(term, bytes);
            changes.take(term);
            for said in std::mem::take(&mut *lock(&listener.0)) {
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
        }
        if !answers.is_empty() {
            let mut writer = lock(writer);
            for text in answers {
                let _ = writer.write_all(text.as_bytes());
            }
            let _ = writer.flush();
        }
        for happening in happened {
            on(happening);
        }
    }
}
