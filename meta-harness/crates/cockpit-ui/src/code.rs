use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use arch_graph::{Alignment, ComponentId, FileView, RelationKind};
use egui::{Frame, Margin, Pos2, Rect, RichText, Stroke, Vec2};

use crate::feedback::FeedbackInspectorState;
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

/// Gutter marks by line, from the components in a file.
pub fn gutter_marks(view: &FileView) -> BTreeMap<usize, Vec<Mark>> {
    let mut marks: BTreeMap<usize, Vec<Mark>> = BTreeMap::new();
    let mut start_of: BTreeMap<&ComponentId, usize> = BTreeMap::new();
    let in_file: Vec<&arch_graph::Component> = view.components.iter().map(|(c, _)| c).filter(|c| c.anchor.as_ref().is_some_and(|a| a.file.as_str() == view.file)).collect();
    // The file itself (its span holds every other component) gets no mark.
    let whole_file = in_file.iter().find(|c| {
        let span = c.anchor.as_ref().map(|a| a.span).unwrap_or_default();
        in_file.iter().all(|o| o.anchor.as_ref().is_some_and(|a| span.contains(&a.span)))
    });
    for c in &in_file {
        let Some(anchor) = &c.anchor else { continue };
        if in_file.len() > 1 && whole_file.is_some_and(|w| w.id == c.id) {
            continue;
        }
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

pub struct CodeViewerState {
    pub current_file: String,
    pub content: String,
    pub lines: Vec<String>,
    pub target_line: Option<usize>,
    /// Lines shown highlighted (e.g. the component opened from the graph).
    pub highlight: Option<(usize, usize)>,
    pub is_loading: bool,
    pub error_message: Option<String>,
    /// The fetched file's contents, with the path they're for.
    pending_result: Arc<Mutex<Option<(String, Result<String, String>)>>>,
    marks: BTreeMap<usize, Vec<Mark>>,
    marks_inbox: Arc<Mutex<Option<NetResult<FileView>>>>,
    /// Where to scroll once the right file's lines are in.
    scroll: Option<Scroll>,
    /// The scroll position and viewport height last frame.
    offset_y: f32,
    view_h: f32,
    /// The height a drawn row really takes (labels can be taller than the
    /// font's line height); rows are laid out and scrolled by it.
    row_h: f32,
    /// Asks the app to find this file:line's component in the graph.
    pub reveal: Option<(String, usize)>,
    /// Asks the app to select this component in the graph.
    pub select: Option<ComponentId>,
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
            highlight: None,
            is_loading: false,
            error_message: None,
            pending_result: Arc::new(Mutex::new(None)),
            marks: BTreeMap::new(),
            marks_inbox: Arc::default(),
            scroll: None,
            offset_y: 0.0,
            view_h: 0.0,
            row_h: 0.0,
            reveal: None,
            select: None,
            cursor_moved_at: None,
        }
    }
}

impl CodeViewerState {
    pub fn load_file(&mut self, path: &str, target_line: Option<usize>, net: &NetClient, ctx: egui::Context) {
        self.current_file = path.to_string();
        self.target_line = target_line;
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
        let ctx_clone = ctx.clone();
        let for_path = path.to_owned();
        net.fetch_file(path, ctx_clone, move |result| {
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
        self.target_line = Some(line.clamp(1, self.lines.len().max(1)));
        self.highlight = self.target_line.map(|l| (l, l));
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
    }

    pub fn set_error(&mut self, err: String) {
        self.error_message = Some(err);
        self.is_loading = false;
    }

    pub fn render(&mut self, ui: &mut egui::Ui, net: &NetClient, feedback: &mut FeedbackInspectorState, slot_idx: usize) {
        // Drain any incoming file contents from background fetch
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
            self.marks = gutter_marks(&view);
        }

        ui.vertical(|ui| {
            // Header bar with file path and actions
            let start_y = ui.cursor().min.y;
            let start_x = ui.cursor().min.x;
            let w = ui.available_width();
            let is_narrow = ui.available_width() < 240.0;
            let copy_label = if is_narrow { "📋" } else { "📋 Copy" };
            // Room for the buttons and the `:line` label next to the path.
            let label_w = if self.target_line.is_some() { 48.0 } else { 0.0 };
            let edit_w = if is_narrow {
                (ui.available_width() - 80.0 - label_w).max(50.0)
            } else {
                (ui.available_width() - 120.0 - label_w).max(80.0)
            };

            ui.horizontal(|ui| {
                ui.label(RichText::new("📂").size(13.0));
                let mut path_edit = self.current_file.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut path_edit)
                        .desired_width(edit_w)
                        .font(egui::TextStyle::Monospace)
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let ctx = ui.ctx().clone();
                    self.load_file(&path_edit, None, net, ctx);
                }

                if let Some(target) = self.target_line {
                    ui.label(RichText::new(format!(":{}", target)).monospace().strong().size(11.0).color(colors().warn));
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
                        ui.label(RichText::new(egui_phosphor::regular::DOT).size(10.0).color(colors().warn));
                    }
                });
            });
            let end_y = ui.cursor().min.y;
            let toolbar_rect = Rect::from_min_size(Pos2::new(start_x, start_y), Vec2::new(w, (end_y - start_y).max(22.0)));
            feedback.register_inspectable(ui, &format!("slot_{}:code:toolbar", slot_idx + 1), toolbar_rect);
            ui.separator();

            if let Some(err) = &self.error_message {
                Frame::NONE
                    .fill(colors().error_bg)
                    .stroke(Stroke::new(1.0, colors().error))
                    .corner_radius(4)
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(format!("Error: {}", err)).color(colors().error).size(11.0));
                    });
                return;
            }

            if self.lines.is_empty() && !self.is_loading {
                ui.vertical_centered(|ui| {
                    ui.add_space(30.0);
                    ui.label(RichText::new("No file content loaded.").size(12.0).color(colors().muted));
                    if ui.button("Load Cargo.toml").clicked() {
                        let ctx = ui.ctx().clone();
                        self.load_file("Cargo.toml", None, net, ctx);
                    }
                });
                return;
            }

            // One row per line: the gutter (number and marks) and the code
            // share the row, so marks always sit next to their line.
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace).max(14.0).max(self.row_h);
            let mut measured: f32 = 0.0;
            let gutter_w = ui.fonts_mut(|f| f.layout_no_wrap("00000".into(), egui::FontId::monospace(11.0), colors().faint).size().x) + 3.0 * 16.0;
            let mut clicked: Option<ComponentId> = None;
            let mut put_cursor: Option<usize> = None;
            // Rows are laid out only when visible, so scrolling to a line is
            // done by offset, once the requested file's lines are in.
            let pitch = row_height + ui.spacing().item_spacing.y;
            let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
            if !self.is_loading
                && let (Some(how), Some(line)) = (self.scroll, self.target_line)
            {
                let view_h = if self.view_h > 0.0 { self.view_h } else { ui.available_height() };
                let offset = scroll_offset(how, line, pitch, self.offset_y, view_h);
                area = area.vertical_scroll_offset(offset);
                self.scroll = None;
            }
            let viewer_res = area.show_rows(ui, row_height, self.lines.len(), |ui, rows| {
                for idx in rows {
                    let line_no = idx + 1;
                    let highlighted = self.in_highlight(line_no);
                    let row = ui.horizontal(|ui| {
                        ui.set_min_height(row_height);
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.allocate_ui_with_layout(Vec2::new(gutter_w, row_height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.set_min_width(gutter_w);
                            let color = if highlighted { colors().warn } else { colors().faint };
                            ui.label(RichText::new(format!("{line_no:>4}")).monospace().size(11.0).color(color));
                            for mark in self.marks.get(&line_no).into_iter().flatten() {
                                let (icon, color, tip, id) = mark_look(mark);
                                if ui.add(egui::Label::new(RichText::new(icon).size(11.0).color(color)).sense(egui::Sense::click())).on_hover_text(tip).clicked() {
                                    clicked = Some(id.clone());
                                }
                            }
                        });
                        ui.separator();
                        let line = &self.lines[idx];
                        let text = if highlighted { RichText::new(line).monospace().size(11.0).color(colors().warn).background_color(colors().warn_bg) } else { RichText::new(line).monospace().size(11.0).color(colors().text) };
                        if ui.add(egui::Label::new(text).sense(egui::Sense::click())).clicked() {
                            put_cursor = Some(line_no);
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
            if clicked.is_some() {
                self.select = clicked;
            }
            if let Some(line) = put_cursor {
                self.move_cursor(line, ui.input(|i| i.time));
            }
            feedback.register_inspectable(ui, &format!("slot_{}:code:viewer", slot_idx + 1), viewer_res.inner_rect);
        });
    }
}

impl CodeViewerState {
    fn run_command(&mut self, command: CodeCommand, now: f64) {
        let line = self.target_line.unwrap_or(1);
        match command {
            CodeCommand::Down => {
                self.move_cursor(line + 5, now);
                self.scroll = Some(Scroll::KeepVisible);
            }
            CodeCommand::Up => {
                self.move_cursor(line.saturating_sub(5), now);
                self.scroll = Some(Scroll::KeepVisible);
            }
            CodeCommand::Reveal => self.reveal = Some((self.current_file.clone(), self.highlight.map_or(line, |(a, _)| a))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scroll {
    /// Bring the line to the middle (opening a target).
    Center,
    /// Scroll only as far as needed to keep the line on screen (cursor moves).
    KeepVisible,
}

/// The scroll offset that shows `line` (1-based) the way `how` asks, given
/// the row pitch, the current offset and the viewport height.
fn scroll_offset(how: Scroll, line: usize, pitch: f32, current: f32, view_h: f32) -> f32 {
    let top = line.saturating_sub(1) as f32 * pitch;
    // A couple of rows of context around a cursor kept in view.
    let margin = 2.0 * pitch;
    match how {
        Scroll::Center => (top - (view_h - pitch) / 2.0).max(0.0),
        Scroll::KeepVisible if top < current + margin => (top - margin).max(0.0),
        Scroll::KeepVisible if top + pitch > current + view_h - margin => (top + pitch + margin - view_h).max(0.0),
        Scroll::KeepVisible => current,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CodeCommand {
    Down,
    Up,
    Reveal,
}

impl crate::actions::ActionProvider for CodeViewerState {
    fn actions(&self) -> Vec<crate::actions::Action> {
        use crate::actions::Action;
        vec![
            Action::new("code.down", "Scroll down", CodeCommand::Down).focused(),
            Action::new("code.up", "Scroll up", CodeCommand::Up).focused(),
            Action::new("code.reveal", "Show this code's component in the graph", CodeCommand::Reveal).focused(),
        ]
    }

    fn run(&mut self, action: &crate::actions::Action, _value: Option<String>, cx: &mut crate::actions::RunCx<'_>) {
        if let Some(command) = action.command::<CodeCommand>() {
            self.run_command(*command, cx.ctx.input(|i| i.time));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // req: P9.15
    #[test]
    fn gutter_marks_sit_on_the_row_of_their_line() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let net = NetClient::new("http://localhost:0");
        let mut feedback = FeedbackInspectorState::default();
        let mut code = CodeViewerState::default();
        // Lines of different heights' worth of content, marks deep in the file.
        let text: String = (1..=40).map(|i| if i % 3 == 0 { format!("    let value_{i} = compute({i}); // a longer line\n") } else { format!("fn f{i}() {{}}\n") }).collect();
        code.set_content(text);
        let id = ComponentId::new("rust:x::f29");
        code.marks.insert(29, vec![Mark::Starts(id.clone(), "f29".into()), Mark::Placeholder(id)]);
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let output = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 2000.0))), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| code.render(ui, &net, &mut feedback, 0));
            });
            shapes = placed(&output);
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

    // req: P9.15
    #[test]
    fn opening_a_target_off_screen_scrolls_it_into_view_and_the_cursor_stays_visible() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let net = NetClient::new("http://localhost:0");
        let mut feedback = FeedbackInspectorState::default();
        let mut code = CodeViewerState::default();
        let text: String = (1..=900).map(|i| format!("fn line_{i}() {{}}\n")).collect();
        let frame = |code: &mut CodeViewerState, feedback: &mut FeedbackInspectorState| {
            let output = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| code.render(ui, &net, feedback, 0));
            });
            placed(&output).into_iter().map(|(t, _)| t).collect::<Vec<_>>()
        };
        code.set_content(text.clone());
        frame(&mut code, &mut feedback);
        // A target far down: the file is reloaded (lines stay until the reply),
        // and the scroll waits for the content, then lands on the target.
        // (as load_file leaves it; no network in tests). An earlier load's
        // reply arriving late for another file must not use up the scroll.
        code.current_file = "src/big.rs".into();
        *code.pending_result.lock().unwrap() = Some(("Cargo.toml".into(), Ok("[package]\nname = \"x\"\n".into())));
        code.target_line = Some(700);
        code.highlight = Some((700, 700));
        code.scroll = Some(Scroll::Center);
        code.is_loading = true;
        frame(&mut code, &mut feedback);
        code.set_content(text.clone());
        let mut drawn = Vec::new();
        for _ in 0..3 {
            drawn = frame(&mut code, &mut feedback);
        }
        assert!(drawn.iter().any(|t| t == "fn line_700() {}"), "the target line is on screen");
        assert!(!drawn.iter().any(|t| t == "fn line_10() {}"), "the top of the file isn't");
        // Moving the cursor past the bottom scrolls just enough to keep it visible.
        for _ in 0..12 {
            code.run_command(CodeCommand::Down, 0.0);
            for _ in 0..2 {
                drawn = frame(&mut code, &mut feedback);
            }
        }
        let cursor = code.target_line.unwrap();
        assert_eq!(cursor, 760);
        assert!(drawn.iter().any(|t| *t == format!("fn line_{cursor}() {{}}")), "the cursor line stays on screen");
    }

    #[test]
    fn scroll_offsets_center_targets_and_keep_the_cursor_visible() {
        assert_eq!(scroll_offset(Scroll::Center, 1, 20.0, 0.0, 400.0), 0.0);
        assert_eq!(scroll_offset(Scroll::Center, 101, 20.0, 0.0, 420.0), 2000.0 - 200.0);
        assert_eq!(scroll_offset(Scroll::KeepVisible, 10, 20.0, 0.0, 400.0), 0.0, "already visible");
        assert_eq!(scroll_offset(Scroll::KeepVisible, 30, 20.0, 0.0, 400.0), 600.0 + 40.0 - 400.0, "just below: scroll by what's needed, with context");
        assert_eq!(scroll_offset(Scroll::KeepVisible, 5, 20.0, 500.0, 400.0), 40.0, "above: bring near the top, with context");
    }
}
