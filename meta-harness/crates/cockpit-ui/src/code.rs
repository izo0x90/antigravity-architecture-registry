//! The code view: one file, a cursor line, and what the index knows about it
//! (definitions and call sites, from the file's components). Gutter marks
//! link to the graph; definitions fold; symbols on screen take hints.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use arch_graph::{Alignment, Component, ComponentId, FileView, RelationKind};
use egui::{Frame, Pos2, Rect, RichText, Stroke, Vec2};

use crate::actions::{Action, ActionProvider, Group, RunCx};
use crate::feedback::FeedbackInspectorState;
use crate::nav::{Fold, Item, Motion, Navigable, Place, Scroll, Spot, Step, Target, scroll_offset};
use crate::net::{NetClient, NetResult};
use crate::theme::colors;

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
    in_file.iter().filter(|c| in_file.len() == 1 || !whole_file.is_some_and(|w| w.id == c.id)).copied().collect()
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

/// The marks drawn on one row of the code view, next to that line's text.
pub fn row_marks(marks: &BTreeMap<usize, Vec<Mark>>, line_no: usize) -> &[Mark] {
    marks.get(&line_no).map(Vec::as_slice).unwrap_or_default()
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
pub struct CallSite {
    pub line: usize,
    /// Characters into the line.
    pub col: usize,
    pub callee: ComponentId,
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
    pending_result: Arc<Mutex<Option<(String, Result<String, String>)>>>,
    marks: BTreeMap<usize, Vec<Mark>>,
    marks_inbox: Arc<Mutex<Option<NetResult<FileView>>>>,
    /// What the index knows about the file (kept: call columns are worked
    /// out again when the text arrives after it).
    file_view: Option<FileView>,
    /// Definitions in the file, by start line.
    pub defs: Vec<Def>,
    pub calls: Vec<CallSite>,
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
    /// Asks the app to read this file again.
    pub reread: bool,
    /// When the user last moved the cursor (follow mode syncs the graph
    /// once it settles).
    pub cursor_moved_at: Option<f64>,
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
            pending_result: Arc::new(Mutex::new(None)),
            marks: BTreeMap::new(),
            marks_inbox: Arc::default(),
            file_view: None,
            defs: Vec::new(),
            calls: Vec::new(),
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
            reread: false,
            cursor_moved_at: None,
        }
    }
}

impl CodeViewerState {
    pub fn load_file(&mut self, path: &str, target_line: Option<usize>, net: &NetClient, ctx: egui::Context) {
        if path != self.current_file {
            self.folds.clear();
            self.defs.clear();
            self.calls.clear();
            self.marks.clear();
            self.file_view = None;
        }
        self.current_file = path.to_string();
        self.target_line = target_line;
        self.cursor_col = None;
        self.highlight = target_line.map(|l| (l, l));
        self.scroll = target_line.map(|_| Scroll::Center);
        self.is_loading = true;
        self.error_message = None;
        let marks = self.marks_inbox.clone();
        net.arch_file(path, None, ctx.clone(), move |r| {
            if let Ok(mut m) = marks.lock() {
                *m = Some(r);
            }
        });

        let pending = self.pending_result.clone();
        let for_path = path.to_owned();
        net.fetch_file(path, ctx, move |result| {
            if let Ok(mut lock) = pending.lock() {
                *lock = Some((for_path, result));
            }
        });
    }

    /// Opens `path` with lines `start..=end` highlighted and scrolled to.
    pub fn show_span(&mut self, path: &str, start: usize, end: usize, net: &NetClient, ctx: egui::Context) {
        self.load_file(path, Some(start), net, ctx);
        self.highlight = Some((start, end.max(start)));
    }

    /// Puts the cursor on `line` (the user moved it at `now`).
    pub fn move_cursor(&mut self, line: usize, now: f64) {
        let line = line.clamp(1, self.lines.len().max(1));
        self.target_line = Some(self.unfolded_row_line(line));
        self.cursor_col = None;
        self.highlight = None;
        self.cursor_moved_at = Some(now);
    }

    /// The cursor's file and line once it has rested for `debounce` seconds
    /// since the user moved it (taken once).
    pub fn settled_cursor(&mut self, now: f64, debounce: f64) -> Option<(String, usize)> {
        let moved = self.cursor_moved_at?;
        if now - moved < debounce {
            return None;
        }
        self.cursor_moved_at = None;
        Some((self.current_file.clone(), self.target_line?))
    }

    fn in_highlight(&self, line: usize) -> bool {
        self.highlight.is_some_and(|(a, b)| a <= line && line <= b)
    }

    pub fn set_content(&mut self, content: String) {
        self.lines = content.lines().map(|s| s.to_string()).collect();
        self.content = content;
        self.is_loading = false;
        self.error_message = None;
        if let Some(view) = self.file_view.take() {
            self.set_file_view(view);
        }
    }

    pub fn set_error(&mut self, err: String) {
        self.error_message = Some(err);
        self.is_loading = false;
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
        let line_starts = line_starts(&self.content);
        let in_file: BTreeSet<&ComponentId> = view.components.iter().map(|(c, _)| &c.id).collect();
        let mut calls: Vec<CallSite> = view
            .relations
            .iter()
            .filter(|(r, _)| r.kind == RelationKind::Calls && in_file.contains(&r.source))
            .flat_map(|(r, _)| {
                r.sites.iter().map(|site| {
                    let line = site.start_line as usize;
                    let col = column(&self.content, &line_starts, line, site.start_byte as usize);
                    CallSite { line, col, callee: r.target.clone() }
                })
            })
            .collect();
        calls.sort_by(|a, b| (a.line, a.col).cmp(&(b.line, b.col)).then_with(|| a.callee.cmp(&b.callee)));
        calls.dedup();
        self.calls = calls;
        self.file_view = Some(view);
    }

    /// The innermost definition holding `line`.
    pub fn def_at(&self, line: usize) -> Option<&Def> {
        self.defs.iter().filter(|d| d.start <= line && line <= d.end).min_by_key(|d| d.end - d.start)
    }

    /// What's called on `line` (the call a hint picked first).
    pub fn calls_at(&self, line: usize) -> Vec<ComponentId> {
        let mut here: Vec<&CallSite> = self.calls.iter().filter(|c| c.line == line).collect();
        if let Some(col) = self.cursor_col {
            here.sort_by_key(|c| c.col != col);
        }
        let mut out: Vec<ComponentId> = Vec::new();
        for c in here {
            if !out.contains(&c.callee) {
                out.push(c.callee.clone());
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

    /// Definition names and calls on a line, as hint targets.
    fn symbols_on(&self, line: usize) -> Vec<Symbol> {
        let text = self.lines.get(line.wrapping_sub(1)).map(String::as_str).unwrap_or_default();
        let mut out: Vec<Symbol> = Vec::new();
        for d in self.defs.iter().filter(|d| d.start == line) {
            if let Some(byte) = find_word(text, &d.name) {
                out.push(Symbol { line, col: text[..byte].chars().count(), len: d.name.chars().count() });
            }
        }
        for c in self.calls.iter().filter(|c| c.line == line) {
            if !out.iter().any(|s| s.col == c.col) {
                out.push(Symbol { line, col: c.col, len: 1 });
            }
        }
        out.sort_by_key(|s| s.col);
        out
    }

    fn put_cursor(&mut self, line: usize, how: Scroll, now: f64) {
        self.move_cursor(line, now);
        self.scroll = Some(how);
    }

    fn half_page_lines(&self) -> usize {
        let pitch = if self.row_h > 0.0 { self.row_h } else { 16.0 };
        if self.view_h > 0.0 { ((self.view_h / pitch) / 2.0).floor().max(1.0) as usize } else { 15 }
    }

    /// The next (or previous) definition start from the cursor; `same_level`
    /// keeps to definitions in the same enclosing one. Says why not, when it
    /// can't.
    fn step_def(&mut self, forward: bool, count: usize, same_level: bool, now: f64) -> Result<(), String> {
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
        self.put_cursor(at, Scroll::KeepVisible, now);
        Ok(())
    }

    pub fn render(&mut self, ui: &mut egui::Ui, net: &NetClient, feedback: &mut FeedbackInspectorState, slot_idx: usize) {
        // Replies for a file that's no longer the one shown (an earlier load
        // answering late) are dropped, so they can't use up its scroll.
        let incoming = self.pending_result.lock().ok().and_then(|mut l| l.take());
        if let Some((path, res)) = incoming
            && path == self.current_file
        {
            match res {
                Ok(c) => self.set_content(c),
                Err(e) => self.set_error(e),
            }
        }
        if let Some(Ok(view)) = self.marks_inbox.lock().ok().and_then(|mut m| m.take())
            && view.file == self.current_file
        {
            self.set_file_view(view);
        }

        ui.vertical(|ui| {
            let start_y = ui.cursor().min.y;
            let start_x = ui.cursor().min.x;
            let w = ui.available_width();
            let is_narrow = ui.available_width() < 240.0;
            let copy_label = if is_narrow { "📋" } else { "📋 Copy" };
            // Room for the buttons and the `:line` label next to the path.
            let label_w = if self.target_line.is_some() { 48.0 } else { 0.0 };
            let edit_w = if is_narrow { (ui.available_width() - 80.0 - label_w).max(50.0) } else { (ui.available_width() - 120.0 - label_w).max(80.0) };

            ui.horizontal(|ui| {
                ui.label(RichText::new("📂").size(crate::theme::look().text_title));
                let mut path_edit = self.current_file.clone();
                let resp = ui.add(egui::TextEdit::singleline(&mut path_edit).desired_width(edit_w).font(egui::TextStyle::Monospace));
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let ctx = ui.ctx().clone();
                    self.load_file(&path_edit, None, net, ctx);
                }
                if let Some(target) = self.target_line {
                    ui.label(RichText::new(format!(":{}", target)).monospace().strong().size(crate::theme::look().text_body).color(colors().warn));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(copy_label).clicked() {
                        ui.ctx().copy_text(self.content.clone());
                    }
                    if ui.small_button("🔄").clicked() {
                        let path = self.current_file.clone();
                        let target = self.target_line;
                        let ctx = ui.ctx().clone();
                        self.load_file(&path, target, net, ctx);
                    }
                    if self.is_loading {
                        ui.label(RichText::new(egui_phosphor::regular::DOT).size(crate::theme::look().text_small).color(colors().warn));
                    }
                });
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
                        if ui.small_button("Re-read it").on_hover_text("SPC I f").clicked() {
                            self.reread = true;
                        }
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
                    ui.label(RichText::new("No file content loaded.").size(crate::theme::look().text_label).color(colors().muted));
                    if ui.button("Load Cargo.toml").clicked() {
                        let ctx = ui.ctx().clone();
                        self.load_file("Cargo.toml", None, net, ctx);
                    }
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
            let mut unfold: Option<usize> = None;
            let visible = self.visible_lines();
            // Rows are laid out only when visible, so scrolling to a line is
            // done by offset, once the requested file's lines are in.
            let pitch = row_height + ui.spacing().item_spacing.y;
            let mut area = egui::ScrollArea::both().id_salt("code_lines").auto_shrink([false, false]);
            let mut scrolled: Option<(Scroll, f32)> = None;
            if !self.is_loading
                && let (Some(how), Some(line)) = (self.scroll, self.target_line)
            {
                let row = visible.iter().position(|l| *l >= line).unwrap_or(visible.len().saturating_sub(1));
                let view_h = if self.view_h > 0.0 { self.view_h } else { ui.available_height() };
                area = area.vertical_scroll_offset(scroll_offset(how, row, pitch, self.offset_y, view_h));
                self.scroll = None;
                scrolled = Some((how, view_h));
            }
            let mut drawn = Vec::new();
            let viewer_res = area.show_rows(ui, row_height, visible.len(), |ui, rows| {
                for row in rows {
                    let line_no = visible[row];
                    let idx = line_no - 1;
                    let highlighted = self.in_highlight(line_no);
                    let on_cursor = self.target_line == Some(line_no);
                    let folded = self.defs.iter().filter(|d| d.start == line_no && self.folds.contains(&d.start)).map(|d| d.end - d.start).max();
                    let row = ui.horizontal(|ui| {
                        ui.set_min_height(row_height);
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.allocate_ui_with_layout(Vec2::new(gutter_w, row_height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.set_min_width(gutter_w);
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
                        let line = &self.lines[idx];
                        let mut text = RichText::new(line).monospace().size(crate::theme::look().text_body).color(if highlighted { colors().warn } else { colors().text });
                        if highlighted {
                            text = text.background_color(colors().warn_bg);
                        } else if on_cursor {
                            text = text.background_color(colors().selection);
                        }
                        let label = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
                        drawn.push((line_no, label.rect.left_top()));
                        if label.clicked() {
                            put_cursor = Some(line_no);
                        }
                        if let Some(hidden) = folded
                            && ui.add(egui::Label::new(RichText::new(format!("  … {hidden} lines")).monospace().size(crate::theme::look().text_small).color(colors().muted)).sense(egui::Sense::click())).on_hover_text("Unfold (z o)").clicked()
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
                self.move_cursor(line, ui.input(|i| i.time));
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
}

impl ActionProvider for CodeViewerState {
    fn actions(&self) -> Vec<Action> {
        if self.defs.is_empty() {
            return Vec::new();
        }
        vec![
            Action::new("code.next_def", "Next definition", CodeCommand::Def { forward: true }).focused().group(Group::Navigate),
            Action::new("code.prev_def", "Previous definition", CodeCommand::Def { forward: false }).focused().group(Group::Navigate),
        ]
    }

    fn run(&mut self, action: &Action, _value: Option<String>, cx: &mut RunCx<'_>) {
        if let Some(CodeCommand::Def { forward }) = action.command::<CodeCommand>()
            && let Err(why) = self.step_def(*forward, 1, false, cx.ctx.input(|i| i.time))
        {
            *cx.note = Some(why);
        }
    }
}

impl Navigable for CodeViewerState {
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
        Some(Target::Line { file: self.current_file.clone(), line, within: self.def_at(line).map(|d| d.id.clone()), calls: self.calls_at(line) })
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let now = self.cursor_moved_at.unwrap_or(0.0);
        let visible = self.visible_lines();
        if visible.is_empty() {
            return;
        }
        let row = self.target_line.and_then(|l| visible.iter().position(|v| *v >= l)).unwrap_or(0);
        let to_row = |r: usize| visible[r.min(visible.len() - 1)];
        match motion {
            Motion::Down => self.put_cursor(if self.target_line.is_none() { visible[0] } else { to_row(row + count) }, Scroll::KeepVisible, now),
            Motion::Up => self.put_cursor(to_row(row.saturating_sub(count)), Scroll::KeepVisible, now),
            Motion::HalfDown => self.put_cursor(to_row(row + self.half_page_lines() * count), Scroll::KeepVisible, now),
            Motion::HalfUp => self.put_cursor(to_row(row.saturating_sub(self.half_page_lines() * count)), Scroll::KeepVisible, now),
            Motion::First => self.put_cursor(1, Scroll::KeepVisible, now),
            Motion::Last => self.put_cursor(self.lines.len(), Scroll::KeepVisible, now),
            Motion::Line(n) => self.put_cursor(n, Scroll::Center, now),
            Motion::NextSibling | Motion::PrevSibling => {
                let _ = self.step_def(motion == Motion::NextSibling, count, true, now);
            }
            Motion::Left | Motion::Right => {}
        }
    }

    /// On a call: its definition (the shell opens it). On a folded line: unfold.
    fn enter(&mut self) -> Step {
        match self.target_line {
            Some(line) if !self.calls_at(line).is_empty() => Step::GoTo(crate::nav::GoTo::Code),
            Some(line) if self.folds.contains(&line) => {
                self.folds.remove(&line);
                Step::Done
            }
            _ => Step::Nothing("No call on this line.".into()),
        }
    }

    fn can_enter(&self) -> bool {
        self.target_line.is_some_and(|line| !self.calls_at(line).is_empty() || self.folds.contains(&line))
    }

    fn can_leave(&self) -> bool {
        let line = self.target_line.unwrap_or(1);
        self.defs.iter().any(|d| d.start < line && line <= d.end)
    }

    fn enter_label(&self) -> Option<String> {
        let line = self.target_line?;
        if self.folds.contains(&line) {
            return Some("Unfold".into());
        }
        let calls = self.calls_at(line);
        let name = |id: &ComponentId| id.as_str().rsplit([':', '.']).next().unwrap_or(id.as_str()).to_owned();
        match calls.as_slice() {
            [one] => Some(format!("Go to {}", name(one))),
            [] => None,
            _ => Some(format!("Go to one of {} calls", calls.len())),
        }
    }

    /// Out to the first line of the definition the cursor is in (then the
    /// one around that).
    fn leave(&mut self) -> Step {
        let line = self.target_line.unwrap_or(1);
        let around = self.defs.iter().filter(|d| d.start < line && line <= d.end).min_by_key(|d| d.end - d.start).map(|d| d.start);
        match around {
            Some(start) => {
                let now = self.cursor_moved_at.unwrap_or(0.0);
                self.put_cursor(start, Scroll::KeepVisible, now);
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
        let now = self.cursor_moved_at.unwrap_or(0.0);
        let defs = &self.defs;
        self.folds.retain(|start| !defs.iter().any(|d| d.start == *start && d.start < line && line <= d.end));
        self.put_cursor(line, Scroll::Center, now);
        self.cursor_col = col;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use arch_graph::{CodeAnchor, Relation, RepoPath, Span, Stage, Status};

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
            egui::CentralPanel::default().show(ctx, |ui| code.render(ui, &net, &mut feedback, 0));
        });
        placed(&output)
    }

    // req: P9.15
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
    fn the_cursor_settles_before_follow_syncs() {
        let mut code = CodeViewerState::default();
        code.set_content("a\nb\nc\n".into());
        code.move_cursor(2, 10.0);
        assert_eq!(code.settled_cursor(10.1, 0.25), None, "still moving");
        assert_eq!(code.settled_cursor(10.3, 0.25), Some(("Cargo.toml".into(), 2)));
        assert_eq!(code.settled_cursor(11.0, 0.25), None, "taken once");
        code.move_cursor(99, 12.0);
        assert_eq!(code.target_line, Some(3), "clamped to the file");
    }

    // req: P9.15, N.5, N.6, N.7
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
        *code.pending_result.lock().unwrap() = Some(("Cargo.toml".into(), Ok("[package]\nname = \"x\"\n".into())));
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
        code
    }

    // req: N.16, N.10
    #[test]
    fn the_cursor_knows_its_definition_and_calls_and_steps_between_definitions() {
        let mut code = sample();
        assert_eq!(code.defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["run", "step"], "the file itself isn't a definition");
        assert_eq!(code.calls, vec![CallSite { line: 3, col: 8, callee: ComponentId::new("rust:a::step") }, CallSite { line: 4, col: 8, callee: ComponentId::new("rust:x::log") }]);
        code.motion(Motion::Line(3), 1);
        let Some(Target::Line { within, calls, .. }) = code.target() else { panic!() };
        assert_eq!((within.unwrap().as_str(), calls), ("rust:a::run", vec![ComponentId::new("rust:a::step")]));
        assert_eq!(code.enter(), Step::GoTo(crate::nav::GoTo::Code), "Enter on a call goes to what it calls");
        code.motion(Motion::Line(7), 1);
        assert!(matches!(code.enter(), Step::Nothing(_)), "no call here");
        assert_eq!(code.leave(), Step::Done);
        assert_eq!(code.target_line, Some(6), "Backspace: out to the definition's first line");
        // ]f / [f: definitions in order.
        code.motion(Motion::First, 1);
        assert!(code.step_def(true, 1, false, 0.0).is_ok());
        assert_eq!(code.target_line, Some(2));
        assert!(code.step_def(true, 1, false, 0.0).is_ok());
        assert_eq!(code.target_line, Some(6));
        assert!(code.step_def(true, 1, false, 0.0).is_err(), "no next one: says so");
        assert!(code.step_def(false, 1, false, 0.0).is_ok());
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
        let Some(Target::Line { calls, .. }) = code.target() else { panic!() };
        assert_eq!(calls, vec![ComponentId::new("rust:x::log")]);
    }

    // req: N.11, N.16, N.8
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
}
