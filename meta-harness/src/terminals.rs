//! The server's terminals: shells it runs for the user in the project's
//! folder. Which exist (and how each is) goes on the event stream and into
//! the state database; each one's screen goes to whoever shows it, on its
//! own socket (`routes::terminals`). Shown at several sizes, the smallest
//! wins. Its shell doesn't outlive the server: a kept terminal comes back
//! ended, to be started again.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use harness_contracts::{ClientEvent, Found, Row, ScreenUpdate, Subject, TermSize, Terminal, TerminalId, TerminalInput, TerminalNews, TerminalStatus};
use harness_core::EventHub;
use nah_terminal::{Happening, Options};
use tokio::sync::watch;

/// Lines of scrollback each keeps.
const HISTORY: usize = 10_000;

/// A new terminal's size until it's shown.
const FIRST_SIZE: TermSize = TermSize { cols: 100, rows: 30 };

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("there's no terminal {0}")]
    NotFound(TerminalId),
    #[error("terminal {0}'s shell has ended: start it again first")]
    Ended(TerminalId),
    #[error("{shell} doesn't start: {source}")]
    Start { shell: String, source: std::io::Error },
    #[error("the terminal doesn't answer: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Store(#[from] nah_state::StateError),
    #[error("the work was dropped: {0}")]
    Dropped(#[from] tokio::task::JoinError),
}

/// A command in a line: its number, its command line, how it ended (as
/// notifications say it).
pub fn in_words(command: &harness_contracts::TerminalCommand) -> String {
    let typed = if command.text.is_empty() { "(the shell didn't say)" } else { command.text.as_str() };
    let ended = match (command.end, command.code) {
        (None, _) => "running".to_owned(),
        (Some(_), Some(code)) => format!("exit code {code}"),
        (Some(_), None) => "ended".to_owned(),
    };
    let ran = command.ran_ms.map(|ms| format!(", after {}", nah_notify::rules::took(ms))).unwrap_or_default();
    format!("{}. $ {typed} ({ended}{ran})", command.id)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Terminals {
    /// Where shells start (the project's folder), and where their start-up
    /// hooks are kept.
    folder: PathBuf,
    hooks: PathBuf,
    hub: Arc<EventHub>,
    state: Arc<nah_state::StateDb>,
    open: Mutex<BTreeMap<TerminalId, Arc<Open>>>,
    /// A command that ran at least this long is news when it finishes.
    long: std::time::Duration,
    /// Told when tagged lines may have moved, or their names changed (a
    /// new width, a shell started again or closed): the tags keep them
    /// where they are now.
    moved: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>>,
}

/// One terminal: how it is, its shell (none once the server restarted
/// under it), and who shows it at what size.
pub struct Open {
    info: Mutex<Terminal>,
    shell: Mutex<Option<nah_terminal::Terminal>>,
    /// Bumped whenever its screen (or how it is) changed: its sockets wait on it.
    changed: watch::Sender<u64>,
    viewers: Mutex<HashMap<u64, TermSize>>,
    next_viewer: AtomicU64,
    /// Which start of its shell is the current one: what an earlier one
    /// says (its end, after a restart) is old news.
    generation: AtomicU64,
    /// The current start's own number, told to readers (unlike the
    /// generation, not the same again after the server restarts).
    start: AtomicU64,
}

impl Terminals {
    /// The terminals kept from the last run (ended), told on `hub`; a
    /// command that ran `long` or longer is news when it finishes.
    pub fn load(folder: PathBuf, hooks: PathBuf, hub: Arc<EventHub>, state: Arc<nah_state::StateDb>, long: std::time::Duration) -> Result<Arc<Self>, nah_state::StateError> {
        let kept = state.terminals()?;
        let open = kept.into_iter().map(|t| (t.id.clone(), Arc::new(Open::new(t)))).collect();
        let terminals = Arc::new(Self { folder, hooks, hub, state, open: Mutex::new(open), long, moved: std::sync::OnceLock::new() });
        for t in terminals.list() {
            terminals.hub.publish(ClientEvent::TerminalUpdated { terminal: t });
        }
        Ok(terminals)
    }

    /// Tells `moved` when tagged lines may have moved, from now on.
    pub fn when_moved(&self, moved: impl Fn() + Send + Sync + 'static) {
        let _ = self.moved.set(Box::new(moved));
    }

    fn tell_moved(&self) {
        if let Some(moved) = self.moved.get() {
            moved();
        }
    }

    pub fn list(&self) -> Vec<Terminal> {
        lock(&self.open).values().map(|o| o.info()).collect()
    }

    pub fn get(&self, id: &TerminalId) -> Result<Arc<Open>, TerminalError> {
        lock(&self.open).get(id).cloned().ok_or_else(|| TerminalError::NotFound(id.clone()))
    }

    /// A new terminal: the user's shell in the project's folder.
    pub fn open(self: &Arc<Self>) -> Result<Terminal, TerminalError> {
        let id = {
            let open = lock(&self.open);
            let n = (1..).find(|n| !open.contains_key(&TerminalId::new(format!("t{n}")))).unwrap_or(1);
            TerminalId::new(format!("t{n}"))
        };
        let shell = nah_terminal::users_shell();
        let info = Terminal { id: id.clone(), title: String::new(), status: TerminalStatus::Running, size: FIRST_SIZE, shell, folder: self.folder.display().to_string() };
        let open = Arc::new(Open::new(info));
        lock(&self.open).insert(id.clone(), open.clone());
        self.start(&open)?;
        Ok(open.info())
    }

    /// Starts its shell again (in its folder; one still running ends first).
    pub fn restart(self: &Arc<Self>, id: &TerminalId) -> Result<Terminal, TerminalError> {
        let open = self.get(id)?;
        open.generation.fetch_add(1, Ordering::SeqCst);
        lock(&open.shell).take();
        self.start(&open)?;
        Ok(open.info())
    }

    /// Ends its shell and forgets it.
    pub fn close(&self, id: &TerminalId) -> Result<(), TerminalError> {
        let open = lock(&self.open).remove(id).ok_or_else(|| TerminalError::NotFound(id.clone()))?;
        lock(&open.shell).take();
        self.state.forget_terminal(id)?;
        self.hub.publish(ClientEvent::TerminalClosed { terminal_id: id.clone() });
        self.tell_moved();
        Ok(())
    }

    /// Starts `open`'s shell, at its size, in its folder.
    fn start(self: &Arc<Self>, open: &Arc<Open>) -> Result<(), TerminalError> {
        let info = open.info();
        let options = Options { shell: info.shell.clone(), folder: PathBuf::from(&info.folder), size: info.size, hooks: self.hooks.clone(), history: HISTORY };
        let generation = open.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(generation, |d| d.as_nanos() as u64);
        open.start.store(now.max(open.start.load(Ordering::SeqCst) + 1), Ordering::SeqCst);
        let (terminals, weak) = (Arc::downgrade(self), Arc::downgrade(open));
        let shell = nah_terminal::Terminal::open(options, move |happening| {
            if let (Some(terminals), Some(open)) = (terminals.upgrade(), weak.upgrade())
                && open.generation.load(Ordering::SeqCst) == generation
            {
                terminals.happened(&open, &happening);
            }
        })
        .map_err(|source| TerminalError::Start { shell: info.shell.clone(), source })?;
        {
            let mut info = lock(&open.info);
            info.status = TerminalStatus::Running;
            info.title = shell.title();
        }
        *lock(&open.shell) = Some(shell);
        open.bump();
        self.said(open);
        self.tell_moved();
        Ok(())
    }

    /// Something happened in `open`'s shell (on its reading thread).
    fn happened(&self, open: &Arc<Open>, happening: &Happening) {
        match happening {
            Happening::Changed => open.bump(),
            Happening::Title(title) => {
                lock(&open.info).title = title.clone();
                self.said(open);
            }
            Happening::Ended(code) => {
                lock(&open.info).status = TerminalStatus::Ended { code: *code };
                open.bump();
                self.said(open);
            }
            Happening::Bell | Happening::CommandFinished { .. } => {}
        }
        self.news(open, happening);
    }

    /// What's news in what happened (a bell, the shell's end, a long
    /// command finishing): told on the event stream, while it's open.
    fn news(&self, open: &Arc<Open>, happening: &Happening) {
        let news = match happening {
            Happening::Bell => TerminalNews::Bell,
            Happening::Ended(code) => TerminalNews::Ended { code: *code },
            Happening::CommandFinished { code, ran } if *ran >= self.long => TerminalNews::Finished { code: *code, ran_ms: ran.as_millis() as u64 },
            _ => return,
        };
        let terminal = open.info();
        if lock(&self.open).contains_key(&terminal.id) {
            self.hub.publish(ClientEvent::TerminalNews { terminal, news });
        }
    }

    /// How `open` is now, told and kept (unless it was closed meanwhile).
    fn said(&self, open: &Arc<Open>) {
        let info = open.info();
        if !lock(&self.open).contains_key(&info.id) {
            return;
        }
        if let Err(e) = self.state.keep_terminal(&info) {
            tracing::warn!("Terminal {} can't be kept: {e}", info.id);
        }
        self.hub.publish(ClientEvent::TerminalUpdated { terminal: info });
    }

    /// Shown by `viewer` at `size` (none: no longer shown by it): resized
    /// to the smallest it's shown at.
    pub fn shown(&self, open: &Arc<Open>, viewer: u64, size: Option<TermSize>) {
        let smallest = {
            let mut viewers = lock(&open.viewers);
            match size {
                Some(size) => viewers.insert(viewer, size),
                None => viewers.remove(&viewer),
            };
            viewers.values().copied().reduce(|a, b| TermSize { cols: a.cols.min(b.cols), rows: a.rows.min(b.rows) })
        };
        let Some(size) = smallest.filter(|s| *s != open.info().size) else { return };
        if let Some(shell) = lock(&open.shell).as_ref()
            && let Err(e) = shell.resize(size)
        {
            tracing::warn!("Terminal {} can't be resized: {e}", open.info().id);
            return;
        }
        let new_width = std::mem::replace(&mut lock(&open.info).size, size).cols != size.cols;
        open.bump();
        self.said(open);
        // A new width lays its lines out anew.
        if new_width {
            self.tell_moved();
        }
    }
}

impl Open {
    fn new(info: Terminal) -> Self {
        Self { info: Mutex::new(info), shell: Mutex::default(), changed: watch::channel(0).0, viewers: Mutex::default(), next_viewer: AtomicU64::new(1), generation: AtomicU64::new(0), start: AtomicU64::new(0) }
    }

    pub fn info(&self) -> Terminal {
        lock(&self.info).clone()
    }

    fn bump(&self) {
        self.changed.send_modify(|n| *n += 1);
    }

    /// Which start of its shell is the current one (a reader of an
    /// earlier one needs the whole screen).
    pub fn started(&self) -> u64 {
        self.start.load(Ordering::SeqCst)
    }

    /// A number for a new reader of it.
    pub fn viewer(&self) -> u64 {
        self.next_viewer.fetch_add(1, Ordering::Relaxed)
    }

    /// Wakes when it changes.
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// What changed after `since` (the whole screen given none); an ended
    /// one with no shell left shows nothing.
    pub fn screen(&self, since: Option<u64>) -> ScreenUpdate {
        let mut update = match lock(&self.shell).as_ref() {
            Some(shell) => shell.screen(since),
            None => {
                let info = self.info();
                ScreenUpdate { seq: 0, size: info.size, full: since.is_none(), rows: Vec::new(), cursor: Default::default(), top: 0, oldest: 0, alternate: false, title: info.title, shell: 0, commands: Vec::new() }
            }
        };
        update.shell = self.started();
        update
    }

    /// The scrollback's lines from `from` (`nah_terminal::Terminal::scrollback`).
    pub fn scrollback(&self, from: u64, count: u32) -> (u64, Vec<Row>) {
        lock(&self.shell).as_ref().map_or((from, Vec::new()), |s| s.scrollback(from, count))
    }

    pub fn search(&self, query: &str) -> Vec<Found> {
        lock(&self.shell).as_ref().map_or_else(Vec::new, |s| s.search(query))
    }

    /// The commands its shell ran (oldest first), as far back as kept.
    pub fn commands(&self) -> Vec<harness_contracts::TerminalCommand> {
        lock(&self.shell).as_ref().map_or_else(Vec::new, |s| s.commands())
    }

    /// Command `id`, while it's kept.
    pub fn command(&self, id: u64) -> Option<harness_contracts::TerminalCommand> {
        lock(&self.shell).as_ref().and_then(|s| s.command(id))
    }

    /// Command `id`'s output as text, and how many of its first lines
    /// were left out (it had more than `most`).
    pub fn command_output(&self, id: u64, most: u64) -> Option<(String, u64)> {
        lock(&self.shell).as_ref().and_then(|s| s.command_output(id, most))
    }

    /// The screen's rows (with `back` more above), each with its number.
    pub fn numbered(&self, back: u32) -> Vec<(u64, String)> {
        lock(&self.shell).as_ref().map_or_else(Vec::new, |s| s.numbered(back))
    }

    /// Where lines numbered `first` to `last` are now (after a new width).
    pub fn lines_now(&self, first: u64, last: u64) -> Option<(u64, u64)> {
        lock(&self.shell).as_ref().and_then(|s| s.lines_now(first, last))
    }

    /// Lines `from..to` as text, when they're kept.
    pub fn lines_text(&self, from: u64, to: u64) -> Option<String> {
        lock(&self.shell).as_ref().and_then(|s| s.lines_text(from, to))
    }

    /// The screen as text, with up to `back` lines of scrollback.
    pub fn text(&self, back: u32) -> String {
        lock(&self.shell).as_ref().map_or_else(String::new, |s| s.text(back))
    }

    /// Types `input` into its shell.
    pub fn input(&self, input: &TerminalInput) -> Result<(), TerminalError> {
        let shell = lock(&self.shell);
        let shell = shell.as_ref().filter(|s| s.ended().is_none()).ok_or_else(|| TerminalError::Ended(self.info().id))?;
        Ok(shell.input(input)?)
    }
}

/// The terminals own their lines and commands as subjects of tags: their
/// names (`terminal t1 lines 120-124`, `terminal t1 command 3`; another
/// start's: `terminal t1@<start> …`), what they say now (in the start of
/// the shell they were tagged in; of many lines, the first
/// `TAGGED_LINES`), and where lines are after a new width.
impl nah_tags::Resolve for Terminals {
    fn name(&self, subject: &Subject) -> Option<String> {
        match subject {
            Subject::TerminalLines { terminal, shell, first, last } => Some(self.words(terminal, *shell, &format!("lines {first}-{last}"))),
            Subject::TerminalCommand { terminal, shell, command } => Some(self.words(terminal, *shell, &format!("command {command}"))),
            _ => None,
        }
    }

    fn now(&self, subject: &Subject) -> Option<String> {
        let started = |terminal: &TerminalId, shell: u64| self.get(terminal).ok().filter(|o| o.started() == shell);
        match subject {
            Subject::TerminalLines { terminal, shell, first, last } => {
                let end = last.checked_add(1)?.min(first.saturating_add(TAGGED_LINES));
                let text = started(terminal, *shell)?.lines_text(*first, end)?;
                Some(if end <= *last { format!("{text}\n(and {} lines more)", last + 1 - end) } else { text })
            }
            Subject::TerminalCommand { terminal, shell, command } => {
                let open = started(terminal, *shell)?;
                let ran = open.command(*command).filter(|c| c.output.is_some())?;
                let output = open.command_output(ran.id, TAGGED_OUTPUT).map(|(text, _)| text).unwrap_or_default();
                Some(if output.is_empty() { in_words(&ran) } else { format!("{}\n{output}", in_words(&ran)) })
            }
            _ => None,
        }
    }

    fn named(&self, words: &str) -> Option<Result<Subject, String>> {
        let rest = words.trim().strip_prefix("terminal ")?;
        let (who, what) = rest.split_once(' ')?;
        let (id, start) = match who.split_once('@') {
            Some((id, start)) => (id, Some(start)),
            None => (who, None),
        };
        let terminal = TerminalId::new(id);
        Some((|| {
            let shell = match start {
                Some(start) => start.parse().map_err(|_| format!("{start:?} isn't a start of a shell"))?,
                None => self.get(&terminal).map_err(|e| e.to_string())?.started(),
            };
            let number = |n: &str| n.trim().parse::<u64>().map_err(|_| format!("{n:?} isn't a number"));
            if let Some(command) = what.strip_prefix("command ") {
                return Ok(Subject::TerminalCommand { terminal, shell, command: number(command)? });
            }
            let lines = what.strip_prefix("lines ").or_else(|| what.strip_prefix("line ")).ok_or("a terminal's `command <n>`, `line <n>` or `lines <a>-<b>`")?;
            let (first, last) = match lines.split_once('-') {
                Some((a, b)) => (number(a)?, number(b)?),
                None => (number(lines)?, number(lines)?),
            };
            let (first, last) = (first.min(last), first.max(last));
            if last - first >= TAGGED_LINES {
                return Err(format!("at most {TAGGED_LINES} lines"));
            }
            Ok(Subject::TerminalLines { terminal, shell, first, last })
        })())
    }

    /// Lines laid out anew (a new width) since they were numbered: where they are now.
    fn moved(&self, subject: &Subject) -> Option<Subject> {
        let Subject::TerminalLines { terminal, shell, first, last } = subject else { return None };
        let open = self.get(terminal).ok().filter(|o| o.started() == *shell)?;
        let (now_first, now_last) = open.lines_now(*first, *last)?;
        ((now_first, now_last) != (*first, *last)).then(|| Subject::TerminalLines { terminal: terminal.clone(), shell: *shell, first: now_first, last: now_last })
    }
}

impl Terminals {
    /// A subject's name: in the shell's current start as it is, in another
    /// with which start.
    fn words(&self, terminal: &TerminalId, shell: u64, what: &str) -> String {
        if self.get(terminal).is_ok_and(|o| o.started() == shell) { format!("terminal {terminal} {what}") } else { format!("terminal {terminal}@{shell} {what}") }
    }
}

/// How much of a tagged command's output it says (its last lines), and
/// how many lines can be tagged at once (agents name at most these; of
/// more, what they say is their first these).
const TAGGED_OUTPUT: u64 = 20;
const TAGGED_LINES: u64 = 200;
