use egui::{
    Color32, CornerRadius, Pos2, Rect, RichText, Stroke, StrokeKind, Vec2,
};
use harness_protocol::arch::{
    AlignmentStatus, ComponentSpec, GraphNode, GraphPerspective, NodeKind, SystemArchitecture,
    UnifiedArchitectureGraph,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::feedback::FeedbackInspectorState;
use crate::graph_draw::{self, DisplayMode};
use crate::graph_layout::{self, LayoutDirection, LayoutSettings};
use crate::samples;

// Re-export ComponentSpec as ArchComponent for tree.rs backwards-compatibility
pub use harness_protocol::arch::ComponentSpec as ArchComponent;
pub use harness_protocol::arch::TaskSpec as ArchTask;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphPreset {
    #[default]
    LiveSystem,
    SampleCallGraph,
    SampleDataFlow,
    SampleTaskHierarchy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ZoomStrategy {
    #[default]
    Geometric,
    Semantic,
}

impl ZoomStrategy {
    /// Returns the effective visual scale factor to apply to card dimensions, fonts, and pins.
    #[inline]
    pub fn card_scale(&self, zoom: f32) -> f32 {
        match self {
            Self::Geometric => zoom,
            Self::Semantic => 1.0,
        }
    }

    /// Whether this strategy enables Level-of-Detail (LOD) decluttering for small card scales.
    #[inline]
    pub fn enables_lod(&self) -> bool {
        match self {
            Self::Geometric => true,
            Self::Semantic => false,
        }
    }
}

pub struct ArchitectureGraphState {
    pub raw_architecture: Option<SystemArchitecture>,
    pub components: BTreeMap<String, ComponentSpec>,
    pub graph: UnifiedArchitectureGraph,
    pub node_positions: BTreeMap<String, Pos2>,
    pub selected_node_id: Option<String>,
    pub active_preset: GraphPreset,
    pub perspective: GraphPerspective,
    pub layout_direction: LayoutDirection,
    pub display_mode: DisplayMode,
    pub zoom_strategy: ZoomStrategy,
    pub search_filter: String,

    // Pan & Zoom Camera
    pub pan: Vec2,
    pub zoom: f32,
    pub dragging_node_id: Option<String>,

    pub is_loaded: bool,
    pub load_error: Option<String>,
    pending_load: Arc<Mutex<Option<Result<String, String>>>>,
}

impl Default for ArchitectureGraphState {
    fn default() -> Self {
        let mut state = Self {
            raw_architecture: None,
            components: BTreeMap::new(),
            graph: UnifiedArchitectureGraph::default(),
            node_positions: BTreeMap::new(),
            selected_node_id: None,
            active_preset: GraphPreset::LiveSystem,
            perspective: GraphPerspective::CallFlow,
            layout_direction: LayoutDirection::LeftToRight,
            display_mode: DisplayMode::Detailed,
            zoom_strategy: ZoomStrategy::Geometric,
            search_filter: String::new(),
            pan: Vec2::new(40.0, 40.0),
            zoom: 1.0,
            dragging_node_id: None,
            is_loaded: false,
            load_error: None,
            pending_load: Arc::new(Mutex::new(None)),
        };
        // Pre-load default topology
        state.apply_preset(GraphPreset::SampleCallGraph);
        state
    }
}

impl ArchitectureGraphState {
    pub fn load_architecture(&mut self, net: &crate::net::NetClient, ctx: egui::Context) {
        self.is_loaded = false;
        self.load_error = None;
        let pending = self.pending_load.clone();
        let ctx_clone = ctx.clone();
        net.fetch_file("system_architecture.json", ctx_clone, move |result| {
            if let Ok(mut lock) = pending.lock() {
                *lock = Some(result);
            }
        });
    }

    pub fn load_from_json(&mut self, json_str: &str) -> Result<(), String> {
        match serde_json::from_str::<SystemArchitecture>(json_str) {
            Ok(arch) => {
                self.graph = arch.to_graph(self.perspective);
                self.raw_architecture = Some(arch);
                self.active_preset = GraphPreset::LiveSystem;
                self.is_loaded = true;
                self.load_error = None;
                self.recalculate_layout();
                Ok(())
            }
            Err(e) => {
                let err = format!("Failed to parse system_architecture.json: {}", e);
                self.load_error = Some(err.clone());
                Err(err)
            }
        }
    }

    pub fn set_perspective(&mut self, perspective: GraphPerspective) {
        self.perspective = perspective;
        if self.active_preset == GraphPreset::LiveSystem {
            if let Some(arch) = &self.raw_architecture {
                self.graph = arch.to_graph(perspective);
                self.recalculate_layout();
            }
        }
    }

    pub fn set_layout_direction(&mut self, direction: LayoutDirection) {
        self.layout_direction = direction;
        self.recalculate_layout();
    }

    pub fn apply_preset(&mut self, preset: GraphPreset) {
        self.active_preset = preset;
        match preset {
            GraphPreset::LiveSystem => {
                if let Some(arch) = &self.raw_architecture {
                    self.graph = arch.to_graph(self.perspective);
                    self.recalculate_layout();
                }
            }
            GraphPreset::SampleCallGraph => {
                self.graph = samples::sample_calculation_call_graph();
                self.recalculate_layout();
            }
            GraphPreset::SampleDataFlow => {
                self.graph = samples::sample_dataflow_pipeline();
                self.recalculate_layout();
            }
            GraphPreset::SampleTaskHierarchy => {
                self.graph = samples::sample_task_hierarchy();
                self.recalculate_layout();
            }
        }
    }

    pub fn recalculate_layout(&mut self) {
        let dims = self.display_mode.dimensions();
        let settings = LayoutSettings {
            card_width: dims.width,
            card_height: dims.height,
            col_gap: 80.0,
            row_gap: 36.0,
            origin_x: 60.0,
            origin_y: 60.0,
            direction: self.layout_direction,
        };
        self.node_positions = graph_layout::compute_hierarchical_layout(&self.graph, &settings);
        self.components.clear();
        for (id, node) in &self.graph.nodes {
            if let NodeKind::Component(c) = &node.kind {
                self.components.insert(id.clone(), c.clone());
            }
        }
        self.pan = Vec2::new(40.0, 40.0);
        self.zoom = 1.0;
    }


    // Camera coordinate conversions
    pub fn world_to_screen(&self, world_pos: Pos2, canvas_rect: Rect) -> Pos2 {
        canvas_rect.min + self.pan + (world_pos.to_vec2() * self.zoom)
    }

    pub fn screen_to_world(&self, screen_pos: Pos2, canvas_rect: Rect) -> Pos2 {
        let rel = screen_pos - canvas_rect.min - self.pan;
        Pos2::new(rel.x / self.zoom, rel.y / self.zoom)
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        feedback: &mut FeedbackInspectorState,
        slot_idx: usize,
    ) {
        // 1. Process asynchronous file fetch
        let incoming = self.pending_load.lock().ok().and_then(|mut l| l.take());
        if let Some(res) = incoming {
            match res {
                Ok(json_str) => {
                    let _ = self.load_from_json(&json_str);
                }
                Err(e) => {
                    self.load_error = Some(e);
                    self.is_loaded = true;
                }
            }
        }

        ui.vertical(|ui| {
            // 2. Toolbar Bar
            let start_y = ui.cursor().min.y;
            let start_x = ui.cursor().min.x;
            let toolbar_width = ui.available_width();

            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{} Arch Graph", egui_phosphor::regular::SHARE_NETWORK))
                        .strong()
                        .size(12.0)
                        .color(Color32::from_rgb(140, 200, 255)),
                );

                ui.separator();

                // Preset selector
                let mut current_preset = self.active_preset;
                egui::ComboBox::from_id_salt(format!("slot_{}_graph_preset", slot_idx))
                    .selected_text(match current_preset {
                        GraphPreset::LiveSystem => format!("{} Live System", egui_phosphor::regular::GLOBE),
                        GraphPreset::SampleCallGraph => format!("{} Call Graph", egui_phosphor::regular::PHONE_CALL),
                        GraphPreset::SampleDataFlow => format!("{} Data Flow", egui_phosphor::regular::WAVES),
                        GraphPreset::SampleTaskHierarchy => format!("{} Tasks", egui_phosphor::regular::TREE_STRUCTURE),
                    })
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_value(
                                &mut current_preset,
                                GraphPreset::LiveSystem,
                                format!("{} Live System", egui_phosphor::regular::GLOBE),
                            )
                            .clicked()
                        {
                            self.apply_preset(GraphPreset::LiveSystem);
                        }
                        if ui
                            .selectable_value(
                                &mut current_preset,
                                GraphPreset::SampleCallGraph,
                                format!("{} Sample: Call Graph", egui_phosphor::regular::PHONE_CALL),
                            )
                            .clicked()
                        {
                            self.apply_preset(GraphPreset::SampleCallGraph);
                        }
                        if ui
                            .selectable_value(
                                &mut current_preset,
                                GraphPreset::SampleDataFlow,
                                format!("{} Sample: Data Flow", egui_phosphor::regular::WAVES),
                            )
                            .clicked()
                        {
                            self.apply_preset(GraphPreset::SampleDataFlow);
                        }
                        if ui
                            .selectable_value(
                                &mut current_preset,
                                GraphPreset::SampleTaskHierarchy,
                                format!("{} Sample: Task Hierarchy", egui_phosphor::regular::TREE_STRUCTURE),
                            )
                            .clicked()
                        {
                            self.apply_preset(GraphPreset::SampleTaskHierarchy);
                        }
                    });

                // Perspective selector (when live architecture exists)
                if self.raw_architecture.is_some() || self.active_preset == GraphPreset::LiveSystem {
                    ui.separator();
                    let mut cur_perspective = self.perspective;
                    egui::ComboBox::from_id_salt(format!("slot_{}_graph_perspective", slot_idx))
                        .selected_text(match cur_perspective {
                            GraphPerspective::CallFlow => format!("{} Call Flow", egui_phosphor::regular::PHONE_CALL),
                            GraphPerspective::ComponentHierarchy => format!("{} Components", egui_phosphor::regular::PUZZLE_PIECE),
                            GraphPerspective::DataFlow => format!("{} Data Flow", egui_phosphor::regular::WAVES),
                            GraphPerspective::Unified => format!("{} Unified", egui_phosphor::regular::SHARE_NETWORK),
                        })
                        .show_ui(ui, |ui| {
                            if ui.selectable_value(&mut cur_perspective, GraphPerspective::CallFlow, format!("{} Call Flow", egui_phosphor::regular::PHONE_CALL)).clicked() {
                                self.set_perspective(GraphPerspective::CallFlow);
                            }
                            if ui.selectable_value(&mut cur_perspective, GraphPerspective::ComponentHierarchy, format!("{} Components", egui_phosphor::regular::PUZZLE_PIECE)).clicked() {
                                self.set_perspective(GraphPerspective::ComponentHierarchy);
                            }
                            if ui.selectable_value(&mut cur_perspective, GraphPerspective::DataFlow, format!("{} Data Flow", egui_phosphor::regular::WAVES)).clicked() {
                                self.set_perspective(GraphPerspective::DataFlow);
                            }
                            if ui.selectable_value(&mut cur_perspective, GraphPerspective::Unified, format!("{} Unified Graph", egui_phosphor::regular::SHARE_NETWORK)).clicked() {
                                self.set_perspective(GraphPerspective::Unified);
                            }
                        });
                }

                // Layout Direction Toggle Button
                ui.separator();
                let (dir_icon, dir_label) = match self.layout_direction {
                    LayoutDirection::LeftToRight => (egui_phosphor::regular::ARROWS_LEFT_RIGHT, "LR"),
                    LayoutDirection::TopToBottom => (egui_phosphor::regular::ARROWS_DOWN_UP, "TB"),
                };
                if ui.button(format!("{} {}", dir_icon, dir_label))
                    .on_hover_text("Toggle Layout Direction (Left-to-Right / Top-to-Bottom)")
                    .clicked()
                {
                    let next_dir = match self.layout_direction {
                        LayoutDirection::LeftToRight => LayoutDirection::TopToBottom,
                        LayoutDirection::TopToBottom => LayoutDirection::LeftToRight,
                    };
                    self.set_layout_direction(next_dir);
                }

                // Display Mode selector
                ui.separator();
                let mut current_mode = self.display_mode;
                egui::ComboBox::from_id_salt(format!("slot_{}_graph_mode", slot_idx))
                    .selected_text(match current_mode {
                        DisplayMode::Detailed => format!("{} Cards", egui_phosphor::regular::CARDS),
                        DisplayMode::Compact => format!("{} Pills", egui_phosphor::regular::PILL),
                        DisplayMode::AlignmentDiff => format!("{} Diff", egui_phosphor::regular::GIT_DIFF),
                    })
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_value(&mut current_mode, DisplayMode::Detailed, format!("{} Detailed Cards", egui_phosphor::regular::CARDS))
                            .clicked()
                        {
                            self.display_mode = DisplayMode::Detailed;
                            self.recalculate_layout();
                        }
                        if ui
                            .selectable_value(&mut current_mode, DisplayMode::Compact, format!("{} Compact Pills", egui_phosphor::regular::PILL))
                            .clicked()
                        {
                            self.display_mode = DisplayMode::Compact;
                            self.recalculate_layout();
                        }
                        if ui
                            .selectable_value(
                                &mut current_mode,
                                DisplayMode::AlignmentDiff,
                                format!("{} AST Alignment Diff", egui_phosphor::regular::GIT_DIFF),
                            )
                            .clicked()
                        {
                            self.display_mode = DisplayMode::AlignmentDiff;
                            self.recalculate_layout();
                        }
                    });

                // Zoom Strategy Selector
                ui.separator();
                let mut cur_zoom_strat = self.zoom_strategy;
                egui::ComboBox::from_id_salt(format!("slot_{}_zoom_strategy", slot_idx))
                    .selected_text(match cur_zoom_strat {
                        ZoomStrategy::Geometric => format!("{} Geometric", egui_phosphor::regular::MAGNIFYING_GLASS_PLUS),
                        ZoomStrategy::Semantic => format!("{} Semantic", egui_phosphor::regular::FRAME_CORNERS),
                    })
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_value(
                                &mut cur_zoom_strat,
                                ZoomStrategy::Geometric,
                                format!("{} Geometric (CAD / Scale)", egui_phosphor::regular::MAGNIFYING_GLASS_PLUS),
                            )
                            .clicked()
                        {
                            self.zoom_strategy = ZoomStrategy::Geometric;
                        }
                        if ui
                            .selectable_value(
                                &mut cur_zoom_strat,
                                ZoomStrategy::Semantic,
                                format!("{} Semantic (Fixed Cards)", egui_phosphor::regular::FRAME_CORNERS),
                            )
                            .clicked()
                        {
                            self.zoom_strategy = ZoomStrategy::Semantic;
                        }
                    });

                // Zoom Controls (+ / - / Percentage)
                if ui.button(egui_phosphor::regular::MINUS).on_hover_text("Zoom Out").clicked() {
                    self.zoom = (self.zoom / 1.2).clamp(0.2, 4.0);
                }
                ui.label(
                    RichText::new(format!("{:.0}%", self.zoom * 100.0))
                        .monospace()
                        .size(11.0)
                        .color(Color32::from_rgb(200, 220, 240)),
                );
                if ui.button(egui_phosphor::regular::PLUS).on_hover_text("Zoom In").clicked() {
                    self.zoom = (self.zoom * 1.2).clamp(0.2, 4.0);
                }

                // Layout Reset Button
                if ui.button(format!("{} Reset", egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE))
                    .on_hover_text("Reset Layout and Zoom")
                    .clicked()
                {
                    self.zoom = 1.0;
                    self.pan = Vec2::ZERO;
                    self.recalculate_layout();
                }

                // Node Count Badge
                ui.label(
                    RichText::new(format!("{} Nodes • {} Edges", self.graph.nodes.len(), self.graph.edges.len()))
                        .monospace()
                        .size(11.0)
                        .color(Color32::from_rgb(180, 200, 220)),
                );

                // Search Filter Box
                ui.add(
                    egui::TextEdit::singleline(&mut self.search_filter)
                        .desired_width(100.0)
                        .hint_text("Search..."),
                );
            });

            let end_y = ui.cursor().min.y;
            let toolbar_rect = Rect::from_min_size(
                Pos2::new(start_x, start_y),
                Vec2::new(toolbar_width, (end_y - start_y).max(22.0)),
            );
            feedback.register_inspectable(
                ui,
                &format!("slot_{}:graph:toolbar", slot_idx + 1),
                toolbar_rect,
            );
            ui.separator();

            // Error notice if failed to load
            if let Some(err) = &self.load_error {
                ui.label(RichText::new(err).color(Color32::from_rgb(255, 100, 100)).size(11.0));
            }

            // 3. Interactive Pan & Zoom Canvas
            let available_size = ui.available_size();
            let (response, painter) = ui.allocate_painter(available_size, egui::Sense::click_and_drag());
            let canvas_rect = response.rect;

            feedback.register_inspectable(
                ui,
                &format!("slot_{}:graph:canvas", slot_idx + 1),
                canvas_rect,
            );

            // Background Fill & Subtle Grid
            painter.rect_filled(canvas_rect, CornerRadius::ZERO, Color32::from_rgb(12, 15, 20));

            // Pan & Zoom input handling
            if response.hovered() {
                let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
                if scroll_delta.abs() > 0.0 {
                    let old_zoom = self.zoom;
                    let zoom_factor = (1.0 + scroll_delta * 0.002).clamp(0.8, 1.25);
                    self.zoom = (self.zoom * zoom_factor).clamp(0.3, 2.5);

                    // Zoom centering toward pointer
                    if let Some(mouse_pos) = response.hover_pos() {
                        let pointer_world = (mouse_pos - canvas_rect.min - self.pan) / old_zoom;
                        self.pan = mouse_pos - canvas_rect.min - (pointer_world * self.zoom);
                    }
                }
            }

            let is_space_down = ui.input(|i| i.key_down(egui::Key::Space));
            if is_space_down {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
            }

            // Canvas dragging (panning vs node dragging)
            if response.dragged() {
                if is_space_down {
                    // Holding space down forces canvas panning regardless of what was clicked
                    self.pan += response.drag_delta();
                } else if let Some(dragged_id) = &self.dragging_node_id {
                    // Moving a specific node
                    if let Some(pos) = self.node_positions.get_mut(dragged_id) {
                        pos.x += response.drag_delta().x / self.zoom;
                        pos.y += response.drag_delta().y / self.zoom;
                    }
                } else {
                    // Panning entire canvas
                    self.pan += response.drag_delta();
                }
            }

            if response.drag_stopped() {
                self.dragging_node_id = None;
            }

            // Paint Subtle 2D Canvas Grid
            let grid_step = 40.0 * self.zoom;
            if grid_step > 8.0 {
                let start_x = canvas_rect.min.x + (self.pan.x % grid_step);
                let start_y = canvas_rect.min.y + (self.pan.y % grid_step);
                let mut x = start_x;
                while x < canvas_rect.max.x {
                    painter.line_segment(
                        [Pos2::new(x, canvas_rect.min.y), Pos2::new(x, canvas_rect.max.y)],
                        Stroke::new(1.0, Color32::from_rgba_premultiplied(25, 32, 45, 60)),
                    );
                    x += grid_step;
                }
                let mut y = start_y;
                while y < canvas_rect.max.y {
                    painter.line_segment(
                        [Pos2::new(canvas_rect.min.x, y), Pos2::new(canvas_rect.max.x, y)],
                        Stroke::new(1.0, Color32::from_rgba_premultiplied(25, 32, 45, 60)),
                    );
                    y += grid_step;
                }
            }

            // 4. Calculate Screen Positions for all nodes
            let card_scale = self.zoom_strategy.card_scale(self.zoom);
            let dims = self.display_mode.dimensions();
            let scaled_dims = Vec2::new(dims.width * card_scale, dims.height * card_scale);
            let mut screen_rects = BTreeMap::new();

            for (id, &world_pos) in &self.node_positions {
                // Apply search filter
                if !self.search_filter.is_empty() {
                    let match_id = id.to_lowercase().contains(&self.search_filter.to_lowercase());
                    let match_label = self
                        .graph
                        .nodes
                        .get(id)
                        .map(|n| n.label.to_lowercase().contains(&self.search_filter.to_lowercase()))
                        .unwrap_or(false);
                    if !match_id && !match_label {
                        continue;
                    }
                }

                let screen_min = self.world_to_screen(world_pos, canvas_rect);
                let node_screen_rect = Rect::from_min_size(screen_min, scaled_dims);
                screen_rects.insert(id.clone(), node_screen_rect);
            }

            // 5. Draw Edges (curves and arrows)
            for edge in &self.graph.edges {
                if let (Some(&src_rect), Some(&tgt_rect)) = (
                    screen_rects.get(&edge.source_id),
                    screen_rects.get(&edge.target_id),
                ) {
                    graph_draw::draw_edge(
                        &painter,
                        edge,
                        src_rect,
                        tgt_rect,
                        self.layout_direction,
                        card_scale,
                    );
                }
            }

            // 6. Draw Nodes
            let mouse_pressed = ui.input(|i| i.pointer.primary_pressed());

            // Node dragging start: on primary_pressed, find which node is under hover (unless space is held)
            if !is_space_down && mouse_pressed && self.dragging_node_id.is_none() {
                if let Some(mouse_pos) = response.hover_pos() {
                    for (id, _) in &self.graph.nodes {
                        if let Some(&node_rect) = screen_rects.get(id) {
                            if node_rect.contains(mouse_pos) {
                                self.dragging_node_id = Some(id.clone());
                                break;
                            }
                        }
                    }
                }
            }

            let enable_lod = self.zoom_strategy.enables_lod();
            for (id, node) in &self.graph.nodes {
                if let Some(&node_rect) = screen_rects.get(id) {
                    let is_selected = self.selected_node_id.as_deref() == Some(id.as_str());
                    graph_draw::render_node(
                        &painter,
                        node,
                        node_rect,
                        self.display_mode,
                        is_selected,
                        self.layout_direction,
                        card_scale,
                        enable_lod,
                    );
                }
            }

            // Completed click on canvas
            if response.clicked() {
                if let Some(click_pos) = response.hover_pos() {
                    let panel_w = 280.0;
                    let panel_rect = Rect::from_min_max(
                        Pos2::new(canvas_rect.max.x - panel_w, canvas_rect.min.y + 12.0),
                        Pos2::new(canvas_rect.max.x - 12.0, canvas_rect.max.y - 12.0),
                    );

                    let mut clicked_node = None;
                    for (id, _) in &self.graph.nodes {
                        if let Some(&node_rect) = screen_rects.get(id) {
                            if node_rect.contains(click_pos) {
                                clicked_node = Some(id.clone());
                                break;
                            }
                        }
                    }

                    if let Some(id) = clicked_node {
                        self.selected_node_id = Some(id);
                    } else if !panel_rect.contains(click_pos) {
                        self.selected_node_id = None;
                    }
                }
            }

            // 7. Contract Inspector Drawer (Overlay on right side if a node is selected)
            if let Some(sel_id) = &self.selected_node_id.clone() {
                if let Some(node) = self.graph.nodes.get(sel_id) {
                    if self.render_inspector_drawer(ui, &painter, canvas_rect, node) {
                        self.selected_node_id = None;
                    }
                }
            }
        });
    }

    pub fn execute_action(&mut self, action: crate::keymap::GraphAction) {
        match action {
            crate::keymap::GraphAction::ZoomIn => {
                self.zoom = (self.zoom * 1.15).clamp(0.3, 2.5);
            }
            crate::keymap::GraphAction::ZoomOut => {
                self.zoom = (self.zoom / 1.15).clamp(0.3, 2.5);
            }
            crate::keymap::GraphAction::ResetLayout => {
                self.recalculate_layout();
            }
            crate::keymap::GraphAction::CycleDisplayMode => {
                self.display_mode = match self.display_mode {
                    DisplayMode::Detailed => DisplayMode::Compact,
                    DisplayMode::Compact => DisplayMode::AlignmentDiff,
                    DisplayMode::AlignmentDiff => DisplayMode::Detailed,
                };
                self.recalculate_layout();
            }
            crate::keymap::GraphAction::CloseInspector => {
                self.selected_node_id = None;
            }
        }
    }

    pub fn handle_input(&mut self, i: &egui::InputState, keymap: &crate::keymap::GraphKeymap) {
        if keymap.zoom_in.is_pressed(i) {
            self.execute_action(crate::keymap::GraphAction::ZoomIn);
        }
        if keymap.zoom_out.is_pressed(i) {
            self.execute_action(crate::keymap::GraphAction::ZoomOut);
        }
        if keymap.reset_layout.is_pressed(i) {
            self.execute_action(crate::keymap::GraphAction::ResetLayout);
        }
        if keymap.cycle_mode.is_pressed(i) {
            self.execute_action(crate::keymap::GraphAction::CycleDisplayMode);
        }
        if keymap.close_drawer.is_pressed(i) || i.key_pressed(egui::Key::Escape) {
            self.execute_action(crate::keymap::GraphAction::CloseInspector);
        }
    }

    fn render_inspector_drawer(&self, ui: &mut egui::Ui, painter: &egui::Painter, canvas_rect: Rect, node: &GraphNode) -> bool {
        let panel_w = 280.0;
        let panel_rect = Rect::from_min_max(
            Pos2::new(canvas_rect.max.x - panel_w, canvas_rect.min.y + 12.0),
            Pos2::new(canvas_rect.max.x - 12.0, canvas_rect.max.y - 12.0),
        );

        let mut close_requested = false;
        let close_rect = Rect::from_min_size(
            Pos2::new(panel_rect.max.x - 26.0, panel_rect.min.y + 10.0),
            Vec2::new(16.0, 16.0),
        );
        if let Some(mouse_pos) = ui.input(|i| i.pointer.hover_pos()) {
            if close_rect.contains(mouse_pos) {
                painter.rect_filled(close_rect, CornerRadius::same(3), Color32::from_rgb(45, 55, 75));
                if ui.input(|i| i.pointer.primary_clicked()) {
                    close_requested = true;
                }
            }
        }
        painter.text(
            close_rect.center(),
            egui::Align2::CENTER_CENTER,
            "✕",
            egui::FontId::monospace(11.0),
            Color32::from_rgb(180, 200, 220),
        );

        painter.rect_filled(
            panel_rect,
            CornerRadius::same(6),
            Color32::from_rgba_premultiplied(16, 20, 26, 245),
        );
        painter.rect_stroke(
            panel_rect,
            CornerRadius::same(6),
            Stroke::new(1.0, Color32::from_rgb(60, 80, 110)),
            StrokeKind::Inside,
        );

        let text_x = panel_rect.min.x + 14.0;
        let mut text_y = panel_rect.min.y + 14.0;

        painter.text(
            Pos2::new(text_x, text_y),
            egui::Align2::LEFT_TOP,
            "CONTRACT INSPECTOR",
            egui::FontId::proportional(11.0),
            Color32::from_rgb(250, 180, 50),
        );
        text_y += 20.0;

        painter.text(
            Pos2::new(text_x, text_y),
            egui::Align2::LEFT_TOP,
            &node.label,
            egui::FontId::proportional(13.0),
            Color32::WHITE,
        );
        text_y += 18.0;

        painter.text(
            Pos2::new(text_x, text_y),
            egui::Align2::LEFT_TOP,
            format!("Node ID: {}", node.id),
            egui::FontId::monospace(10.0),
            Color32::GRAY,
        );
        text_y += 18.0;

        // Alignment Pill
        let (status_text, status_color) = match &node.alignment {
            AlignmentStatus::VerifiedInCode { location } => (
                format!("✓ Verified ({}:{})", location.file_path, location.start_line),
                Color32::from_rgb(80, 200, 120),
            ),
            AlignmentStatus::SignatureMismatch { diff, .. } => (
                format!("⚠ Signature Mismatch ({})", diff),
                Color32::from_rgb(250, 90, 80),
            ),
            AlignmentStatus::MissingCallSite { expected_in } => (
                format!("○ Missing in {}", expected_in),
                Color32::from_rgb(250, 180, 50),
            ),
            AlignmentStatus::PlannedOnly => ("○ Planned Only".to_string(), Color32::from_rgb(100, 160, 240)),
        };

        painter.text(
            Pos2::new(text_x, text_y),
            egui::Align2::LEFT_TOP,
            status_text,
            egui::FontId::monospace(10.0),
            status_color,
        );
        text_y += 24.0;

        // Kind details
        match &node.kind {
            NodeKind::Component(comp) => {
                painter.text(
                    Pos2::new(text_x, text_y),
                    egui::Align2::LEFT_TOP,
                    format!("Type: {} | Stage: {}", comp.comp_type, comp.stage),
                    egui::FontId::monospace(10.5),
                    Color32::from_rgb(140, 200, 255),
                );
                text_y += 20.0;

                painter.text(
                    Pos2::new(text_x, text_y),
                    egui::Align2::LEFT_TOP,
                    "Description:",
                    egui::FontId::proportional(11.0),
                    Color32::LIGHT_GRAY,
                );
                text_y += 16.0;

                let desc = if comp.description.is_empty() { "No description declared" } else { &comp.description };
                painter.text(
                    Pos2::new(text_x, text_y),
                    egui::Align2::LEFT_TOP,
                    desc,
                    egui::FontId::proportional(10.5),
                    Color32::WHITE,
                );
                text_y += 35.0;

                painter.text(
                    Pos2::new(text_x, text_y),
                    egui::Align2::LEFT_TOP,
                    format!("Modification Tasks: {}", comp.modification_tasks.len()),
                    egui::FontId::proportional(11.0),
                    Color32::from_rgb(180, 220, 255),
                );
            }
            NodeKind::UsageCall(call) => {
                painter.text(
                    Pos2::new(text_x, text_y),
                    egui::Align2::LEFT_TOP,
                    format!("Caller: {}", call.caller_id),
                    egui::FontId::monospace(10.5),
                    Color32::from_rgb(180, 140, 255),
                );
                text_y += 18.0;

                painter.text(
                    Pos2::new(text_x, text_y),
                    egui::Align2::LEFT_TOP,
                    format!("Callee: {}", call.component_id),
                    egui::FontId::monospace(10.5),
                    Color32::from_rgb(140, 200, 255),
                );
                text_y += 22.0;

                painter.text(
                    Pos2::new(text_x, text_y),
                    egui::Align2::LEFT_TOP,
                    &call.description,
                    egui::FontId::proportional(10.5),
                    Color32::WHITE,
                );
            }
            NodeKind::PlanStep(step) => {
                let status = if step.completed { "Completed [✓]" } else { "Pending [ ]" };
                painter.text(
                    Pos2::new(text_x, text_y),
                    egui::Align2::LEFT_TOP,
                    format!("Status: {}", status),
                    egui::FontId::monospace(11.0),
                    if step.completed { Color32::from_rgb(80, 200, 120) } else { Color32::from_rgb(250, 180, 50) },
                );
                text_y += 20.0;

                painter.text(
                    Pos2::new(text_x, text_y),
                    egui::Align2::LEFT_TOP,
                    &step.task,
                    egui::FontId::proportional(11.0),
                    Color32::WHITE,
                );
            }
        }

        close_requested
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zoom_strategy_defaults_and_card_scale() {
        assert_eq!(ZoomStrategy::default(), ZoomStrategy::Geometric);

        let geo = ZoomStrategy::Geometric;
        assert_eq!(geo.card_scale(1.0), 1.0);
        assert_eq!(geo.card_scale(1.75), 1.75);
        assert_eq!(geo.card_scale(0.5), 0.5);
        assert!(geo.enables_lod());

        let sem = ZoomStrategy::Semantic;
        assert_eq!(sem.card_scale(1.0), 1.0);
        assert_eq!(sem.card_scale(1.75), 1.0);
        assert_eq!(sem.card_scale(0.5), 1.0);
        assert!(!sem.enables_lod());
    }

    #[test]
    fn test_state_initializes_with_geometric_strategy() {
        let state = ArchitectureGraphState::default();
        assert_eq!(state.zoom_strategy, ZoomStrategy::Geometric);
    }

    #[test]
    fn test_geometric_vs_semantic_screen_rect_computation() {
        let mut state = ArchitectureGraphState::default();
        let dims = state.display_mode.dimensions(); // 230.0 x 105.0

        // In Geometric mode with zoom = 2.0:
        state.zoom = 2.0;
        state.zoom_strategy = ZoomStrategy::Geometric;
        let scale_geo = state.zoom_strategy.card_scale(state.zoom);
        assert_eq!(scale_geo, 2.0);
        let geo_scaled_dims = egui::Vec2::new(dims.width * scale_geo, dims.height * scale_geo);
        assert_eq!(geo_scaled_dims, egui::Vec2::new(460.0, 210.0));

        // In Semantic mode with zoom = 2.0:
        state.zoom_strategy = ZoomStrategy::Semantic;
        let scale_sem = state.zoom_strategy.card_scale(state.zoom);
        assert_eq!(scale_sem, 1.0);
        let sem_scaled_dims = egui::Vec2::new(dims.width * scale_sem, dims.height * scale_sem);
        assert_eq!(sem_scaled_dims, egui::Vec2::new(230.0, 105.0));
    }
}
