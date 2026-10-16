//! The code view: one file, a cursor line, and what the index knows about it
//! (definitions and call sites, from the file's components). Gutter marks
//! link to the graph; definitions fold; symbols on screen take hints. The
//! desk's comparison marks the lines it changed, and shows the deleted ones
//! in place.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use arch_graph::{Alignment, Component, ComponentId, FileView, RelationKind};
use harness_contracts::changes::{Comparison, FileChanges, Group as ChangeGroup, LineKind};
use egui::{Frame, Pos2, Rect, RichText, Stroke, Vec2};

use crate::actions::{Action, ActionProvider, Group, MINOR, Param, RunCx, Style};
use crate::panel::{Status, Tone};
use crate::feedback::FeedbackInspectorState;
use crate::nav::{Fold, Item, Motion, Move, Navigable, Place, Scroll, Spot, Step, Target, scroll_offset};
use crate::net::{NetClient, NetResult};
use crate::theme::colors;

/// A file's analysis, by the file it was asked for.
type AnalysisReply = (String, NetResult<arch_graph::code::CodeAnalysis>);

/// A file's changes, by the comparison and file they were asked for.
type ChangesReply = (Comparison, String, NetResult<FileChanges>);

/// How a line changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineChange {
    Added,
    Modified,
}

/// A file's changes by line: lines added or changed, and the lines deleted
/// before a line (the line after the last: at the end).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OnLines {
    pub changed: BTreeMap<usize, LineChange>,
    pub deleted: BTreeMap<usize, Vec<String>>,
}

impl OnLines {
    /// From a file's changes with no context around them.
    pub fn of(changes: &FileChanges) -> Self {
        let mut out = Self::default();
        for h in &changes.hunks {
            let deleted: Vec<String> = h.lines.iter().filter(|l| l.kind == LineKind::Removed).map(|l| l.text.clone()).collect();
            let kind = if deleted.is_empty() { LineChange::Added } else { LineChange::Modified };
            for line in h.lines.iter().filter(|l| l.kind == LineKind::Added).filter_map(|l| l.new) {
                out.changed.insert(line as usize, kind);
            }
            if !deleted.is_empty() {
                // Only deleted: git says the line they were after.
                let before = if h.new_lines == 0 { h.new_start + 1 } else { h.new_start };
                out.deleted.entry(before as usize).or_default().extend(deleted);
            }
        }
        out
    }
}

/// A drawn row: a line of the file, or a deleted one shown before `before`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Line(usize),
    Deleted { before: usize, idx: usize },
}

/// What the gutter shows on a line, linked to its component.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    /// A component starts here.
    Starts(ComponentId, String),
    /// A placeholder body (skeleton) inside this component.
    Placeholder(ComponentId),
    /// The plan says this component calls something it doesn't (yet).
    MissingCall(ComponentId, ComponentId),
}

/// The components defined in the file, without the file itself (whose span
/// holds every other one).
fn definitions(view: &FileView) -> Vec<&Component> {
    let in_file: Vec<&Component> = view.components.iter().map(|(c, _)| c).filter(|c| c.anchor.as_ref().is_some_and(|a| a.file.as_str() == view.file)).collect();
    let whole_file = in_file.iter().find(|c| {
        let span = c.anchor.as_ref().map(|a| a.span).unwrap_or_default();
        in_file.iter().all(|o| o.anchor.as_ref().is_some_and(|a| span.contains(&a.span)))
    });
    in_file.iter().filter(|c| in_file.len() == 1 || whole_file.is_none_or(|w| w.id != c.id)).copied().collect()
}

/// Gutter marks by line, from the components in a file.
pub fn gutter_marks(view: &FileView) -> BTreeMap<usize, Vec<Mark>> {
    let mut marks: BTreeMap<usize, Vec<Mark>> = BTreeMap::new();
    let mut start_of: BTreeMap<&ComponentId, usize> = BTreeMap::new();
    for c in definitions(view) {
        let Some(anchor) = &c.anchor else { continue };
        let line = anchor.span.start_line as usize;
        start_of.insert(&c.id, line);
        marks.entry(line).or_default().push(Mark::Starts(c.id.clone(), c.name.clone()));
        for p in &c.placeholders {
            marks.entry(p.start_line as usize).or_default().push(Mark::Placeholder(c.id.clone()));
        }
    }
    for (r, alignment) in &view.relations {
        if r.kind == RelationKind::Calls
            && *alignment == Alignment::Missing
            && let Some(line) = start_of.get(&r.source)
        {
            marks.entry(*line).or_default().push(Mark::MissingCall(r.source.clone(), r.target.clone()));
        }
    }
    marks
}

/// How a gutter mark looks: icon, color, tooltip and its component.
fn mark_look(mark: &Mark) -> (&'static str, egui::Color32, String, &ComponentId) {
    match mark {
        Mark::Starts(id, name) => (egui_phosphor::regular::DIAMOND, colors().accent, format!("{name}: show in the graph"), id),
        Mark::Placeholder(id) => (egui_phosphor::regular::HOURGLASS, colors().warn, "placeholder body (skeleton)".to_owned(), id),
        Mark::MissingCall(id, target) => (egui_phosphor::regular::WARNING, colors().error, format!("planned call to {target} is missing"), id),
    }
}

/// A definition in the file: its component and lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Def {
    pub id: ComponentId,
    pub name: String,
    pub start: usize,
    pub end: usize,
}

/// A call in the file: where, and what it calls.
#[derive(Clone, Debug, PartialEq)]
pub struct Named {
    pub line: usize,
    /// Characters into the line, and how many.
    pub col: usize,
    pub len: usize,
    pub target: ComponentId,
    /// Where it's defined (rather than a reference to it).
    pub definition: bool,
}

/// A symbol a hint can land on: a definition's name or a call.
#[derive(Clone, Debug, PartialEq)]
struct Symbol {
    line: usize,
    col: usize,
    len: usize,
}

pub struct CodeViewerState {
    pub current_file: String,
    pub content: String,
    pub lines: Vec<String>,
    /// The cursor line (1-based).
    pub target_line: Option<usize>,
    /// The symbol on the cursor line a hint landed on (its column).
    cursor_col: Option<usize>,
    /// Lines shown highlighted (e.g. the component opened from the graph).
    pub highlight: Option<(usize, usize)>,
    pub is_loading: bool,
    pub error_message: Option<String>,
    /// The fetched file's contents, with the path they're for.
    marks: BTreeMap<usize, Vec<Mark>>,
    /// What the index knows about the file (kept: call columns are worked
    /// out again when the text arrives after it).
    file_view: Option<FileView>,
    /// Definitions in the file, by start line.
    pub defs: Vec<Def>,
    /// What each named place refers to (from the analysis), in order.
    pub names: Vec<Named>,
    /// The file as the analysis has it: colours, named places.
    analysis: Option<arch_graph::code::CodeAnalysis>,
    /// The text and analysis asked for, by the file they were asked for.
    analysis_inbox: Arc<Mutex<Option<AnalysisReply>>>,
    /// Syntax colours (from the UI config).
    pub ui: crate::config::CodeUi,
    /// Asks the app to open this component's code (a name ctrl-clicked).
    pub open: Option<ComponentId>,
    /// Folded definitions, by start line.
    folds: BTreeSet<usize>,
    /// Where to scroll once the right file's lines are in.
    scroll: Option<Scroll>,
    /// The scroll position and viewport height last frame.
    offset_y: f32,
    view_h: f32,
    /// The height a drawn row really takes (labels can be taller than the
    /// font's line height); rows are laid out and scrolled by it.
    row_h: f32,
    /// Where each drawn row's text starts, and a character's width.
    drawn: Vec<(usize, Pos2)>,
    char_w: f32,
    /// Asks the app to select this component in the graph.
    pub select: Option<ComponentId>,
    /// The server says this file changed since it was indexed.
    pub file_stale: bool,
    /// Out of date, by the server's word or the index status (set by the app).
    pub out_of_date: bool,
    /// The desk's comparison on this file's lines.
    pub on_lines: OnLines,
    /// Deleted lines shown in place (`z d`).
    pub show_deleted: bool,
    /// What its changes were last asked for: the comparison, the file and
    /// the comparison's list as it was (asked again when that's read again).
    changes_asked: Option<(Comparison, String, u64)>,
    changes_inbox: Arc<Mutex<Option<ChangesReply>>>,
}

impl Default for CodeViewerState {
    fn default() -> Self {
        Self {
            current_file: "Cargo.toml".to_string(),
            content: String::new(),
            lines: Vec::new(),
            target_line: None,
            cursor_col: None,
            highlight: None,
            is_loading: false,
            error_message: None,
            marks: BTreeMap::new(),
            file_view: None,
            defs: Vec::new(),
            names: Vec::new(),
            analysis: None,
            analysis_inbox: Arc::default(),
            ui: crate::config::CodeUi::default(),
            open: None,
            folds: BTreeSet::new(),
            scroll: None,
            offset_y: 0.0,
            view_h: 0.0,
            row_h: 0.0,
            drawn: Vec::new(),
            char_w: 7.0,
            select: None,
            file_stale: false,
            out_of_date: false,
            on_lines: OnLines::default(),
            show_deleted: true,
            changes_asked: None,
            changes_inbox: Arc::default(),
        }
    }
}

impl CodeViewerState {
    /// Whether `path` is the file shown (or on its way).
    pub fn shows(&self, path: &str) -> bool {
        path == self.current_file && (self.is_loading || !self.content.is_empty())
    }

    pub fn load_file(&mut self, path: &str, target_line: Option<usize>, net: &NetClient, ctx: egui::Context) {
        if path != self.current_file {
            self.folds.clear();
            self.defs.clear();
            self.names.clear();
            self.marks.clear();
            self.file_view = None;
            self.analysis = None;
            self.on_lines = OnLines::default();
        }
        self.changes_asked = None;
        self.current_file = path.to_string();
        self.target_line = target_line;
        self.cursor_col = None;
        self.highlight = target_line.map(|l| (l, l));
        self.scroll = target_line.map(|_| Scroll::Center);
        self.is_loading = true;
        self.error_message = None;
        // The text, its components and its analysis, in one answer (the
        // same content).
        let analysis = self.analysis_inbox.clone();
        let for_path = path.to_owned();
        net.code_analysis(path, ctx, move |r| {
            if let Ok(mut a) = analysis.lock() {
                *a = Some((for_path, r));
            }
        });
    }

    /// Marks what `comparison` changed in the file shown: asked for when
    /// the file, the comparison or its list (`read`, a count of the times
    /// it was read) changes, once the file's text is in.
    pub fn follow_changes(&mut self, comparison: &Comparison, read: u64, net: &NetClient, ctx: &egui::Context) {
        let on_files = comparison.on_files();
        if let Some((asked, path, result)) = self.changes_inbox.lock().ok().and_then(|mut r| r.take())
            && asked == on_files
            && path == self.current_file
        {
            // Not a repository (or gone): nothing to mark.
            self.on_lines = result.map(|c| OnLines::of(&c)).unwrap_or_default();
        }
        let want = (on_files, self.current_file.clone(), read);
        if self.is_loading || self.lines.is_empty() || self.changes_asked.as_ref() == Some(&want) {
            return;
        }
        let inbox = self.changes_inbox.clone();
        let (asked, path) = (want.0.clone(), want.1.clone());
        net.change_file(&want.0, &want.1, ChangeGroup::Changes, 0, ctx.clone(), move |r| {
            if let Ok(mut i) = inbox.lock() {
                *i = Some((asked, path, r));
            }
        });
        self.changes_asked = Some(want);
    }

    /// Opens `path` with lines `start..=end` highlighted and scrolled to.
    pub fn show_span(&mut self, path: &str, start: usize, end: usize, net: &NetClient, ctx: egui::Context) {
        self.load_file(path, Some(start), net, ctx);
        self.highlight = Some((start, end.max(start)));
    }

    /// Puts the cursor on `line`.
    pub fn move_cursor(&mut self, line: usize) {
        let line = line.clamp(1, self.lines.len().max(1));
        self.target_line = Some(self.unfolded_row_line(line));
        self.cursor_col = None;
        self.highlight = None;
    }

    fn in_highlight(&self, line: usize) -> bool {
        self.highlight.is_some_and(|(a, b)| a <= line && line <= b)
    }

    pub fn set_content(&mut self, content: String) {
        self.changes_asked = None;
        self.lines = content.lines().map(|s| s.to_string()).collect();
        self.content = content;
        self.is_loading = false;
        self.error_message = None;
        if let Some(view) = self.file_view.take() {
            self.set_file_view(view);
        }
        if let Some(analysis) = self.analysis.take() {
            self.set_analysis(analysis);
        }
    }

    pub fn set_error(&mut self, err: String) {
        self.error_message = Some(err);
        self.is_loading = false;
    }

    /// Takes what the analysis says about the file: its colours and what
    /// each named place refers to (worked out again when the text arrives
    /// after it).
    pub fn set_analysis(&mut self, analysis: arch_graph::code::CodeAnalysis) {
        let starts = line_starts(&self.content);
        let mut names: Vec<Named> = analysis
            .occurrences
            .iter()
            .filter_map(|o| {
                let target = o.target.clone()?;
                let (start, end) = (o.start as usize, o.end as usize);
                let line = starts.partition_point(|s| *s <= start);
                let col = column(&self.content, &starts, line, start);
                let len = self.content.get(start..end)?.chars().count();
                Some(Named { line, col, len, target, definition: o.definition })
            })
            .collect();
        names.sort_by(|a, b| (a.line, a.col).cmp(&(b.line, b.col)).then_with(|| a.target.cmp(&b.target)));
        names.dedup();
        self.names = names;
        self.analysis = Some(analysis);
    }

    /// The named place under column `col` of `line`, if any.
    fn name_at(&self, line: usize, col: usize) -> Option<&Named> {
        self.names.iter().find(|n| n.line == line && n.col <= col && col < n.col + n.len.max(1))
    }

    /// What a name's target is, in words (for hover).
    fn describe(&self, id: &ComponentId) -> String {
        let target = self.analysis.as_ref().and_then(|a| a.targets.iter().find(|t| &t.id == id));
        match target {
            Some(t) => {
                let at = t.at.as_ref().map(|(f, l)| format!("\n{f}:{l}")).unwrap_or_default();
                let sig = t.signature.as_ref().map(|s| format!("\n{s}")).unwrap_or_default();
                format!("{} ({}){sig}{at}", t.name, t.type_name)
            }
            None => id.as_str().to_owned(),
        }
    }

    /// A line drawn with its syntax colours (base color where nothing says).
    fn coloured(&self, idx: usize, base: egui::Color32, background: Option<egui::Color32>) -> egui::text::LayoutJob {
        let font = egui::FontId::monospace(crate::theme::look().text_body);
        let line = &self.lines[idx];
        let mut job = egui::text::LayoutJob::default();
        let format = |color: egui::Color32| egui::TextFormat { font_id: font.clone(), color, background: background.unwrap_or(egui::Color32::TRANSPARENT), ..Default::default() };
        let Some(analysis) = self.analysis.as_ref().filter(|a| !a.stale) else {
            job.append(line, 0.0, format(base));
            return job;
        };
        let starts = line_starts(&self.content);
        let line_start = starts.get(idx).copied().unwrap_or(0);
        let line_end = line_start + line.len();
        let first = analysis.highlights.runs.partition_point(|r| (r[1] as usize) <= line_start);
        let mut at = line_start;
        for run in &analysis.highlights.runs[first..] {
            let (s, e) = ((run[0] as usize).max(line_start), (run[1] as usize).min(line_end));
            if s >= line_end {
                break;
            }
            if s > at {
                job.append(&self.content[at..s], 0.0, format(base));
            }
            let color = analysis.highlights.names.get(run[2] as usize).and_then(|n| self.ui.token(n)).and_then(|t| colors().token(t)).unwrap_or(base);
            if e > s.max(at) {
                job.append(&self.content[s.max(at)..e], 0.0, format(color));
                at = e;
            }
        }
        if at < line_end {
            job.append(&self.content[at..line_end], 0.0, format(base));
        }
        job
    }

    /// Takes what the index knows about the file: marks, definitions, calls.
    pub fn set_file_view(&mut self, view: FileView) {
        self.file_stale = view.stale;
        self.marks = gutter_marks(&view);
        let mut defs: Vec<Def> = definitions(&view)
            .into_iter()
            .filter_map(|c| c.anchor.as_ref().map(|a| Def { id: c.id.clone(), name: c.name.clone(), start: a.span.start_line as usize, end: a.span.end_line.max(a.span.start_line) as usize }))
            .collect();
        defs.sort_by_key(|d| (d.start, std::cmp::Reverse(d.end)));
        self.defs = defs;
        self.file_view = Some(view);
    }

    /// The innermost definition holding `line`.
    pub fn def_at(&self, line: usize) -> Option<&Def> {
        self.defs.iter().filter(|d| d.start <= line && line <= d.end).min_by_key(|d| d.end - d.start)
    }

    /// The components named on `line` (calls, types: not what's defined
    /// there), the one under the cursor first.
    pub fn refs_at(&self, line: usize) -> Vec<ComponentId> {
        let mut here: Vec<&Named> = self.names.iter().filter(|n| n.line == line && !n.definition).collect();
        if let Some(col) = self.cursor_col {
            here.sort_by_key(|n| !(n.col <= col && col < n.col + n.len.max(1)));
        }
        let mut out: Vec<ComponentId> = Vec::new();
        for n in here {
            if !out.contains(&n.target) {
                out.push(n.target.clone());
            }
        }
        out
    }

    /// A folded definition hiding `line` (not its first line).
    fn fold_hiding(&self, line: usize) -> Option<&Def> {
        self.defs.iter().filter(|d| self.folds.contains(&d.start) && d.start < line && line <= d.end).min_by_key(|d| d.start)
    }

    /// The line a cursor on `line` sits on: the first line of a fold hiding it.
    fn unfolded_row_line(&self, line: usize) -> usize {
        self.fold_hiding(line).map_or(line, |d| d.start)
    }

    /// Lines drawn, in order (folded definitions show their first line).
    pub fn visible_lines(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.lines.len());
        let mut line = 1;
        while line <= self.lines.len() {
            out.push(line);
            match self.defs.iter().filter(|d| d.start == line && self.folds.contains(&d.start)).max_by_key(|d| d.end) {
                Some(d) => line = d.end + 1,
                None => line += 1,
            }
        }
        out
    }

    /// Rows drawn, in order: the visible lines, and the deleted lines
    /// before them when they're shown.
    pub fn rows(&self) -> Vec<Row> {
        let visible = self.visible_lines();
        let mut out = Vec::with_capacity(visible.len());
        let deleted = |before: usize, out: &mut Vec<Row>| {
            if self.show_deleted {
                let n = self.on_lines.deleted.get(&before).map_or(0, Vec::len);
                out.extend((0..n).map(|idx| Row::Deleted { before, idx }));
            }
        };
        for line in visible {
            deleted(line, &mut out);
            out.push(Row::Line(line));
        }
        deleted(self.lines.len() + 1, &mut out);
        out
    }

    /// The gutter's change marks on a line: a bar on an added or changed
    /// line, and, while deleted lines are hidden, a notch where some were.
    fn change_marks(&self, ui: &egui::Ui, line: usize, last: usize, row_height: f32) {
        let at = ui.max_rect().left_top();
        let bar = |color: egui::Color32, from: f32, h: f32| {
            ui.painter().rect_filled(Rect::from_min_size(at + Vec2::new(0.0, from), Vec2::new(3.0, h)), 0.0, color);
        };
        match self.on_lines.changed.get(&line) {
            Some(LineChange::Added) => bar(colors().ok, 0.0, row_height),
            Some(LineChange::Modified) => bar(colors().info, 0.0, row_height),
            None => {}
        }
        if !self.show_deleted {
            if self.on_lines.deleted.contains_key(&line) {
                bar(colors().error, -1.5, 3.0);
            }
            if line == last && self.on_lines.deleted.contains_key(&(last + 1)) {
                bar(colors().error, row_height - 1.5, 3.0);
            }
        }
    }

    /// A deleted line, shown in place: greyed, not a line of the file.
    fn deleted_row(&self, ui: &mut egui::Ui, text: &str, gutter_w: f32, row_height: f32) -> egui::Response {
        let size = crate::theme::look().text_body;
        ui.horizontal(|ui| {
            ui.set_min_height(row_height);
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.allocate_ui_with_layout(Vec2::new(gutter_w, row_height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_width(gutter_w);
                let at = ui.max_rect().left_top();
                ui.painter().rect_filled(Rect::from_min_size(at, Vec2::new(3.0, row_height)), 0.0, colors().error);
                ui.label(RichText::new("   −").monospace().size(size).color(colors().error));
            });
            ui.separator();
            ui.label(RichText::new(text).monospace().size(size).color(colors().muted).strikethrough().background_color(colors().error_bg));
        })
        .response
    }

    /// Definition names and calls on a line, as hint targets.
    fn symbols_on(&self, line: usize) -> Vec<Symbol> {
        let text = self.lines.get(line.wrapping_sub(1)).map(String::as_str).unwrap_or_default();
        let mut out: Vec<Symbol> = Vec::new();
        for d in self.defs.iter().filter(|d| d.start == line) {
            if let Some(byte) = find_word(text, &d.name) {
                out.push(Symbol { line, col: text[..byte].chars().count(), len: d.name.chars().count() });
            }
        }
        for n in self.names.iter().filter(|n| n.line == line && !n.definition) {
            if !out.iter().any(|s| s.col == n.col) {
                out.push(Symbol { line, col: n.col, len: n.len.max(1) });
            }
        }
        out.sort_by_key(|s| s.col);
        out
    }

    fn put_cursor(&mut self, line: usize, how: Scroll) {
        self.move_cursor(line);
        self.scroll = Some(how);
    }

    fn half_page_lines(&self) -> usize {
        let pitch = if self.row_h > 0.0 { self.row_h } else { 16.0 };
        if self.view_h > 0.0 { ((self.view_h / pitch) / 2.0).floor().max(1.0) as usize } else { 15 }
    }

    /// The next (or previous) definition start from the cursor; `same_level`
    /// keeps to definitions in the same enclosing one. Says why not, when it
    /// can't.
    fn step_def(&mut self, forward: bool, count: usize, same_level: bool) -> Result<(), String> {
        let line = self.target_line.unwrap_or(1);
        let parent_of = |d: &Def| self.defs.iter().filter(|o| o.start <= d.start && d.end <= o.end && *o != d).min_by_key(|o| o.end - o.start).map(|o| o.start);
        let level = if same_level { self.def_at(line).map(parent_of) } else { None };
        let mut starts: Vec<usize> = self.defs.iter().filter(|d| level.is_none_or(|l| parent_of(d) == l)).map(|d| d.start).collect();
        starts.dedup();
        let mut at = line;
        for _ in 0..count {
            let next = if forward { starts.iter().copied().find(|s| *s > at) } else { starts.iter().rev().copied().find(|s| *s < at) };
            match next {
                Some(s) => at = s,
                None if at == line => return Err(format!("No {} definition.", if forward { "next" } else { "previous" })),
                None => break,
            }
        }
        self.put_cursor(at, Scroll::KeepVisible);
        Ok(())
    }

    pub fn render(&mut self, ui: &mut egui::Ui, feedback: &mut FeedbackInspectorState, cx: &mut RunCx<'_>) {
        let (net, slot_idx) = (cx.net, cx.slot);
        // Replies for a file that's no longer the one shown (an earlier load
        // answering late) are dropped, so they can't use up its scroll.
        let incoming = self.analysis_inbox.lock().ok().and_then(|mut a| a.take());
        if let Some((path, res)) = incoming
            && path == self.current_file
        {
            match res {
                Ok(mut analysis) => {
                    self.set_content(analysis.text.clone());
                    self.set_file_view(std::mem::take(&mut analysis.view));
                    if !analysis.stale {
                        self.set_analysis(analysis);
                    }
                }
                Err(e) => self.set_error(e.to_string()),
            }
        }

        ui.vertical(|ui| {
            let start_y = ui.cursor().min.y;
            let start_x = ui.cursor().min.x;
            let w = ui.available_width();
            // Room for the `:line` label next to the path.
            let label_w = if self.target_line.is_some() { 48.0 } else { 0.0 };
            let edit_w = (ui.available_width() - 40.0 - label_w).max(50.0);

            ui.horizontal(|ui| {
                ui.label(RichText::new("📂").size(crate::theme::look().text_title));
                let mut path_edit = self.current_file.clone();
                let resp = ui.add(egui::TextEdit::singleline(&mut path_edit).desired_width(edit_w).font(egui::TextStyle::Monospace));
                if resp.lost_focus() && ui.input(|i| crate::keymap::local().input.sent(i).is_some()) {
                    let ctx = ui.ctx().clone();
                    self.load_file(&path_edit, None, net, ctx);
                }
                if let Some(target) = self.target_line {
                    ui.label(RichText::new(format!(":{}", target)).monospace().strong().size(crate::theme::look().text_body).color(colors().warn));
                }
            });
            let end_y = ui.cursor().min.y;
            let toolbar_rect = Rect::from_min_size(Pos2::new(start_x, start_y), Vec2::new(w, (end_y - start_y).max(22.0)));
            feedback.register_inspectable(ui, &format!("slot_{}:code:toolbar", slot_idx + 1), toolbar_rect);
            ui.separator();

            // Out of date: the marks and spans below come from an older read.
            if self.out_of_date && !self.lines.is_empty() {
                let (_, _, color) = crate::status::look("stale");
                Frame::NONE.fill(color.gamma_multiply(0.12)).stroke(Stroke::new(crate::theme::look().line_thin, color.gamma_multiply(0.5))).corner_radius(crate::theme::look().radius_m).inner_margin(crate::theme::margin2(crate::theme::look().space_m, crate::theme::look().space_s)).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        crate::status::chip(ui, "stale", None);
                        ui.label(RichText::new("This file changed since it was indexed: marks and spans may be off.").size(crate::theme::look().text_body).color(colors().text));
                    });
                });
            }

            if let Some(err) = &self.error_message {
                Frame::NONE.fill(colors().error_bg).stroke(Stroke::new(crate::theme::look().line_thin, colors().error)).corner_radius(crate::theme::look().radius_m).inner_margin(crate::theme::margin(crate::theme::look().space_m)).show(ui, |ui| {
                    ui.label(RichText::new(format!("Error: {}", err)).color(colors().error).size(crate::theme::look().text_body));
                });
                return;
            }

            if self.lines.is_empty() && !self.is_loading {
                ui.vertical_centered(|ui| {
                    ui.add_space(crate::theme::look().space_xxxl);
                    ui.label(RichText::new("No file open: type a path above, or go to one from the graph.").size(crate::theme::look().text_label).color(colors().muted));
                });
                return;
            }

            // One row per drawn line: the gutter (number and marks) and the
            // code share the row, so marks always sit next to their line.
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace).max(14.0).max(self.row_h);
            let mut measured: f32 = 0.0;
            let gutter_w = ui.fonts_mut(|f| f.layout_no_wrap("00000".into(), egui::FontId::monospace(crate::theme::look().text_body), colors().faint).size().x) + 3.0 * 16.0;
            self.char_w = ui.fonts_mut(|f| f.glyph_width(&egui::FontId::monospace(crate::theme::look().text_body), 'x'));
            let mut clicked: Option<ComponentId> = None;
            let mut put_cursor: Option<usize> = None;
            let mut opened = false;
            let mut go_to: Option<ComponentId> = None;
            let mut unfold: Option<usize> = None;
            let rows = self.rows();
            // Rows are laid out only when visible, so scrolling to a line is
            // done by offset, once the requested file's lines are in.
            let pitch = row_height + ui.spacing().item_spacing.y;
            let mut area = egui::ScrollArea::both().id_salt("code_lines").auto_shrink([false, false]);
            let mut scrolled: Option<(Scroll, f32)> = None;
            if !self.is_loading
                && let (Some(how), Some(line)) = (self.scroll, self.target_line)
            {
                let row = rows.iter().position(|r| matches!(r, Row::Line(l) if *l >= line)).unwrap_or(rows.len().saturating_sub(1));
                let view_h = if self.view_h > 0.0 { self.view_h } else { ui.available_height() };
                area = area.vertical_scroll_offset(scroll_offset(how, row, pitch, self.offset_y, view_h));
                self.scroll = None;
                scrolled = Some((how, view_h));
            }
            let mut drawn = Vec::new();
            let last = self.lines.len();
            let viewer_res = area.show_rows(ui, row_height, rows.len(), |ui, shown| {
                for row in shown {
                    let line_no = match rows[row] {
                        Row::Line(l) => l,
                        Row::Deleted { before, idx } => {
                            let text = self.on_lines.deleted.get(&before).and_then(|d| d.get(idx)).map_or("", String::as_str);
                            let drawn_row = self.deleted_row(ui, text, gutter_w, row_height);
                            measured = measured.max(drawn_row.rect.height());
                            continue;
                        }
                    };
                    let idx = line_no - 1;
                    let highlighted = self.in_highlight(line_no);
                    let on_cursor = self.target_line == Some(line_no);
                    let folded = self.defs.iter().filter(|d| d.start == line_no && self.folds.contains(&d.start)).map(|d| d.end - d.start).max();
                    let row = ui.horizontal(|ui| {
                        ui.set_min_height(row_height);
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.allocate_ui_with_layout(Vec2::new(gutter_w, row_height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.set_min_width(gutter_w);
                            self.change_marks(ui, line_no, last, row_height);
                            let color = if on_cursor || highlighted { colors().warn } else { colors().faint };
                            ui.label(RichText::new(format!("{line_no:>4}")).monospace().size(crate::theme::look().text_body).color(color));
                            for mark in self.marks.get(&line_no).into_iter().flatten() {
                                let (icon, color, tip, id) = mark_look(mark);
                                if ui.add(egui::Label::new(RichText::new(icon).size(crate::theme::look().text_body).color(color)).sense(egui::Sense::click())).on_hover_text(tip).clicked() {
                                    clicked = Some(id.clone());
                                }
                            }
                        });
                        ui.separator();
                        let background = if highlighted { Some(colors().warn_bg) } else if on_cursor { Some(colors().selection) } else { None };
                        let job = self.coloured(idx, if highlighted { colors().warn } else { colors().text }, background);
                        let label = ui.add(egui::Label::new(job).sense(egui::Sense::click()));
                        drawn.push((line_no, label.rect.left_top()));
                        // The name under the pointer: what it is; ctrl+click goes to it.
                        let col_at = |x: f32| ((x - label.rect.left()) / self.char_w.max(1.0)).floor().max(0.0) as usize;
                        let hovered = label.hover_pos().and_then(|p| self.name_at(line_no, col_at(p.x))).filter(|n| !n.definition).map(|n| n.target.clone());
                        let label = match &hovered {
                            Some(id) => label.on_hover_text(self.describe(id)),
                            None => label,
                        };
                        // The pointer grammar, read here (a name under it, the
                        // go-to modifier): a click puts the cursor on the
                        // line, a double click opens what's there (Enter).
                        if label.clicked() {
                            match (&hovered, ui.input(|i| crate::keymap::local().pointer.go_to.held(i.modifiers))) {
                                (Some(id), true) => go_to = Some(id.clone()),
                                _ => put_cursor = Some(line_no),
                            }
                        }
                        if label.double_clicked() {
                            opened = true;
                        }
                        if let Some(hidden) = folded
                            && ui.add(egui::Label::new(RichText::new(format!("  … {hidden} lines")).monospace().size(crate::theme::look().text_small).color(colors().muted)).sense(egui::Sense::click())).on_hover_text("Unfold").clicked()
                        {
                            unfold = Some(line_no);
                        }
                    });
                    measured = measured.max(row.response.rect.height());
                }
            });
            if measured > self.row_h + 0.5 {
                self.row_h = measured;
                ui.ctx().request_repaint();
            }
            self.offset_y = viewer_res.state.offset.y;
            self.view_h = viewer_res.inner_rect.height();
            // Scrolled for a viewport of another size (the slot just opened
            // or changed): once more for the real one.
            if let Some((how, used)) = scrolled
                && (used - self.view_h).abs() > 1.0
            {
                self.scroll = Some(how);
                ui.ctx().request_repaint();
            }
            let clip = viewer_res.inner_rect;
            self.drawn = drawn.into_iter().filter(|(_, at)| clip.contains(*at + Vec2::new(1.0, 1.0))).collect();
            if clicked.is_some() {
                self.select = clicked;
            }
            if let Some(line) = put_cursor {
                self.move_cursor(line);
            }
            if opened {
                *cx.asked = Some(("nav.enter".to_owned(), None));
            }
            if go_to.is_some() {
                self.open = go_to;
            }
            if let Some(start) = unfold {
                self.folds.remove(&start);
            }
            feedback.register_inspectable(ui, &format!("slot_{}:code:viewer", slot_idx + 1), viewer_res.inner_rect);
        });
    }
}

/// Byte offsets where each line starts.
fn line_starts(content: &str) -> Vec<usize> {
    std::iter::once(0).chain(content.match_indices('\n').map(|(i, _)| i + 1)).collect()
}

/// The character column of a byte offset on a (1-based) line.
fn column(content: &str, starts: &[usize], line: usize, byte: usize) -> usize {
    let Some(&start) = starts.get(line.wrapping_sub(1)) else { return 0 };
    let end = byte.clamp(start, content.len());
    content.get(start..end).map_or(0, |s| s.chars().count())
}

/// Where `word` appears in `text` as a whole word.
fn find_word(text: &str, word: &str) -> Option<usize> {
    if word.is_empty() {
        return None;
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).map(|(i, _)| i).find(|&i| {
        let before = text[..i].chars().next_back().is_none_or(|c| !is_ident(c));
        let after = text[i + word.len()..].chars().next().is_none_or(|c| !is_ident(c));
        before && after
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CodeCommand {
    Def { forward: bool },
    Copy,
    Reload,
    Deleted,
}

impl CodeViewerState {
    /// The shared moves where the cursor is, in the code's words: through
    /// the lines, between definitions, into what a line names, out to its
    /// definition, folding definitions.
    fn moves(&self) -> Vec<Action> {
        let mut out = crate::nav::list_moves();
        let line = self.target_line.unwrap_or(1);
        let defs = !self.defs.is_empty();
        out.push(Move::Motion(Motion::NextSibling).offer_if(defs, "Next definition at this level"));
        out.push(Move::Motion(Motion::PrevSibling).offer_if(defs, "Previous definition at this level"));
        let refs = self.refs_at(line);
        let enter = match refs.as_slice() {
            _ if self.target_line.is_none() => None,
            _ if self.folds.contains(&line) => Some("Unfold".to_owned()),
            [] => None,
            [one] => Some(format!("Go to {}", crate::nav::short_name(one))),
            _ => Some(format!("Go to one of {} named here", refs.len())),
        };
        out.push(Move::Enter.offer_if(enter.is_some(), enter.unwrap_or_else(|| "Go to what's named here".into())));
        out.push(Move::Leave.offer_if(self.defs.iter().any(|d| d.start < line && line <= d.end), "Out to the definition's start"));
        // Definitions spanning lines fold: the innermost the cursor is in.
        let def = self.defs.iter().filter(|d| d.start <= line && line <= d.end && d.end > d.start).min_by_key(|d| d.end - d.start);
        out.extend(crate::nav::fold_here(def.map(|d| (d.name.as_str(), self.folds.contains(&d.start)))));
        out.extend(crate::nav::fold_all(self.defs.iter().any(|d| d.end > d.start), "Unfold every definition", "Fold every definition"));
        out
    }
}

impl ActionProvider for CodeViewerState {
    fn actions(&self) -> Vec<Action> {
        let mut out = self.moves();
        if !self.defs.is_empty() {
            out.push(Action::new("code.next_def", "Next definition", CodeCommand::Def { forward: true }).focused().group(Group::Navigate));
            out.push(Action::new("code.prev_def", "Previous definition", CodeCommand::Def { forward: false }).focused().group(Group::Navigate));
        }
        out.push(Action::new("code.deleted", "Show deleted lines in place", CodeCommand::Deleted).focused().param(Param::Toggle { on: self.show_deleted }).header(Style::Setting, "Deleted lines", None).group(Group::View).importance(MINOR));
        out.push(Action::new("code.reload", "Load the file again", CodeCommand::Reload).focused().group(Group::View).importance(MINOR).header(Style::Button, "", Some(egui_phosphor::regular::ARROW_CLOCKWISE)));
        out.push(Action::new("code.copy", "Copy the file", CodeCommand::Copy).focused().importance(MINOR).header(Style::Button, "", Some(egui_phosphor::regular::COPY)));
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        match action.command::<CodeCommand>() {
            Some(CodeCommand::Def { forward }) => {
                if let Err(why) = self.step_def(*forward, 1, false) {
                    *cx.note = Some(why);
                }
            }
            Some(CodeCommand::Copy) => cx.ctx.copy_text(self.content.clone()),
            Some(CodeCommand::Deleted) => {
                self.show_deleted = match value.as_deref() {
                    Some("true") => true,
                    Some("false") => false,
                    _ => !self.show_deleted,
                };
            }
            Some(CodeCommand::Reload) => {
                let path = self.current_file.clone();
                self.load_file(&path, self.target_line, cx.net, cx.ctx.clone());
            }
            None => {}
        }
    }
}

impl crate::panel::Panel for CodeViewerState {
    /// The file it shows (its path on hover).
    fn title(&self) -> Status {
        let name = self.current_file.rsplit('/').next().unwrap_or(&self.current_file);
        Status::new(format!("{} {name}", egui_phosphor::regular::FILE_CODE), Tone::Plain).detail(self.current_file.clone())
    }

    /// Its text: a click puts the cursor on a line, the go-to modifier
    /// with a click goes to the name under it.
    fn own_pointer(&self) -> bool {
        true
    }

    /// Out of date with the index: reading it again, on its top line.
    fn placed(&self) -> Vec<(String, crate::actions::Control)> {
        if self.out_of_date && !self.lines.is_empty() {
            vec![("index.file".to_owned(), crate::actions::Control::new(crate::actions::At::Header, Style::Button, "Re-read it", None))]
        } else {
            Vec::new()
        }
    }

    /// Loading, and out of date with the index.
    fn status(&self) -> Vec<Status> {
        let mut out = Vec::new();
        if self.is_loading {
            out.push(Status::new(egui_phosphor::regular::HOURGLASS, Tone::Warn).detail("Loading"));
        }
        out
    }
}

impl Navigable for CodeViewerState {
    /// Anything with code: a line, a file link, a component (its code is
    /// opened by the app, which knows where components are).
    fn can_show(&self, target: &Target) -> bool {
        target.code().is_some() || target.component().is_some()
    }

    fn place(&self) -> Place {
        Place::Code { file: self.current_file.clone(), line: self.target_line.unwrap_or(1) }
    }

    /// Puts the cursor back (the shell loads the file when it differs).
    fn restore(&mut self, place: &Place) {
        if let Place::Code { line, .. } = place {
            self.target_line = Some(*line);
            self.cursor_col = None;
            self.scroll = Some(Scroll::Center);
        }
    }

    fn target(&self) -> Option<Target> {
        let line = self.target_line?;
        Some(Target::Line { file: self.current_file.clone(), line, within: self.def_at(line).map(|d| d.id.clone()), refs: self.refs_at(line) })
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let visible = self.visible_lines();
        if visible.is_empty() {
            return;
        }
        let row = self.target_line.and_then(|l| visible.iter().position(|v| *v >= l)).unwrap_or(0);
        let to_row = |r: usize| visible[r.min(visible.len() - 1)];
        match motion {
            Motion::Down => self.put_cursor(if self.target_line.is_none() { visible[0] } else { to_row(row + count) }, Scroll::KeepVisible),
            Motion::Up => self.put_cursor(to_row(row.saturating_sub(count)), Scroll::KeepVisible),
            Motion::HalfDown => self.put_cursor(to_row(row + self.half_page_lines() * count), Scroll::KeepVisible),
            Motion::HalfUp => self.put_cursor(to_row(row.saturating_sub(self.half_page_lines() * count)), Scroll::KeepVisible),
            Motion::First => self.put_cursor(1, Scroll::KeepVisible),
            Motion::Last => self.put_cursor(self.lines.len(), Scroll::KeepVisible),
            Motion::Line(n) => self.put_cursor(n, Scroll::Center),
            Motion::NextSibling | Motion::PrevSibling => {
                let _ = self.step_def(motion == Motion::NextSibling, count, true);
            }
            Motion::Left | Motion::Right => {}
        }
    }

    /// On a name: its definition (the shell opens it). On a folded line: unfold.
    fn enter(&mut self) -> Step {
        match self.target_line {
            Some(line) if !self.refs_at(line).is_empty() => Step::GoTo(crate::nav::GoTo::Code),
            Some(line) if self.folds.contains(&line) => {
                self.folds.remove(&line);
                Step::Done
            }
            _ => Step::Nothing("Nothing named here goes anywhere.".into()),
        }
    }

    /// Out to the first line of the definition the cursor is in (then the
    /// one around that).
    fn leave(&mut self) -> Step {
        let line = self.target_line.unwrap_or(1);
        let around = self.defs.iter().filter(|d| d.start < line && line <= d.end).min_by_key(|d| d.end - d.start).map(|d| d.start);
        match around {
            Some(start) => {
                self.put_cursor(start, Scroll::KeepVisible);
                Step::Done
            }
            None => Step::Nothing("Not inside a definition.".into()),
        }
    }

    fn fold(&mut self, fold: Fold) {
        match fold {
            Fold::OpenAll => self.folds.clear(),
            Fold::CloseAll => {
                // Outermost definitions (folding them hides the rest).
                self.folds = self.defs.iter().filter(|d| d.end > d.start && !self.defs.iter().any(|o| o != *d && o.start <= d.start && d.end <= o.end && o.end > o.start)).map(|d| d.start).collect();
            }
            Fold::Open | Fold::Close | Fold::Toggle => {
                let line = self.target_line.unwrap_or(1);
                let Some(def) = self.defs.iter().filter(|d| d.start <= line && line <= d.end && d.end > d.start).min_by_key(|d| d.end - d.start).cloned() else { return };
                let open = match fold {
                    Fold::Open => true,
                    Fold::Close => false,
                    _ => self.folds.contains(&def.start),
                };
                if open {
                    self.folds.remove(&def.start);
                } else {
                    self.folds.insert(def.start);
                    self.scroll = Some(Scroll::KeepVisible);
                }
            }
        }
        if let Some(line) = self.target_line {
            self.target_line = Some(self.unfolded_row_line(line));
        }
    }

    fn items(&self) -> Vec<Item> {
        self.lines.iter().enumerate().map(|(i, text)| Item::new((i + 1).to_string(), "", text.as_str())).collect()
    }

    /// Definition names and calls on the lines drawn now.
    fn spots(&self) -> Vec<Spot> {
        let pitch = if self.row_h > 0.0 { self.row_h } else { 16.0 };
        self.drawn
            .iter()
            .flat_map(|(line, at)| {
                self.symbols_on(*line).into_iter().map(move |s| {
                    let min = *at + Vec2::new(s.col as f32 * self.char_w, 0.0);
                    Spot { key: format!("{}:{}", s.line, s.col), rect: Rect::from_min_size(min, Vec2::new(s.len.max(1) as f32 * self.char_w, pitch)) }
                })
            })
            .collect()
    }

    fn cursor_key(&self) -> Option<String> {
        self.target_line.map(|l| l.to_string())
    }

    /// A line ("12") or a symbol on it ("12:4").
    fn move_to(&mut self, key: &str) {
        let mut parts = key.split(':');
        let Some(line) = parts.next().and_then(|l| l.parse::<usize>().ok()) else { return };
        let col = parts.next().and_then(|c| c.parse::<usize>().ok());
        let defs = &self.defs;
        self.folds.retain(|start| !defs.iter().any(|d| d.start == *start && d.start < line && line <= d.end));
        self.put_cursor(line, Scroll::Center);
        self.cursor_col = col;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use arch_graph::{CodeAnchor, Relation, RepoPath, Span, Stage, Status};

    /// The marks on one row of the code view.
    fn row_marks(marks: &BTreeMap<usize, Vec<Mark>>, line_no: usize) -> &[Mark] {
        marks.get(&line_no).map(Vec::as_slice).unwrap_or_default()
    }

    /// Text drawn this frame with where it is.
    fn placed(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(t) => out.push((t.galley.text().to_owned(), t.visual_bounding_rect())),
                egui::Shape::Vec(list) => list.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn render(ctx: &egui::Context, code: &mut CodeViewerState, h: f32) -> Vec<(String, egui::Rect)> {
        let net = NetClient::new("http://localhost:0");
        let mut feedback = FeedbackInspectorState::default();
        let output = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, h))), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (mut mode, mut note, mut asked) = (crate::keymap::InputMode::Normal, None, None);
                code.render(ui, &mut feedback, &mut RunCx { net: &net, ctx, input_mode: &mut mode, slot: 0, note: &mut note, asked: &mut asked });
            });
        });
        placed(&output)
    }

    #[test]
    fn gutter_marks_sit_on_the_row_of_their_line() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut code = CodeViewerState::default();
        // Lines of different heights' worth of content, marks deep in the file.
        let text: String = (1..=40).map(|i| if i % 3 == 0 { format!("    let value_{i} = compute({i}); // a longer line\n") } else { format!("fn f{i}() {{}}\n") }).collect();
        code.set_content(text);
        let id = ComponentId::new("rust:x::f29");
        code.marks.insert(29, vec![Mark::Starts(id.clone(), "f29".into()), Mark::Placeholder(id)]);
        let mut shapes = Vec::new();
        for _ in 0..2 {
            shapes = render(&ctx, &mut code, 2000.0);
        }
        let row_of = |needle: &str| shapes.iter().find(|(t, _)| t == needle).map(|(_, r)| r.center().y).unwrap_or_else(|| panic!("{needle} not drawn"));
        let line = row_of("fn f29() {}");
        for (what, needle) in [("line number", "  29"), ("component mark", egui_phosphor::regular::DIAMOND), ("placeholder mark", egui_phosphor::regular::HOURGLASS)] {
            assert!((row_of(needle) - line).abs() < 4.0, "{what} is on line 29's row ({} vs {line})", row_of(needle));
        }
        assert_eq!(row_marks(&code.marks, 29).len(), 2);
        assert!(row_marks(&code.marks, 28).is_empty());
    }

    #[test]
    fn opening_a_target_off_screen_scrolls_it_into_view_and_the_cursor_stays_visible() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut code = CodeViewerState::default();
        let text: String = (1..=900).map(|i| format!("fn line_{i}() {{}}\n")).collect();
        let frame = |code: &mut CodeViewerState| render(&ctx, code, 600.0).into_iter().map(|(t, _)| t).collect::<Vec<_>>();
        code.set_content(text.clone());
        frame(&mut code);
        // A target far down: the file is reloaded (lines stay until the reply),
        // and the scroll waits for the content, then lands on the target
        // (as load_file leaves it; no network in tests). An earlier load's
        // reply arriving late for another file must not use up the scroll.
        code.current_file = "src/big.rs".into();
        *code.analysis_inbox.lock().unwrap() = Some(("Cargo.toml".into(), Ok(arch_graph::code::CodeAnalysis { file: "Cargo.toml".into(), text: "[package]\nname = \"x\"\n".into(), stale: true, ..Default::default() })));
        code.target_line = Some(700);
        code.highlight = Some((700, 700));
        code.scroll = Some(Scroll::Center);
        code.is_loading = true;
        frame(&mut code);
        code.set_content(text.clone());
        let mut drawn = Vec::new();
        for _ in 0..3 {
            drawn = frame(&mut code);
        }
        assert!(drawn.iter().any(|t| t == "fn line_700() {}"), "the target line is on screen");
        assert!(!drawn.iter().any(|t| t == "fn line_10() {}"), "the top of the file isn't");
        // Moving the cursor past the bottom scrolls just enough to keep it
        // visible (5j: a count).
        for _ in 0..12 {
            code.motion(Motion::Down, 5);
            for _ in 0..2 {
                drawn = frame(&mut code);
            }
        }
        assert_eq!(code.target_line, Some(760));
        assert!(drawn.iter().any(|t| t == "fn line_760() {}"), "the cursor line stays on screen");
        // Half a page moves by half the rows that fit, and stays in view.
        let half = code.half_page_lines();
        assert!(half > 5 && half < 30, "{half}");
        code.motion(Motion::HalfDown, 1);
        for _ in 0..2 {
            drawn = frame(&mut code);
        }
        assert_eq!(code.target_line, Some(760 + half));
        assert!(drawn.iter().any(|t| *t == format!("fn line_{}() {{}}", 760 + half)));
        code.motion(Motion::HalfUp, 2);
        assert_eq!(code.target_line, Some(760 - half));
        code.motion(Motion::Last, 1);
        assert_eq!(code.target_line, Some(900));
        code.motion(Motion::First, 1);
        assert_eq!(code.target_line, Some(1));
        code.motion(Motion::Line(42), 1);
        assert_eq!(code.target_line, Some(42));
    }

    fn component(id: &str, name: &str, start: u32, end: u32, (start_byte, end_byte): (usize, usize)) -> Component {
        Component {
            id: ComponentId::new(id),
            name: name.into(),
            type_name: "function".into(),
            description: String::new(),
            status: Status::Existing,
            stage: Stage::Implemented,
            anchor: Some(CodeAnchor { language: "rust".into(), file: RepoPath::new("src/a.rs").unwrap(), symbol: name.into(), syntax_kind: "function_item".into(), span: Span { start_line: start, end_line: end, start_byte: start_byte as u32, end_byte: end_byte as u32 }, content_hash: String::new() }),
            proposed: None,
            signature: None,
            schemas: Default::default(),
            spec: None,
            placeholders: vec![],
        }
    }

    /// `src/a.rs`: a module holding `run` (calls `step` on line 3 and
    /// `log` on line 4) and `step`.
    pub(crate) fn sample() -> CodeViewerState {
        let text = "mod a {\n    fn run() {\n        step();\n        log();\n    }\n    fn step() {\n        let x = 1;\n    }\n}\n";
        let mut code = CodeViewerState { current_file: "src/a.rs".into(), ..Default::default() };
        code.set_content(text.into());
        let starts = line_starts(text);
        let mut call = Relation::new(RelationKind::Calls, ComponentId::new("rust:a::run"), ComponentId::new("rust:a::step"), arch_graph::Provenance::OBSERVED);
        call.sites = vec![Span { start_line: 3, end_line: 3, start_byte: (starts[2] + 8) as u32, end_byte: (starts[2] + 12) as u32 }];
        let mut log = Relation::new(RelationKind::Calls, ComponentId::new("rust:a::run"), ComponentId::new("rust:x::log"), arch_graph::Provenance::OBSERVED);
        log.sites = vec![Span { start_line: 4, end_line: 4, start_byte: (starts[3] + 8) as u32, end_byte: (starts[3] + 11) as u32 }];
        let view = FileView {
            file: "src/a.rs".into(),
            components: vec![
                (component("rust:a", "a", 1, 9, (0, text.len())), Alignment::Observed),
                (component("rust:a::run", "run", 2, 5, (starts[1], starts[5] - 1)), Alignment::Observed),
                (component("rust:a::step", "step", 6, 8, (starts[5], starts[8] - 1)), Alignment::Observed),
            ],
            relations: vec![(call, Alignment::Observed), (log, Alignment::Observed)],
            stale: false,
        };
        code.set_file_view(view);
        // The analysis: where each name is and what it refers to.
        let at = |line: usize, col: usize| (starts[line - 1] + col) as u32;
        let named = |line, col, len: u32, id: &str, definition| arch_graph::code::Occurrence { start: at(line, col), end: at(line, col) + len, target: Some(ComponentId::new(id)), definition };
        code.set_analysis(arch_graph::code::CodeAnalysis {
            file: "src/a.rs".into(),
            text: code.content.clone(),
            content_hash: String::new(),
            stale: false,
            highlights: arch_graph::code::Highlights { names: vec!["keyword".into()], runs: vec![[at(2, 4), at(2, 6), 0]] },
            occurrences: vec![named(2, 7, 3, "rust:a::run", true), named(3, 8, 4, "rust:a::step", false), named(4, 8, 3, "rust:x::log", false), named(6, 7, 4, "rust:a::step", true)],
            targets: vec![arch_graph::code::CodeTarget { id: ComponentId::new("rust:a::step"), name: "step".into(), type_name: "function".into(), at: Some(("src/a.rs".into(), 6)), signature: Some("step()".into()) }],
            view: FileView::default(),
        });
        code
    }

    #[test]
    fn the_cursor_knows_its_definition_and_calls_and_steps_between_definitions() {
        let mut code = sample();
        assert_eq!(code.defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["run", "step"], "the file itself isn't a definition");
        let refs: Vec<(usize, usize, usize, &str)> = code.names.iter().filter(|n| !n.definition).map(|n| (n.line, n.col, n.len, n.target.as_str())).collect();
        assert_eq!(refs, [(3, 8, 4, "rust:a::step"), (4, 8, 3, "rust:x::log")], "where each name is, from the analysis");
        code.motion(Motion::Line(3), 1);
        let Some(Target::Line { within, refs, .. }) = code.target() else { panic!() };
        assert_eq!((within.unwrap().as_str(), refs), ("rust:a::run", vec![ComponentId::new("rust:a::step")]));
        assert_eq!(code.enter(), Step::GoTo(crate::nav::GoTo::Code), "Enter on a call goes to what it calls");
        code.motion(Motion::Line(7), 1);
        assert!(matches!(code.enter(), Step::Nothing(_)), "no call here");
        assert_eq!(code.leave(), Step::Done);
        assert_eq!(code.target_line, Some(6), "Backspace: out to the definition's first line");
        // ]f / [f: definitions in order.
        code.motion(Motion::First, 1);
        assert!(code.step_def(true, 1, false).is_ok());
        assert_eq!(code.target_line, Some(2));
        assert!(code.step_def(true, 1, false).is_ok());
        assert_eq!(code.target_line, Some(6));
        assert!(code.step_def(true, 1, false).is_err(), "no next one: says so");
        assert!(code.step_def(false, 1, false).is_ok());
        assert_eq!(code.target_line, Some(2));
        // ]] / [[: siblings (run and step share the module).
        code.motion(Motion::NextSibling, 1);
        assert_eq!(code.target_line, Some(6));
        // / finds lines; :N goes to a line.
        let found = crate::nav::rank_matches(&code.items(), "step");
        assert_eq!(found.iter().map(|i| i + 1).collect::<Vec<_>>(), [3, 6]);
        code.motion(Motion::Line(4), 1);
        assert_eq!(code.target_line, Some(4));
        // A hint on a call puts the cursor there.
        code.move_to("4:8");
        let Some(Target::Line { refs, .. }) = code.target() else { panic!() };
        assert_eq!(refs, vec![ComponentId::new("rust:x::log")]);
        // What a name is, for hover; and the colours.
        assert!(code.describe(&ComponentId::new("rust:a::step")).contains("step()"));
        assert_eq!(code.name_at(3, 9).map(|n| n.target.as_str()), Some("rust:a::step"));
        assert_eq!(code.name_at(3, 2), None);
        code.ui = crate::config::UiConfig::with_overrides("").unwrap().code;
        let job = code.coloured(1, egui::Color32::WHITE, None);
        let keyword = colors().token(code.ui.token("keyword").unwrap()).unwrap();
        assert!(job.sections.iter().any(|s| s.format.color == keyword && &job.text[s.byte_range.clone()] == "fn"), "fn is coloured as a keyword");
    }

    #[test]
    fn definitions_fold_and_symbols_on_screen_take_hints() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut code = sample();
        code.motion(Motion::Line(3), 1);
        code.fold(Fold::Close);
        assert_eq!(code.target_line, Some(2), "the cursor moves to the folded line");
        assert_eq!(code.visible_lines(), vec![1, 2, 6, 7, 8, 9]);
        let drawn: Vec<String> = render(&ctx, &mut code, 600.0).into_iter().map(|(t, _)| t).collect();
        assert!(drawn.iter().any(|t| t.contains("… 3 lines")), "{drawn:?}");
        assert!(!drawn.iter().any(|t| t.contains("step();")), "folded away");
        code.motion(Motion::Down, 1);
        assert_eq!(code.target_line, Some(6), "moves skip folded lines");
        code.fold(Fold::OpenAll);
        assert_eq!(code.visible_lines().len(), 9);
        code.fold(Fold::CloseAll);
        assert_eq!(code.visible_lines(), vec![1, 2, 6, 9], "outermost definitions (the module is the file)");
        code.fold(Fold::OpenAll);
        for _ in 0..2 {
            render(&ctx, &mut code, 600.0);
        }
        // Hints: definition names and calls on screen, where they're drawn.
        let spots = code.spots();
        let keys: Vec<&str> = spots.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["2:7", "3:8", "4:8", "6:7"], "run, the step call, the log call, step");
        assert!(spots[1].rect.min.x > spots[0].rect.min.x && spots[1].rect.min.y > spots[0].rect.min.y, "{spots:?}");
        code.move_to(&spots[1].key);
        assert_eq!(code.target_line, Some(3));
    }

    fn hunk(old_start: u32, old_lines: u32, new_start: u32, new_lines: u32, lines: &[(LineKind, Option<u32>, &str)]) -> harness_contracts::changes::Hunk {
        let lines = lines.iter().map(|(kind, new, text)| harness_contracts::changes::DiffLine { kind: *kind, old: None, new: *new, text: (*text).to_owned() }).collect();
        harness_contracts::changes::Hunk { old_start, old_lines, new_start, new_lines, lines }
    }

    /// A five-line file with line 2 changed, a line added after line 3, a
    /// line deleted from the top and one from the end.
    fn changed() -> CodeViewerState {
        use LineKind::{Added, Removed};
        let changes = FileChanges {
            comparison: Comparison::default().on_files(),
            path: "a.rs".into(),
            group: ChangeGroup::Changes,
            context: 0,
            binary: false,
            hunks: vec![
                hunk(1, 1, 0, 0, &[(Removed, None, "// gone from the top")]),
                hunk(3, 1, 2, 1, &[(Removed, None, "let b = 1;"), (Added, Some(2), "let b = 2;")]),
                hunk(4, 0, 4, 1, &[(Added, Some(4), "let d = 4;")]),
                hunk(6, 1, 5, 0, &[(Removed, None, "// gone from the end")]),
            ],
        };
        let mut code = CodeViewerState::default();
        code.set_content("let a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;\nlet e = 5;\n".into());
        code.on_lines = OnLines::of(&changes);
        code
    }

    #[test]
    fn changed_lines_are_marked_and_deleted_ones_sit_where_they_were() {
        let mut code = changed();
        assert_eq!(code.on_lines.changed, BTreeMap::from([(2, LineChange::Modified), (4, LineChange::Added)]));
        let deleted: Vec<(usize, usize)> = code.on_lines.deleted.iter().map(|(b, d)| (*b, d.len())).collect();
        assert_eq!(deleted, [(1, 1), (2, 1), (6, 1)], "before line 1, before the changed line, after the last");
        let (d, l) = (|before, idx| Row::Deleted { before, idx }, Row::Line);
        assert_eq!(code.rows(), [d(1, 0), l(1), d(2, 0), l(2), l(3), l(4), l(5), d(6, 0)]);
        code.show_deleted = false;
        assert_eq!(code.rows(), (1..=5).map(Row::Line).collect::<Vec<_>>(), "z d: hidden, only the file's lines");
        // Drawn: the deleted lines' text is on screen between the file's.
        code.show_deleted = true;
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts = render(&ctx, &mut code, 400.0);
        }
        let y = |text: &str| texts.iter().find(|(t, _)| t == text).map(|(_, r)| r.top()).unwrap_or_else(|| panic!("{text} isn't drawn"));
        assert!(y("// gone from the top") < y("let a = 1;") && y("let a = 1;") < y("let b = 1;") && y("let b = 1;") < y("let b = 2;") && y("let e = 5;") < y("// gone from the end"));
        // The cursor walks the file's lines only.
        code.motion(Motion::First, 1);
        code.motion(Motion::Down, 1);
        assert_eq!(code.target_line, Some(2));
    }


    #[test]
    fn its_changes_are_asked_for_the_files_against_the_comparison_and_again_when_the_list_is_read_again() {
        let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
        let mut code = changed();
        code.current_file = "a.rs".into();
        code.follow_changes(&Comparison::default(), 1, &net, &ctx);
        let on_files = Comparison { from: harness_contracts::changes::Point::Rev("HEAD".into()), to: harness_contracts::changes::Point::Files };
        assert_eq!(code.changes_asked, Some((on_files.clone(), "a.rs".into(), 1)), "staged and unstaged together: the files against the last commit");
        code.changes_asked = Some((on_files.clone(), "a.rs".into(), 0));
        code.follow_changes(&Comparison::default(), 1, &net, &ctx);
        assert_eq!(code.changes_asked.as_ref().map(|a| a.2), Some(1), "the list read again: asked again");
        code.set_content("let a = 1;\n".into());
        assert_eq!(code.changes_asked, None, "new text: asked again");
    }

}
