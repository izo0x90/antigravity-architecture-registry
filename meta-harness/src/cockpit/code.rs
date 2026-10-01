use egui::{Color32, RichText, Ui};

pub struct CodeViewerState {
    pub file_path: String,
    pub content: String,
    pub lines: Vec<String>,
    pub highlight_line: Option<usize>,
}

impl Default for CodeViewerState {
    fn default() -> Self {
        let initial_path = "Cargo.toml".to_string();
        let content = std::fs::read_to_string(&initial_path).unwrap_or_else(|_| "# Select a file to view".to_string());
        let lines = content.lines().map(|s| s.to_string()).collect();

        Self {
            file_path: initial_path,
            content,
            lines,
            highlight_line: None,
        }
    }
}

impl CodeViewerState {
    pub fn load_file(&mut self, path: String, content: String, highlight_line: Option<usize>) {
        self.file_path = path;
        self.lines = content.lines().map(|s| s.to_string()).collect();
        self.content = content;
        self.highlight_line = highlight_line;
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            // Header bar
            ui.horizontal(|ui| {
                ui.label(RichText::new("📜").size(12.0));
                ui.label(RichText::new(&self.file_path).strong().color(Color32::from_rgb(147, 197, 253)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{} lines", self.lines.len())).size(11.0).color(Color32::GRAY));
                });
            });
            ui.separator();

            // Code Content with Line Number Gutter
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (idx, line_str) in self.lines.iter().enumerate() {
                        let line_no = idx + 1;
                        let is_highlight = self.highlight_line == Some(line_no);

                        ui.horizontal(|ui| {
                            // Line Number Gutter
                            let gutter_text = RichText::new(format!("{:4} │ ", line_no))
                                .monospace()
                                .size(11.0)
                                .color(if is_highlight {
                                    Color32::from_rgb(245, 158, 11)
                                } else {
                                    Color32::from_rgb(100, 116, 139)
                                });
                            ui.label(gutter_text);

                            // Code Line
                            let line_text = RichText::new(line_str)
                                .monospace()
                                .size(11.0)
                                .color(if is_highlight {
                                    Color32::from_rgb(254, 240, 138)
                                } else {
                                    Color32::from_rgb(226, 232, 240)
                                });
                            ui.label(line_text);
                        });
                    }
                });
        });
    }
}
