use std::sync::{Arc, Mutex};
use egui::{Color32, Frame, Margin, Pos2, Rect, RichText, Stroke, StrokeKind, Vec2};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::net::NetClient;

fn default_status() -> String {
    "open".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackIssue {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub target: serde_json::Value,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub context: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FeedbackCategory {
    Bug,
    UxFriction,
    MissingInfo,
    ModelReasoning,
    LayoutVisual,
}

impl FeedbackCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bug => "Bug",
            Self::UxFriction => "UX Friction",
            Self::MissingInfo => "Missing Info",
            Self::ModelReasoning => "Model Reasoning",
            Self::LayoutVisual => "Layout / Visual",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "Bug" => Self::Bug,
            "UX Friction" => Self::UxFriction,
            "Missing Info" => Self::MissingInfo,
            "Model Reasoning" => Self::ModelReasoning,
            "Layout / Visual" => Self::LayoutVisual,
            _ => Self::Bug,
        }
    }

    pub fn all() -> &'static [FeedbackCategory] {
        &[
            Self::UxFriction,
            Self::Bug,
            Self::MissingInfo,
            Self::ModelReasoning,
            Self::LayoutVisual,
        ]
    }

    pub fn color(&self) -> Color32 {
        match self {
            Self::Bug => Color32::from_rgb(240, 80, 80),
            Self::UxFriction => Color32::from_rgb(230, 150, 40),
            Self::MissingInfo => Color32::from_rgb(80, 160, 240),
            Self::ModelReasoning => Color32::from_rgb(180, 100, 240),
            Self::LayoutVisual => Color32::from_rgb(80, 200, 140),
        }
    }
}

pub struct FeedbackInspectorState {
    pub is_inspecting: bool,
    pub just_toggled: bool,
    pub hovered_target: Option<(String, Rect)>,

    // Feedback modal (create issue)
    pub is_modal_open: bool,
    pub target_id: String,
    pub selected_category: FeedbackCategory,
    pub comment: String,
    pub toast_message: Option<(String, f64)>, // (message, timestamp_seconds)
    pub should_focus_modal: bool,

    // Issues explorer window (view & edit issues)
    pub is_issues_window_open: bool,
    pub issues: Vec<FeedbackIssue>,
    pub selected_issue_id: Option<String>,
    pub editing_status: String,
    pub editing_category: FeedbackCategory,
    pub editing_comment: String,
    pub status_filter: String,
    pub is_loading_issues: bool,
    pub pending_issues_load: Arc<Mutex<Option<Result<Vec<FeedbackIssue>, String>>>>,
    pub pending_issue_update: Arc<Mutex<Option<Result<String, String>>>>,
}

impl Default for FeedbackInspectorState {
    fn default() -> Self {
        Self {
            is_inspecting: false,
            just_toggled: false,
            hovered_target: None,
            is_modal_open: false,
            target_id: "general_cockpit".to_string(),
            selected_category: FeedbackCategory::UxFriction,
            comment: String::new(),
            toast_message: None,
            should_focus_modal: false,

            is_issues_window_open: false,
            issues: Vec::new(),
            selected_issue_id: None,
            editing_status: "open".to_string(),
            editing_category: FeedbackCategory::Bug,
            editing_comment: String::new(),
            status_filter: "all".to_string(),
            is_loading_issues: false,
            pending_issues_load: Arc::new(Mutex::new(None)),
            pending_issue_update: Arc::new(Mutex::new(None)),
        }
    }
}

impl FeedbackInspectorState {
    pub fn begin_frame(&mut self) {
        if self.is_inspecting {
            self.hovered_target = None;
        }
    }

    pub fn toggle_inspection(&mut self) {
        self.is_inspecting = !self.is_inspecting;
        self.hovered_target = None;
        if self.is_inspecting {
            self.just_toggled = true;
        }
    }

    /// Registers a candidate interactable element rect during frame rendering.
    /// If the cursor is currently hovering inside this rect, sets it as the hovered target.
    /// Nested / smaller rects take priority so inner components are accurately targeted.
    pub fn register_inspectable(&mut self, ui: &egui::Ui, id: &str, rect: Rect) {
        if !self.is_inspecting || id == "top_bar:inspector_button" {
            return;
        }
        if let Some(hover_pos) = ui.input(|i| i.pointer.hover_pos()) {
            if rect.contains(hover_pos) {
                let should_replace = match &self.hovered_target {
                    Some((_, existing_rect)) => rect.area() <= existing_rect.area(),
                    None => true,
                };
                if should_replace {
                    self.hovered_target = Some((id.to_string(), rect));
                }
            }
        }
    }

    /// Targets an element, immediately turns off inspection mode, and opens the feedback modal.
    pub fn target_element(&mut self, element_id: &str) {
        self.target_id = element_id.to_string();
        self.is_inspecting = false;
        self.hovered_target = None;
        self.is_modal_open = true;
        self.should_focus_modal = true;
    }

    /// Draws the real-time targeting overlay (crosshair, highlight bounding box, label badge)
    /// and listens for user click to lock onto the selected element.
    pub fn render_inspection_overlay(&mut self, ctx: &egui::Context) {
        if !self.is_inspecting {
            return;
        }

        if self.just_toggled {
            self.just_toggled = false;
            return;
        }

        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);

        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("inspector_overlay")));

        if let Some((ref target_id, rect)) = self.hovered_target {
            // Draw translucent amber box
            painter.rect_filled(rect, 4.0, Color32::from_rgba_unmultiplied(250, 180, 50, 35));
            painter.rect_stroke(rect, 4.0, Stroke::new(2.0, Color32::from_rgb(250, 180, 50)), StrokeKind::Outside);

            // Draw floating target label badge
            let badge_pos = Pos2::new(rect.min.x.max(8.0), (rect.min.y - 22.0).max(8.0));
            let badge_text = format!("🎯 Target: {}", target_id);
            let font = egui::FontId::monospace(11.0);
            let text_rect = painter.text(badge_pos, egui::Align2::LEFT_TOP, badge_text, font, Color32::BLACK);

            let padded_badge = text_rect.expand2(Vec2::new(6.0, 3.0));
            let bg_painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("inspector_overlay_bg")));
            bg_painter.rect_filled(padded_badge, 3.0, Color32::from_rgb(250, 180, 50));
        }

        // Intercept primary click to lock onto element and open modal
        let clicked = ctx.input(|i| i.pointer.primary_clicked());
        if clicked {
            if let Some((target_id, _)) = self.hovered_target.take() {
                self.target_element(&target_id);
            } else {
                self.target_element("general_cockpit");
            }
        }
    }

    /// Renders the enlarged Feedback Modal when an element is targeted.
    pub fn render_modal(&mut self, ctx: &egui::Context, net: &NetClient, active_layout: usize, active_slots: &[String]) {
        if !self.is_modal_open {
            return;
        }

        let mut is_open = self.is_modal_open;
        let mut close_modal = false;

        egui::Window::new(RichText::new("🎯 Developer Feedback on Targeted Element").strong().color(Color32::from_rgb(250, 180, 50)))
            .open(&mut is_open)
            .collapsible(false)
            .resizable(true)
            .default_size(Vec2::new(650.0, 480.0))
            .min_size(Vec2::new(500.0, 380.0))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Target Element:").strong().size(12.0));
                        Frame::NONE
                            .fill(Color32::from_rgb(20, 26, 36))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(45, 60, 85)))
                            .corner_radius(4)
                            .inner_margin(Margin::symmetric(8, 4))
                            .show(ui, |ui| {
                                ui.label(RichText::new(&self.target_id).monospace().size(12.0).color(Color32::from_rgb(140, 205, 255)));
                            });
                    });
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Category:").strong().size(12.0));
                        for cat in FeedbackCategory::all() {
                            let is_selected = &self.selected_category == cat;
                            let color = if is_selected { cat.color() } else { Color32::GRAY };
                            if ui.selectable_label(is_selected, RichText::new(cat.as_str()).color(color).strong()).clicked() {
                                self.selected_category = cat.clone();
                            }
                        }
                    });
                    ui.add_space(10.0);

                    ui.label(RichText::new("Critique, Friction & Bug Notes:").strong().size(12.0));
                    let text_resp = ui.add(
                        egui::TextEdit::multiline(&mut self.comment)
                            .id(egui::Id::new("feedback_modal_comment_edit"))
                            .desired_rows(11)
                            .desired_width(ui.available_width())
                            .hint_text("Explain what felt broken, unresponsive, misaligned, or unexpected...")
                    );
                    if self.should_focus_modal {
                        text_resp.request_focus();
                        self.should_focus_modal = false;
                    }
                    ui.add_space(12.0);

                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("🚀 Submit Feedback").strong().color(Color32::GREEN)).clicked() {
                            if !self.comment.trim().is_empty() {
                                let payload = json!({
                                    "target": { "id": self.target_id },
                                    "category": self.selected_category.as_str(),
                                    "comment": self.comment.trim(),
                                    "status": "open",
                                    "context": {
                                        "activeLayout": active_layout,
                                        "activeSlots": active_slots
                                    }
                                });

                                let net_ctx = ctx.clone();
                                net.submit_feedback(payload, net_ctx, |_| {});

                                self.toast_message = Some((
                                    format!("Feedback recorded for target '{}'!", self.target_id),
                                    ctx.input(|i| i.time),
                                ));
                                self.comment.clear();
                                close_modal = true;
                            }
                        }

                        if ui.button("Cancel").clicked() {
                            close_modal = true;
                        }
                    });
                });
            });

        self.is_modal_open = is_open && !close_modal;
    }

    /// Initiates loading of all issues from the backend server.
    pub fn load_issues(&mut self, net: &NetClient, ctx: egui::Context) {
        self.is_loading_issues = true;
        let mailbox = self.pending_issues_load.clone();
        net.fetch_feedback(ctx, move |result| {
            let parsed = match result {
                Ok(raw_items) => {
                    let mut list = Vec::new();
                    for val in raw_items {
                        if let Ok(issue) = serde_json::from_value::<FeedbackIssue>(val) {
                            list.push(issue);
                        }
                    }
                    Ok(list)
                }
                Err(e) => Err(e),
            };
            if let Ok(mut lock) = mailbox.lock() {
                *lock = Some(parsed);
            }
        });
    }

    /// Renders the Issues Explorer window allowing developers to browse, filter,
    /// view, and edit all submitted feedback issues.
    pub fn render_issues_window(&mut self, ctx: &egui::Context, net: &NetClient) {
        // Drain pending loads
        if let Ok(mut lock) = self.pending_issues_load.lock() {
            if let Some(res) = lock.take() {
                self.is_loading_issues = false;
                if let Ok(issues) = res {
                    self.issues = issues;
                }
            }
        }

        // Drain pending updates
        let mut should_reload = false;
        if let Ok(mut lock) = self.pending_issue_update.lock() {
            if let Some(res) = lock.take() {
                match res {
                    Ok(msg) => {
                        self.toast_message = Some((msg, ctx.input(|i| i.time)));
                        should_reload = true;
                    }
                    Err(e) => {
                        self.toast_message = Some((format!("Error updating: {}", e), ctx.input(|i| i.time)));
                    }
                }
            }
        }
        if should_reload {
            self.load_issues(net, ctx.clone());
        }

        if !self.is_issues_window_open {
            return;
        }

        let mut is_open = self.is_issues_window_open;
        egui::Window::new(RichText::new(format!("{} Issues & Feedback Explorer", egui_phosphor::regular::WARNING_CIRCLE)).strong().color(Color32::from_rgb(100, 180, 255)))
            .open(&mut is_open)
            .collapsible(false)
            .resizable(true)
            .default_size(Vec2::new(820.0, 540.0))
            .min_size(Vec2::new(600.0, 400.0))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Total: {} issues", self.issues.len())).strong().size(12.0));
                    ui.separator();

                    ui.label("Filter:");
                    let filters = [("All", "all"), ("Open", "open"), ("In Progress", "in_progress"), ("Resolved", "resolved")];
                    for (lbl, val) in filters {
                        let is_active = self.status_filter == val;
                        if ui.selectable_label(is_active, lbl).clicked() {
                            self.status_filter = val.to_string();
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(format!("{} Refresh", egui_phosphor::regular::ARROWS_CLOCKWISE)).clicked() {
                            self.load_issues(net, ctx.clone());
                        }
                    });
                });
                ui.separator();

                // Master-detail split layout
                let total_w = ui.available_width();
                let list_w = (total_w * 0.42).clamp(260.0, 360.0);

                ui.horizontal(|ui| {
                    // Left column: Scrollable list of issues
                    ui.allocate_ui_with_layout(
                        Vec2::new(list_w, ui.available_height()),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::ScrollArea::vertical().id_salt("issues_list_scroll").show(ui, |ui| {
                                let filtered: Vec<&FeedbackIssue> = self.issues.iter().filter(|i| {
                                    if self.status_filter == "all" {
                                        true
                                    } else {
                                        i.status.eq_ignore_ascii_case(&self.status_filter)
                                    }
                                }).collect();

                                if filtered.is_empty() {
                                    ui.add_space(20.0);
                                    ui.label(RichText::new("No issues match current filter.").italics().color(Color32::GRAY));
                                } else if self.selected_issue_id.is_none() {
                                    if let Some(first) = filtered.first() {
                                        self.selected_issue_id = Some(first.id.clone());
                                        self.editing_status = first.status.clone();
                                        self.editing_category = FeedbackCategory::from_str_lossy(&first.category);
                                        self.editing_comment = first.comment.clone();
                                    }
                                }

                                for issue in filtered {
                                    let is_selected = self.selected_issue_id.as_deref() == Some(&issue.id);
                                    let target_id_str = issue.target.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
                                    let cat = FeedbackCategory::from_str_lossy(&issue.category);

                                    let bg = if is_selected {
                                        Color32::from_rgb(30, 45, 65)
                                    } else {
                                        Color32::from_rgb(18, 22, 28)
                                    };

                                    let card_res = Frame::NONE
                                        .fill(bg)
                                        .stroke(Stroke::new(1.0, if is_selected { Color32::from_rgb(80, 160, 240) } else { Color32::from_rgb(35, 42, 52) }))
                                        .corner_radius(4)
                                        .inner_margin(Margin::same(8))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(RichText::new(cat.as_str()).size(10.0).color(cat.color()).strong());
                                                ui.label(RichText::new(&issue.status).size(10.0).color(Color32::LIGHT_GRAY));
                                            });
                                            ui.label(RichText::new(target_id_str).monospace().size(11.0).color(Color32::WHITE));
                                            let snippet = if issue.comment.len() > 45 {
                                                format!("{}...", &issue.comment[..45])
                                            } else {
                                                issue.comment.clone()
                                            };
                                            ui.label(RichText::new(snippet).size(10.0).color(Color32::GRAY));
                                        });

                                    let card_interact = ui.interact(card_res.response.rect, ui.id().with(&issue.id), egui::Sense::click());
                                    if card_interact.clicked() {
                                        self.selected_issue_id = Some(issue.id.clone());
                                        self.editing_status = issue.status.clone();
                                        self.editing_category = FeedbackCategory::from_str_lossy(&issue.category);
                                        self.editing_comment = issue.comment.clone();
                                    }
                                    if card_interact.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    ui.add_space(4.0);
                                }
                            });
                        },
                    );

                    ui.separator();

                    // Right column: Detailed View & Edit Panel
                    ui.allocate_ui_with_layout(
                        Vec2::new(ui.available_width(), ui.available_height()),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            if let Some(selected_id) = self.selected_issue_id.clone() {
                                if let Some(issue) = self.issues.iter().find(|i| i.id == selected_id).cloned() {
                                    let target_id_str = issue.target.get("id").and_then(|v| v.as_str()).unwrap_or("general").to_string();

                                    ui.label(RichText::new(format!("Issue: {}", issue.id)).monospace().strong().size(13.0));
                                    ui.add_space(4.0);

                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("Target:").strong().size(11.0));
                                        ui.label(RichText::new(&target_id_str).monospace().size(11.0).color(Color32::from_rgb(140, 205, 255)));
                                    });
                                    ui.add_space(6.0);

                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("Status:").strong().size(11.0));
                                        let statuses = ["open", "in_progress", "resolved"];
                                        for st in statuses {
                                            let is_st = self.editing_status.eq_ignore_ascii_case(st);
                                            let label = match st {
                                                "open" => "Open",
                                                "in_progress" => "In Progress",
                                                "resolved" => "Resolved",
                                                _ => st,
                                            };
                                            if ui.selectable_label(is_st, label).clicked() {
                                                self.editing_status = st.to_string();
                                            }
                                        }
                                    });
                                    ui.add_space(6.0);

                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("Category:").strong().size(11.0));
                                        for cat in FeedbackCategory::all() {
                                            let is_cat = &self.editing_category == cat;
                                            if ui.selectable_label(is_cat, cat.as_str()).clicked() {
                                                self.editing_category = cat.clone();
                                            }
                                        }
                                    });
                                    ui.add_space(8.0);

                                    ui.label(RichText::new("Critique & Notes (Editable):").strong().size(11.0));
                                    ui.add(
                                        egui::TextEdit::multiline(&mut self.editing_comment)
                                            .desired_rows(7)
                                            .desired_width(ui.available_width())
                                    );
                                    ui.add_space(10.0);

                                    if let Some(ctx_val) = &issue.context {
                                        ui.collapsing("System Context Metadata", |ui| {
                                            let pretty = serde_json::to_string_pretty(ctx_val).unwrap_or_default();
                                            ui.label(RichText::new(pretty).monospace().size(10.0).color(Color32::GRAY));
                                        });
                                        ui.add_space(10.0);
                                    }

                                    ui.horizontal(|ui| {
                                        if ui.button(RichText::new(format!("{} Save Changes", egui_phosphor::regular::FLOPPY_DISK)).strong().color(Color32::from_rgb(100, 220, 140))).clicked() {
                                            let updated_item = FeedbackIssue {
                                                id: issue.id.clone(),
                                                timestamp: issue.timestamp.clone(),
                                                target: issue.target.clone(),
                                                category: self.editing_category.as_str().to_string(),
                                                comment: self.editing_comment.clone(),
                                                status: self.editing_status.clone(),
                                                context: issue.context.clone(),
                                            };

                                            let json_val = serde_json::to_value(&updated_item).unwrap_or_default();
                                            let mailbox = self.pending_issue_update.clone();
                                            let net_ctx = ctx.clone();

                                            net.update_feedback(json_val, net_ctx, move |result| {
                                                if let Ok(mut lock) = mailbox.lock() {
                                                    *lock = Some(result);
                                                }
                                            });
                                        }
                                    });
                                }
                            } else {
                                ui.vertical_centered(|ui| {
                                    ui.add_space(50.0);
                                    ui.label(RichText::new("Select an issue from the list to view or edit.").italics().color(Color32::GRAY));
                                });
                            }
                        },
                    );
                });
            });

        self.is_issues_window_open = is_open;
    }

    pub fn render_toast(&mut self, ctx: &egui::Context) {
        if let Some((msg, created_at)) = &self.toast_message {
            let now = ctx.input(|i| i.time);
            if now - *created_at < 3.5 {
                egui::Area::new(egui::Id::new("toast_area"))
                    .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-20.0, -20.0))
                    .show(ctx, |ui| {
                        Frame::NONE
                            .fill(Color32::from_rgb(25, 45, 30))
                            .stroke(Stroke::new(1.0, Color32::GREEN))
                            .corner_radius(6)
                            .inner_margin(Margin::same(10))
                            .show(ui, |ui| {
                                ui.label(RichText::new(msg).color(Color32::WHITE).size(12.0));
                            });
                    });
            } else {
                self.toast_message = None;
            }
        }
    }
}
