use std::sync::{Arc, Mutex};
use egui::{Color32, Frame, Margin, Pos2, Rect, RichText, Stroke, Vec2};

use crate::feedback::FeedbackInspectorState;
use crate::net::NetClient;

pub struct CodeViewerState {
    pub current_file: String,
    pub content: String,
    pub lines: Vec<String>,
    pub target_line: Option<usize>,
    pub is_loading: bool,
    pub error_message: Option<String>,
    pending_result: Arc<Mutex<Option<Result<String, String>>>>,
}

impl Default for CodeViewerState {
    fn default() -> Self {
        Self {
            current_file: "Cargo.toml".to_string(),
            content: String::new(),
            lines: Vec::new(),
            target_line: None,
            is_loading: false,
            error_message: None,
            pending_result: Arc::new(Mutex::new(None)),
        }
    }
}

impl CodeViewerState {
    pub fn load_file(&mut self, path: &str, target_line: Option<usize>, net: &NetClient, ctx: egui::Context) {
        self.current_file = path.to_string();
        self.target_line = target_line;
        self.is_loading = true;
        self.error_message = None;

        let pending = self.pending_result.clone();
        let ctx_clone = ctx.clone();
        net.fetch_file(path, ctx_clone, move |result| {
            if let Ok(mut lock) = pending.lock() {
                *lock = Some(result);
            }
        });
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

    pub fn handle_input(&mut self, i: &egui::InputState, keymap: &crate::keymap::CodeKeymap) {
        if keymap.scroll_down.is_pressed(i) {
            let next = self.target_line.unwrap_or(1) + 5;
            self.target_line = Some(next.min(self.lines.len().max(1)));
        }
        if keymap.scroll_up.is_pressed(i) {
            let prev = self.target_line.unwrap_or(1).saturating_sub(5);
            self.target_line = Some(prev.max(1));
        }
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
                    ui.label(RichText::new(format!(":{}", target)).monospace().strong().size(11.0).color(Color32::from_rgb(250, 180, 50)));
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
                        ui.label(RichText::new("●").size(10.0).color(Color32::YELLOW));
                    }
                });
            });
            let end_y = ui.cursor().min.y;
            let toolbar_rect = Rect::from_min_size(Pos2::new(start_x, start_y), Vec2::new(w, (end_y - start_y).max(22.0)));
            feedback.register_inspectable(ui, &format!("slot_{}:code:toolbar", slot_idx + 1), toolbar_rect);
            ui.separator();

            if let Some(err) = &self.error_message {
                Frame::NONE
                    .fill(Color32::from_rgb(40, 20, 20))
                    .stroke(Stroke::new(1.0, Color32::RED))
                    .corner_radius(4)
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(format!("Error: {}", err)).color(Color32::from_rgb(255, 120, 120)).size(11.0));
                    });
                return;
            }

            if self.lines.is_empty() && !self.is_loading {
                ui.vertical_centered(|ui| {
                    ui.add_space(30.0);
                    ui.label(RichText::new("No file content loaded.").size(12.0).color(Color32::GRAY));
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
                        // Line number gutter
                        ui.vertical(|ui| {
                            for (idx, _) in self.lines.iter().enumerate() {
                                let line_no = idx + 1;
                                let is_target = self.target_line == Some(line_no);
                                let (color, text) = if is_target {
                                    (Color32::from_rgb(250, 180, 50), RichText::new(format!("{:>4} ▶", line_no)).strong())
                                } else {
                                    (Color32::from_rgb(80, 90, 110), RichText::new(format!("{:>4}  ", line_no)))
                                };
                                ui.label(text.monospace().size(11.0).color(color));
                            }
                        });

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(6.0);

                        // Code text lines
                        ui.vertical(|ui| {
                            for (idx, line) in self.lines.iter().enumerate() {
                                let line_no = idx + 1;
                                let is_target = self.target_line == Some(line_no);
                                if is_target {
                                    Frame::NONE
                                        .fill(Color32::from_rgb(45, 40, 25))
                                        .corner_radius(2)
                                        .inner_margin(Margin::symmetric(2, 1))
                                        .show(ui, |ui| {
                                            ui.label(RichText::new(line).monospace().size(11.0).color(Color32::from_rgb(255, 230, 150)));
                                        });
                                } else {
                                    ui.label(RichText::new(line).monospace().size(11.0).color(Color32::from_rgb(220, 225, 235)));
                                }
                            }
                        });
                    });
                });
            feedback.register_inspectable(ui, &format!("slot_{}:code:viewer", slot_idx + 1), viewer_res.inner_rect);
        });
    }
}
