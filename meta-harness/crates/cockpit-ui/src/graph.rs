//! The architecture graph: server views drawn with pan and zoom, a navigation
//! history (view, plan, focus, depth), vim-style commands, hint targeting, and
//! a compact inspector. Everything shown comes from the server's views; the
//! graph refreshes itself when the architecture changes.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use arch_graph::view::Rect as WorldRect;
use arch_graph::{Alignment, ArchEvent, ChangeReason, Component, ComponentId, PlanId, RelationKind, Requester, ViewCatalog, ViewRequest, ViewResult};
use egui::{Align2, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Vec2};

use crate::actions::{Action, ActionProvider, Choice, Param, RunCx, ToolbarStyle, ToolbarUse, toolbar_control};
use crate::feedback::FeedbackInspectorState;
use crate::keymap::InputMode;
use crate::net::{NetClient, NetResult};
use crate::graph_style::{provenance_of, style};
use crate::theme::colors;

/// Where the graph is looking: which view, laid over which plan, rooted where,
/// how deep. Every change is a step in the history.
#[derive(Clone, Debug, PartialEq)]
pub struct Nav {
    pub view: String,
    pub plan: Option<PlanId>,
    pub roots: Vec<ComponentId>,
    /// None means the view's own depth.
    pub depth: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NavChange {
    View(String),
    Plan(Option<PlanId>),
    Focus(ComponentId),
    Unfocus,
    /// More (+) or fewer (-) hops; `base` is the view's own depth.
    Depth { delta: i32, base: u32 },
    Back,
    Forward,
    Request(Box<ViewRequest>),
}

/// The navigation history (a jump list): one reducer for keys, touch,
/// palette and agent requests alike.
#[derive(Clone, Debug)]
pub struct Navigator {
    history: Vec<Nav>,
    cursor: usize,
}

const HISTORY: usize = 100;

impl Navigator {
    pub fn new(view: &str) -> Self {
        Self { history: vec![Nav { view: view.to_owned(), plan: None, roots: Vec::new(), depth: None }], cursor: 0 }
    }

    pub fn current(&self) -> &Nav {
        &self.history[self.cursor]
    }

    pub fn can_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_forward(&self) -> bool {
        self.cursor + 1 < self.history.len()
    }

    /// Applies a change; returns whether the view must be fetched again.
    pub fn apply(&mut self, change: NavChange) -> bool {
        let mut next = self.current().clone();
        match change {
            NavChange::Back => {
                if !self.can_back() {
                    return false;
                }
                self.cursor -= 1;
                return true;
            }
            NavChange::Forward => {
                if !self.can_forward() {
                    return false;
                }
                self.cursor += 1;
                return true;
            }
            NavChange::View(view) => next.view = view,
            NavChange::Plan(plan) => next.plan = plan,
            NavChange::Focus(id) => next.roots = vec![id],
            NavChange::Unfocus => next.roots.clear(),
            NavChange::Depth { delta, base } => {
                let current = next.depth.unwrap_or(base) as i32;
                next.depth = Some((current + delta).clamp(1, 12) as u32);
            }
            NavChange::Request(request) => {
                let request = *request;
                if let Some(view) = request.view {
                    next.view = view;
                }
                if request.plan.is_some() {
                    next.plan = request.plan;
                }
                if !request.roots.is_empty() {
                    next.roots = request.roots;
                }
                if request.depth.is_some() {
                    next.depth = request.depth;
                }
            }
        }
        if &next == self.current() {
            return false;
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(next);
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
        self.cursor = self.history.len() - 1;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Down,
    Up,
    Right,
}

/// What a hint does to the node it lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintAction {
    Select,
    Focus,
    OpenCode,
    OpenSpec,
}

#[derive(Clone, Debug)]
pub struct Hint {
    pub action: HintAction,
    pub typed: String,
    pub labels: Vec<(String, ComponentId)>,
}

/// Minimal, unique, prefix-free labels for `n` targets: all the same length,
/// the shortest that the alphabet allows.
pub fn hint_labels(n: usize, alphabet: &[char]) -> Vec<String> {
    let k = alphabet.len().max(1);
    if n == 0 || alphabet.is_empty() {
        return Vec::new();
    }
    let mut len = 1;
    while k.pow(len as u32) < n {
        len += 1;
    }
    (0..n)
        .map(|mut i| {
            let mut label = vec![alphabet[0]; len];
            for slot in (0..len).rev() {
                label[slot] = alphabet[i % k];
                i /= k;
            }
            label.into_iter().collect()
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Camera {
    pan: Vec2,
    zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { pan: Vec2::new(24.0, 24.0), zoom: 1.0 }
    }
}

/// Things the graph asks other parts of the cockpit to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphRequests {
    /// Open this file at these lines in the code view.
    pub open_code: Option<(String, u32, u32)>,
    pub open_spec: bool,
}

enum Reply {
    Catalog(NetResult<ViewCatalog>),
    Plans(NetResult<Vec<arch_graph::Plan>>),
    View(u64, Box<NetResult<ViewResult>>),
}

#[derive(Clone, Debug, PartialEq)]
enum Load {
    Idle,
    Loading,
    Failed(String),
}

pub struct GraphState {
    catalog: Option<ViewCatalog>,
    catalog_requested: bool,
    plans: Vec<PlanId>,
    plans_stale: bool,
    pub nav: Navigator,
    pub result: Option<ViewResult>,
    /// Ids drawn inside some container (hidden when containers collapse).
    nested: BTreeSet<ComponentId>,
    load: Load,
    stale: bool,
    generation: u64,
    pub selection: Option<ComponentId>,
    /// Select this once it shows up (agent focus, reveal from code).
    pending_select: Option<ComponentId>,
    camera: Camera,
    fit: bool,
    pub hint: Option<Hint>,
    pub drawer_open: bool,
    /// The agent moved the view (shown until the user navigates).
    pub agent_moved: bool,
    pub requests: GraphRequests,
    canvas: Rect,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

impl Default for GraphState {
    fn default() -> Self {
        Self {
            catalog: None,
            catalog_requested: false,
            plans: Vec::new(),
            plans_stale: true,
            nav: Navigator::new("structure"),
            result: None,
            nested: BTreeSet::new(),
            load: Load::Idle,
            stale: true,
            generation: 0,
            selection: None,
            pending_select: None,
            camera: Camera::default(),
            fit: true,
            hint: None,
            drawer_open: true,
            agent_moved: false,
            requests: GraphRequests::default(),
            canvas: Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)),
            inbox: Arc::default(),
        }
    }
}

impl GraphState {
    fn base_depth(&self) -> u32 {
        self.catalog.as_ref().and_then(|c| c.get(&self.nav.current().view)).map_or(3, |v| v.depth)
    }

    pub fn component(&self, id: &ComponentId) -> Option<&Component> {
        self.result.as_ref().and_then(|r| r.components.get(id))
    }

    pub fn selected(&self) -> Option<&Component> {
        self.selection.as_ref().and_then(|id| self.component(id))
    }

    /// Navigates; the view is fetched again when it changed.
    pub fn navigate(&mut self, change: NavChange) {
        let user = !matches!(change, NavChange::Request(_));
        if self.nav.apply(change) {
            self.stale = true;
            self.fit = true;
        }
        if user {
            self.agent_moved = false;
        }
    }

    pub fn select(&mut self, id: Option<ComponentId>) {
        if id.is_some() {
            self.drawer_open = true;
        }
        self.selection = id;
    }

    /// Selects `id`, refocusing the view on it when it isn't shown.
    pub fn reveal(&mut self, id: ComponentId) {
        if self.component(&id).is_some() {
            self.select(Some(id));
        } else {
            self.pending_select = Some(id.clone());
            self.navigate(NavChange::Focus(id));
        }
    }

    pub fn on_event(&mut self, event: &ArchEvent) {
        match event {
            ArchEvent::Changed { reason, .. } => {
                self.stale = true;
                if matches!(reason, ChangeReason::PlanEdited) {
                    self.plans_stale = true;
                }
            }
            ArchEvent::Validated { .. } => self.stale = true,
            ArchEvent::ViewRequested { request, by } => {
                if *by == Requester::Agent {
                    self.agent_moved = true;
                }
                self.pending_select = request.focus.clone();
                self.navigate(NavChange::Request(request.clone()));
                if let Some(focus) = &request.focus
                    && self.component(focus).is_some()
                {
                    self.select(Some(focus.clone()));
                }
            }
        }
    }

    /// Applies a fetched view, keeping selection and camera when possible.
    pub fn set_result(&mut self, result: ViewResult) {
        if let Some(sel) = &self.selection
            && !result.components.contains_key(sel)
        {
            self.selection = None;
        }
        if let Some(pending) = self.pending_select.take() {
            if result.components.contains_key(&pending) {
                self.selection = Some(pending);
                self.drawer_open = true;
            } else {
                self.pending_select = Some(pending);
            }
        }
        self.nested = result
            .layout
            .containers
            .iter()
            .filter_map(|c| result.layout.rects.get(c))
            .flat_map(|outer| result.layout.rects.iter().filter(move |(_, r)| *r != outer && outer.contains(r)).map(|(id, _)| id.clone()))
            .collect();
        self.result = Some(result);
        self.load = Load::Idle;
    }

    /// Zoomed out past the point where container headers are legible: each
    /// top-level container is drawn as one labelled block.
    fn collapsed(&self) -> bool {
        arch_graph::view::layout::HEADER * self.camera.zoom < 11.0
    }

    fn shown(&self, id: &ComponentId) -> bool {
        !self.collapsed() || !self.nested.contains(id)
    }

    /// Fetches what's missing or stale and applies replies. Call every frame.
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::Catalog(Ok(catalog)) => {
                    if catalog.get(&self.nav.current().view).is_none()
                        && let Some(first) = catalog.views.keys().next()
                    {
                        self.nav = Navigator::new(first);
                    }
                    self.catalog = Some(catalog);
                }
                Reply::Catalog(Err(e)) => {
                    self.catalog_requested = false;
                    self.load = Load::Failed(e);
                }
                Reply::Plans(Ok(plans)) => self.plans = plans.into_iter().map(|p| p.id).collect(),
                Reply::Plans(Err(_)) => {}
                Reply::View(generation, result) => {
                    if generation != self.generation {
                        continue;
                    }
                    match *result {
                        Ok(result) => self.set_result(result),
                        Err(e) => self.load = Load::Failed(e),
                    }
                }
            }
        }
        if !self.catalog_requested {
            self.catalog_requested = true;
            let inbox = self.inbox.clone();
            net.arch_views(ctx.clone(), move |r| push(&inbox, Reply::Catalog(r)));
        }
        if self.plans_stale {
            self.plans_stale = false;
            let inbox = self.inbox.clone();
            net.arch_plans(ctx.clone(), move |r| push(&inbox, Reply::Plans(r)));
        }
        if self.stale && self.catalog.is_some() {
            self.stale = false;
            self.generation += 1;
            self.load = Load::Loading;
            let nav = self.nav.current().clone();
            let (inbox, generation) = (self.inbox.clone(), self.generation);
            net.arch_view(&nav.view, nav.plan.as_ref(), &nav.roots, nav.depth, ctx.clone(), move |r| push(&inbox, Reply::View(generation, Box::new(r))));
        }
    }

    /// Moves the selection to the nearest node in a direction (on the layout).
    pub fn move_selection(&mut self, dir: Dir) {
        let Some(result) = &self.result else { return };
        let Some(current) = self.selection.as_ref().and_then(|s| result.layout.rects.get(s)) else {
            let first = result.view.roots.first().or(result.view.nodes.first().map(|n| &n.id)).cloned();
            self.select(first);
            return;
        };
        let center = |r: &WorldRect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let (cx, cy) = center(current);
        let best = result
            .layout
            .rects
            .iter()
            .filter(|(id, _)| Some(*id) != self.selection.as_ref())
            .filter_map(|(id, r)| {
                let (x, y) = center(r);
                let (along, across) = match dir {
                    Dir::Right => (x - cx, y - cy),
                    Dir::Left => (cx - x, y - cy),
                    Dir::Down => (y - cy, x - cx),
                    Dir::Up => (cy - y, x - cx),
                };
                (along > 1.0 && across.abs() <= along * 2.0).then_some((along + 2.0 * across.abs(), id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id.clone());
        if best.is_some() {
            self.select(best);
        }
    }

    /// Neighbors of the selection along `kind` (outgoing or incoming), as shown.
    pub fn neighbors(&self, kind: RelationKind, outgoing: bool) -> Vec<ComponentId> {
        let (Some(result), Some(sel)) = (&self.result, &self.selection) else { return Vec::new() };
        result
            .view
            .edges
            .iter()
            .filter(|e| e.kind == kind)
            .filter_map(|e| match outgoing {
                true if &e.source == sel => Some(e.target.clone()),
                false if &e.target == sel => Some(e.source.clone()),
                _ => None,
            })
            .collect()
    }

    fn to_screen(&self, r: &WorldRect) -> Rect {
        let z = self.camera.zoom;
        Rect::from_min_size(self.canvas.min + self.camera.pan + Vec2::new(r.x, r.y) * z, Vec2::new(r.w, r.h) * z)
    }

    /// Nodes on screen, nearest to the selection (or the canvas center) first.
    pub fn visible_nodes(&self) -> Vec<ComponentId> {
        let Some(result) = &self.result else { return Vec::new() };
        let anchor = self.selection.as_ref().and_then(|s| result.layout.rects.get(s)).map(|r| self.to_screen(r).center()).unwrap_or(self.canvas.center());
        let mut visible: Vec<(f32, ComponentId)> = result
            .layout
            .rects
            .iter()
            .filter(|(id, _)| self.shown(id))
            .map(|(id, r)| (self.to_screen(r), id))
            .filter(|(rect, _)| rect.intersects(self.canvas))
            .map(|(rect, id)| (rect.center().distance(anchor), id.clone()))
            .collect();
        visible.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        visible.into_iter().map(|(_, id)| id).collect()
    }

    pub fn start_hint(&mut self, action: HintAction, alphabet: &str) -> bool {
        let targets = self.visible_nodes();
        let chars: Vec<char> = alphabet.chars().collect();
        let labels = hint_labels(targets.len(), &chars);
        if labels.is_empty() {
            return false;
        }
        self.hint = Some(Hint { action, typed: String::new(), labels: labels.into_iter().zip(targets).collect() });
        true
    }

    /// Feeds typed characters to an active hint; returns whether it's still
    /// waiting for more.
    pub fn hint_input(&mut self, typed: &[char], back: bool) -> bool {
        let Some(hint) = self.hint.as_mut() else { return false };
        if back {
            self.hint = None;
            return false;
        }
        for c in typed {
            hint.typed.push(*c);
            if let Some((_, target)) = hint.labels.iter().find(|(label, _)| *label == hint.typed) {
                let (action, target) = (hint.action, target.clone());
                self.hint = None;
                self.hint_hit(action, target);
                return false;
            }
            if !hint.labels.iter().any(|(label, _)| label.starts_with(&hint.typed)) {
                self.hint = None;
                return false;
            }
        }
        true
    }

    fn hint_hit(&mut self, action: HintAction, target: ComponentId) {
        self.select(Some(target.clone()));
        match action {
            HintAction::Select => {}
            HintAction::Focus => self.navigate(NavChange::Focus(target)),
            HintAction::OpenCode => self.request_code(),
            HintAction::OpenSpec => self.requests.open_spec = true,
        }
    }

    fn request_code(&mut self) {
        if let Some(c) = self.selected() {
            self.requests.open_code = match (&c.anchor, &c.proposed) {
                (Some(a), _) => Some((a.file.to_string(), a.span.start_line, a.span.end_line)),
                (None, Some(p)) => Some((p.file.to_string(), 1, 1)),
                (None, None) => None,
            };
        }
    }

    fn zoom_by(&mut self, factor: f32, around: Pos2) {
        let old = self.camera.zoom;
        let new = (old * factor).clamp(0.03, 3.0);
        let world = (around - self.canvas.min - self.camera.pan) / old;
        self.camera.zoom = new;
        self.camera.pan = around - self.canvas.min - world * new;
    }

    fn fit_view(&mut self) {
        let Some(result) = &self.result else { return };
        let rects: Vec<&WorldRect> = result.layout.rects.values().collect();
        if rects.is_empty() {
            return;
        }
        let (min_x, min_y) = rects.iter().fold((f32::MAX, f32::MAX), |(x, y), r| (x.min(r.x), y.min(r.y)));
        let (max_x, max_y) = rects.iter().fold((f32::MIN, f32::MIN), |(x, y), r| (x.max(r.x + r.w), y.max(r.y + r.h)));
        let (w, h) = ((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));
        let pad = 32.0;
        let zoom = ((self.canvas.width() - pad * 2.0) / w).min((self.canvas.height() - pad * 2.0) / h).clamp(0.03, 1.4);
        self.camera.zoom = zoom;
        let content = Vec2::new(w, h) * zoom;
        self.camera.pan = (self.canvas.size() - content) / 2.0 - Vec2::new(min_x, min_y) * zoom;
    }

    // ---- drawing ----

    pub fn render(&mut self, ui: &mut egui::Ui, feedback: &mut FeedbackInspectorState, slot_idx: usize, pan_held: bool, cx: &mut RunCx<'_>) {
        self.tick(cx.net, cx.ctx);
        self.render_toolbar(ui, cx);
        let area = ui.available_rect_before_wrap();
        let drawer_w = if self.drawer_open && self.selected().is_some() { (area.width() * 0.38).clamp(220.0, 340.0) } else { 0.0 };
        let canvas = Rect::from_min_max(area.min, Pos2::new(area.max.x - drawer_w, area.max.y));
        self.canvas = canvas;
        feedback.register_inspectable(ui, &format!("slot_{}:graph:canvas", slot_idx + 1), canvas);
        let response = ui.allocate_rect(canvas, Sense::click_and_drag());
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 0.0, colors().bg);

        if self.fit && self.result.is_some() {
            self.fit = false;
            self.fit_view();
        }
        // Pan (drag, or hold the leader key) and zoom (wheel, pinch).
        if response.dragged() {
            self.camera.pan += response.drag_delta();
        }
        if pan_held && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if response.hovered() {
            let (scroll, pinch, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
            let around = pointer.unwrap_or(canvas.center());
            if pinch != 1.0 {
                self.zoom_by(pinch, around);
            } else if scroll != 0.0 {
                self.zoom_by((1.0 + scroll * 0.0015).clamp(0.8, 1.25), around);
            }
        }

        match (&self.result, &self.load) {
            (_, Load::Failed(err)) if self.result.is_none() => {
                painter.text(canvas.center(), Align2::CENTER_CENTER, format!("Couldn't load the view: {err}"), FontId::proportional(13.0), colors().error);
            }
            (None, _) => {
                painter.text(canvas.center(), Align2::CENTER_CENTER, "Loading the architecture…", FontId::proportional(13.0), colors().muted);
            }
            (Some(result), _) if result.view.nodes.is_empty() => {
                painter.text(
                    canvas.center(),
                    Align2::CENTER_CENTER,
                    "Nothing to show in this view yet. Ingest the code (Space p… or the agent's ingest tool), or pick another view.",
                    FontId::proportional(13.0),
                    colors().muted,
                );
            }
            (Some(_), _) => self.draw_graph(&painter),
        }
        if let Load::Failed(err) = &self.load
            && self.result.is_some()
        {
            painter.text(canvas.left_bottom() + Vec2::new(8.0, -8.0), Align2::LEFT_BOTTOM, format!("refresh failed: {err}"), FontId::proportional(11.0), colors().error);
        }

        // Click selects, double click focuses.
        if (response.clicked() || response.double_clicked())
            && let Some(pos) = response.interact_pointer_pos()
        {
            let hit = self.hit(pos);
            if response.double_clicked()
                && let Some(id) = hit.clone()
            {
                self.navigate(NavChange::Focus(id));
            }
            self.select(hit);
        }
        if drawer_w > 0.0 {
            let drawer = Rect::from_min_max(Pos2::new(canvas.max.x, area.min.y), area.max);
            ui.scope_builder(egui::UiBuilder::new().max_rect(drawer.shrink(6.0)), |ui| self.render_drawer(ui));
        }
        if *cx.input_mode == (InputMode::Capture { slot: cx.slot }) && self.hint.is_none() {
            *cx.input_mode = InputMode::Normal;
        }
    }

    fn hit(&self, pos: Pos2) -> Option<ComponentId> {
        let result = self.result.as_ref()?;
        // Innermost first: smallest rect containing the point.
        result
            .layout
            .rects
            .iter()
            .map(|(id, r)| (self.to_screen(r), id))
            .filter(|(rect, _)| rect.contains(pos))
            .min_by(|a, b| a.0.area().total_cmp(&b.0.area()))
            .map(|(_, id)| id.clone())
    }

    fn render_toolbar(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        let actions = self.actions();
        let mut used: Option<(Action, ToolbarUse)> = None;
        ui.horizontal_wrapped(|ui| {
            for action in actions.iter().filter(|a| a.toolbar.is_some()) {
                if let Some(u) = toolbar_control(ui, action) {
                    used = Some((action.clone(), u));
                }
            }
            if self.load == Load::Loading {
                ui.label(RichText::new(egui_phosphor::regular::HOURGLASS).color(colors().warn));
            }
        });
        // Where we are: plan, focus, depth; and whether the agent moved us.
        ui.horizontal_wrapped(|ui| {
            let nav = self.nav.current().clone();
            let focus = if nav.roots.is_empty() {
                "entry points".to_owned()
            } else {
                nav.roots.iter().map(|r| self.component(r).map_or_else(|| r.to_string(), |c| c.name.clone())).collect::<Vec<_>>().join(", ")
            };
            let depth = nav.depth.unwrap_or(self.base_depth());
            let shown = self.result.as_ref().map_or(0, |r| r.view.nodes.len());
            ui.label(RichText::new(format!("{} · from {focus} · depth {depth} · {shown} nodes", nav.plan.as_ref().map_or("code".to_owned(), |p| format!("plan {p}")))).size(11.0).color(colors().muted));
            if self.agent_moved {
                ui.label(RichText::new(format!("{} agent moved the view (Ctrl-o goes back)", egui_phosphor::regular::ROBOT)).size(11.0).color(colors().special));
            }
        });
        ui.separator();
        match used {
            Some((action, ToolbarUse::Run(value))) => self.run(&action, value, cx),
            Some((_, ToolbarUse::OpenPalette)) | None => {}
        }
    }

    fn draw_graph(&self, painter: &egui::Painter) {
        let Some(result) = &self.result else { return };
        let z = self.camera.zoom;
        let collapsed = self.collapsed();
        // Containers first, then edges, then nodes on top.
        for id in result.layout.containers.iter().filter(|id| self.shown(id)) {
            let Some(r) = result.layout.rects.get(id) else { continue };
            let rect = self.to_screen(r);
            let selected = self.selection.as_ref() == Some(id);
            painter.rect_filled(rect, 6.0, colors().surface.gamma_multiply(0.6));
            painter.rect_stroke(rect, 6.0, Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { colors().focus } else { colors().border_strong }), StrokeKind::Inside);
            let Some(c) = result.components.get(id) else { continue };
            if collapsed {
                // One block per container: its name, as large as fits, and how much is inside.
                let inner = result.layout.rects.iter().filter(|(_, k)| *k != r && r.contains(k)).count();
                let size = (rect.width() / (c.name.len().max(4) as f32 * 0.6)).min(rect.height() / 2.6).clamp(8.0, 20.0);
                let clip = painter.with_clip_rect(rect.expand(2.0));
                clip.text(rect.center() - Vec2::new(0.0, size * 0.4), Align2::CENTER_CENTER, &c.name, FontId::proportional(size), colors().text_strong);
                clip.text(rect.center() + Vec2::new(0.0, size * 0.7), Align2::CENTER_CENTER, format!("{} · {inner}", c.type_name), FontId::proportional((size * 0.7).max(8.0)), colors().muted);
            } else {
                let size = (12.0 * z).clamp(8.0, 16.0);
                painter.with_clip_rect(rect).text(rect.min + Vec2::new(10.0 * z, (arch_graph::view::layout::HEADER * z - size) / 2.0), Align2::LEFT_TOP, format!("{} {}", c.name, c.type_name), FontId::proportional(size), colors().muted);
            }
        }
        for edge in &result.view.edges {
            if !self.shown(&edge.source) || !self.shown(&edge.target) {
                continue;
            }
            let (Some(a), Some(b)) = (result.layout.rects.get(&edge.source), result.layout.rects.get(&edge.target)) else { continue };
            if edge.kind == RelationKind::Contains && result.layout.containers.contains(&edge.source) {
                continue; // drawn as nesting
            }
            let (ra, rb) = (self.to_screen(a), self.to_screen(b));
            let line = style().edge(&colors(), edge.kind, provenance_of(edge.alignment), edge.alignment, edge.overlay);
            let (color, stroke) = (line.color, Stroke::new(line.width, line.color));
            let (from, to) = if rb.left() > ra.right() { (ra.right_center(), rb.left_center()) } else { (ra.center_bottom(), rb.center_top()) };
            let mid = (to.x - from.x).abs().max(40.0) * 0.5;
            let (c1, c2) = if rb.left() > ra.right() { (from + Vec2::new(mid, 0.0), to - Vec2::new(mid, 0.0)) } else { (from + Vec2::new(0.0, mid), to - Vec2::new(0.0, mid)) };
            let points: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let t = i as f32 / 16.0;
                    let u = 1.0 - t;
                    Pos2::new(
                        u * u * u * from.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * to.x,
                        u * u * u * from.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * to.y,
                    )
                })
                .collect();
            if line.dashed {
                painter.extend(Shape::dashed_line(&points, stroke, 6.0, 4.0));
            } else {
                painter.add(Shape::line(points.clone(), stroke));
            }
            // Arrow head.
            let dir = (to - points[14]).normalized();
            let side = Vec2::new(-dir.y, dir.x) * 4.0;
            painter.add(Shape::convex_polygon(vec![to, to - dir * 8.0 + side, to - dir * 8.0 - side], color, Stroke::NONE));
        }
        for node in &result.view.nodes {
            if result.layout.containers.contains(&node.id) || !self.shown(&node.id) {
                continue;
            }
            let (Some(r), Some(c)) = (result.layout.rects.get(&node.id), result.components.get(&node.id)) else { continue };
            self.draw_node(painter, self.to_screen(r), c, node.alignment, node.truncated);
        }
        if let Some(hint) = &self.hint {
            for (label, id) in &hint.labels {
                if !label.starts_with(&hint.typed) {
                    continue;
                }
                let Some(r) = result.layout.rects.get(id) else { continue };
                let at = self.to_screen(r).left_top() + Vec2::new(-4.0, -4.0);
                let galley = painter.layout_no_wrap(label.to_uppercase(), FontId::monospace(13.0), colors().on_bright);
                let chip = Rect::from_min_size(at, galley.size() + Vec2::new(8.0, 4.0));
                painter.rect_filled(chip, 3.0, colors().key);
                painter.galley(chip.min + Vec2::new(4.0, 2.0), galley, colors().on_bright);
            }
        }
    }

    fn draw_node(&self, painter: &egui::Painter, rect: Rect, c: &Component, alignment: Alignment, truncated: bool) {
        let z = self.camera.zoom;
        let selected = self.selection.as_ref() == Some(&c.id);
        let (theme, style) = (colors(), style());
        let border = style.node_outline(&theme, alignment, false).color;
        let outline = style.node_outline(&theme, alignment, selected);
        painter.rect_filled(rect, 6.0, style.node_fill(&theme, selected));
        let stroke = Stroke::new(outline.width, outline.color);
        if outline.dashed {
            let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
            painter.extend(Shape::dashed_line(&corners, stroke, 6.0, 4.0));
        } else {
            painter.rect_stroke(rect, 6.0, stroke, StrokeKind::Inside);
        }
        // Too small to read: the card's color and outline carry it.
        if rect.height() < 9.0 {
            return;
        }
        let clip = painter.with_clip_rect(rect.shrink(2.0));
        let size = (13.0 * z).clamp(6.0, 18.0).min(rect.height() * 0.45);
        clip.text(rect.min + Vec2::new(10.0, 8.0) * z, Align2::LEFT_TOP, &c.name, FontId::proportional(size), colors().text_strong);
        if z > 0.45 {
            let small = (10.0 * z).clamp(7.0, 13.0);
            let stage = format!("{} · {:?}{}", c.type_name, c.stage, if c.placeholders.is_empty() { String::new() } else { format!(" · {} placeholders", c.placeholders.len()) });
            clip.text(rect.min + Vec2::new(10.0, 28.0) * z, Align2::LEFT_TOP, stage, FontId::proportional(small), border);
            let at = match (&c.anchor, &c.proposed) {
                (Some(a), _) => format!("{}:{}", a.file, a.span.start_line),
                (None, Some(p)) => format!("→ {}", p.file),
                (None, None) => "unplaced".to_owned(),
            };
            clip.text(rect.min + Vec2::new(10.0, 44.0) * z, Align2::LEFT_TOP, at, FontId::monospace(small * 0.95), colors().muted);
        }
        if truncated {
            painter.text(rect.right_center() + Vec2::new(4.0, 0.0), Align2::LEFT_CENTER, "…", FontId::proportional(14.0 * z.max(0.6)), colors().muted);
        }
    }

    /// The compact inspector: a navigable summary of the selection.
    fn render_drawer(&mut self, ui: &mut egui::Ui) {
        let Some(c) = self.selected().cloned() else { return };
        let alignment = self.result.as_ref().and_then(|r| r.view.node(&c.id)).map(|n| n.alignment);
        egui::Frame::NONE.fill(colors().surface).stroke(Stroke::new(1.0, colors().border_strong)).corner_radius(6).inner_margin(8).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&c.name).strong().size(14.0).color(colors().text_strong));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(egui_phosphor::regular::X).on_hover_text("Close (x)").clicked() {
                            self.drawer_open = false;
                        }
                    });
                });
                ui.label(RichText::new(c.id.as_str()).monospace().size(10.0).color(colors().muted));
                ui.label(RichText::new(format!("{} · {:?} · {:?}{}", c.type_name, c.status, c.stage, alignment.map(|a| format!(" · {a:?}")).unwrap_or_default())).size(11.0));
                match (&c.anchor, &c.proposed) {
                    (Some(a), _) => ui.label(RichText::new(format!("{} {}:{}–{}", egui_phosphor::regular::FILE_CODE, a.file, a.span.start_line, a.span.end_line)).monospace().size(11.0)),
                    (None, Some(p)) => ui.label(RichText::new(format!("{} planned in {}", egui_phosphor::regular::FILE_DASHED, p.file)).monospace().size(11.0).color(colors().warn)),
                    (None, None) => ui.label(RichText::new("not placed yet").size(11.0).color(colors().warn)),
                };
                if !c.description.is_empty() {
                    ui.label(RichText::new(&c.description).size(11.0));
                }
                let spec = c.spec.as_ref();
                ui.label(
                    RichText::new(format!(
                        "{} logic steps · {} invariants · {} placeholders",
                        spec.map_or(0, |s| s.logic_steps.len()),
                        spec.map_or(0, |s| s.invariants.len()),
                        c.placeholders.len()
                    ))
                    .size(11.0)
                    .color(colors().muted),
                );
                ui.horizontal(|ui| {
                    if ui.button(format!("{} Spec view", egui_phosphor::regular::ARTICLE)).on_hover_text("s").clicked() {
                        self.requests.open_spec = true;
                    }
                    if ui.button(format!("{} Code", egui_phosphor::regular::CODE)).on_hover_text("g d").clicked() {
                        self.request_code();
                    }
                });
                ui.separator();
                let mut go: Option<ComponentId> = None;
                if let Some(result) = &self.result {
                    for kind in RelationKind::ALL {
                        let out: Vec<&ComponentId> = result.view.edges.iter().filter(|e| e.kind == kind && e.source == c.id).map(|e| &e.target).collect();
                        let inc: Vec<&ComponentId> = result.view.edges.iter().filter(|e| e.kind == kind && e.target == c.id).map(|e| &e.source).collect();
                        for (label, list) in [(format!("{} →", kind.name()), out), (format!("← {}", kind.name()), inc)] {
                            if list.is_empty() {
                                continue;
                            }
                            ui.label(RichText::new(label).size(10.0).color(colors().muted));
                            for id in list.into_iter().take(8) {
                                let name = result.components.get(id).map_or(id.to_string(), |x| x.name.clone());
                                if ui.link(RichText::new(name).size(11.0)).clicked() {
                                    go = Some(id.clone());
                                }
                            }
                        }
                    }
                }
                if let Some(id) = go {
                    self.select(Some(id));
                }
            });
        });
    }
}

fn push(inbox: &Arc<Mutex<Vec<Reply>>>, reply: Reply) {
    if let Ok(mut i) = inbox.lock() {
        i.push(reply);
    }
}

#[derive(Clone, Debug, PartialEq)]
enum GraphCommand {
    Move(Dir),
    Follow { kind: RelationKind, outgoing: bool },
    Focus,
    Unfocus,
    Depth(i32),
    SetView,
    NextView,
    SetPlan,
    Back,
    Forward,
    OpenCode,
    OpenSpec,
    Hint(HintAction),
    Zoom(f32),
    Fit,
    CloseInspector,
    Refresh,
}

impl ActionProvider for GraphState {
    fn actions(&self) -> Vec<Action> {
        use GraphCommand as G;
        use ToolbarStyle::{Button, Control};
        let mut out = Vec::new();
        let f = |id: &str, label: &str, c: GraphCommand| Action::new(id, label, c).focused();
        if let Some(catalog) = &self.catalog {
            let current = &self.nav.current().view;
            let choices = catalog.views.iter().map(|(name, v)| Choice::new(name, name).detail(Some(v.description.clone())).current(name == current)).collect();
            out.push(f("graph.view", "Switch view", G::SetView).param(Param::Choice(choices)).toolbar(Control, "View", Some(egui_phosphor::regular::GRAPH)));
            out.push(f("graph.next_view", "Next view", G::NextView));
        }
        let plan = self.nav.current().plan.clone();
        let mut plans = vec![Choice::new("", "code only").current(plan.is_none())];
        plans.extend(self.plans.iter().map(|p| Choice::new(&p.0, &p.0).current(plan.as_ref() == Some(p))));
        out.push(f("graph.plan", "Lay a plan over the code", G::SetPlan).param(Param::Choice(plans)).toolbar(Control, "Plan", None));
        out.push(f("graph.back", "Back", G::Back).toolbar(Button, "", Some(egui_phosphor::regular::ARROW_LEFT)));
        out.push(f("graph.forward", "Forward", G::Forward).toolbar(Button, "", Some(egui_phosphor::regular::ARROW_RIGHT)));
        out.push(f("graph.depth_less", "Show fewer hops", G::Depth(-1)).toolbar(Button, "", Some(egui_phosphor::regular::MINUS)));
        out.push(f("graph.depth_more", "Show more hops", G::Depth(1)).toolbar(Button, "", Some(egui_phosphor::regular::PLUS)));
        if !self.nav.current().roots.is_empty() {
            out.push(f("graph.unfocus", "Back to the view's entry points", G::Unfocus).toolbar(Button, "All", Some(egui_phosphor::regular::ARROWS_OUT)));
        }
        out.push(f("graph.fit", "Fit to screen", G::Fit).toolbar(Button, "", Some(egui_phosphor::regular::CORNERS_OUT)));
        out.push(f("graph.zoom_in", "Zoom in", G::Zoom(1.2)));
        out.push(f("graph.zoom_out", "Zoom out", G::Zoom(1.0 / 1.2)));
        out.push(f("graph.refresh", "Refresh the view", G::Refresh));
        for (id, label, dir) in [("graph.left", "Select left", Dir::Left), ("graph.down", "Select below", Dir::Down), ("graph.up", "Select above", Dir::Up), ("graph.right", "Select right", Dir::Right)] {
            out.push(f(id, label, G::Move(dir)));
        }
        out.push(f("graph.hint", "Pick a node by hint", G::Hint(HintAction::Select)));
        out.push(f("graph.hint_focus", "Focus a node by hint", G::Hint(HintAction::Focus)));
        out.push(f("graph.hint_code", "Open a node's code by hint", G::Hint(HintAction::OpenCode)));
        out.push(f("graph.hint_spec", "Open a node's spec by hint", G::Hint(HintAction::OpenSpec)));
        if self.selection.is_some() {
            out.push(f("graph.focus", "Focus on the selection", G::Focus));
            out.push(f("graph.open_code", "Open in code", G::OpenCode));
            out.push(f("graph.open_spec", "Open the spec view", G::OpenSpec));
            out.push(f("graph.close_inspector", "Close the inspector", G::CloseInspector));
            for (id, label, kind, outgoing) in [
                ("graph.callees", "Go to a callee", RelationKind::Calls, true),
                ("graph.callers", "Go to a caller", RelationKind::Calls, false),
                ("graph.parent", "Go to the container", RelationKind::Contains, false),
                ("graph.children", "Go to a member", RelationKind::Contains, true),
                ("graph.implements", "Go to what it implements", RelationKind::Implements, true),
                ("graph.flows_to", "Follow data flow", RelationKind::DataFlow, true),
            ] {
                let targets = self.neighbors(kind, outgoing);
                if targets.is_empty() {
                    continue;
                }
                let param = if targets.len() == 1 {
                    Param::None
                } else {
                    Param::Choice(targets.iter().map(|t| Choice::new(t.as_str(), self.component(t).map_or(t.to_string(), |c| c.name.clone())).detail(Some(t.to_string()))).collect())
                };
                out.push(f(id, label, G::Follow { kind, outgoing }).param(param));
            }
        }
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        let Some(command) = action.command::<GraphCommand>().cloned() else { return };
        let base = self.base_depth();
        match command {
            GraphCommand::Move(dir) => self.move_selection(dir),
            GraphCommand::Follow { kind, outgoing } => {
                let targets = self.neighbors(kind, outgoing);
                let target = match value {
                    Some(v) => targets.into_iter().find(|t| t.as_str() == v),
                    None => targets.into_iter().next(),
                };
                if let Some(t) = target {
                    self.select(Some(t));
                }
            }
            GraphCommand::Focus => {
                if let Some(id) = self.selection.clone() {
                    self.navigate(NavChange::Focus(id));
                }
            }
            GraphCommand::Unfocus => self.navigate(NavChange::Unfocus),
            GraphCommand::Depth(delta) => self.navigate(NavChange::Depth { delta, base }),
            GraphCommand::SetView => {
                if let Some(v) = value {
                    self.navigate(NavChange::View(v));
                }
            }
            GraphCommand::NextView => {
                if let Some(catalog) = &self.catalog {
                    let names: Vec<&String> = catalog.views.keys().collect();
                    let at = names.iter().position(|n| **n == self.nav.current().view).unwrap_or(0);
                    let next = names[(at + 1) % names.len()].clone();
                    self.navigate(NavChange::View(next));
                }
            }
            GraphCommand::SetPlan => {
                let plan = value.filter(|v| !v.is_empty()).and_then(|v| PlanId::new(&v).ok());
                self.navigate(NavChange::Plan(plan));
            }
            GraphCommand::Back => self.navigate(NavChange::Back),
            GraphCommand::Forward => self.navigate(NavChange::Forward),
            GraphCommand::OpenCode => self.request_code(),
            GraphCommand::OpenSpec => self.requests.open_spec = true,
            GraphCommand::Hint(action) => {
                if self.start_hint(action, cx.hint_chars) {
                    *cx.input_mode = InputMode::Capture { slot: cx.slot };
                }
            }
            GraphCommand::Zoom(factor) => {
                let center = self.canvas.center();
                self.zoom_by(factor, center);
            }
            GraphCommand::Fit => self.fit_view(),
            GraphCommand::CloseInspector => self.drawer_open = false,
            GraphCommand::Refresh => {
                self.stale = true;
                self.plans_stale = true;
            }
        }
    }

    fn on_capture(&mut self, typed: &[char], back: bool, _cx: &mut RunCx<'_>) -> bool {
        self.hint_input(typed, back)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use arch_graph::view::{Layout, ViewEdge, ViewNode};

    /// A graph waiting for its first view (nothing in flight, nothing shown).
    pub(crate) fn loading() -> GraphState {
        GraphState { catalog: Some(ViewCatalog::defaults().unwrap()), catalog_requested: true, plans_stale: false, stale: false, load: Load::Loading, ..Default::default() }
    }

    /// A graph already showing `result` (catalog loaded, nothing to fetch).
    pub(crate) fn showing(result: ViewResult) -> GraphState {
        GraphState { catalog: Some(ViewCatalog::defaults().unwrap()), catalog_requested: true, plans_stale: false, stale: false, result: Some(result), ..Default::default() }
    }
    use arch_graph::{Stage, Status, View, ViewSpec};

    pub(crate) fn sample_result() -> ViewResult {
        fn id(s: &str) -> ComponentId {
            ComponentId::new(s)
        }
        let ids = ["rust:a::run", "rust:a::step", "rust:a::save", "rust:a::log"];
        let mut layout = Layout::default();
        for (i, (x, y)) in [(0.0, 0.0), (300.0, 0.0), (600.0, 0.0), (300.0, 100.0)].into_iter().enumerate() {
            layout.rects.insert(id(ids[i]), WorldRect { x, y, w: 220.0, h: 64.0 });
        }
        let component = |s: &str| Component {
            id: id(s),
            name: s.rsplit("::").next().unwrap().into(),
            type_name: "function".into(),
            description: String::new(),
            status: Status::Existing,
            stage: Stage::Implemented,
            anchor: None,
            proposed: None,
            signature: None,
            schemas: Default::default(),
            spec: None,
            placeholders: vec![],
        };
        let edge = |a: &str, b: &str| ViewEdge { id: arch_graph::RelationId(format!("calls:{a}->{b}")), kind: RelationKind::Calls, source: id(a), target: id(b), alignment: Alignment::Observed, overlay: false };
        ViewResult {
            view: View {
                spec: ViewSpec { name: "calls".into(), description: String::new(), kinds: vec![RelationKind::Calls], direction: Default::default(), depth: 3, layout: Default::default(), overlays: vec![], include_ancestors: false, roots: vec![], filters: Default::default() },
                roots: vec![id("rust:a::run")],
                nodes: ids.iter().map(|s| ViewNode { id: id(s), depth: Some(0), truncated: false, alignment: Alignment::Observed }).collect(),
                edges: vec![edge("rust:a::run", "rust:a::step"), edge("rust:a::step", "rust:a::save"), edge("rust:a::run", "rust:a::log")],
            },
            layout,
            text: String::new(),
            components: ids.iter().map(|s| (id(s), component(s))).collect(),
        }
    }

    // req: P9.3
    #[test]
    fn navigator_is_one_reducer_with_back_and_forward() {
        let mut nav = Navigator::new("structure");
        assert!(nav.apply(NavChange::View("calls".into())));
        assert!(nav.apply(NavChange::Focus(ComponentId::new("rust:a::run"))));
        assert!(nav.apply(NavChange::Depth { delta: 1, base: 3 }));
        assert_eq!(nav.current().depth, Some(4));
        assert!(!nav.apply(NavChange::Focus(ComponentId::new("rust:a::run"))), "no-op changes add no history");
        assert!(nav.apply(NavChange::Back));
        assert_eq!(nav.current().depth, None);
        assert!(nav.apply(NavChange::Back));
        assert!(nav.current().roots.is_empty());
        assert!(nav.apply(NavChange::Forward));
        assert_eq!(nav.current().roots.len(), 1);
        // A new step after going back drops the forward history.
        assert!(nav.apply(NavChange::Plan(Some(PlanId::new("p").unwrap()))));
        assert!(!nav.can_forward());
        // Agent requests go through the same reducer.
        assert!(nav.apply(NavChange::Request(Box::new(ViewRequest { view: Some("structure".into()), roots: vec![ComponentId::new("rust:b")], ..Default::default() }))));
        assert_eq!((nav.current().view.as_str(), nav.current().roots.len(), nav.current().plan.is_some()), ("structure", 1, true));
        assert!(nav.apply(NavChange::Depth { delta: -50, base: 3 }));
        assert_eq!(nav.current().depth, Some(1), "clamped");
    }

    // req: P9.13
    #[test]
    fn hint_labels_are_unique_minimal_and_drive_actions() {
        let home: Vec<char> = "asdfghjkl".chars().collect();
        assert_eq!(hint_labels(3, &home), ["a", "s", "d"]);
        let many = hint_labels(20, &home);
        assert!(many.iter().all(|l| l.len() == 2), "9 < 20 <= 81");
        let unique: std::collections::BTreeSet<_> = many.iter().collect();
        assert_eq!(unique.len(), 20);
        assert!(many.iter().all(|a| many.iter().filter(|b| b.starts_with(a.as_str())).count() == 1), "prefix-free");
        assert!(hint_labels(0, &home).is_empty());

        let mut g = GraphState { result: Some(sample_result()), canvas: Rect::from_min_size(Pos2::ZERO, Vec2::new(2000.0, 1000.0)), ..Default::default() };
        g.select(Some(ComponentId::new("rust:a::run")));
        assert!(g.start_hint(HintAction::Focus, "asdfghjkl"));
        let labels = g.hint.as_ref().unwrap().labels.clone();
        assert_eq!(labels.len(), 4);
        assert_eq!(labels[0].1, ComponentId::new("rust:a::run"), "nearest the selection gets the first label");
        let (label, target) = labels.iter().find(|(_, id)| id.as_str() == "rust:a::save").unwrap().clone();
        let chars: Vec<char> = label.chars().collect();
        assert!(!g.hint_input(&chars, false), "done");
        assert_eq!(g.selection, Some(target.clone()));
        assert_eq!(g.nav.current().roots, vec![target], "focus hint refocused the view");
        assert!(g.start_hint(HintAction::Select, "asdfghjkl"));
        assert!(!g.hint_input(&['z'], false), "no label starts with z: cancelled");
        assert!(g.hint.is_none());
    }

    // req: P9.4
    #[test]
    fn graph_commands_are_typed_actions_that_move_follow_focus_and_open() {
        let mut g = GraphState { result: Some(sample_result()), ..Default::default() };
        let run = |g: &mut GraphState, id: &str, value: Option<&str>| {
            let action = g.actions().into_iter().find(|a| a.id == id).unwrap_or_else(|| panic!("{id} offered"));
            let net = NetClient::new("http://127.0.0.1:9");
            let ctx = egui::Context::default();
            let mut mode = InputMode::Normal;
            g.run(&action, value.map(str::to_owned), &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, hint_chars: "asdf" });
        };
        assert!(!g.actions().iter().any(|a| a.id == "graph.callees"), "nothing selected yet");
        run(&mut g, "graph.right", None);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::run", "first move selects a root");
        run(&mut g, "graph.right", None);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::step");
        run(&mut g, "graph.down", None);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::log");
        run(&mut g, "graph.callers", None);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::run");
        let callees = g.actions().into_iter().find(|a| a.id == "graph.callees").unwrap();
        assert!(matches!(callees.param, Param::Choice(ref c) if c.len() == 2), "two callees: a choice");
        run(&mut g, "graph.callees", Some("rust:a::log"));
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::log");
        run(&mut g, "graph.focus", None);
        assert_eq!(g.nav.current().roots[0].as_str(), "rust:a::log");
        run(&mut g, "graph.back", None);
        assert!(g.nav.current().roots.is_empty());
        run(&mut g, "graph.depth_more", None);
        assert_eq!(g.nav.current().depth, Some(4));
        run(&mut g, "graph.open_spec", None);
        assert!(g.requests.open_spec);
        let mut with_anchor = sample_result();
        with_anchor.components.get_mut(&ComponentId::new("rust:a::log")).unwrap().anchor = Some(arch_graph::CodeAnchor {
            language: "rust".into(),
            file: arch_graph::RepoPath::new("src/a.rs").unwrap(),
            symbol: "a::log".into(),
            syntax_kind: "function_item".into(),
            span: arch_graph::Span { start_line: 7, end_line: 9, start_byte: 0, end_byte: 1 },
            content_hash: String::new(),
        });
        g.set_result(with_anchor);
        run(&mut g, "graph.open_code", None);
        assert_eq!(g.requests.open_code, Some(("src/a.rs".into(), 7, 9)));
        run(&mut g, "graph.plan", Some("p1"));
        assert_eq!(g.nav.current().plan.as_ref().unwrap().0, "p1");
        run(&mut g, "graph.plan", Some(""));
        assert!(g.nav.current().plan.is_none(), "code only");
    }

    // req: P9.2
    #[test]
    fn architecture_events_refresh_and_keep_selection() {
        let mut g = GraphState { result: Some(sample_result()), stale: false, catalog: Some(ViewCatalog::defaults().unwrap()), ..Default::default() };
        g.select(Some(ComponentId::new("rust:a::step")));
        let camera = g.camera;
        g.on_event(&ArchEvent::Changed { reason: ChangeReason::Ingested, plans: vec![], components: vec![] });
        assert!(g.stale, "refetch requested");
        g.set_result(sample_result());
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::step", "selection kept");
        assert_eq!(g.camera, camera, "camera kept on refresh");
        let mut gone = sample_result();
        gone.components.remove(&ComponentId::new("rust:a::step"));
        g.set_result(gone);
        assert!(g.selection.is_none(), "selection dropped when the node is gone");
        g.stale = false;
        g.on_event(&ArchEvent::ViewRequested { request: Box::new(ViewRequest { view: Some("calls".into()), focus: Some(ComponentId::new("rust:a::save")), ..Default::default() }), by: Requester::Agent });
        assert!(g.agent_moved && g.stale);
        g.set_result(sample_result());
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::save", "agent focus selected once shown");
        g.navigate(NavChange::Back);
        assert!(!g.agent_moved, "the user took over");
    }
}
