//! The changes view: what the desk's comparison changes, the diff run
//! together (files, then their changes). The comparison and its list are
//! the desk's (`Comparing`, shared: the code view and the graph read them
//! too); the view shows them and asks to compare something else.
//!
//! A file's changes are asked for when it's opened (a few small ones open
//! by themselves); the list is asked for again when files change, and an
//! opened file's changes when its counts do. The cursor walks files and
//! their changes; Enter opens the spot in the code view; `/` searches
//! file names and the changes' lines, removed ones too. One column (the
//! lines interleaved) or side by side, or by the panel's width.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use egui::RichText;
use harness_contracts::changes::{ChangeList, ChangedFile, Comparison, ComponentChangeList, Timeline, TurnChanges, Window, DiffLine, FileChanges, FileStatus, Group, Hunk, LineKind, Point};
use serde::{Deserialize, Serialize};

use crate::actions::{Action, ActionProvider, Choice, Group as ActionGroup, MINOR, Param, RunCx, SECONDARY, Style};
use crate::config::ChangesUi;
use crate::nav::{Ask, Fold, GoTo, Item, Motion, Move, Navigable, Place, Refresh, Spot, Step, Target};
use crate::viewport::{Go, Show, Stop, Viewport};
use crate::net::{NetClient, NetResult};
use crate::hosted::{Hosted, Status, Tone};
use crate::diff::{draw_gap, draw_line, paired};
use crate::theme::colors;

/// How a file's changes are laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLayout {
    /// Side by side when the column is wide enough, else one column.
    #[default]
    Auto,
    /// One column, removed and added lines interleaved.
    Unified,
    SideBySide,
}

impl DiffLayout {
    const ALL: [(Self, &'static str); 3] = [(Self::Auto, "Auto"), (Self::Unified, "One column"), (Self::SideBySide, "Side by side")];

    fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Unified => "unified",
            Self::SideBySide => "side_by_side",
        }
    }
}

/// A file in the list: its group and path.
type FileKey = (Group, String);

mod comparing;
pub use comparing::Comparing;

enum Reply {
    File(Comparison, FileKey, NetResult<FileChanges>),
    /// Staged or unstaged: read it all again (or say why not).
    Staged(NetResult<serde_json::Value>),
}

/// An opened file: how much context it asks for, and its changes once in.
#[derive(Default)]
struct Opened {
    context: u32,
    changes: Option<FileChanges>,
    loading: bool,
    error: Option<String>,
    /// Its counts when it was asked for (asked again when they change).
    asked_for: Option<(u32, u32)>,
}

/// A drawn row: a group's heading (staged, unstaged, untracked); a file;
/// a change's first line (`@@ … @@`); one of its lines (side by side, a
/// pair of them); on the timeline, a window (newest first). Every row but
/// a heading is a cursor stop.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Group(Group),
    File(usize),
    Hunk(usize, usize),
    Line(usize, usize, usize),
    Window(usize),
}

impl Row {
    fn stop(&self) -> bool {
        !matches!(self, Self::Group(_))
    }
}

/// A row's height by its kind, as last drawn (rows are laid out by them).
#[derive(Clone, Copy, Debug)]
struct Heights {
    group: f32,
    file: f32,
    hunk: f32,
    line: f32,
    window: f32,
}

impl Default for Heights {
    fn default() -> Self {
        Self { group: 22.0, file: 24.0, hunk: 18.0, line: 18.0, window: 26.0 }
    }
}

impl Heights {
    fn of(&self, row: &Row) -> f32 {
        match row {
            Row::Group(_) => self.group,
            Row::File(_) => self.file,
            Row::Hunk(..) => self.hunk,
            Row::Line(..) => self.line,
            Row::Window(_) => self.window,
        }
    }

    /// Taller than it was drawn: kept (so rows don't overlap next frame).
    fn grow(&mut self, row: &Row, drawn: f32) -> bool {
        let h = match row {
            Row::Group(_) => &mut self.group,
            Row::File(_) => &mut self.file,
            Row::Hunk(..) => &mut self.hunk,
            Row::Line(..) => &mut self.line,
            Row::Window(_) => &mut self.window,
        };
        let taller = drawn > *h + 0.5;
        if taller {
            *h = drawn;
        }
        taller
    }
}

#[derive(Clone, Debug)]
enum Command {
    Compare,
    Layout,
    Refresh,
    MoreContext,
    Timeline,
    Since,
    /// Stage (true) or unstage the cursor's file or change.
    Stage(bool),
}

/// The comparisons the picker offers (any other, a timeline window, shows
/// as itself).
fn offered() -> Vec<(Comparison, &'static str)> {
    vec![
        (Comparison::default(), "Uncommitted changes"),
        (Comparison { from: Point::Head, to: Point::Index }, "Staged (against the last commit)"),
        (Comparison { from: Point::Index, to: Point::Files }, "Unstaged (against what's staged)"),
        (Comparison { from: Point::Rev("HEAD~1".into()), to: Point::Head }, "The last commit"),
        (Comparison { from: Point::Rev("main".into()), to: Point::Files }, "Against main"),
    ]
}

fn compared(value: &str) -> Option<Comparison> {
    value.parse().ok()
}

pub struct ChangesView {
    /// The desk's comparison, and its list (as last read there).
    comparison: Comparison,
    list: Option<ChangeList>,
    pub layout: DiffLayout,
    pub ui: ChangesUi,
    /// What it took last from what the buffers share.
    seen: crate::shared::Seen,
    /// The desk reading the list, and why it couldn't.
    loading: bool,
    list_error: Option<String>,
    /// Why staging failed.
    error: Option<String>,
    /// What it asked of the rest of the cockpit, not taken yet.
    asks: Vec<Ask>,
    /// Files whose changes are to be asked for (when next drawn), with the
    /// lines of context around them.
    to_ask: Vec<(FileKey, u32)>,
    opened: BTreeMap<FileKey, Opened>,
    /// Files the user closed (not opened by themselves again).
    closed: std::collections::BTreeSet<FileKey>,
    /// Changes folded to their one line, by file and place in it.
    folded: std::collections::BTreeSet<(FileKey, usize)>,
    cursor: usize,
    /// A place to put the cursor back on once the list is in.
    restore: Option<String>,
    /// How to show the cursor once laid out (asked before it was).
    show: Option<Show>,
    /// Where it's looking (the shared rules move it), the time last drawn,
    /// rows' heights, and whether changes were last laid out side by side.
    viewport: Viewport,
    clock: f64,
    heights: Heights,
    side: bool,
    spots: Vec<Spot>,
    /// The read of the desk's list it took.
    read: u64,
    /// Which components the changes touch, and the read of them it took.
    components: Option<ComponentChangeList>,
    components_read: u64,
    /// A spot to put the cursor on once its file's changes are in (`g x`).
    revealing: Option<(FileKey, usize)>,
    /// When the files changed and who was working (the read of it it took),
    /// and whether the view shows it instead of the changes.
    timeline: Option<Timeline>,
    timeline_read: u64,
    pub on_timeline: bool,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

impl Default for ChangesView {
    fn default() -> Self {
        Self {
            comparison: Comparison::default(),
            list: None,
            layout: DiffLayout::default(),
            ui: ChangesUi::default(),
            seen: Default::default(),
            loading: false,
            list_error: None,
            error: None,
            asks: Vec::new(),
            to_ask: Vec::new(),
            opened: BTreeMap::new(),
            closed: Default::default(),
            folded: Default::default(),
            cursor: 0,
            restore: None,
            show: None,
            viewport: Viewport::default(),
            clock: 0.0,
            heights: Heights::default(),
            side: false,
            spots: Vec::new(),
            read: 0,
            components: None,
            components_read: 0,
            revealing: None,
            timeline: None,
            timeline_read: 0,
            on_timeline: false,
            inbox: Arc::default(),
        }
    }
}

fn push(inbox: &Arc<Mutex<Vec<Reply>>>, reply: Reply) {
    if let Ok(mut i) = inbox.lock() {
        i.push(reply);
    }
}

fn file_key(f: &ChangedFile) -> FileKey {
    (f.group, f.path.clone())
}

/// What happened to a file: its mark, colour and word.
fn status_look(status: FileStatus) -> (&'static str, egui::Color32, &'static str) {
    use egui_phosphor::regular as icon;
    let c = colors();
    match status {
        FileStatus::Added => (icon::PLUS_SQUARE, c.ok, "added"),
        FileStatus::Modified => (icon::PENCIL_SIMPLE, c.warn, "modified"),
        FileStatus::Deleted => (icon::MINUS_SQUARE, c.error, "deleted"),
        FileStatus::Renamed => (icon::ARROW_RIGHT, c.info, "renamed"),
        FileStatus::TypeChanged => (icon::SWAP, c.info, "type changed"),
    }
}

impl ChangesView {
    /// The desk compares `comparison` now: its files' changes start over.
    fn compared(&mut self, comparison: Comparison) {
        self.comparison = comparison;
        self.list = None;
        self.opened.clear();
        self.closed.clear();
        self.folded.clear();
        self.cursor = 0;
    }

    /// Asks the desk to compare `comparison`.
    fn compare(&mut self, comparison: Comparison) {
        if comparison != self.comparison {
            self.asks.push(Ask::Compare(comparison));
        }
    }

    /// The list in, if it's for the comparison in use.
    fn list(&self) -> Option<&ChangeList> {
        self.list.as_ref().filter(|l| l.comparison == self.comparison)
    }

    /// A file's changes, as the view has them (opened files only).
    pub fn file_changes(&self, group: Group, path: &str) -> Option<&FileChanges> {
        self.opened.get(&(group, path.to_owned())).and_then(|o| o.changes.as_ref())
    }

    /// The timeline's windows, newest first (as its rows are).
    fn windows(&self) -> Vec<&Window> {
        self.timeline.iter().flat_map(|t| t.windows.iter().rev()).collect()
    }

    fn rows(&self) -> Vec<Row> {
        if self.on_timeline {
            return (0..self.windows().len()).map(Row::Window).collect();
        }
        let Some(list) = self.list() else { return Vec::new() };
        let mut rows = Vec::new();
        let mut group = None;
        for (i, f) in list.files.iter().enumerate() {
            if list.comparison.is_uncommitted() && group != Some(f.group) {
                group = Some(f.group);
                rows.push(Row::Group(f.group));
            }
            rows.push(Row::File(i));
            if let Some(changes) = self.opened.get(&file_key(f)).and_then(|o| o.changes.as_ref()) {
                for (h, hunk) in changes.hunks.iter().enumerate() {
                    rows.push(Row::Hunk(i, h));
                    if self.folded.contains(&(file_key(f), h)) {
                        continue;
                    }
                    let lines = if self.side { paired(&hunk.lines).len() } else { hunk.lines.len() };
                    rows.extend((0..lines).map(|n| Row::Line(i, h, n)));
                }
            }
        }
        rows
    }

    /// Where the cursor can stop, laid out (each row's top and height), and
    /// which row each is.
    fn stops(&self, rows: &[Row]) -> (Vec<Stop>, Vec<usize>) {
        let mut top = 0.0;
        let mut out = (Vec::new(), Vec::new());
        for (at, row) in rows.iter().enumerate() {
            let h = self.heights.of(row);
            if row.stop() {
                out.0.push(Stop::new(top, h));
                out.1.push(at);
            }
            top += h;
        }
        out
    }

    /// Moves by the shared rules from where the cursor is.
    fn go(&mut self, go: Go) {
        // A key's move: what was asked before it is done with.
        self.show = None;
        let rows = self.rows();
        let (stops, at) = self.stops(&rows);
        let cursor = at.iter().position(|r| *r == self.cursor);
        if let Some(moved) = self.viewport.go(&stops, cursor, go) {
            self.cursor = at[moved.cursor];
            self.viewport.take(&moved, self.clock, true);
        }
    }

    /// The change a row is in (its file's index, the change's).
    fn hunk_of(row: &Row) -> Option<(usize, usize)> {
        match row {
            Row::Hunk(i, h) | Row::Line(i, h, _) => Some((*i, *h)),
            _ => None,
        }
    }

    /// The diff line a row is (one column: that line; side by side, the
    /// pair's new side, else its old).
    fn line_of(&self, row: &Row) -> Option<&DiffLine> {
        let Row::Line(_, h, n) = row else { return None };
        let file = self.file_of(row)?;
        let hunk = self.file_changes(file.group, &file.path)?.hunks.get(*h)?;
        if self.side { paired(&hunk.lines).get(*n).and_then(|(l, r)| r.or(*l)) } else { hunk.lines.get(*n) }
    }

    fn row_key(&self, row: &Row) -> String {
        if let Row::Window(w) = row {
            return self.windows().get(*w).map(|w| format!("w:{}", w.from)).unwrap_or_default();
        }
        let Some(list) = self.list() else { return String::new() };
        match row {
            Row::Window(_) => String::new(),
            Row::Group(g) => format!("g:{g:?}"),
            Row::File(i) => format!("f:{:?}:{}", list.files[*i].group, list.files[*i].path),
            Row::Hunk(i, h) => format!("h:{:?}:{}:{h}", list.files[*i].group, list.files[*i].path),
            Row::Line(i, h, n) => format!("l:{:?}:{}:{h}:{n}", list.files[*i].group, list.files[*i].path),
        }
    }

    fn cursor_row(&self) -> Option<Row> {
        self.rows().get(self.cursor).cloned()
    }

    /// The cursor on row `at` (a heading: the row after it), shown as
    /// asked once laid out.
    fn put_cursor(&mut self, at: usize, show: Show) {
        let rows = self.rows();
        let at = at.min(rows.len().saturating_sub(1));
        self.cursor = if rows.get(at).is_some_and(Row::stop) { at } else { (at + 1).min(rows.len().saturating_sub(1)) };
        self.show = Some(show);
    }

    /// The file a row is in.
    fn file_of(&self, row: &Row) -> Option<&ChangedFile> {
        let (Row::File(i) | Row::Hunk(i, _) | Row::Line(i, ..)) = row else { return None };
        self.list()?.files.get(*i)
    }

    /// A turn's changes, if the timeline saw it.
    fn turn(&self, turn: &str) -> Option<&TurnChanges> {
        self.timeline.as_ref()?.turn(turn)
    }

    /// Shows the timeline, or the changes.
    pub fn show_timeline(&mut self, on: bool) {
        if on != self.on_timeline {
            self.on_timeline = on;
            self.put_cursor(0, Show::InView);
        }
    }

    fn open(&mut self, key: FileKey) {
        self.closed.remove(&key);
        let context = self.opened.get(&key).map_or(self.ui.context, |o| o.context);
        self.ask_file(key, context);
    }

    /// Opens a file, its changes (with `context` lines around) asked for
    /// when next drawn.
    fn ask_file(&mut self, key: FileKey, context: u32) {
        let counts = self.list().and_then(|l| l.files.iter().find(|f| file_key(f) == key)).map(|f| (f.added, f.removed));
        let opened = self.opened.entry(key.clone()).or_default();
        opened.context = context;
        opened.loading = true;
        opened.asked_for = counts;
        self.to_ask.retain(|(k, _)| *k != key);
        self.to_ask.push((key, context));
    }

    fn close(&mut self, key: &FileKey) {
        self.opened.remove(key);
        self.closed.insert(key.clone());
    }

    fn toggle(&mut self, key: FileKey) {
        if self.opened.contains_key(&key) { self.close(&key) } else { self.open(key) }
    }

    /// The desk's list read again: its files' changes too.
    fn read_again(&mut self) {
        self.opened.values_mut().for_each(|o| o.asked_for = None);
        self.asks.push(Ask::Refresh(Refresh::Changes));
    }

    /// Folds or unfolds what the cursor is in: its change (on a change or
    /// one of its lines), or its file (on the file); or everything: every
    /// file folded, or every file and change unfolded.
    fn apply_fold(&mut self, fold: Fold) {
        let all: Vec<FileKey> = self.list().map(|l| l.files.iter().map(file_key).collect()).unwrap_or_default();
        let row = self.cursor_row();
        let file = row.as_ref().and_then(|r| self.file_of(r).map(file_key));
        let change = row.as_ref().and_then(Self::hunk_of).zip(file.clone()).map(|((_, h), k)| (k, h));
        match (fold, change, file) {
            (Fold::OpenAll, ..) => {
                self.folded.clear();
                for k in all.into_iter().filter(|k| !self.opened.contains_key(k)).collect::<Vec<_>>() {
                    self.open(k);
                }
            }
            (Fold::CloseAll, ..) => all.iter().for_each(|k| self.close(k)),
            (Fold::Toggle, Some(c), _) => {
                if !self.folded.remove(&c) {
                    self.folded.insert(c);
                }
            }
            (Fold::Open, Some(c), _) => {
                self.folded.remove(&c);
            }
            (Fold::Close, Some(c), _) => {
                self.folded.insert(c);
            }
            (Fold::Toggle, None, Some(k)) => self.toggle(k),
            (Fold::Open, None, Some(k)) if !self.opened.contains_key(&k) => self.open(k),
            (Fold::Close, None, Some(k)) => self.close(&k),
            _ => {}
        }
        // The cursor stays on what it was on, or on what folded it away:
        // its change, else its file.
        let Some(row) = row else { return };
        let rows = self.rows();
        let up = match &row {
            Row::Line(i, h, _) => vec![row.clone(), Row::Hunk(*i, *h), Row::File(*i)],
            Row::Hunk(i, _) => vec![row.clone(), Row::File(*i)],
            _ => vec![row.clone()],
        };
        if let Some(at) = up.iter().find_map(|r| rows.iter().position(|x| x == r)) {
            self.put_cursor(at, Show::InView);
        }
    }

    /// Whether it changed component `id`.
    fn touches(&self, id: &arch_graph::ComponentId) -> bool {
        self.components.as_ref().is_some_and(|l| l.components.iter().any(|c| c.component == id.as_str()))
    }

    /// Puts the cursor on component `id`'s first change (its file opened),
    /// once it's in. False when it changed nothing there.
    fn reveal(&mut self, id: &arch_graph::ComponentId) -> bool {
        let Some(changed) = self.components.as_ref().and_then(|l| l.components.iter().find(|c| c.component == id.as_str())) else { return false };
        let line = changed.runs.first().map_or(1, |(start, _)| (*start).max(1) as usize);
        // The file as it is now: its unstaged changes before its staged ones.
        let keys: Vec<FileKey> = self.list().map(|l| l.files.iter().filter(|f| f.path == changed.file).map(file_key).collect()).unwrap_or_default();
        let Some(key) = keys.iter().find(|(g, _)| *g != Group::Staged).or(keys.first()).cloned() else { return false };
        self.revealing = Some((key, line));
        true
    }

    /// The spot asked for by `reveal`: its file opened, then the cursor on
    /// the change at its line.
    fn go_to_revealed(&mut self) {
        let Some((key, line)) = self.revealing.clone() else { return };
        let Some(opened) = self.opened.get(&key) else {
            self.open(key);
            return;
        };
        let Some(changes) = &opened.changes else {
            if !opened.loading {
                self.revealing = None;
            }
            return;
        };
        let at = changes.hunks.iter().position(|h| h.new_range().contains(&(line as u32))).unwrap_or(0);
        let rows = self.rows();
        if let Some(row) = rows.iter().position(|r| matches!(r, Row::Hunk(_, h) if *h == at) && self.file_of(r).map(file_key).as_ref() == Some(&key)) {
            self.put_cursor(row, Show::Middle);
        }
        self.revealing = None;
    }

    /// The desk's list, read again: gone files go, opened ones whose
    /// counts moved are asked for again, a few small ones open by themselves.
    fn took_list(&mut self, list: ChangeList) {
        let keys: Vec<(FileKey, (u32, u32))> = list.files.iter().map(|f| (file_key(f), (f.added, f.removed))).collect();
        self.list = Some(list);
        // Gone files go; opened ones whose counts moved are asked again.
        self.opened.retain(|k, _| keys.iter().any(|(key, _)| key == k));
        for (key, counts) in &keys {
            if let Some(o) = self.opened.get(key)
                && o.asked_for != Some(*counts)
                && !o.loading
            {
                let context = o.context;
                self.ask_file(key.clone(), context);
            }
        }
        // A few small ones open by themselves: the diff run together.
        let lines: u32 = keys.iter().map(|(_, (a, r))| a + r).sum();
        if keys.len() <= self.ui.open_up_to_files && lines <= self.ui.open_up_to_lines {
            for (key, _) in keys {
                if !self.opened.contains_key(&key) && !self.closed.contains(&key) {
                    self.ask_file(key, self.ui.context);
                }
            }
        }
        if let Some(key) = self.restore.take() {
            match self.rows().iter().position(|r| self.row_key(r) == key) {
                Some(at) => self.put_cursor(at, Show::Middle),
                None => self.restore = Some(key),
            }
        }
    }

    /// Runs `change` keeping the cursor on the same row when it's still
    /// there (the list and files read again under it), else on the nearest
    /// stop.
    fn keeping_cursor(&mut self, change: impl FnOnce(&mut Self)) {
        let here = self.cursor_row().map(|r| self.row_key(&r));
        change(self);
        let rows = self.rows();
        // A line no longer there (laid out the other way, read again): its change.
        let change_of = |key: &str| key.strip_prefix("l:").and_then(|k| k.rsplit_once(':')).map(|(k, _)| format!("h:{k}"));
        let found = |key: &str| rows.iter().position(|r| self.row_key(r) == key);
        match here.and_then(|k| found(&k).or_else(|| change_of(&k).and_then(|h| found(&h)))) {
            Some(at) => self.cursor = at,
            None => self.cursor = self.cursor.min(rows.len().saturating_sub(1)),
        }
        if rows.get(self.cursor).is_some_and(|r| !r.stop()) {
            self.cursor = (self.cursor + 1).min(rows.len().saturating_sub(1));
        }
    }

    /// Whether files show side by side in a panel `width` wide.
    fn side_by_side(&self, width: f32) -> bool {
        match self.layout {
            DiffLayout::Auto => width >= self.ui.side_by_side_from,
            DiffLayout::Unified => false,
            DiffLayout::SideBySide => true,
        }
    }

    pub(crate) fn draw(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        let look = crate::theme::look();
        if self.on_timeline {
            if self.windows().is_empty() {
                let words = if self.timeline.is_some() { "Nothing yet: a snapshot is taken whenever a turn starts or ends." } else { "Reading the timeline…" };
                ui.label(RichText::new(words).size(look.text_body).color(colors().muted));
                self.spots.clear();
                return;
            }
            return self.render_rows(ui, ("timeline", cx.at));
        }
        let Some(list) = self.list() else {
            let words = match &self.list_error {
                Some(e) => e.clone(),
                None => "Reading the changes…".to_owned(),
            };
            ui.label(RichText::new(words).size(look.text_body).color(colors().muted));
            return;
        };
        if list.files.is_empty() {
            ui.label(RichText::new(format!("No changes: {}.", self.comparison)).size(look.text_body).color(colors().muted));
            self.spots.clear();
            return;
        }
        // Laid out side by side or not by the panel's width: the cursor
        // stays on its change when that changes the rows.
        let side = self.side_by_side(ui.available_width());
        if side != self.side {
            self.keeping_cursor(|v| v.side = side);
        }
        self.render_rows(ui, ("changes", cx.at));
    }

    /// The rows, laid out by their heights and drawn only where they show;
    /// where it looks is the shared rules' (a cursor asked for before it
    /// was laid out is shown now). The cursor's row is outlined.
    fn render_rows(&mut self, ui: &mut egui::Ui, id: (&str, crate::keymap::Host)) {
        let rows = self.rows();
        self.clock = ui.input(|i| i.time);
        self.viewport.margin = 2.0 * self.heights.line;
        if self.viewport.window.y > 0.0
            && let Some(show) = self.show.take()
        {
            let (stops, at) = self.stops(&rows);
            if let Some(stop) = at.iter().position(|r| *r == self.cursor)
                && let Some(moved) = self.viewport.go(&stops, Some(stop), Go::To(stop, show))
            {
                self.viewport.take(&moved, self.clock, false);
            }
        }
        let tops: Vec<f32> = rows.iter().scan(0.0, |top, row| {
            let at = *top;
            *top += self.heights.of(row);
            Some(at)
        }).collect();
        let total = tops.last().zip(rows.last()).map_or(0.0, |(t, r)| t + self.heights.of(r));
        let mut area = egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
        if let Some(offset) = self.viewport.wanted(self.clock) {
            area = area.vertical_scroll_offset(offset.y);
        }
        let (mut drawn, mut spots) = (Vec::new(), Vec::new());
        let output = area.show_viewport(ui, |ui, view| {
            ui.set_height(total);
            let (origin, width) = (ui.min_rect().min, ui.available_width());
            let first = tops.partition_point(|t| *t <= view.min.y).saturating_sub(1);
            for (at, row) in rows.iter().enumerate().skip(first).take_while(|(at, _)| tops[*at] < view.max.y) {
                let rect = egui::Rect::from_min_size(origin + egui::vec2(0.0, tops[at]), egui::vec2(width, self.heights.of(row)));
                let shown = ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| self.draw_row(ui, row));
                if at == self.cursor {
                    let look = crate::theme::look();
                    ui.painter().rect_stroke(rect, look.radius_s, egui::Stroke::new(look.line_thin * 1.5, colors().focus), egui::StrokeKind::Inside);
                }
                drawn.push((row.clone(), shown.response.rect.height()));
                if row.stop() {
                    spots.push(Spot { key: self.row_key(row), rect });
                }
            }
        });
        let grew = drawn.iter().fold(false, |grew, (row, h)| self.heights.grow(row, *h) | grew);
        self.viewport.drawn(output.state.offset, output.inner_rect.size(), output.content_size);
        self.spots = spots.into_iter().filter(|s| s.rect.intersects(output.inner_rect)).collect();
        if grew || self.viewport.gliding() || self.show.is_some() {
            ui.ctx().request_repaint();
        }
    }

    /// One row, in its own space.
    fn draw_row(&self, ui: &mut egui::Ui, row: &Row) {
        let look = crate::theme::look();
        let Some(list) = self.list().filter(|_| !self.on_timeline) else {
            if let Row::Window(w) = row
                && let Some(w) = self.windows().get(*w)
            {
                self.draw_window(ui, w);
            }
            return;
        };
        match row {
            Row::Group(g) => {
                let count = list.files.iter().filter(|f| f.group == *g).count();
                ui.add_space(look.space_s);
                ui.label(RichText::new(format!("{} · {count}", g.title())).strong().size(look.text_small).color(colors().muted));
            }
            Row::File(i) => self.draw_file(ui, &list.files[*i]),
            Row::Hunk(i, h) => {
                let file = &list.files[*i];
                let Some(hunk) = self.file_changes(file.group, &file.path).and_then(|c| c.hunks.get(*h)) else { return };
                let folded = self.folded.contains(&(file_key(file), *h));
                ui.horizontal(|ui| {
                    let caret = if folded { egui_phosphor::regular::CARET_RIGHT } else { egui_phosphor::regular::CARET_DOWN };
                    ui.label(RichText::new(caret).size(look.text_small).color(colors().muted));
                    if !folded {
                        ui.label(RichText::new(format!("@@ −{},{} +{},{} @@", hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines)).monospace().size(look.text_small).color(colors().faint));
                        // A new or deleted file's change is the whole of it: nothing around.
                        if !matches!(file.status, FileStatus::Added | FileStatus::Deleted) {
                            ui.label(RichText::new(around_words(hunk)).size(look.text_small).color(colors().muted));
                        }
                        return;
                    }
                    // Folded: where it is and how much changed.
                    let count = |kind: LineKind| hunk.lines.iter().filter(|l| l.kind == kind).count();
                    ui.label(RichText::new(format!("line {}", hunk.new_start.max(1))).monospace().size(look.text_small).color(colors().text));
                    ui.label(RichText::new(format!("+{}", count(LineKind::Added))).monospace().size(look.text_small).color(colors().ok));
                    ui.label(RichText::new(format!("−{}", count(LineKind::Removed))).monospace().size(look.text_small).color(colors().error));
                });
            }
            Row::Line(i, h, n) => {
                let Some(hunk) = self.file_changes(list.files[*i].group, &list.files[*i].path).and_then(|c| c.hunks.get(*h)) else { return };
                if !self.side {
                    if let Some(line) = hunk.lines.get(*n) {
                        draw_line(ui, line, true);
                    }
                    return;
                }
                if let Some((left, right)) = paired(&hunk.lines).get(*n).copied() {
                    ui.columns(2, |cols| {
                        match left {
                            Some(l) => draw_line(&mut cols[0], l, true),
                            None => draw_gap(&mut cols[0]),
                        }
                        match right {
                            Some(r) => draw_line(&mut cols[1], r, true),
                            None => draw_gap(&mut cols[1]),
                        }
                    });
                }
            }
            Row::Window(_) => {}
        }
    }

    /// A timeline window: when, how many files, who was working.
    fn draw_window(&self, ui: &mut egui::Ui, w: &Window) {
        let look = crate::theme::look();
        let now = crate::attention::now_ms();
        let compared = w.comparison() == self.comparison;
        ui.horizontal(|ui| {
            let when = match w.ended_ms {
                Some(end) => format!("{} → {}", crate::attention::age(w.started_ms, now), crate::attention::age(end, now)),
                None => format!("since {}", crate::attention::age(w.started_ms, now)),
            };
            ui.label(RichText::new(when).size(look.text_body).color(if compared { colors().accent } else { colors().text_strong }));
            let files = if w.files == 1 { "1 file".to_owned() } else { format!("{} files", w.files) };
            ui.label(RichText::new(files).monospace().size(look.text_small).color(if w.files > 0 { colors().ok } else { colors().muted }));
            let who = if w.working.is_empty() { "no session working".to_owned() } else { w.working.iter().map(|x| if x.slot.is_empty() { x.thread.clone() } else { x.slot.clone() }).collect::<Vec<_>>().join(", ") };
            ui.add(egui::Label::new(RichText::new(who).size(look.text_small).color(colors().muted)).truncate());
        });
    }

    fn draw_file(&self, ui: &mut egui::Ui, file: &ChangedFile) {
        let look = crate::theme::look();
        let opened = self.opened.get(&file_key(file));
        // One line (rows are laid out by height): a long path is cut.
        ui.horizontal(|ui| {
            let caret = if opened.is_some() { egui_phosphor::regular::CARET_DOWN } else { egui_phosphor::regular::CARET_RIGHT };
            ui.label(RichText::new(caret).size(look.text_body).color(colors().muted));
            let (mark, color, word) = status_look(file.status);
            ui.label(RichText::new(mark).size(look.text_body).color(color)).on_hover_text(word);
            ui.add(egui::Label::new(RichText::new(&file.path).monospace().size(look.text_body).color(colors().text_strong)).truncate());
            if let Some(old) = &file.old_path {
                ui.label(RichText::new(format!("← {old}")).monospace().size(look.text_small).color(colors().muted));
            }
            if file.binary {
                ui.label(RichText::new("binary").size(look.text_small).color(colors().muted));
            } else {
                ui.label(RichText::new(format!("+{}", file.added)).monospace().size(look.text_small).color(colors().ok));
                ui.label(RichText::new(format!("−{}", file.removed)).monospace().size(look.text_small).color(colors().error));
            }
            match opened {
                Some(o) if o.loading && o.changes.is_none() => {
                    ui.label(RichText::new(egui_phosphor::regular::HOURGLASS).color(colors().warn));
                }
                Some(Opened { error: Some(e), .. }) => {
                    ui.label(RichText::new(e).size(look.text_small).color(colors().error));
                }
                _ => {}
            }
        });
    }
}

impl Hosted for ChangesView {
    fn title(&self) -> Status {
        Status::new(format!("{} Changes", egui_phosphor::regular::GIT_DIFF), Tone::Plain)
    }

    fn busy(&self) -> Option<String> {
        self.loading.then(|| "Reading the changes".into())
    }

    /// What's compared, and how much changed.
    fn status(&self) -> Vec<Status> {
        let mut out = vec![Status::new(self.comparison.to_string(), Tone::Info)];
        if let Some(list) = self.list() {
            let (added, removed): (u32, u32) = list.files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
            out.push(Status::new(format!("{} files", list.total), Tone::Muted));
            out.push(Status::new(format!("+{added}"), Tone::Ok));
            out.push(Status::new(format!("−{removed}"), Tone::Error));
        }
        if let Some(e) = self.list_error.as_ref().or(self.error.as_ref()) {
            out.push(Status::new(e.clone(), Tone::Error));
        }
        out
    }
}

impl ChangesView {
    /// The shared moves where the cursor is, in the Changes view's words:
    /// through files and their changes, into the code (or, on the
    /// timeline, comparing a window), up to a change's file, folding files.
    fn moves(&self) -> Vec<Action> {
        let mut out = crate::nav::list_moves();
        let row = self.cursor_row();
        if let Some(Row::Window(_)) = row {
            out.push(Move::Enter.offer("Compare what changed then"));
            return out;
        }
        out.push(Move::Motion(Motion::NextSibling).offer("Next change"));
        out.push(Move::Motion(Motion::PrevSibling).offer("Previous change"));
        let file = row.as_ref().and_then(|r| self.file_of(r));
        // A folded file or change opens (as a double click does); else the code.
        out.push(match self.folded_here() {
            Some(what) => Move::Enter.offer(format!("Unfold {what}")),
            None => Move::Enter.offer_if(file.is_some_and(|f| f.status != FileStatus::Deleted), "Open it in the code"),
        });
        out.push(match row {
            Some(Row::Line(..)) => Move::Leave.offer("Up to its change"),
            Some(Row::Hunk(..)) => Move::Leave.offer("Up to its file"),
            _ => Move::Leave.offer_if(false, "Up to its change or file"),
        });
        let change = row.as_ref().and_then(Self::hunk_of).zip(file).map(|((_, h), f)| self.folded.contains(&(file_key(f), h)));
        out.extend(crate::nav::fold_here(match (change, file) {
            (Some(folded), _) => Some(("this change", folded)),
            (None, Some(f)) => Some((f.path.as_str(), !self.opened.contains_key(&file_key(f)))),
            _ => None,
        }));
        let closed = self.list().is_some_and(|l| l.files.iter().any(|f| !self.opened.contains_key(&file_key(f))));
        out.extend(crate::nav::fold_all((closed || !self.folded.is_empty(), "Unfold everything"), (!self.opened.is_empty(), "Fold everything")));
        out
    }
}

/// The unchanged lines shown before and after a change.
fn around(hunk: &Hunk) -> (usize, usize) {
    let before = hunk.lines.iter().take_while(|l| l.kind == LineKind::Context).count();
    let after = hunk.lines.iter().rev().take_while(|l| l.kind == LineKind::Context).count();
    if before == hunk.lines.len() { (before, 0) } else { (before, after) }
}

/// The unchanged lines around a change, in words.
fn around_words(hunk: &Hunk) -> String {
    let lines = |n: usize| if n == 1 { "1 unchanged line".to_owned() } else { format!("{n} unchanged lines") };
    match around(hunk) {
        (b, a) if b == a => format!("{} around", lines(b)),
        (b, a) => format!("{} before, {a} after", lines(b)),
    }
}

impl ChangesView {
    /// The file whose changes more lines can be shown around: the cursor's,
    /// open, edited (a new or deleted file's change is the whole of it),
    /// and not already shown whole (one change from its first line that
    /// stops short of the lines asked for after it: its end).
    fn more_context_here(&self) -> Option<FileKey> {
        let file = self.file_of(&self.cursor_row()?)?;
        if file.binary || !matches!(file.status, FileStatus::Modified | FileStatus::Renamed | FileStatus::TypeChanged) {
            return None;
        }
        let key = file_key(file);
        let opened = self.opened.get(&key)?;
        let hunks = &opened.changes.as_ref()?.hunks;
        let whole = match hunks.as_slice() {
            [only] => only.old_start <= 1 && only.new_start <= 1 && around(only).1 < opened.context as usize,
            _ => false,
        };
        (!hunks.is_empty() && !whole).then_some(key)
    }

    /// What's folded where the cursor is, in words: its file (on a folded
    /// file's row) or its change (on a folded change).
    fn folded_here(&self) -> Option<String> {
        let row = self.cursor_row()?;
        let file = self.file_of(&row)?;
        match row {
            Row::File(_) if !self.opened.contains_key(&file_key(file)) => Some(file.path.clone()),
            Row::Hunk(_, h) if self.folded.contains(&(file_key(file), h)) => Some("this change".into()),
            _ => None,
        }
    }
}

impl ActionProvider for ChangesView {
    fn actions(&self) -> Vec<Action> {
        let f = |id: &str, label: &str, c: Command| Action::new(id, label, c).focused();
        let mut choices: Vec<Choice> = offered().into_iter().map(|(c, label)| Choice::new(c.range(), label).current(c == self.comparison)).collect();
        if !offered().iter().any(|(c, _)| *c == self.comparison) {
            choices.push(Choice::new(self.comparison.range(), self.comparison.to_string()).current(true));
        }
        let layouts = DiffLayout::ALL.iter().map(|(l, label)| Choice::new(l.name(), *label).current(*l == self.layout)).collect();
        let mut out = self.moves();
        out.extend([
            f("changes.compare", "Compare what with what", Command::Compare).param(Param::Choice(choices)).header(Style::Setting, "Compare", None).group(ActionGroup::View).importance(SECONDARY),
            f("changes.layout", "Lay changes out", Command::Layout).param(Param::Choice(layouts)).header(Style::Setting, "Layout", None).group(ActionGroup::View).importance(MINOR),
            f("changes.refresh", "Read the changes again", Command::Refresh).header(Style::Button, "", Some(egui_phosphor::regular::ARROW_CLOCKWISE)).group(ActionGroup::View).importance(MINOR),
        ]);
        out.push(f("changes.timeline", "Show when the files changed and who was working", Command::Timeline).param(Param::Toggle { on: self.on_timeline }).header(Style::Setting, "Timeline", None).group(ActionGroup::View).importance(SECONDARY));
        // Staging: the user's own act, on the uncommitted changes.
        if let Some(row) = self.cursor_row().filter(|_| self.comparison.is_uncommitted())
            && let Some(stage) = self.file_of(&row).and_then(|f| f.group.staging())
        {
            let (id, label, caption) = if stage { ("changes.stage", "Stage it", "Stage") } else { ("changes.unstage", "Unstage it", "Unstage") };
            out.push(f(id, label, Command::Stage(stage)).on_item(Style::Button, caption, None).group(ActionGroup::Act).importance(SECONDARY));
        }
        if let Some(Row::Window(_)) = self.cursor_row() {
            out.push(f("changes.since", "Compare from here to the files now", Command::Since).on_item(Style::Button, "Since here", None).group(ActionGroup::View).importance(MINOR));
        } else if self.more_context_here().is_some() {
            out.push(f("changes.more_context", &format!("Show {} more lines around its changes", self.ui.more_context), Command::MoreContext).on_item(Style::Button, "More context", None).group(ActionGroup::View).importance(MINOR));
        }
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        match action.command::<Command>() {
            Some(Command::Compare) => match value.as_deref().and_then(compared) {
                Some(c) => self.compare(c),
                None => *cx.note = Some("That isn't a comparison.".into()),
            },
            Some(Command::Layout) => {
                if let Some(layout) = DiffLayout::ALL.iter().find(|(l, _)| Some(l.name()) == value.as_deref()) {
                    self.layout = layout.0;
                }
            }
            Some(Command::Timeline) => {
                let on = match value.as_deref() {
                    Some("true") => true,
                    Some("false") => false,
                    _ => !self.on_timeline,
                };
                self.show_timeline(on);
            }
            Some(Command::Stage(stage)) => {
                let Some(row) = self.cursor_row() else { return };
                let Some(file) = self.file_of(&row).cloned() else { return };
                let hunk = Self::hunk_of(&row).and_then(|(_, h)| self.opened.get(&file_key(&file)).and_then(|o| Some((o.changes.as_ref()?.hunks.get(h)?.clone(), o.context)))).map(|(h, context)| (h.old_start, h.new_start, context));
                // A whole file: the cursor follows it to its new group.
                if hunk.is_none() {
                    let to = if *stage { Group::Staged } else if file.status == FileStatus::Added && file.old_path.is_none() { Group::Untracked } else { Group::Unstaged };
                    self.restore = Some(format!("f:{to:?}:{}", file.path));
                }
                let inbox = self.inbox.clone();
                cx.net.change_stage(*stage, &file.path, hunk, cx.ctx.clone(), move |r| push(&inbox, Reply::Staged(r)));
            }
            Some(Command::Since) => {
                if let Some(Row::Window(w)) = self.cursor_row()
                    && let Some(from) = self.windows().get(w).map(|w| w.from.clone())
                {
                    self.compare(Comparison { from: Point::Snapshot(from), to: Point::Files });
                    self.show_timeline(false);
                }
            }
            Some(Command::Refresh) => self.read_again(),
            Some(Command::MoreContext) => match self.more_context_here() {
                Some(key) => {
                    let context = self.opened.get(&key).map_or(self.ui.context, |o| o.context) + self.ui.more_context;
                    self.ask_file(key, context);
                }
                None => *cx.note = Some("There's nothing more to show around these changes.".into()),
            },
            None => {}
        }
    }
}

impl crate::buffer::Buffer for ChangesView {
    fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, _drawing: &mut crate::buffer::Drawing<'_>) {
        self.draw(ui, cx);
    }

    /// Applies replies (its files' changes, staging), lands on what was
    /// revealed, and asks for the files' changes asked for.
    fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::Staged(result) => {
                    self.read_again();
                    self.error = result.err().map(String::from);
                }
                Reply::File(asked, key, result) if asked == self.comparison => self.keeping_cursor(|v| {
                    if let Some(opened) = v.opened.get_mut(&key) {
                        opened.loading = false;
                        match result {
                            Ok(changes) => {
                                // Its changes cut differently: what was folded is no longer where it was.
                                if opened.changes.as_ref().is_some_and(|c| c.hunks.len() != changes.hunks.len()) {
                                    v.folded.retain(|(k, _)| *k != key);
                                }
                                opened.changes = Some(changes);
                                opened.error = None;
                            }
                            Err(e) => opened.error = Some(e.into()),
                        }
                    }
                }),
                _ => {}
            }
        }
        self.go_to_revealed();
        for (key, context) in std::mem::take(&mut self.to_ask) {
            let (inbox, comparison) = (self.inbox.clone(), self.comparison.clone());
            let (asked, path, group) = (comparison.clone(), key.1.clone(), key.0);
            net.change_file(&comparison, &path, group, context, ctx.clone(), move |r| push(&inbox, Reply::File(asked, key, r)));
        }
    }


    /// How its changes are laid out.
    fn keep(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self.layout).ok()
    }

    fn restore_kept(&mut self, kept: &serde_json::Value) {
        if let Ok(layout) = serde_json::from_value(kept.clone()) {
            self.layout = layout;
        }
    }

    fn describe(&self) -> String {
        self.comparison.to_string()
    }
}

impl Navigable for ChangesView {
    /// Its settings, and the desk's comparison: its list (read again: the
    /// cursor stays on its row), the components it touches, the timeline.
    fn share(&mut self, shared: &crate::shared::Shared) {
        if let Some(settings) = shared.settings.since(&mut self.seen.settings) {
            self.ui = settings.changes.clone();
        }
        let desk = &shared.changes;
        if *desk.comparison() != self.comparison {
            self.compared(desk.comparison().clone());
        }
        if desk.read() != self.read {
            self.read = desk.read();
            if let Some(list) = desk.list().cloned() {
                self.keeping_cursor(|v| v.took_list(list));
            }
        }
        let (components, read) = desk.components();
        if read != self.components_read {
            (self.components, self.components_read) = (components.cloned(), read);
        }
        let (timeline, read) = desk.timeline();
        if read != self.timeline_read {
            (self.timeline, self.timeline_read) = (timeline.cloned(), read);
        }
        self.loading = desk.loading();
        if self.list_error.as_deref() != desk.error() {
            self.list_error = desk.error().map(str::to_owned);
        }
    }

    fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    /// A message whose turn the timeline saw, or a component the changes
    /// touch (a line whose component isn't known yet may be one); not a
    /// change: that's what it shows.
    fn can_show(&self, target: &Target) -> bool {
        match target {
            Target::Message { turn, .. } => turn.as_deref().is_some_and(|t| self.turn(t).is_some()),
            Target::Change { .. } => false,
            other => other.in_component() && other.component().is_none_or(|id| self.touches(id)),
        }
    }

    /// A message: its turn's changes, compared. A component: its first
    /// change.
    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        if let Target::Message { turn, .. } = target {
            let comparison = turn.as_deref().and_then(|t| self.turn(t)).map(|t| t.comparison.clone()).ok_or("The timeline didn't see this turn's start.")?;
            self.compare(comparison);
            self.show_timeline(false);
            return Ok(());
        }
        let id = target.component().ok_or("There's no component here.")?;
        if self.reveal(id) { Ok(()) } else { Err(format!("The changes ({}) don't touch {}.", self.comparison, crate::nav::short_name(id))) }
    }

    fn place(&self) -> Place {
        Place::of(&self.cursor_row().map(|r| self.row_key(&r)))
    }

    fn place_in_words(&self, place: &Place) -> String {
        place.read::<Option<String>>().flatten().unwrap_or_default()
    }

    fn restore(&mut self, place: &Place) {
        if let Some(Some(key)) = place.read::<Option<String>>() {
            match self.rows().iter().position(|r| self.row_key(r) == key) {
                Some(at) => self.put_cursor(at, Show::Middle),
                None => self.restore = Some(key),
            }
        }
    }

    /// A line: its place in the file (a removed one, where it was). A
    /// file or a change: its first changed line.
    fn target(&self) -> Option<Target> {
        let row = self.cursor_row()?;
        let file = self.file_of(&row)?;
        if let Some(line) = self.line_of(&row) {
            return Some(Target::Change { file: file.path.clone(), line: line.new.or(line.old).unwrap_or(1) as usize });
        }
        let hunks = self.file_changes(file.group, &file.path).map(|c| c.hunks.as_slice()).unwrap_or_default();
        let hunk = match row {
            Row::Hunk(_, h) => hunks.get(h),
            _ => hunks.first(),
        };
        let line = hunk.map_or(1, |h| h.lines.iter().find(|l| l.kind != LineKind::Context).and_then(|l| l.new.or(l.old)).unwrap_or(h.new_start.max(1)));
        Some(Target::Change { file: file.path.clone(), line: line as usize })
    }

    /// By the shared rules, a line at a time; `]]` / `[[` from change to
    /// change.
    fn motion(&mut self, motion: Motion, count: usize) {
        let rows = self.rows();
        let change = |r: &Row| matches!(r, Row::Hunk(..));
        match motion {
            Motion::Down => self.go(Go::Down(count)),
            Motion::Up => self.go(Go::Up(count)),
            Motion::HalfDown => self.go(Go::HalfDown(count)),
            Motion::HalfUp => self.go(Go::HalfUp(count)),
            Motion::First => self.go(Go::First),
            Motion::Last => self.go(Go::Last),
            Motion::Line(i) => self.put_cursor(i.saturating_sub(1), Show::Middle),
            Motion::NextSibling | Motion::PrevSibling => {
                let forward = motion == Motion::NextSibling;
                let mut at = Some(self.cursor);
                for _ in 0..count {
                    at = at.and_then(|from| if forward { rows.iter().enumerate().skip(from + 1).find(|(_, r)| change(r)).map(|(i, _)| i) } else { rows.iter().enumerate().take(from).rev().find(|(_, r)| change(r)).map(|(i, _)| i) }).or(at);
                }
                if let Some(at) = at.filter(|a| *a != self.cursor) {
                    let (_, stops) = self.stops(&rows);
                    if let Some(stop) = stops.iter().position(|r| *r == at) {
                        self.go(Go::To(stop, Show::InView));
                    }
                }
            }
            Motion::Left | Motion::Right | Motion::HalfLeft | Motion::HalfRight | Motion::FewLeft | Motion::FewRight => {}
        }
    }

    /// Opens a folded file or change; else the spot in the code view; on
    /// the timeline, compares the window.
    fn enter(&mut self) -> Step {
        if let Some(Row::Window(w)) = self.cursor_row() {
            let Some(comparison) = self.windows().get(w).map(|w| w.comparison()) else { return Step::Nothing("No window here.".into()) };
            self.compare(comparison);
            self.show_timeline(false);
            return Step::Done;
        }
        // A folded file or change: it opens.
        if self.folded_here().is_some() {
            self.apply_fold(Fold::Open);
            return Step::Done;
        }
        match self.cursor_row().and_then(|r| self.file_of(&r).cloned()) {
            Some(f) if f.status == FileStatus::Deleted => Step::Nothing(format!("{} was deleted: there's nothing to open.", f.path)),
            Some(_) => Step::GoTo(GoTo::Code),
            None => Step::Nothing("No changes here.".into()),
        }
    }

    /// Up: from a line to its change, from a change to its file.
    fn leave(&mut self) -> Step {
        let up = match self.cursor_row() {
            Some(Row::Line(i, h, _)) => Row::Hunk(i, h),
            Some(Row::Hunk(i, _)) => Row::File(i),
            _ => return Step::Nothing("Already on a file.".into()),
        };
        if let Some(at) = self.rows().iter().position(|r| *r == up) {
            let (_, stops) = self.stops(&self.rows());
            if let Some(stop) = stops.iter().position(|r| *r == at) {
                self.go(Go::To(stop, Show::InView));
            }
        }
        Step::Done
    }

    fn fold(&mut self, fold: Fold) {
        self.apply_fold(fold);
    }

    fn items(&self) -> Vec<Item> {
        let windows = self.windows();
        let list = self.list();
        self.rows()
            .iter()
            .filter_map(|row| {
                let (name, text) = match row {
                    Row::Group(_) => return None,
                    Row::Window(w) => (String::new(), windows.get(*w).map(|w| w.working.iter().map(|x| format!("{} {}", x.slot, x.thread)).collect::<Vec<_>>().join(" ")).unwrap_or_default()),
                    Row::File(i) => (list?.files[*i].path.clone(), list?.files[*i].path.clone()),
                    Row::Hunk(..) => (String::new(), String::new()),
                    // Each line found where it is, removed ones too.
                    Row::Line(..) => (String::new(), self.line_of(row).map(|l| l.text.clone()).unwrap_or_default()),
                };
                Some(Item::new(self.row_key(row), name, text))
            })
            .collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.cursor_row().map(|r| self.row_key(&r))
    }

    fn move_to(&mut self, key: &str) {
        if let Some(at) = self.rows().iter().position(|r| self.row_key(r) == key) {
            self.put_cursor(at, Show::Middle);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
