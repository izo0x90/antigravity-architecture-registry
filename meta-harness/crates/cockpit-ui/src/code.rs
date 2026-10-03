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

pub struct CodeViewerState {
    pub current_file: String,
    pub content: String,
    pub lines: Vec<String>,
    pub target_line: Option<usize>,
    /// Lines shown highlighted (e.g. the component opened from the graph).
    pub highlight: Option<(usize, usize)>,
    pub is_loading: bool,
    pub error_message: Option<String>,
    pending_result: Arc<Mutex<Option<Result<String, String>>>>,
    marks: BTreeMap<usize, Vec<Mark>>,
    marks_inbox: Arc<Mutex<Option<NetResult<FileView>>>>,
    scroll_pending: bool,
    /// Asks the app to find this file:line's component in the graph.
    pub reveal: Option<(String, usize)>,
    /// Asks the app to select this component in the graph.
    pub select: Option<ComponentId>,
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
            scroll_pending: false,
            reveal: None,
            select: None,
        }
    }
}

impl CodeViewerState {
    pub fn load_file(&mut self, path: &str, target_line: Option<usize>, net: &NetClient, ctx: egui::Context) {
        self.current_file = path.to_string();
        self.target_line = target_line;
        self.highlight = target_line.map(|l| (l, l));
        self.scroll_pending = target_line.is_some();
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
        net.fetch_file(path, ctx_clone, move |result| {
            if let Ok(mut lock) = pending.lock() {
                *lock = Some(result);
            }
        });
    }

    /// Opens `path` with lines `start..=end` highlighted and scrolled to.
    pub fn show_span(&mut self, path: &str, start: usize, end: usize, net: &NetClient, ctx: egui::Context) {
        self.load_file(path, Some(start), net, ctx);
        self.highlight = Some((start, end.max(start)));
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
        let incoming = self.pending_result.lock().ok().and_then(|mut l| l.take());
        if let Some(res) = incoming {
            match res {
                Ok(c) => self.set_content(c),
                Err(e) => self.set_error(e),
            }
        }
        if let Some(Ok(view)) = self.marks_inbox.lock().ok().and_then(|mut m| m.take()) {
            self.marks = gutter_marks(&view);
        }

        ui.vertical(|ui| {
            // Header bar with file path and actions
            let start_y = ui.cursor().min.y;
            let start_x = ui.cursor().min.x;
            let w = ui.available_width();
            let is_narrow = ui.available_width() < 240.0;
            let copy_label = if is_narrow { "📋" } else { "📋 Copy" };
            let edit_w = if is_narrow {
                (ui.available_width() - 80.0).max(50.0)
            } else {
                (ui.available_width() - 120.0).max(80.0)
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
                        ui.label(RichText::new("●").size(10.0).color(colors().warn));
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

            // Gutter + Code Viewer
            let viewer_res = egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        // Line numbers, plus marks linking lines to the graph.
                        let mut clicked: Option<ComponentId> = None;
                        ui.vertical(|ui| {
                            for line_no in 1..=self.lines.len() {
                                let marks = self.marks.get(&line_no).cloned().unwrap_or_default();
                                let highlighted = self.in_highlight(line_no);
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    let color = if highlighted { colors().warn } else { colors().faint };
                                    ui.label(RichText::new(format!("{line_no:>4}")).monospace().size(11.0).color(color));
                                    for mark in &marks {
                                        let (icon, color, tip, id) = match mark {
                                            Mark::Starts(id, name) => (egui_phosphor::regular::DIAMOND, colors().accent, format!("{name}: show in the graph"), id),
                                            Mark::Placeholder(id) => (egui_phosphor::regular::HOURGLASS, colors().warn, "placeholder body (skeleton)".to_owned(), id),
                                            Mark::MissingCall(id, target) => (egui_phosphor::regular::WARNING, colors().error, format!("planned call to {target} is missing"), id),
                                        };
                                        if ui.add(egui::Label::new(RichText::new(icon).size(11.0).color(color)).sense(egui::Sense::click())).on_hover_text(tip).clicked() {
                                            clicked = Some(id.clone());
                                        }
                                    }
                                });
                            }
                        });
                        if clicked.is_some() {
                            self.select = clicked;
                        }

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(6.0);

                        // Code text lines
                        ui.vertical(|ui| {
                            let scroll_to = if self.scroll_pending { self.target_line } else { None };
                            for (idx, line) in self.lines.iter().enumerate() {
                                let line_no = idx + 1;
                                let response = if self.in_highlight(line_no) {
                                    Frame::NONE
                                        .fill(colors().warn_bg)
                                        .corner_radius(2)
                                        .inner_margin(Margin::symmetric(2, 1))
                                        .show(ui, |ui| {
                                            ui.label(RichText::new(line).monospace().size(11.0).color(colors().warn));
                                        })
                                        .response
                                } else {
                                    ui.label(RichText::new(line).monospace().size(11.0).color(colors().text))
                                };
                                if scroll_to == Some(line_no) {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                }
                            }
                        });
                    });
                });
            feedback.register_inspectable(ui, &format!("slot_{}:code:viewer", slot_idx + 1), viewer_res.inner_rect);
            if !self.lines.is_empty() {
                self.scroll_pending = false;
            }
        });
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

    fn run(&mut self, action: &crate::actions::Action, _value: Option<String>, _cx: &mut crate::actions::RunCx<'_>) {
        let line = self.target_line.unwrap_or(1);
        match action.command::<CodeCommand>() {
            Some(CodeCommand::Down) => {
                self.target_line = Some((line + 5).min(self.lines.len().max(1)));
                self.highlight = self.target_line.map(|l| (l, l));
                self.scroll_pending = true;
            }
            Some(CodeCommand::Up) => {
                self.target_line = Some(line.saturating_sub(5).max(1));
                self.highlight = self.target_line.map(|l| (l, l));
                self.scroll_pending = true;
            }
            Some(CodeCommand::Reveal) => self.reveal = Some((self.current_file.clone(), self.highlight.map_or(line, |(a, _)| a))),
            None => {}
        }
    }
}
