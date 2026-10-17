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

use harness_contracts::{ClientEvent, Found, Row, ScreenUpdate, TermSize, Terminal, TerminalId, TerminalInput, TerminalStatus};
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
    #[error("{0}")]
    Failed(String),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a terminal's shell did that someone may want to hear about
/// (notifications take it from here).
pub type Tell = Arc<dyn Fn(&Terminal, &Happening) + Send + Sync>;

pub struct Terminals {
    /// Where shells start (the project's folder), and where their start-up
    /// hooks are kept.
    folder: PathBuf,
    hooks: PathBuf,
    hub: Arc<EventHub>,
    state: Arc<nah_state::StateDb>,
    open: Mutex<BTreeMap<TerminalId, Arc<Open>>>,
    tell: Tell,
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
}

impl Terminals {
    /// The terminals kept from the last run (ended), told on `hub`.
    pub fn load(folder: PathBuf, hooks: PathBuf, hub: Arc<EventHub>, state: Arc<nah_state::StateDb>, tell: Tell) -> Result<Arc<Self>, nah_state::StateError> {
        let kept = state.terminals()?;
        let open = kept.into_iter().map(|t| (t.id.clone(), Arc::new(Open::new(t)))).collect();
        let terminals = Arc::new(Self { folder, hooks, hub, state, open: Mutex::new(open), tell });
        for t in terminals.list() {
            terminals.hub.publish(ClientEvent::TerminalUpdated { terminal: t });
        }
        Ok(terminals)
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
        self.state.forget_terminal(id).map_err(|e| TerminalError::Failed(e.to_string()))?;
        self.hub.publish(ClientEvent::TerminalClosed { terminal_id: id.clone() });
        Ok(())
    }

    /// Starts `open`'s shell, at its size, in its folder.
    fn start(self: &Arc<Self>, open: &Arc<Open>) -> Result<(), TerminalError> {
        let info = open.info();
        let options = Options { shell: info.shell.clone(), folder: PathBuf::from(&info.folder), size: info.size, hooks: self.hooks.clone(), history: HISTORY };
        let generation = open.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (terminals, weak) = (Arc::downgrade(self), Arc::downgrade(open));
        let shell = nah_terminal::Terminal::open(options, move |happening| {
            if let (Some(terminals), Some(open)) = (terminals.upgrade(), weak.upgrade())
                && open.generation.load(Ordering::SeqCst) == generation
            {
                terminals.happened(&open, &happening);
            }
        })
        .map_err(|e| TerminalError::Failed(format!("{}: {e}", info.shell)))?;
        {
            let mut info = lock(&open.info);
            info.status = TerminalStatus::Running;
            info.title = shell.title();
        }
        *lock(&open.shell) = Some(shell);
        open.bump();
        self.said(open);
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
        (self.tell)(&open.info(), happening);
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
        lock(&open.info).size = size;
        open.bump();
        self.said(open);
    }
}

impl Open {
    fn new(info: Terminal) -> Self {
        Self { info: Mutex::new(info), shell: Mutex::default(), changed: watch::channel(0).0, viewers: Mutex::default(), next_viewer: AtomicU64::new(1), generation: AtomicU64::new(0) }
    }

    pub fn info(&self) -> Terminal {
        lock(&self.info).clone()
    }

    fn bump(&self) {
        self.changed.send_modify(|n| *n += 1);
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
        match lock(&self.shell).as_ref() {
            Some(shell) => shell.screen(since),
            None => {
                let info = self.info();
                ScreenUpdate { seq: 0, size: info.size, full: since.is_none(), rows: Vec::new(), cursor: Default::default(), history: 0, alternate: false, title: info.title }
            }
        }
    }

    pub fn lines(&self, from: i64, count: u32) -> (i64, Vec<Row>) {
        lock(&self.shell).as_ref().map_or((from, Vec::new()), |s| s.lines(from, count))
    }

    pub fn search(&self, query: &str) -> Vec<Found> {
        lock(&self.shell).as_ref().map_or_else(Vec::new, |s| s.search(query))
    }

    /// The screen as text, with up to `back` lines of scrollback.
    pub fn text(&self, back: u32) -> String {
        lock(&self.shell).as_ref().map_or_else(String::new, |s| s.text(back))
    }

    /// Types `input` into its shell.
    pub fn input(&self, input: &TerminalInput) -> Result<(), TerminalError> {
        let shell = lock(&self.shell);
        let shell = shell.as_ref().filter(|s| s.ended().is_none()).ok_or_else(|| TerminalError::Ended(self.info().id))?;
        shell.input(input).map_err(|e| TerminalError::Failed(e.to_string()))
    }
}
