use egui::{Color32, Pos2, Rect, RichText, Stroke, Ui, Vec2};

pub struct ArchNode {
    pub id: &'static str,
    pub title: &'static str,
    pub role: &'static str,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub inputs: &'static [&'static str],
    pub outputs: &'static [&'static str],
}

pub struct ArchEdge {
    pub from: &'static str,
    pub to: &'static str,
    pub label: &'static str,
}

pub struct ArchGraphState {
    pub nodes: Vec<ArchNode>,
    pub edges: Vec<ArchEdge>,
    pub selected_node: Option<String>,
    pub focused_components: Vec<String>,
}

impl Default for ArchGraphState {
    fn default() -> Self {
        Self {
            nodes: vec![
                ArchNode {
                    id: "driver",
                    title: "ProviderDriver",
                    role: "Discovery & Auth Engine",
                    x: 30.0,
                    y: 40.0,
                    w: 180.0,
                    h: 90.0,
                    inputs: &["binary_path", "oauth_creds"],
                    outputs: &["discovered_models", "driver_lease"],
                },
                ArchNode {
                    id: "adapter",
                    title: "AntigravityAdapter",
                    role: "Session Orchestrator",
                    x: 270.0,
                    y: 40.0,
                    w: 200.0,
                    h: 95.0,
                    inputs: &["turn_prompt", "steer_cmd"],
                    outputs: &["runtime_events", "tool_approvals"],
                },
                ArchNode {
                    id: "transport",
                    title: "JsonRpcTransport",
                    role: "Stdio Pipe RPC Engine",
                    x: 530.0,
                    y: 40.0,
                    w: 190.0,
                    h: 90.0,
                    inputs: &["stdio_in"],
                    outputs: &["rpc_frames", "reverse_rpc"],
                },
                ArchNode {
                    id: "fsproxy",
                    title: "FsProxy Sandbox",
                    role: "Security Traversal Barrier",
                    x: 530.0,
                    y: 200.0,
                    w: 190.0,
                    h: 85.0,
                    inputs: &["raw_path"],
                    outputs: &["canonical_safe_path"],
                },
                ArchNode {
                    id: "cockpit",
                    title: "Native Egui Cockpit",
                    role: "UI & Steering Viewports",
                    x: 270.0,
                    y: 200.0,
                    w: 200.0,
                    h: 85.0,
                    inputs: &["app_events"],
                    outputs: &["user_prompts", "steer_cmds"],
                },
            ],
            edges: vec![
                ArchEdge { from: "driver", to: "adapter", label: "driver_lease" },
                ArchEdge { from: "adapter", to: "transport", label: "stdio_rpc" },
                ArchEdge { from: "transport", to: "fsproxy", label: "fs_check" },
                ArchEdge { from: "cockpit", to: "adapter", label: "session_ctrl" },
            ],
            selected_node: Some("adapter".to_string()),
            focused_components: vec![],
        }
    }
}

impl ArchGraphState {
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            // Header
            ui.horizontal(|ui| {
                ui.label(RichText::new("⚡").size(12.0));
                ui.label(RichText::new("ARCHITECTURE DAG (Deterministic Layout)").strong().color(Color32::from_rgb(147, 197, 253)));
                ui.label(RichText::new("• Zero physics").size(10.0).color(Color32::from_rgb(16, 185, 129)));
            });
            ui.separator();

            let (response, painter) = ui.allocate_painter(Vec2::new(ui.available_width(), 320.0), egui::Sense::click());
            let origin = response.rect.min;

            // Draw Edges first
            for edge in &self.edges {
                if let (Some(from_n), Some(to_n)) = (
                    self.nodes.iter().find(|n| n.id == edge.from),
                    self.nodes.iter().find(|n| n.id == edge.to),
                ) {
                    let p1 = origin + Vec2::new(from_n.x + from_n.w, from_n.y + from_n.h / 2.0);
                    let p2 = origin + Vec2::new(to_n.x, to_n.y + to_n.h / 2.0);

                    // Bezier connector
                    let ctrl1 = p1 + Vec2::new(30.0, 0.0);
                    let ctrl2 = p2 - Vec2::new(30.0, 0.0);

                    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                        [p1, ctrl1, ctrl2, p2],
                        false,
                        Color32::TRANSPARENT,
                        Stroke::new(1.5, Color32::from_rgb(71, 85, 105)),
                    ));

                    // Label at midpoint
                    let mid = Pos2::new((p1.x + p2.x) / 2.0, (p1.y + p2.y) / 2.0 - 8.0);
                    painter.text(
                        mid,
                        egui::Align2::CENTER_CENTER,
                        edge.label,
                        egui::FontId::monospace(9.0),
                        Color32::from_rgb(148, 163, 184),
                    );
                }
            }

            // Draw Nodes
            let mut clicked_node: Option<&'static str> = None;
            for node in &self.nodes {
                let rect = Rect::from_min_size(origin + Vec2::new(node.x, node.y), Vec2::new(node.w, node.h));
                let is_selected = self.selected_node.as_deref() == Some(node.id);
                let is_focused = self.focused_components.is_empty()
                    || self.focused_components.iter().any(|c| c == node.id);

                let fill_color = if is_selected {
                    Color32::from_rgb(26, 35, 54)
                } else if is_focused {
                    Color32::from_rgb(15, 23, 42)
                } else {
                    Color32::from_rgba_premultiplied(10, 15, 28, 100)
                };
                let stroke_color = if is_selected {
                    Color32::from_rgb(59, 130, 246)
                } else if is_focused {
                    Color32::from_rgb(51, 65, 85)
                } else {
                    Color32::from_rgba_premultiplied(30, 41, 59, 80)
                };

                painter.rect_filled(rect, 4.0, fill_color);
                painter.rect_stroke(rect, 4.0, Stroke::new(if is_selected { 2.0 } else { 1.0 }, stroke_color), egui::StrokeKind::Outside);

                // Title
                painter.text(
                    rect.min + Vec2::new(8.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    node.title,
                    egui::FontId::proportional(12.0),
                    if is_focused { Color32::WHITE } else { Color32::from_rgb(100, 116, 139) },
                );

                // Role
                painter.text(
                    rect.min + Vec2::new(8.0, 26.0),
                    egui::Align2::LEFT_TOP,
                    node.role,
                    egui::FontId::proportional(10.0),
                    if is_focused { Color32::from_rgb(148, 163, 184) } else { Color32::from_rgb(71, 85, 105) },
                );

                // Port count
                let ports_str = format!("in: {} | out: {}", node.inputs.len(), node.outputs.len());
                painter.text(
                    rect.min + Vec2::new(8.0, node.h - 18.0),
                    egui::Align2::LEFT_TOP,
                    ports_str,
                    egui::FontId::monospace(9.0),
                    Color32::from_rgb(100, 116, 139),
                );

                // Click detection
                if response.clicked()
                    && let Some(pos) = response.interact_pointer_pos()
                    && rect.contains(pos)
                {
                    clicked_node = Some(node.id);
                }
            }

            if let Some(node_id) = clicked_node {
                self.selected_node = Some(node_id.to_string());
            }

            ui.add_space(8.0);
            ui.separator();

            // Selected Node Details Pane
            if let Some(ref sel_id) = self.selected_node
                && let Some(node) = self.nodes.iter().find(|n| n.id == sel_id)
            {
                ui.label(RichText::new(format!("Selected: {}", node.title)).strong().color(Color32::from_rgb(96, 165, 250)));
                ui.label(RichText::new(format!("Role: {}", node.role)).size(11.0).color(Color32::GRAY));

                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Inputs:").strong().size(10.0));
                            for inp in node.inputs {
                                ui.label(RichText::new(format!("• {inp}")).monospace().size(10.0).color(Color32::from_rgb(203, 213, 225)));
                            }
                        });
                        ui.add_space(20.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Outputs:").strong().size(10.0));
                            for out in node.outputs {
                                ui.label(RichText::new(format!("• {out}")).monospace().size(10.0).color(Color32::from_rgb(203, 213, 225)));
                            }
                        });
                    });
                }
            ui.add_space(8.0);
        });
    }
}
