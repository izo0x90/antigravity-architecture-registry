use egui::{Color32, Frame, Margin, Pos2, Rect, RichText, Stroke, Vec2};
use std::collections::BTreeMap;

use crate::feedback::FeedbackInspectorState;
use crate::graph::ArchComponent;

pub struct ArchitectureTreeState {
    pub search_filter: String,
}

impl Default for ArchitectureTreeState {
    fn default() -> Self {
        Self {
            search_filter: String::new(),
        }
    }
}

impl ArchitectureTreeState {
    pub fn handle_input(&mut self, _i: &egui::InputState, _keymap: &crate::keymap::TreeKeymap) {
        // Tree-specific shortcuts
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        components: &BTreeMap<String, ArchComponent>,
        feedback: &mut FeedbackInspectorState,
        slot_idx: usize,
    ) {
        ui.vertical(|ui| {
            // Header bar
            let start_y = ui.cursor().min.y;
            let start_x = ui.cursor().min.x;
            let w = ui.available_width();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} Requirements & Task Tree", egui_phosphor::regular::TREE_STRUCTURE)).strong().size(12.0).color(Color32::from_rgb(140, 200, 255)));

                let mut total_tasks = 0;
                let mut completed_tasks = 0;
                for comp in components.values() {
                    for task in &comp.modification_tasks {
                        total_tasks += 1;
                        if task.completed {
                            completed_tasks += 1;
                        }
                    }
                }

                let badge_color = if completed_tasks == total_tasks && total_tasks > 0 {
                    Color32::from_rgb(80, 200, 120)
                } else {
                    Color32::from_rgb(250, 180, 50)
                };
                ui.label(RichText::new(format!("{}/{} Tasks Complete", completed_tasks, total_tasks)).monospace().size(11.0).color(badge_color));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.search_filter).desired_width(120.0).hint_text("Filter components..."));
                });
            });
            let end_y = ui.cursor().min.y;
            let toolbar_rect = Rect::from_min_size(Pos2::new(start_x, start_y), Vec2::new(w, (end_y - start_y).max(22.0)));
            feedback.register_inspectable(ui, &format!("slot_{}:tree:toolbar", slot_idx + 1), toolbar_rect);
            ui.separator();

            if components.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(30.0);
                    ui.label(RichText::new("No components loaded from system_architecture.json").color(Color32::GRAY).size(12.0));
                });
                return;
            }

            let tree_res = egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (id, comp) in components {
                        if !self.search_filter.is_empty() {
                            let match_id = id.to_lowercase().contains(&self.search_filter.to_lowercase());
                            let match_name = comp.name.to_lowercase().contains(&self.search_filter.to_lowercase());
                            if !match_id && !match_name {
                                continue;
                            }
                        }

                        let (stage_badge, stage_color) = match comp.stage.as_str() {
                            "implemented" => (format!("{} IMPLEMENTED", egui_phosphor::regular::CHECK), Color32::from_rgb(80, 200, 120)),
                            "in_progress" => (format!("{} IN PROGRESS", egui_phosphor::regular::LIGHTNING), Color32::from_rgb(100, 160, 240)),
                            _ => (format!("{} PENDING", egui_phosphor::regular::CIRCLE), Color32::GRAY),
                        };

                        let header_text = format!("{} ({})", comp.name, id);
                        ui.collapsing(RichText::new(header_text).strong().size(12.0), |ui| {
                            Frame::NONE
                                .fill(Color32::from_rgb(16, 20, 26))
                                .stroke(Stroke::new(1.0, Color32::from_rgb(30, 36, 48)))
                                .corner_radius(4)
                                .inner_margin(Margin::same(8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(stage_badge).size(10.0).strong().color(stage_color));
                                        ui.label(RichText::new(format!("Type: {}", comp.comp_type)).monospace().size(10.0).color(Color32::LIGHT_GRAY));
                                    });

                                    if !comp.description.is_empty() {
                                        ui.add_space(2.0);
                                        ui.label(RichText::new(&comp.description).size(11.0).color(Color32::from_rgb(180, 190, 205)));
                                    }

                                    if !comp.modification_tasks.is_empty() {
                                        ui.add_space(4.0);
                                        ui.label(RichText::new("Modification Invariants & Tasks:").strong().size(11.0).color(Color32::from_rgb(250, 180, 50)));

                                        for (t_idx, task) in comp.modification_tasks.iter().enumerate() {
                                            ui.horizontal_top(|ui| {
                                                let (icon, color) = if task.completed {
                                                    (egui_phosphor::regular::CHECK, Color32::from_rgb(80, 200, 120))
                                                } else {
                                                    (egui_phosphor::regular::CIRCLE, Color32::from_rgb(200, 100, 100))
                                                };
                                                ui.label(RichText::new(icon).strong().size(11.0).color(color));
                                                ui.label(RichText::new(format!("{}. {}", t_idx + 1, task.task)).size(11.0).color(Color32::from_rgb(210, 215, 225)));
                                            });
                                        }
                                    }
                                });
                            ui.add_space(4.0);
                        });
                        ui.add_space(2.0);
                    }
                });
            feedback.register_inspectable(ui, &format!("slot_{}:tree:tasks", slot_idx + 1), tree_res.inner_rect);
        });
    }
}
