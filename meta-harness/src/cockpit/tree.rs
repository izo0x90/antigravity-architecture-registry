use egui::{Color32, RichText, Ui};

pub enum NodeStatus {
    Pass,
    Active,
    Pending,
}

pub struct PlanTreeNode {
    pub id: &'static str,
    pub title: &'static str,
    pub kind: &'static str,
    pub status: NodeStatus,
    pub detail: &'static str,
    pub children: Vec<PlanTreeNode>,
}

pub struct PlanTreeState {
    pub root: PlanTreeNode,
}

impl Default for PlanTreeState {
    fn default() -> Self {
        Self {
            root: PlanTreeNode {
                id: "root",
                title: "Meta-Harness Control System Plan",
                kind: "ROOT",
                status: NodeStatus::Active,
                detail: "High-integrity architecture plan and execution invariants",
                children: vec![
                    PlanTreeNode {
                        id: "process",
                        title: "Process Execution & Stdio Transport",
                        kind: "REQUIREMENT",
                        status: NodeStatus::Pass,
                        detail: "TokioProcessSpawner running real agy binary over async stdio",
                        children: vec![],
                    },
                    PlanTreeNode {
                        id: "transport",
                        title: "Transport & Protocol Engine",
                        kind: "REQUIREMENT",
                        status: NodeStatus::Pass,
                        detail: "Non-blocking line-delimited JSON-RPC with reverse-RPC support",
                        children: vec![],
                    },
                    PlanTreeNode {
                        id: "sandboxing",
                        title: "Filesystem Sandboxing & Traversal Guard",
                        kind: "INVARIANT",
                        status: NodeStatus::Pass,
                        detail: "FsProxy canonicalizes paths and rejects symlink/directory escapes",
                        children: vec![],
                    },
                    PlanTreeNode {
                        id: "cockpit",
                        title: "Native Egui Cockpit Ergonomics",
                        kind: "INVARIANT",
                        status: NodeStatus::Active,
                        detail: "100% pure Rust desktop UI, persistent prompt focus, CommonMark GFM rendering",
                        children: vec![
                            PlanTreeNode {
                                id: "cockpit_chat",
                                title: "Harness Execution Stream (Chat)",
                                kind: "LEAF",
                                status: NodeStatus::Pass,
                                detail: "Real-time stream, collapsible thinking, tool cards, GFM markdown",
                                children: vec![],
                            },
                            PlanTreeNode {
                                id: "cockpit_code",
                                title: "Code Viewer with Auto-Open Links",
                                kind: "LEAF",
                                status: NodeStatus::Pass,
                                detail: "Line numbers, gutter styling, and instant file jumps",
                                children: vec![],
                            },
                            PlanTreeNode {
                                id: "cockpit_graph",
                                title: "Deterministic Architecture DAG",
                                kind: "LEAF",
                                status: NodeStatus::Pass,
                                detail: "Zero-physics box-and-wire layout with port contract inspections",
                                children: vec![],
                            },
                        ],
                    },
                ],
            },
        }
    }
}

impl PlanTreeState {
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            // Header
            ui.horizontal(|ui| {
                ui.label(RichText::new("🌳").size(12.0));
                ui.label(RichText::new("INVARIANTS & PLAN DECOMPOSITION").strong().color(Color32::from_rgb(147, 197, 253)));
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    render_tree_node(ui, &self.root);
                });
        });
    }
}

fn render_tree_node(ui: &mut Ui, node: &PlanTreeNode) {
    let (status_text, status_color) = match node.status {
        NodeStatus::Pass => ("PASS", Color32::from_rgb(16, 185, 129)),
        NodeStatus::Active => ("ACTIVE", Color32::from_rgb(245, 158, 11)),
        NodeStatus::Pending => ("PENDING", Color32::from_rgb(148, 163, 184)),
    };

    if node.children.is_empty() {
        ui.horizontal(|ui| {
            ui.label(RichText::new("•").size(11.0).color(Color32::GRAY));
            ui.label(RichText::new(node.title).size(11.0).color(Color32::WHITE));
            ui.label(RichText::new(format!("[{}]", node.kind)).monospace().size(9.0).color(Color32::from_rgb(100, 116, 139)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(status_text).size(9.0).color(status_color).strong());
            });
        });
        if !node.detail.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new(node.detail).size(10.0).color(Color32::from_rgb(148, 163, 184)));
            });
        }
        ui.add_space(2.0);
    } else {
        egui::CollapsingHeader::new(RichText::new(node.title).strong().size(11.0).color(Color32::WHITE))
            .default_open(true)
            .show(ui, |ui| {
                if !node.detail.is_empty() {
                    ui.label(RichText::new(node.detail).size(10.0).color(Color32::from_rgb(148, 163, 184)));
                    ui.add_space(2.0);
                }
                for child in &node.children {
                    render_tree_node(ui, child);
                }
            });
    }
}
