use egui::{Frame, Margin, Pos2, Rect, RichText, Stroke, Vec2};
use std::collections::BTreeMap;

use crate::feedback::FeedbackInspectorState;
use crate::graph::ArchComponent;
use crate::theme::colors;

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
                ui.label(RichText::new(format!("{} Requirements & Task Tree", egui_phosphor::regular::TREE_STRUCTURE)).strong().size(12.0).color(colors().info));

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
                    colors().ok
                } else {
                    colors().warn
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
                    ui.label(RichText::new("No components loaded from the registry").color(colors().muted).size(12.0));
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
                            "implemented" => (format!("{} IMPLEMENTED", egui_phosphor::regular::CHECK), colors().ok),
                            "in_progress" => (format!("{} IN PROGRESS", egui_phosphor::regular::LIGHTNING), colors().accent),
                            _ => (format!("{} PENDING", egui_phosphor::regular::CIRCLE), colors().muted),
                        };

                        let header_text = format!("{} ({})", comp.name, id);
                        ui.collapsing(RichText::new(header_text).strong().size(12.0), |ui| {
                            Frame::NONE
                                .fill(colors().bg)
                                .stroke(Stroke::new(1.0, colors().border))
                                .corner_radius(4)
                                .inner_margin(Margin::same(8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(stage_badge).size(10.0).strong().color(stage_color));
                                        ui.label(RichText::new(format!("Type: {}", comp.comp_type)).monospace().size(10.0).color(colors().text));
                                    });

                                    if !comp.description.is_empty() {
                                        ui.add_space(2.0);
                                        ui.label(RichText::new(&comp.description).size(11.0).color(colors().muted));
                                    }

                                    if !comp.modification_tasks.is_empty() {
                                        ui.add_space(4.0);
                                        ui.label(RichText::new("Modification Invariants & Tasks:").strong().size(11.0).color(colors().warn));

                                        for (t_idx, task) in comp.modification_tasks.iter().enumerate() {
                                            ui.horizontal_top(|ui| {
                                                let (icon, color) = if task.completed {
                                                    (egui_phosphor::regular::CHECK, colors().ok)
                                                } else {
                                                    (egui_phosphor::regular::CIRCLE, colors().error)
                                                };
                                                ui.label(RichText::new(icon).strong().size(11.0).color(color));
                                                ui.label(RichText::new(format!("{}. {}", t_idx + 1, task.task)).size(11.0).color(colors().text));
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

impl crate::actions::ActionProvider for ArchitectureTreeState {
    fn actions(&self) -> Vec<crate::actions::Action> {
        Vec::new()
    }

    fn run(&mut self, _action: &crate::actions::Action, _value: Option<String>, _cx: &mut crate::actions::RunCx<'_>) {}
}
