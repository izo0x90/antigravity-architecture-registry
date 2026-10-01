use egui::{
    epaint::CubicBezierShape, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind,
    Vec2,
};
use harness_protocol::arch::{
    AlignmentStatus, ComponentSpec, EdgeKind, GraphEdge, GraphNode, NodeKind, TaskSpec, UsageNode,
};

use crate::graph_layout::LayoutDirection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayMode {
    #[default]
    Detailed,
    Compact,
    AlignmentDiff,
}

pub struct NodeDimensions {
    pub width: f32,
    pub height: f32,
}

impl DisplayMode {
    pub fn dimensions(&self) -> NodeDimensions {
        match self {
            DisplayMode::Detailed => NodeDimensions {
                width: 230.0,
                height: 105.0,
            },
            DisplayMode::Compact => NodeDimensions {
                width: 170.0,
                height: 48.0,
            },
            DisplayMode::AlignmentDiff => NodeDimensions {
                width: 230.0,
                height: 115.0,
            },
        }
    }
}

/// Renders a node based on its kind, display mode, selection state, and layout direction.
pub fn render_node(
    painter: &Painter,
    node: &GraphNode,
    rect: Rect,
    mode: DisplayMode,
    is_selected: bool,
    direction: LayoutDirection,
    card_scale: f32,
    enable_lod: bool,
) -> Rect {
    match mode {
        DisplayMode::Compact => render_compact_pill(painter, node, rect, is_selected, direction, card_scale),
        DisplayMode::Detailed | DisplayMode::AlignmentDiff => match &node.kind {
            NodeKind::Component(comp) => {
                render_component_card(painter, node, comp, rect, is_selected, mode, direction, card_scale, enable_lod)
            }
            NodeKind::UsageCall(call) => {
                render_usage_call_card(painter, node, call, rect, is_selected, mode, direction, card_scale, enable_lod)
            }
            NodeKind::PlanStep(step) => {
                render_plan_step_card(painter, node, step, rect, is_selected, mode, direction, card_scale, enable_lod)
            }
        },
    }

    rect
}

fn draw_node_pins(
    painter: &Painter,
    rect: Rect,
    in_color: Color32,
    out_color: Color32,
    direction: LayoutDirection,
    card_scale: f32,
) {
    let (in_pin, out_pin) = match direction {
        LayoutDirection::LeftToRight => (
            Pos2::new(rect.min.x, rect.center().y),
            Pos2::new(rect.max.x, rect.center().y),
        ),
        LayoutDirection::TopToBottom => (
            Pos2::new(rect.center().x, rect.min.y),
            Pos2::new(rect.center().x, rect.max.y),
        ),
    };
    let pin_radius = (4.0 * card_scale).clamp(2.0, 8.0);
    painter.circle_filled(in_pin, pin_radius, in_color);
    painter.circle_filled(out_pin, pin_radius, out_color);
}

fn render_compact_pill(
    painter: &Painter,
    node: &GraphNode,
    rect: Rect,
    is_selected: bool,
    direction: LayoutDirection,
    card_scale: f32,
) {
    let bg = if is_selected {
        Color32::from_rgb(28, 42, 60)
    } else {
        Color32::from_rgb(18, 22, 30)
    };
    let stroke = if is_selected {
        Stroke::new((1.5 * card_scale).max(1.0), Color32::from_rgb(100, 180, 255))
    } else {
        Stroke::new((1.0 * card_scale).max(0.5), Color32::from_rgb(45, 55, 75))
    };

    let corner_radius = ((12.0 * card_scale) as u8).max(3);
    painter.rect_filled(rect, CornerRadius::same(corner_radius), bg);
    painter.rect_stroke(rect, CornerRadius::same(corner_radius), stroke, StrokeKind::Inside);

    // Indicator Dot
    let dot_color = match &node.alignment {
        AlignmentStatus::VerifiedInCode { .. } => Color32::from_rgb(80, 200, 120),
        AlignmentStatus::SignatureMismatch { .. } => Color32::from_rgb(250, 90, 80),
        AlignmentStatus::MissingCallSite { .. } => Color32::from_rgb(250, 180, 50),
        AlignmentStatus::PlannedOnly => Color32::from_rgb(100, 160, 240),
    };
    let dot_radius = (4.0 * card_scale).clamp(1.5, 8.0);
    painter.circle_filled(
        Pos2::new(rect.min.x + 14.0 * card_scale, rect.center().y),
        dot_radius,
        dot_color,
    );

    // Label
    let label = if node.label.len() > 18 {
        format!("{}...", &node.label[..16])
    } else {
        node.label.clone()
    };
    let font_size = (11.0 * card_scale).max(5.0);
    painter.text(
        Pos2::new(rect.min.x + 24.0 * card_scale, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(font_size),
        Color32::WHITE,
    );

    draw_node_pins(
        painter,
        rect,
        Color32::from_rgb(80, 180, 255),
        Color32::from_rgb(250, 180, 50),
        direction,
        card_scale,
    );
}

fn render_component_card(
    painter: &Painter,
    node: &GraphNode,
    comp: &ComponentSpec,
    rect: Rect,
    is_selected: bool,
    mode: DisplayMode,
    direction: LayoutDirection,
    card_scale: f32,
    enable_lod: bool,
) {
    let bg = if is_selected {
        Color32::from_rgb(24, 34, 48)
    } else {
        Color32::from_rgb(18, 22, 30)
    };
    let border_stroke = if is_selected {
        Stroke::new((1.5 * card_scale).max(1.0), Color32::from_rgb(100, 180, 255))
    } else {
        Stroke::new((1.0 * card_scale).max(0.5), Color32::from_rgb(45, 55, 75))
    };

    let corner_radius = ((6.0 * card_scale) as u8).max(2);
    painter.rect_filled(rect, CornerRadius::same(corner_radius), bg);
    painter.rect_stroke(rect, CornerRadius::same(corner_radius), border_stroke, StrokeKind::Inside);

    // Header Accent Line
    let accent_height = (3.0 * card_scale).clamp(1.0, 6.0);
    let accent_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), accent_height));
    painter.rect_filled(
        accent_rect,
        CornerRadius::same(1),
        Color32::from_rgb(80, 180, 255),
    );

    // Title
    let title = if comp.name.len() > 22 {
        format!("{}...", &comp.name[..20])
    } else {
        comp.name.clone()
    };
    let title_font_size = (12.0 * card_scale).max(5.0);
    painter.text(
        Pos2::new(rect.min.x + 10.0 * card_scale, rect.min.y + 8.0 * card_scale),
        egui::Align2::LEFT_TOP,
        title,
        FontId::proportional(title_font_size),
        Color32::WHITE,
    );

    let show_details = !enable_lod || card_scale >= 0.65;
    if show_details {
        // Badges: Type & Stage
        let (stage_icon, stage_color) = match comp.stage.as_str() {
            "implemented" => (egui_phosphor::regular::CHECK, Color32::from_rgb(80, 200, 120)),
            "in_progress" => (egui_phosphor::regular::LIGHTNING, Color32::from_rgb(100, 160, 240)),
            _ => (egui_phosphor::regular::CIRCLE, Color32::GRAY),
        };
        let badge_font_size = (9.5 * card_scale).max(4.5);
        painter.text(
            Pos2::new(rect.min.x + 10.0 * card_scale, rect.min.y + 26.0 * card_scale),
            egui::Align2::LEFT_TOP,
            format!("{} | {} {}", comp.comp_type, stage_icon, comp.stage.to_uppercase()),
            FontId::monospace(badge_font_size),
            stage_color,
        );

        // Description snippet
        let desc = if comp.description.len() > 30 {
            format!("{}...", &comp.description[..28])
        } else {
            comp.description.clone()
        };
        let desc_font_size = (10.0 * card_scale).max(5.0);
        painter.text(
            Pos2::new(rect.min.x + 10.0 * card_scale, rect.min.y + 44.0 * card_scale),
            egui::Align2::LEFT_TOP,
            desc,
            FontId::proportional(desc_font_size),
            Color32::from_rgb(160, 165, 175),
        );

        // Footer / Alignment
        render_alignment_footer(painter, &node.alignment, rect, mode, card_scale);
    }

    // Pins
    draw_node_pins(
        painter,
        rect,
        Color32::from_rgb(80, 180, 255),
        Color32::from_rgb(250, 180, 50),
        direction,
        card_scale,
    );
}

fn render_usage_call_card(
    painter: &Painter,
    node: &GraphNode,
    call: &UsageNode,
    rect: Rect,
    is_selected: bool,
    mode: DisplayMode,
    direction: LayoutDirection,
    card_scale: f32,
    enable_lod: bool,
) {
    let bg = if is_selected {
        Color32::from_rgb(32, 28, 48)
    } else {
        Color32::from_rgb(22, 19, 32)
    };
    let border_stroke = if is_selected {
        Stroke::new((1.5 * card_scale).max(1.0), Color32::from_rgb(180, 120, 255))
    } else {
        Stroke::new((1.0 * card_scale).max(0.5), Color32::from_rgb(70, 55, 95))
    };

    let corner_radius = ((6.0 * card_scale) as u8).max(2);
    painter.rect_filled(rect, CornerRadius::same(corner_radius), bg);
    painter.rect_stroke(rect, CornerRadius::same(corner_radius), border_stroke, StrokeKind::Inside);

    // Purple Top Accent
    let accent_height = (3.0 * card_scale).clamp(1.0, 6.0);
    let accent_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), accent_height));
    painter.rect_filled(
        accent_rect,
        CornerRadius::same(1),
        Color32::from_rgb(180, 120, 255),
    );

    // Header: CALL
    let header_font_size = (11.0 * card_scale).max(5.0);
    painter.text(
        Pos2::new(rect.min.x + 10.0 * card_scale, rect.min.y + 8.0 * card_scale),
        egui::Align2::LEFT_TOP,
        format!("CALL: {}", call.node_id),
        FontId::monospace(header_font_size),
        Color32::from_rgb(200, 160, 255),
    );

    let show_details = !enable_lod || card_scale >= 0.65;
    if show_details {
        // Callee target
        let callee_font_size = (10.0 * card_scale).max(5.0);
        painter.text(
            Pos2::new(rect.min.x + 10.0 * card_scale, rect.min.y + 26.0 * card_scale),
            egui::Align2::LEFT_TOP,
            format!("→ {}", call.component_id),
            FontId::monospace(callee_font_size),
            Color32::from_rgb(140, 200, 255),
        );

        // Description snippet
        let desc = if call.description.len() > 30 {
            format!("{}...", &call.description[..28])
        } else {
            call.description.clone()
        };
        let desc_font_size = (10.0 * card_scale).max(5.0);
        painter.text(
            Pos2::new(rect.min.x + 10.0 * card_scale, rect.min.y + 44.0 * card_scale),
            egui::Align2::LEFT_TOP,
            desc,
            FontId::proportional(desc_font_size),
            Color32::from_rgb(160, 165, 175),
        );

        render_alignment_footer(painter, &node.alignment, rect, mode, card_scale);
    }

    // Pins
    draw_node_pins(
        painter,
        rect,
        Color32::from_rgb(180, 120, 255),
        Color32::from_rgb(250, 180, 50),
        direction,
        card_scale,
    );
}

fn render_plan_step_card(
    painter: &Painter,
    node: &GraphNode,
    step: &TaskSpec,
    rect: Rect,
    is_selected: bool,
    mode: DisplayMode,
    direction: LayoutDirection,
    card_scale: f32,
    enable_lod: bool,
) {
    let bg = if is_selected {
        Color32::from_rgb(26, 36, 30)
    } else {
        Color32::from_rgb(18, 26, 22)
    };
    let border_stroke = if is_selected {
        Stroke::new((1.5 * card_scale).max(1.0), Color32::from_rgb(80, 220, 140))
    } else {
        Stroke::new((1.0 * card_scale).max(0.5), Color32::from_rgb(40, 65, 50))
    };

    let corner_radius = ((6.0 * card_scale) as u8).max(2);
    painter.rect_filled(rect, CornerRadius::same(corner_radius), bg);
    painter.rect_stroke(rect, CornerRadius::same(corner_radius), border_stroke, StrokeKind::Inside);

    let accent_height = (3.0 * card_scale).clamp(1.0, 6.0);
    let accent_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), accent_height));
    painter.rect_filled(accent_rect, CornerRadius::same(1), Color32::from_rgb(80, 200, 120));

    let check_icon = if step.completed { "[✓] DONE" } else { "[ ] PENDING" };
    let check_color = if step.completed {
        Color32::from_rgb(80, 200, 120)
    } else {
        Color32::from_rgb(250, 180, 50)
    };

    let check_font_size = (11.0 * card_scale).max(5.0);
    painter.text(
        Pos2::new(rect.min.x + 10.0 * card_scale, rect.min.y + 8.0 * card_scale),
        egui::Align2::LEFT_TOP,
        check_icon,
        FontId::monospace(check_font_size),
        check_color,
    );

    let show_details = !enable_lod || card_scale >= 0.65;
    if show_details {
        let task_label = if step.task.len() > 28 {
            format!("{}...", &step.task[..26])
        } else {
            step.task.clone()
        };
        let task_font_size = (11.0 * card_scale).max(5.0);
        painter.text(
            Pos2::new(rect.min.x + 10.0 * card_scale, rect.min.y + 26.0 * card_scale),
            egui::Align2::LEFT_TOP,
            task_label,
            FontId::proportional(task_font_size),
            Color32::WHITE,
        );

        render_alignment_footer(painter, &node.alignment, rect, mode, card_scale);
    }

    // Pins
    draw_node_pins(
        painter,
        rect,
        Color32::from_rgb(80, 200, 120),
        Color32::from_rgb(80, 200, 120),
        direction,
        card_scale,
    );
}

fn render_alignment_footer(
    painter: &Painter,
    alignment: &AlignmentStatus,
    rect: Rect,
    _mode: DisplayMode,
    card_scale: f32,
) {
    let footer_height = (20.0 * card_scale).max(12.0);
    let footer_y = rect.max.y - footer_height;
    let footer_line = Rect::from_min_size(
        Pos2::new(rect.min.x + 8.0 * card_scale, footer_y - 2.0 * card_scale),
        Vec2::new(rect.width() - 16.0 * card_scale, (1.0 * card_scale).max(0.5)),
    );
    painter.rect_filled(footer_line, CornerRadius::ZERO, Color32::from_rgb(32, 40, 54));

    let (icon, label, color) = match alignment {
        AlignmentStatus::VerifiedInCode { location } => (
            egui_phosphor::regular::CHECK,
            format!("Code: {}:{}", location.file_path, location.start_line),
            Color32::from_rgb(80, 200, 120),
        ),
        AlignmentStatus::SignatureMismatch { diff, .. } => (
            egui_phosphor::regular::WARNING,
            format!("Mismatch: {}", diff),
            Color32::from_rgb(250, 90, 80),
        ),
        AlignmentStatus::MissingCallSite { expected_in } => (
            egui_phosphor::regular::WARNING_CIRCLE,
            format!("Missing in: {}", expected_in),
            Color32::from_rgb(250, 180, 50),
        ),
        AlignmentStatus::PlannedOnly => (
            egui_phosphor::regular::CIRCLE,
            "Plan spec only".to_string(),
            Color32::from_rgb(100, 160, 240),
        ),
    };

    let max_len = if card_scale < 0.85 { 18 } else { 26 };
    let display_str = if label.len() > max_len {
        format!("{} {}...", icon, &label[..max_len - 2])
    } else {
        format!("{} {}", icon, label)
    };

    let footer_font_size = (9.0 * card_scale).max(4.5);
    painter.text(
        Pos2::new(rect.min.x + 10.0 * card_scale, footer_y + 2.0 * card_scale),
        egui::Align2::LEFT_TOP,
        display_str,
        FontId::monospace(footer_font_size),
        color,
    );
}

/// Draws directed bezier curves and arrowheads between connected node pins based on layout direction.
pub fn draw_edge(
    painter: &Painter,
    edge: &GraphEdge,
    source_rect: Rect,
    target_rect: Rect,
    direction: LayoutDirection,
    card_scale: f32,
) {
    let arrow_size = (5.0 * card_scale).clamp(2.5, 12.0);
    let (p0, p3, p1, p2, tip, left, right) = match direction {
        LayoutDirection::LeftToRight => {
            let p0 = Pos2::new(source_rect.max.x, source_rect.center().y);
            let p3 = Pos2::new(target_rect.min.x, target_rect.center().y);

            let (p1, p2) = if p3.x >= p0.x {
                let dx = (p3.x - p0.x).max(20.0 * card_scale) * 0.5;
                (Pos2::new(p0.x + dx, p0.y), Pos2::new(p3.x - dx, p3.y))
            } else {
                let arc_y = (p0.y.min(p3.y) - 60.0 * card_scale).min(p0.y - 40.0 * card_scale);
                (Pos2::new(p0.x + 40.0 * card_scale, arc_y), Pos2::new(p3.x - 40.0 * card_scale, arc_y))
            };

            let tip = p3;
            let left = Pos2::new(tip.x - arrow_size * 1.5, tip.y - arrow_size);
            let right = Pos2::new(tip.x - arrow_size * 1.5, tip.y + arrow_size);
            (p0, p3, p1, p2, tip, left, right)
        }
        LayoutDirection::TopToBottom => {
            let p0 = Pos2::new(source_rect.center().x, source_rect.max.y);
            let p3 = Pos2::new(target_rect.center().x, target_rect.min.y);

            let (p1, p2) = if p3.y >= p0.y {
                let dy = (p3.y - p0.y).max(20.0 * card_scale) * 0.5;
                (Pos2::new(p0.x, p0.y + dy), Pos2::new(p3.x, p3.y - dy))
            } else {
                let arc_x = (p0.x.min(p3.x) - 60.0 * card_scale).min(p0.x - 40.0 * card_scale);
                (Pos2::new(arc_x, p0.y + 40.0 * card_scale), Pos2::new(arc_x, p3.y - 40.0 * card_scale))
            };

            let tip = p3;
            let left = Pos2::new(tip.x - arrow_size, tip.y - arrow_size * 1.5);
            let right = Pos2::new(tip.x + arrow_size, tip.y - arrow_size * 1.5);
            (p0, p3, p1, p2, tip, left, right)
        }
    };

    let (color, base_stroke_width) = match &edge.kind {
        EdgeKind::CallSite { .. } => (Color32::from_rgb(80, 180, 255), 1.5),
        EdgeKind::DataFlow { .. } => (Color32::from_rgb(250, 180, 50), 2.0),
        EdgeKind::Decomposition => (Color32::from_rgb(180, 120, 240), 1.5),
    };
    let stroke_width = (base_stroke_width * card_scale).clamp(0.8, 5.0);

    // Draw Cubic Bezier curve
    painter.add(CubicBezierShape::from_points_stroke(
        [p0, p1, p2, p3],
        false,
        Color32::TRANSPARENT,
        Stroke::new(stroke_width, color),
    ));

    // Draw Arrowhead at Target Pin
    painter.add(egui::Shape::convex_polygon(vec![tip, left, right], color, Stroke::NONE));
}

