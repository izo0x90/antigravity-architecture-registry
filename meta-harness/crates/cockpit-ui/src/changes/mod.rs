//! The changes view: what the desk's comparison changes, the diff run
//! together (files, then their changes). The comparison is the desk's one
//! in use (the code view and the graph read it too); the view holds it.
//!
//! A file's changes are asked for when it's opened (a few small ones open
//! by themselves); the list is asked for again when files change, and an
//! opened file's changes when its counts do. The cursor walks files and
//! their changes; Enter opens the spot in the code view; `/` searches
//! file names and the changes' lines, removed ones too. One column (the
//! lines interleaved) or side by side, or by the column's width.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use egui::RichText;
use harness_contracts::changes::{ChangeList, ChangedFile, Comparison, ComponentChangeList, ComponentChanges, DiffLine, FileChanges, FileStatus, Group, Hunk, LineKind, Point};
use serde::{Deserialize, Serialize};

use crate::actions::{Action, ActionProvider, Choice, Group as ActionGroup, MINOR, Param, RunCx, SECONDARY, Style};
use crate::config::ChangesUi;
use crate::nav::{Fold, GoTo, Item, Motion, Navigable, Place, Spot, Step, Target};
use crate::net::{NetClient, NetResult};
use crate::panel::{Panel, Status, Tone};
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

enum Reply {
    List(Comparison, NetResult<ChangeList>),
    File(Comparison, FileKey, NetResult<FileChanges>),
    Components(Comparison, NetResult<ComponentChangeList>),
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

/// A cursor stop: a file, or one of its changes.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    File(usize),
    Hunk(usize, usize),
}

#[derive(Clone, Debug)]
enum Command {
    Compare,
    Layout,
    Refresh,
    MoreContext,
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
    /// The desk's comparison in use.
    pub comparison: Comparison,
    pub layout: DiffLayout,
    pub ui: ChangesUi,
    list: Option<ChangeList>,
    loading: bool,
    error: Option<String>,
    /// Asked for again (files changed, or the comparison did).
    stale: bool,
    opened: BTreeMap<FileKey, Opened>,
    /// Files the user closed (not opened by themselves again).
    closed: std::collections::BTreeSet<FileKey>,
    cursor: usize,
    /// A place to put the cursor back on once the list is in.
    restore: Option<String>,
    /// A fold asked for (done when the network's at hand: opening asks).
    folding: Option<Fold>,
    scroll: bool,
    spots: Vec<Spot>,
    drawing: Vec<Spot>,
    view_h: f32,
    /// When the list was last asked for (it's asked again now and then
    /// while shown: the index and staging change outside the files).
    asked_at: f64,
    /// How many times the list was read (views that show the changes too
    /// ask for theirs again when it is).
    read: u64,
    /// Which components it changed (asked for each time the list is read),
    /// and how many times that was read.
    components: Option<ComponentChangeList>,
    components_read: u64,
    /// A spot to put the cursor on once its file's changes are in (`g x`).
    revealing: Option<(FileKey, usize)>,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

impl Default for ChangesView {
    fn default() -> Self {
        Self {
            comparison: Comparison::default(),
            layout: DiffLayout::default(),
            ui: ChangesUi::default(),
            list: None,
            loading: false,
            error: None,
            stale: true,
            opened: BTreeMap::new(),
            closed: Default::default(),
            cursor: 0,
            restore: None,
            folding: None,
            scroll: false,
            spots: Vec::new(),
            drawing: Vec::new(),
            view_h: 0.0,
            asked_at: f64::NEG_INFINITY,
            read: 0,
            components: None,
            components_read: 0,
            revealing: None,
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
    /// Asks for everything again (files changed).
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Compares `comparison` from now on (the files' changes asked afresh).
    pub fn compare(&mut self, comparison: Comparison) {
        if comparison != self.comparison {
            self.comparison = comparison;
            self.list = None;
            self.opened.clear();
            self.closed.clear();
            self.cursor = 0;
            self.stale = true;
        }
    }

    /// The list in, if it's for the comparison in use.
    pub fn list(&self) -> Option<&ChangeList> {
        self.list.as_ref().filter(|l| l.comparison == self.comparison)
    }

    /// A file's changes, as the view has them (opened files only).
    pub fn file_changes(&self, group: Group, path: &str) -> Option<&FileChanges> {
        self.opened.get(&(group, path.to_owned())).and_then(|o| o.changes.as_ref())
    }

    fn rows(&self) -> Vec<Row> {
        let Some(list) = self.list() else { return Vec::new() };
        let mut rows = Vec::new();
        for (i, f) in list.files.iter().enumerate() {
            rows.push(Row::File(i));
            if let Some(changes) = self.opened.get(&file_key(f)).and_then(|o| o.changes.as_ref()) {
                rows.extend((0..changes.hunks.len()).map(|h| Row::Hunk(i, h)));
            }
        }
        rows
    }

    fn row_key(&self, row: &Row) -> String {
        let Some(list) = self.list() else { return String::new() };
        match row {
            Row::File(i) => format!("f:{:?}:{}", list.files[*i].group, list.files[*i].path),
            Row::Hunk(i, h) => format!("h:{:?}:{}:{h}", list.files[*i].group, list.files[*i].path),
        }
    }

    fn cursor_row(&self) -> Option<Row> {
        self.rows().get(self.cursor).cloned()
    }

    fn put_cursor(&mut self, at: usize) {
        self.cursor = at.min(self.rows().len().saturating_sub(1));
        self.scroll = true;
    }

    /// The file a row is in.
    fn file_of(&self, row: &Row) -> Option<&ChangedFile> {
        let (Row::File(i) | Row::Hunk(i, _)) = row;
        self.list()?.files.get(*i)
    }

    fn open(&mut self, key: FileKey, net: &NetClient, ctx: &egui::Context) {
        self.closed.remove(&key);
        let context = self.opened.get(&key).map_or(self.ui.context, |o| o.context);
        self.ask_file(key, context, net, ctx);
    }

    fn ask_file(&mut self, key: FileKey, context: u32, net: &NetClient, ctx: &egui::Context) {
        let counts = self.list().and_then(|l| l.files.iter().find(|f| file_key(f) == key)).map(|f| (f.added, f.removed));
        let opened = self.opened.entry(key.clone()).or_default();
        opened.context = context;
        opened.loading = true;
        opened.asked_for = counts;
        let (inbox, comparison) = (self.inbox.clone(), self.comparison.clone());
        let (asked, path, group) = (comparison.clone(), key.1.clone(), key.0);
        net.change_file(&comparison, &path, group, context, ctx.clone(), move |r| push(&inbox, Reply::File(asked, key, r)));
    }

    fn close(&mut self, key: &FileKey) {
        self.opened.remove(key);
        self.closed.insert(key.clone());
    }

    fn toggle(&mut self, key: FileKey, net: &NetClient, ctx: &egui::Context) {
        if self.opened.contains_key(&key) { self.close(&key) } else { self.open(key, net, ctx) }
    }

    /// Applies replies; asks for the list when it's stale (or now and then
    /// while shown), and an opened file's changes when its counts changed.
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context, shown: bool) {
        let now = ctx.input(|i| i.time);
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::List(asked, result) if asked == self.comparison => {
                    self.loading = false;
                    match result {
                        Ok(list) => self.took_list(list, net, ctx),
                        Err(e) => self.error = Some(e.into()),
                    }
                }
                Reply::Components(asked, result) if asked == self.comparison => {
                    // Not there (not a repository, an older server): none.
                    self.components = result.ok();
                    self.components_read += 1;
                }
                Reply::File(asked, key, result) if asked == self.comparison => {
                    if let Some(opened) = self.opened.get_mut(&key) {
                        opened.loading = false;
                        match result {
                            Ok(changes) => {
                                opened.changes = Some(changes);
                                opened.error = None;
                            }
                            Err(e) => opened.error = Some(e.into()),
                        }
                    }
                }
                _ => {}
            }
        }
        self.go_to_revealed(net, ctx);
        if let Some(fold) = self.folding.take() {
            self.apply_fold(fold, net, ctx);
        }
        let due = shown && now - self.asked_at >= self.ui.refresh_s;
        if (self.stale || due) && !self.loading {
            self.stale = false;
            self.loading = true;
            self.asked_at = now;
            let (inbox, comparison) = (self.inbox.clone(), self.comparison.clone());
            let asked = comparison.clone();
            net.changes(&comparison, 0, nah_page(), ctx.clone(), move |r| push(&inbox, Reply::List(asked, r)));
        }
        if shown {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(self.ui.refresh_s.max(0.5)));
        }
    }

    /// Opens or closes the cursor's file, or every file.
    fn apply_fold(&mut self, fold: Fold, net: &NetClient, ctx: &egui::Context) {
        let all: Vec<FileKey> = self.list().map(|l| l.files.iter().map(file_key).collect()).unwrap_or_default();
        let here = self.cursor_row().and_then(|r| self.file_of(&r).map(file_key));
        match (fold, here.clone()) {
            (Fold::OpenAll, _) => {
                for k in all.into_iter().filter(|k| !self.opened.contains_key(k)).collect::<Vec<_>>() {
                    self.open(k, net, ctx);
                }
            }
            (Fold::CloseAll, _) => all.iter().for_each(|k| self.close(k)),
            (Fold::Toggle, Some(k)) => self.toggle(k, net, ctx),
            (Fold::Open, Some(k)) if !self.opened.contains_key(&k) => self.open(k, net, ctx),
            (Fold::Close, Some(k)) => self.close(&k),
            _ => {}
        }
        // Its file folded: the cursor goes to the file's row.
        let rows = self.rows();
        let file_row = here.as_ref().and_then(|k| rows.iter().position(|r| matches!(r, Row::File(_)) && self.file_of(r).map(file_key).as_ref() == Some(k)));
        match (here, file_row) {
            (Some(k), Some(at)) if !self.opened.contains_key(&k) => self.cursor = at,
            _ => self.cursor = self.cursor.min(rows.len().saturating_sub(1)),
        }
    }

    /// How many times the list was read.
    pub fn read(&self) -> u64 {
        self.read
    }

    /// Which components it changed, as last read, and how many times that
    /// was read.
    pub fn components(&self) -> (Option<&ComponentChangeList>, u64) {
        (self.components.as_ref(), self.components_read)
    }

    /// Whether it changed component `id`.
    pub fn touches(&self, id: &arch_graph::ComponentId) -> bool {
        self.components.as_ref().is_some_and(|l| l.components.iter().any(|c| c.component == id.as_str()))
    }

    /// Puts the cursor on component `id`'s first change (its file opened),
    /// once it's in. False when it changed nothing there.
    pub fn reveal(&mut self, id: &arch_graph::ComponentId) -> bool {
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
    fn go_to_revealed(&mut self, net: &NetClient, ctx: &egui::Context) {
        let Some((key, line)) = self.revealing.clone() else { return };
        let Some(opened) = self.opened.get(&key) else {
            self.open(key, net, ctx);
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
            self.put_cursor(row);
        }
        self.revealing = None;
    }

    /// What it changed in each component.
    pub fn by_component(&self) -> BTreeMap<arch_graph::ComponentId, ComponentChanges> {
        self.components.iter().flat_map(|l| &l.components).map(|c| (arch_graph::ComponentId::new(&c.component), c.clone())).collect()
    }

    fn took_list(&mut self, list: ChangeList, net: &NetClient, ctx: &egui::Context) {
        self.error = None;
        self.read += 1;
        let (inbox, asked) = (self.inbox.clone(), list.comparison.clone());
        net.change_components(&list.comparison, ctx.clone(), move |r| push(&inbox, Reply::Components(asked, r)));
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
                self.ask_file(key.clone(), context, net, ctx);
            }
        }
        // A few small ones open by themselves: the diff run together.
        let lines: u32 = keys.iter().map(|(_, (a, r))| a + r).sum();
        if keys.len() <= self.ui.open_up_to_files && lines <= self.ui.open_up_to_lines {
            for (key, _) in keys {
                if !self.opened.contains_key(&key) && !self.closed.contains(&key) {
                    self.ask_file(key, self.ui.context, net, ctx);
                }
            }
        }
        if let Some(key) = self.restore.take() {
            match self.rows().iter().position(|r| self.row_key(r) == key) {
                Some(at) => self.put_cursor(at),
                None => self.restore = Some(key),
            }
        }
        self.cursor = self.cursor.min(self.rows().len().saturating_sub(1));
    }

    /// The list, as if it came from the server (tests drive the view
    /// without one).
    #[cfg(test)]
    pub(crate) fn take_list_for_test(&mut self, list: ChangeList) {
        self.comparison = list.comparison.clone();
        let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
        self.took_list(list, &net, &ctx);
    }

    /// Whether files show side by side in a column `width` wide.
    fn side_by_side(&self, width: f32) -> bool {
        match self.layout {
            DiffLayout::Auto => width >= self.ui.side_by_side_from,
            DiffLayout::Unified => false,
            DiffLayout::SideBySide => true,
        }
    }

    pub fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        let look = crate::theme::look();
        let Some(list) = self.list().cloned() else {
            let words = match &self.error {
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
        let side = self.side_by_side(ui.available_width());
        let rows = self.rows();
        let (cursor, scroll) = (self.cursor, std::mem::take(&mut self.scroll));
        self.drawing.clear();
        let output = egui::ScrollArea::vertical().id_salt(("changes", cx.slot)).auto_shrink([false, false]).show(ui, |ui| {
            let mut group = None;
            for (at, row) in rows.iter().enumerate() {
                let on = at == cursor;
                let file = &list.files[match row {
                    Row::File(i) | Row::Hunk(i, _) => *i,
                }];
                if let Row::File(_) = row
                    && group != Some(file.group)
                {
                    group = Some(file.group);
                    if list.comparison.is_uncommitted() {
                        let count = list.files.iter().filter(|f| f.group == file.group).count();
                        ui.add_space(look.space_s);
                        ui.label(RichText::new(format!("{} · {count}", file.group.title())).strong().size(look.text_small).color(colors().muted));
                    }
                }
                let fill = if on { colors().selection } else { egui::Color32::TRANSPARENT };
                let shown = egui::Frame::NONE.fill(fill).corner_radius(look.radius_s).inner_margin(crate::theme::margin2(look.space_xs, look.space_hair)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match row {
                        Row::File(_) => self.draw_file(ui, file),
                        Row::Hunk(_, h) => {
                            if let Some(hunk) = self.file_changes(file.group, &file.path).and_then(|c| c.hunks.get(*h)) {
                                draw_hunk(ui, hunk, side);
                            }
                        }
                    }
                });
                if on && scroll {
                    shown.response.scroll_to_me(Some(egui::Align::Center));
                }
                self.drawing.push(Spot { key: self.row_key(row), rect: shown.response.rect });
            }
        });
        self.view_h = output.inner_rect.height();
        self.spots = std::mem::take(&mut self.drawing).into_iter().filter(|s| s.rect.intersects(output.inner_rect)).collect();
    }

    fn draw_file(&self, ui: &mut egui::Ui, file: &ChangedFile) {
        let look = crate::theme::look();
        let opened = self.opened.get(&file_key(file));
        ui.horizontal_wrapped(|ui| {
            let caret = if opened.is_some() { egui_phosphor::regular::CARET_DOWN } else { egui_phosphor::regular::CARET_RIGHT };
            ui.label(RichText::new(caret).size(look.text_body).color(colors().muted));
            let (mark, color, word) = status_look(file.status);
            ui.label(RichText::new(mark).size(look.text_body).color(color)).on_hover_text(word);
            ui.label(RichText::new(&file.path).monospace().size(look.text_body).color(colors().text_strong));
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

/// How many files a page lists (the server's own page).
fn nah_page() -> usize {
    500
}

/// One line of a change: its numbers, its mark and its text, tinted by
/// what happened to it.
fn draw_line(ui: &mut egui::Ui, line: &DiffLine, numbers: bool) {
    let look = crate::theme::look();
    let c = colors();
    let (mark, color, fill) = match line.kind {
        LineKind::Added => ("+", c.ok, c.ok_bg),
        LineKind::Removed => ("−", c.error, c.error_bg),
        LineKind::Context => (" ", c.text, egui::Color32::TRANSPARENT),
    };
    egui::Frame::NONE.fill(fill).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = look.space_xs;
            if numbers {
                let n = |n: Option<u32>| n.map_or_else(|| "    ".to_owned(), |n| format!("{n:>4}"));
                ui.label(RichText::new(format!("{} {}", n(line.old), n(line.new))).monospace().size(look.text_small).color(c.faint));
            }
            ui.label(RichText::new(mark).monospace().size(look.text_body).color(color));
            ui.add(egui::Label::new(RichText::new(&line.text).monospace().size(look.text_body).color(if line.kind == LineKind::Context { c.text } else { color })).truncate());
        });
    });
}

/// A blank half of a side-by-side row (the other side has a line there).
fn draw_gap(ui: &mut egui::Ui) {
    ui.label(RichText::new(" ").monospace().size(crate::theme::look().text_body));
}

/// A change: its header, then its lines (interleaved, or removed lines on
/// the left and added on the right, paired in runs).
pub(crate) fn draw_hunk(ui: &mut egui::Ui, hunk: &Hunk, side_by_side: bool) {
    let look = crate::theme::look();
    ui.label(RichText::new(format!("@@ −{},{} +{},{} @@", hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines)).monospace().size(look.text_small).color(colors().faint));
    if !side_by_side {
        for line in &hunk.lines {
            draw_line(ui, line, true);
        }
        return;
    }
    for (left, right) in paired(&hunk.lines) {
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

/// Lines as side-by-side rows: unchanged ones on both sides; a run of
/// removed lines beside the run of added ones that follows it.
fn paired(lines: &[DiffLine]) -> Vec<(Option<&DiffLine>, Option<&DiffLine>)> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind == LineKind::Context {
            rows.push((Some(&lines[i]), Some(&lines[i])));
            i += 1;
            continue;
        }
        let removed: Vec<&DiffLine> = lines[i..].iter().take_while(|l| l.kind == LineKind::Removed).collect();
        i += removed.len();
        let added: Vec<&DiffLine> = lines[i..].iter().take_while(|l| l.kind == LineKind::Added).collect();
        i += added.len();
        for n in 0..removed.len().max(added.len()) {
            rows.push((removed.get(n).copied(), added.get(n).copied()));
        }
    }
    rows
}

impl Panel for ChangesView {
    fn title(&self) -> Status {
        Status::new(format!("{} Changes", egui_phosphor::regular::GIT_DIFF), Tone::Plain)
    }

    /// What's compared, how much changed, and loading.
    fn status(&self) -> Vec<Status> {
        let mut out = vec![Status::new(self.comparison.to_string(), Tone::Info)];
        if let Some(list) = self.list() {
            let (added, removed): (u32, u32) = list.files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
            out.push(Status::new(format!("{} files", list.total), Tone::Muted));
            out.push(Status::new(format!("+{added}"), Tone::Ok));
            out.push(Status::new(format!("−{removed}"), Tone::Error));
        }
        if self.loading {
            out.push(Status::new(egui_phosphor::regular::HOURGLASS, Tone::Warn).detail("Reading the changes"));
        }
        if let Some(e) = &self.error {
            out.push(Status::new(e.clone(), Tone::Error));
        }
        out
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
        let mut out = vec![
            f("changes.compare", "Compare what with what", Command::Compare).param(Param::Choice(choices)).header(Style::Setting, "Compare", None).group(ActionGroup::View).importance(SECONDARY),
            f("changes.layout", "Lay changes out", Command::Layout).param(Param::Choice(layouts)).header(Style::Setting, "Layout", None).group(ActionGroup::View).importance(MINOR),
            f("changes.refresh", "Read the changes again", Command::Refresh).header(Style::Button, "", Some(egui_phosphor::regular::ARROW_CLOCKWISE)).group(ActionGroup::View).importance(MINOR),
        ];
        if self.cursor_row().is_some() {
            out.push(f("changes.more_context", "Show more lines around its changes", Command::MoreContext).on_item(Style::Button, "More context", None).group(ActionGroup::View).importance(MINOR));
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
            Some(Command::Refresh) => {
                self.stale = true;
                self.opened.values_mut().for_each(|o| o.asked_for = None);
            }
            Some(Command::MoreContext) => {
                if let Some(file) = self.cursor_row().and_then(|r| self.file_of(&r).cloned()) {
                    let key = file_key(&file);
                    let context = self.opened.get(&key).map_or(self.ui.context, |o| o.context) + self.ui.more_context;
                    self.ask_file(key, context, cx.net, cx.ctx);
                }
            }
            None => {}
        }
    }
}

impl Navigable for ChangesView {
    fn place(&self) -> Place {
        Place::Changes { item: self.cursor_row().map(|r| self.row_key(&r)) }
    }

    fn restore(&mut self, place: &Place) {
        if let Place::Changes { item: Some(key) } = place {
            match self.rows().iter().position(|r| self.row_key(r) == *key) {
                Some(at) => self.put_cursor(at),
                None => self.restore = Some(key.clone()),
            }
        }
    }

    /// A file's first change, or a change's first changed line.
    fn target(&self) -> Option<Target> {
        let row = self.cursor_row()?;
        let file = self.file_of(&row)?;
        let hunks = self.file_changes(file.group, &file.path).map(|c| c.hunks.as_slice()).unwrap_or_default();
        let hunk = match row {
            Row::Hunk(_, h) => hunks.get(h),
            Row::File(_) => hunks.first(),
        };
        let line = hunk.map_or(1, |h| h.lines.iter().find(|l| l.kind != LineKind::Context).and_then(|l| l.new.or(l.old)).unwrap_or(h.new_start.max(1)));
        Some(Target::Change { file: file.path.clone(), line: line as usize })
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let rows = self.rows();
        let n = rows.len().max(1);
        let page = (self.view_h / 60.0).max(1.0) as usize;
        let file_at = |r: &Row| matches!(r, Row::File(_));
        match motion {
            Motion::Down => self.put_cursor(self.cursor + count),
            Motion::Up => self.put_cursor(self.cursor.saturating_sub(count)),
            Motion::HalfDown => self.put_cursor(self.cursor + page * count),
            Motion::HalfUp => self.put_cursor(self.cursor.saturating_sub(page * count)),
            Motion::First => self.put_cursor(0),
            Motion::Last => self.put_cursor(n - 1),
            Motion::Line(i) => self.put_cursor(i.saturating_sub(1)),
            Motion::NextSibling => {
                if let Some(at) = rows.iter().enumerate().skip(self.cursor + 1).find(|(_, r)| file_at(r)).map(|(i, _)| i) {
                    self.put_cursor(at);
                }
            }
            Motion::PrevSibling => {
                if let Some(at) = rows.iter().enumerate().take(self.cursor).rev().find(|(_, r)| file_at(r)).map(|(i, _)| i) {
                    self.put_cursor(at);
                }
            }
            Motion::Left | Motion::Right => {}
        }
    }

    /// Opens the spot in the code view.
    fn enter(&mut self) -> Step {
        match self.cursor_row().and_then(|r| self.file_of(&r).cloned()) {
            Some(f) if f.status == FileStatus::Deleted => Step::Nothing(format!("{} was deleted: there's nothing to open.", f.path)),
            Some(_) => Step::GoTo(GoTo::Code),
            None => Step::Nothing("No changes here.".into()),
        }
    }

    fn can_enter(&self) -> bool {
        self.cursor_row().is_some()
    }

    fn enter_label(&self) -> Option<String> {
        Some("Open it in the code".to_owned())
    }

    fn can_leave(&self) -> bool {
        matches!(self.cursor_row(), Some(Row::Hunk(..)))
    }

    fn leave_label(&self) -> Option<String> {
        Some("Up to its file".to_owned())
    }

    fn leave(&mut self) -> Step {
        match self.cursor_row() {
            Some(Row::Hunk(i, _)) => {
                if let Some(at) = self.rows().iter().position(|r| *r == Row::File(i)) {
                    self.put_cursor(at);
                }
                Step::Done
            }
            _ => Step::Nothing("Already on a file.".into()),
        }
    }

    fn fold(&mut self, fold: Fold) {
        // Folds need the network: they're asked here and done next tick.
        self.folding = Some(fold);
    }

    fn items(&self) -> Vec<Item> {
        let Some(list) = self.list() else { return Vec::new() };
        self.rows()
            .iter()
            .map(|row| {
                let (name, text) = match row {
                    Row::File(i) => (list.files[*i].path.clone(), list.files[*i].path.clone()),
                    Row::Hunk(i, h) => {
                        let f = &list.files[*i];
                        let text = self.file_changes(f.group, &f.path).and_then(|c| c.hunks.get(*h)).map(|h| h.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                        (String::new(), text)
                    }
                };
                Item::new(self.row_key(row), name, text)
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
            self.put_cursor(at);
        }
    }
}

#[cfg(test)]
mod tests;
