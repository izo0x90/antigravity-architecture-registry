//! The architecture graph: server views drawn with pan and zoom, one reducer
//! for where it looks (view, plan, focus, depth), modules folded in place,
//! touch gestures and on-screen chips, a legend, semantic zoom and a compact
//! inspector. The universal keys (moves, search, hints, marks, the jump list)
//! drive it through `Navigable`. Everything shown comes from the server's
//! views; the graph refreshes itself when the architecture changes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use arch_graph::view::Rect as WorldRect;
use arch_graph::{Alignment, ArchEvent, ChangeReason, Component, ComponentId, PlanId, RelationKind, Requester, ViewCatalog, ViewRequest, ViewResult};
use egui::{Align2, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Vec2};

use crate::actions::{Action, ActionProvider, Choice, Group, MINOR, PRIMARY, Param, RunCx, SECONDARY, ToolbarStyle, ToolbarUse, toolbar_control};
use crate::feedback::FeedbackInspectorState;
use crate::graph_config::{ChipMode, GraphUi};
use crate::graph_style::{LegendEntry, Swatch, provenance_of, style};
use crate::nav::{Fold, Item, Motion, Navigable, Place, Spot, Step, Target};
use crate::net::{NetClient, NetResult};
use crate::theme::colors;
use crate::touch::{Gesture, Gestures, Swipe};

/// Where the graph is looking: which view, laid over which plan, rooted where,
/// how deep. Two equal `Nav`s fetch the same view.
#[derive(Clone, Debug, PartialEq)]
pub struct Nav {
    pub view: String,
    pub plan: Option<PlanId>,
    pub roots: Vec<ComponentId>,
    /// None means the view's own depth.
    pub depth: Option<u32>,
}

impl Nav {
    pub fn new(view: &str) -> Self {
        Self { view: view.to_owned(), plan: None, roots: Vec::new(), depth: None }
    }
}

/// Every change to where the graph looks and what it has selected, from
/// keys, touch, the palette, the jump list and the agent alike.
#[derive(Clone, Debug, PartialEq)]
pub enum NavChange {
    View(String),
    Plan(Option<PlanId>),
    Focus(ComponentId),
    Unfocus,
    /// More (+) or fewer (-) hops; `base` is the view's own depth.
    Depth { delta: i32, base: u32 },
    Request(Box<ViewRequest>),
    /// Go to a node in the same view.
    Jump(ComponentId),
    /// Return to a recorded place (the jump list, a mark, where the agent
    /// took over).
    Restore { nav: Box<Nav>, selection: Option<ComponentId> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Down,
    Up,
    Right,
}

/// The agent moved the view: what it showed, and where the user was.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentMove {
    pub what: String,
    pub focus: Option<ComponentId>,
    pub return_to: Nav,
    pub return_selection: Option<ComponentId>,
}

/// How much a node card shows (semantic zoom).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Detail {
    Far,
    Mid,
    Near,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineRole {
    Name,
    Meta,
    Signature,
    Location,
    Badges,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CardLine {
    pub role: LineRole,
    pub text: String,
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
    /// Run this action (by id, with its value) the way keys do: from a chip
    /// or the toolbar, for actions that are jumps or belong to the shell.
    pub run: Option<(String, Option<String>)>,
    /// Read the code that changed again.
    pub reindex: bool,
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

/// How much of a wide graph slot the inspector takes, beside the canvas.
const DRAWER_SHARE: f32 = 0.3;
/// How much of a narrow canvas the inspector sheet covers, from the bottom.
const SHEET_SHARE: f32 = 0.32;

pub struct GraphState {
    catalog: Option<ViewCatalog>,
    catalog_requested: bool,
    plans: Vec<PlanId>,
    plans_stale: bool,
    pub nav: Nav,
    pub result: Option<ViewResult>,
    /// The containers each node is drawn inside, outermost first.
    ancestors: BTreeMap<ComponentId, Vec<ComponentId>>,
    /// The one container everything else sits in (a focused module): it
    /// stays open when zoomed out, so its parts show as blocks.
    frame: Option<ComponentId>,
    /// Modules opened (true) or closed (false) by hand; the rest follow the
    /// zoom (closed when zoomed out too far to read them).
    folds: BTreeMap<ComponentId, bool>,
    /// Stepping through a caller's calls in order (`] e`): the caller and
    /// which call the selection is on.
    call_walk: Option<(ComponentId, usize)>,
    /// Where the graph was before a jump made with the pointer or a finger
    /// (the shell records it in the jump list).
    pub jumped: Option<Place>,
    /// What holds the selection, innermost first: where the selection goes
    /// when a new view doesn't have it.
    holders: Vec<ComponentId>,
    /// A selection the view on screen doesn't have (it comes back with a
    /// view that does).
    remembered: Option<ComponentId>,
    /// Files changed since they were indexed (their cards are marked).
    pub stale_files: Vec<String>,
    /// The camera and folds before a search started previewing matches.
    saved_view: Option<(Camera, BTreeMap<ComponentId, bool>, bool)>,
    /// Review badges: changed since the last review, unaddressed comments.
    pub marks: BTreeMap<ComponentId, crate::review::NodeMark>,
    load: Load,
    stale: bool,
    generation: u64,
    pub selection: Option<ComponentId>,
    /// Select this once it shows up (agent focus, reveal from code).
    pending_select: Option<ComponentId>,
    camera: Camera,
    fit: bool,
    /// Fit once the next result (of a view just navigated to) arrives: the
    /// one on screen now is the old view.
    refit: bool,
    /// Bring this node into view once the frame's camera is settled (after a
    /// fit), opening collapsed containers if it's inside one.
    reveal_after_fit: Option<ComponentId>,
    pub drawer_open: bool,
    /// The agent moved the view (shown until the user navigates).
    pub agent: Option<AgentMove>,
    pub requests: GraphRequests,
    canvas: Rect,
    /// The part of the canvas the inspector doesn't cover (fits and reveals
    /// aim here).
    open_area: Rect,
    inbox: Arc<Mutex<Vec<Reply>>>,
    /// Behavior settings (touch, chips, semantic zoom), from the UI config.
    pub ui: GraphUi,
    gestures: Gestures,
    /// A finger has touched the graph (chips show from then on in auto mode).
    touch_seen: bool,
    /// Long-pressed node and where: its actions show as chips there.
    pub context: Option<(ComponentId, Pos2)>,
    /// On-screen chips last drawn: action id and where.
    pub(crate) chip_rects: Vec<(String, Rect)>,
    pub legend_open: bool,
}

impl Default for GraphState {
    fn default() -> Self {
        Self {
            catalog: None,
            catalog_requested: false,
            plans: Vec::new(),
            plans_stale: true,
            nav: Nav::new("structure"),
            result: None,
            ancestors: BTreeMap::new(),
            frame: None,
            folds: BTreeMap::new(),
            call_walk: None,
            jumped: None,
            saved_view: None,
            holders: Vec::new(),
            remembered: None,
            stale_files: Vec::new(),
            marks: BTreeMap::new(),
            load: Load::Idle,
            stale: true,
            generation: 0,
            selection: None,
            pending_select: None,
            camera: Camera::default(),
            fit: true,
            refit: false,
            reveal_after_fit: None,
            drawer_open: true,
            agent: None,
            requests: GraphRequests::default(),
            canvas: Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)),
            open_area: Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)),
            inbox: Arc::default(),
            ui: GraphUi::default(),
            gestures: Gestures::default(),
            touch_seen: false,
            context: None,
            chip_rects: Vec::new(),
            legend_open: false,
        }
    }
}

impl GraphState {
    fn base_depth(&self) -> u32 {
        self.catalog.as_ref().and_then(|c| c.get(&self.nav.view)).map_or(3, |v| v.depth)
    }

    pub fn component(&self, id: &ComponentId) -> Option<&Component> {
        self.result.as_ref().and_then(|r| r.components.get(id))
    }

    pub fn selected(&self) -> Option<&Component> {
        self.selection.as_ref().and_then(|id| self.component(id))
    }

    /// The one reducer: applies a change, fetches the view again when where
    /// it looks changed, and selects the node the change lands on (once it's
    /// shown).
    pub fn navigate(&mut self, change: NavChange) {
        let user = !matches!(change, NavChange::Request(_));
        let before = self.nav.clone();
        let mut land: Option<ComponentId> = None;
        // A new place to look: an old wait for something to show up is over.
        if user {
            self.pending_select = None;
        }
        match change {
            // The selection comes along when the new view has it.
            NavChange::View(view) => {
                self.nav.view = view;
                land = self.current().cloned();
            }
            NavChange::Plan(plan) => {
                self.nav.plan = plan;
                land = self.current().cloned();
            }
            NavChange::Focus(id) => {
                self.nav.roots = vec![id.clone()];
                land = Some(id);
            }
            // The selection stays where the view goes back out to.
            NavChange::Unfocus => {
                self.nav.roots.clear();
                land = self.selection.clone();
                self.holders = self.holders_of_selection();
            }
            NavChange::Depth { delta, base } => {
                let current = self.nav.depth.unwrap_or(base) as i32;
                self.nav.depth = Some((current + delta).clamp(1, 12) as u32);
            }
            NavChange::Request(request) => {
                let request = *request;
                if let Some(view) = request.view {
                    self.nav.view = view;
                }
                if request.plan.is_some() {
                    self.nav.plan = request.plan;
                }
                if !request.roots.is_empty() {
                    self.nav.roots = request.roots;
                }
                if request.depth.is_some() {
                    self.nav.depth = request.depth;
                }
                land = request.focus;
            }
            NavChange::Jump(id) => land = Some(id),
            NavChange::Restore { nav, selection } => {
                self.nav = *nav;
                // Back to nothing selected means nothing selected.
                if selection.is_none() {
                    self.selection = None;
                    self.pending_select = None;
                }
                land = selection;
            }
        }
        let refetch = self.nav != before;
        if refetch {
            self.stale = true;
            self.refit = true;
        }
        if let Some(at) = land {
            if !refetch && self.component(&at).is_some() {
                self.select(Some(at.clone()));
                self.bring_into_view(&at);
            } else {
                self.pending_select = Some(at);
            }
        }
        if user {
            self.agent = None;
        }
    }

    /// A jump made with the pointer or a finger: remembers where it left
    /// from for the jump list, then navigates.
    fn pointer_jump(&mut self, change: NavChange) {
        if self.jumped.is_none() {
            self.jumped = Some(self.place());
        }
        self.navigate(change);
    }

    /// The least zoom at which containers open (their contents are drawn).
    fn open_zoom() -> f32 {
        11.0 / arch_graph::view::layout::HEADER * 1.05
    }

    /// Makes `id` visible: opens modules closed by hand around it, zooms in
    /// when it's inside one closed by the zoom, and pans so it's on screen
    /// (centered) when it isn't.
    fn bring_into_view(&mut self, id: &ComponentId) {
        let Some(r) = self.result.as_ref().and_then(|r| r.layout.rects.get(id)).copied() else { return };
        for outer in self.ancestors.get(id).cloned().unwrap_or_default() {
            if self.folds.get(&outer) == Some(&false) {
                self.folds.remove(&outer);
            }
        }
        if !self.shown(id) {
            // Inside a module closed by the zoom: bring the module that holds
            // it up to fill the view (the rest of the graph stays as it was,
            // off to the sides), and the node with it.
            let holder = self.container_of(id).and_then(|c| self.result.as_ref()?.layout.rects.get(c)).copied();
            if let Some(holder) = holder {
                self.fit_rects(&[holder]);
            }
            if !self.shown(id) {
                self.camera.zoom = Self::open_zoom().max(self.camera.zoom);
            }
            let center = Vec2::new(r.x + r.w / 2.0, r.y + r.h / 2.0) * self.camera.zoom;
            let rect = self.to_screen(&r);
            if !self.open_area.contains_rect(rect) {
                self.camera.pan = (self.open_area.center() - self.canvas.min) - center;
            }
            return;
        }
        let rect = self.to_screen(&r);
        if !self.open_area.contains_rect(rect) {
            self.camera.pan += self.open_area.center() - rect.center();
        }
    }

    pub fn select(&mut self, id: Option<ComponentId>) {
        if id.is_some() {
            self.drawer_open = true;
        }
        self.selection = id;
        self.remembered = None;
    }

    /// What the user has selected: the selection, or one the view on
    /// screen doesn't have (the spec keeps showing it; it comes back when a
    /// view has it).
    pub fn current(&self) -> Option<&ComponentId> {
        self.selection.as_ref().or(self.remembered.as_ref())
    }

    /// Selects `id`, refocusing the view on it when it isn't shown.
    pub fn reveal(&mut self, id: ComponentId) {
        if self.component(&id).is_some() {
            self.select(Some(id.clone()));
            self.bring_into_view(&id);
            // Again once the frame is laid out (the inspector it opens takes room).
            self.reveal_after_fit = Some(id);
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
            // A check result changes no shape on screen (and refetching on it
            // would echo the spec view's own validation forever).
            ArchEvent::Validated { .. } => {}
            // Stages show on nodes; the queue panel follows the rest.
            ArchEvent::QueueUpdated { .. } => {}
            // The shell keeps the index status (and hands the graph its stale files).
            ArchEvent::Index { .. } => {}
            ArchEvent::ViewRequested { request, by } => {
                if *by == Requester::Agent {
                    let (return_to, return_selection) = match &self.agent {
                        Some(a) => (a.return_to.clone(), a.return_selection.clone()),
                        None => (self.nav.clone(), self.selection.clone()),
                    };
                    self.agent = Some(AgentMove { what: self.describe(request), focus: request.focus.clone(), return_to, return_selection });
                }
                self.pending_select = request.focus.clone();
                self.navigate(NavChange::Request(request.clone()));
                if let Some(focus) = &request.focus
                    && self.component(focus).is_some()
                {
                    self.select(Some(focus.clone()));
                    self.reveal_after_fit = Some(focus.clone());
                }
            }
        }
    }

    /// What a view request shows, in words.
    fn describe(&self, request: &ViewRequest) -> String {
        let name = |id: &ComponentId| self.component(id).map_or_else(|| id.as_str().rsplit([':', '.']).next().unwrap_or(id.as_str()).to_owned(), |c| c.name.clone());
        let mut parts = vec![format!("{} view", request.view.as_deref().unwrap_or(&self.nav.view))];
        if let Some(plan) = &request.plan {
            parts.push(format!("plan {plan}"));
        }
        if !request.roots.is_empty() {
            parts.push(format!("from {}", request.roots.iter().map(name).collect::<Vec<_>>().join(", ")));
        }
        if let Some(focus) = &request.focus {
            parts.push(format!("at {}", name(focus)));
        }
        parts.join(" · ")
    }

    /// Fetch everything again (events may have been missed).
    pub fn invalidate(&mut self) {
        self.stale = true;
        self.plans_stale = true;
    }

    /// Applies a fetched view, keeping selection and camera when possible.
    pub fn set_result(&mut self, result: ViewResult) {
        if let Some(sel) = self.selection.clone()
            && !result.components.contains_key(&sel)
        {
            // Not in this view: what holds it (backing out of a focus), or
            // remembered until a view has it again.
            self.selection = self.holders.iter().find(|h| result.components.contains_key(*h)).cloned();
            if self.selection.is_none() {
                self.remembered = Some(sel.clone());
            }
            if self.pending_select.as_ref() == Some(&sel) {
                self.pending_select = self.selection.clone();
            }
        }
        self.holders.clear();
        if self.selection.is_none()
            && let Some(back) = self.remembered.take()
        {
            if result.components.contains_key(&back) {
                self.selection = Some(back);
            } else {
                self.remembered = Some(back);
            }
        }
        if let Some(pending) = self.pending_select.take() {
            if result.components.contains_key(&pending) {
                self.selection = Some(pending.clone());
                self.drawer_open = true;
                self.reveal_after_fit = Some(pending);
            } else {
                self.pending_select = Some(pending);
            }
        }
        let tops: Vec<&ComponentId> = result
            .layout
            .rects
            .iter()
            .filter(|(_, r)| !result.layout.containers.iter().filter_map(|c| result.layout.rects.get(c)).any(|outer| outer != *r && outer.contains(r)))
            .map(|(id, _)| id)
            .collect();
        self.frame = match tops.as_slice() {
            [only] if result.layout.containers.contains(*only) => Some((*only).clone()),
            _ => None,
        };
        let mut ancestors: BTreeMap<ComponentId, Vec<(f32, ComponentId)>> = BTreeMap::new();
        for c in &result.layout.containers {
            let Some(outer) = result.layout.rects.get(c) else { continue };
            for (id, r) in &result.layout.rects {
                if r != outer && outer.contains(r) {
                    ancestors.entry(id.clone()).or_default().push((outer.w * outer.h, c.clone()));
                }
            }
        }
        self.ancestors = ancestors
            .into_iter()
            .map(|(id, mut outer)| {
                outer.sort_by(|a, b| b.0.total_cmp(&a.0));
                (id, outer.into_iter().map(|(_, c)| c).collect())
            })
            .collect();
        self.folds.retain(|c, _| result.layout.containers.contains(c));
        self.result = Some(result);
        self.load = Load::Idle;
        if std::mem::take(&mut self.refit) {
            self.fit = true;
        }
    }

    /// Zoomed out past the point where container headers are legible: each
    /// top-level container is drawn as one labelled block.
    fn collapsed(&self) -> bool {
        arch_graph::view::layout::HEADER * self.camera.zoom < 11.0
    }

    /// Whether a module shows as one closed block: closed by hand, or by the
    /// zoom (all but the focused module).
    fn closed(&self, container: &ComponentId) -> bool {
        self.folds.get(container).map_or_else(|| self.collapsed() && Some(container) != self.frame.as_ref(), |open| !open)
    }

    /// Whether a node is drawn (no module around it is closed).
    fn shown(&self, id: &ComponentId) -> bool {
        self.ancestors.get(id).is_none_or(|outer| outer.iter().all(|c| !self.closed(c)))
    }

    /// Fetches what's missing or stale and applies replies. Call every frame.
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::Catalog(Ok(catalog)) => {
                    if catalog.get(&self.nav.view).is_none()
                        && let Some(first) = catalog.views.keys().next()
                    {
                        self.nav = Nav::new(first);
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
                        // The plan this step shows is gone (deleted): show the code instead.
                        Err(e) if self.nav.plan.as_ref().is_some_and(|p| !self.plans.contains(p)) && e.to_lowercase().contains("not found") => {
                            self.plans_stale = true;
                            self.navigate(NavChange::Plan(None));
                        }
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
            let nav = self.nav.clone();
            let (inbox, generation) = (self.inbox.clone(), self.generation);
            net.arch_view(&nav.view, nav.plan.as_ref(), &nav.roots, nav.depth, ctx.clone(), move |r| push(&inbox, Reply::View(generation, Box::new(r))));
        }
    }

    /// Moves the selection to the nearest node in a direction (on the layout).
    pub fn move_selection(&mut self, dir: Dir) {
        let Some(result) = &self.result else { return };
        let Some(current) = self.selection.as_ref().and_then(|s| result.layout.rects.get(s)) else {
            let first = result.view.roots.first().or(result.view.nodes.first().map(|n| &n.id)).cloned();
            if let Some(first) = first {
                self.bring_into_view(&first);
                self.select(Some(first));
            }
            return;
        };
        let center = |r: &WorldRect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let (cx, cy) = center(current);
        let best = result
            .layout
            .rects
            .iter()
            // Not what it sits in, nor what sits in it: moves go between neighbours.
            .filter(|(id, r)| Some(*id) != self.selection.as_ref() && self.shown(id) && !r.contains(current) && !current.contains(r))
            .filter_map(|(id, r)| {
                let (x, y) = center(r);
                // Sideways distance is the gap between the boxes' spans, so
                // boxes in the same row (whatever their heights) line up.
                let gap = |a0: f32, a1: f32, b0: f32, b1: f32| if a1 < b0 { b0 - a1 } else if b1 < a0 { a0 - b1 } else { 0.0 };
                let (along, across) = match dir {
                    Dir::Right => (x - cx, gap(current.y, current.y + current.h, r.y, r.y + r.h)),
                    Dir::Left => (cx - x, gap(current.y, current.y + current.h, r.y, r.y + r.h)),
                    Dir::Down => (y - cy, gap(current.x, current.x + current.w, r.x, r.x + r.w)),
                    Dir::Up => (cy - y, gap(current.x, current.x + current.w, r.x, r.x + r.w)),
                };
                (along > 1.0 && across.abs() <= along * 2.0).then_some((across.abs() > 0.5, along + 2.0 * across.abs(), id))
            })
            // Boxes in the same row (or column) first, then the nearest.
            .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)))
            .map(|(_, _, id)| id.clone());
        // Nothing beside it: a module's moves go into it (its first part
        // that way).
        let best = best.or_else(|| {
            let inside: Vec<(&ComponentId, &WorldRect)> = result.layout.rects.iter().filter(|(id, r)| self.shown(id) && current.contains(r) && *r != current).collect();
            let key = |r: &WorldRect| match dir {
                Dir::Right | Dir::Down => (r.y + r.x * 0.001, r.x),
                Dir::Left => (-(r.x + r.w), r.y),
                Dir::Up => (-(r.y + r.h), r.x),
            };
            inside.into_iter().min_by(|a, b| key(a.1).partial_cmp(&key(b.1)).unwrap_or(std::cmp::Ordering::Equal)).map(|(id, _)| id.clone())
        });
        if let Some(best) = best {
            self.bring_into_view(&best);
            self.select(Some(best));
        }
    }

    fn to_screen(&self, r: &WorldRect) -> Rect {
        let z = self.camera.zoom;
        Rect::from_min_size(self.canvas.min + self.camera.pan + Vec2::new(r.x, r.y) * z, Vec2::new(r.w, r.h) * z)
    }

    // ---- moves the universal keys ask for ----

    /// What holds the selection, innermost first: its containers in this
    /// view, then the modules its id names (`a::b::c` is in `a::b`).
    fn holders_of_selection(&self) -> Vec<ComponentId> {
        let Some(sel) = &self.selection else { return Vec::new() };
        let mut out: Vec<ComponentId> = Vec::new();
        if let Some(result) = &self.result {
            let mut at = sel.clone();
            while let Some(parent) = result.view.edges.iter().find(|e| e.kind == RelationKind::Contains && e.target == at).map(|e| e.source.clone()) {
                if out.contains(&parent) {
                    break;
                }
                out.push(parent.clone());
                at = parent;
            }
        }
        let mut path = out.last().unwrap_or(sel).as_str().to_owned();
        while let Some((head, _)) = path.rsplit_once("::") {
            path = head.to_owned();
            let id = ComponentId::new(&path);
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }

    /// The innermost module a node is drawn in (None: the top level).
    fn container_of(&self, id: &ComponentId) -> Option<&ComponentId> {
        self.ancestors.get(id).and_then(|outer| outer.last())
    }

    /// Drawn nodes in the same module as the selection, in reading order
    /// (top to bottom, left to right).
    fn siblings(&self) -> Vec<ComponentId> {
        let (Some(result), Some(sel)) = (&self.result, &self.selection) else { return Vec::new() };
        let parent = self.container_of(sel).cloned();
        let mut out: Vec<(&WorldRect, &ComponentId)> =
            result.layout.rects.iter().filter(|(id, _)| self.shown(id) && self.container_of(id).cloned() == parent).map(|(id, r)| (r, id)).collect();
        out.sort_by(|a, b| a.0.y.total_cmp(&b.0.y).then(a.0.x.total_cmp(&b.0.x)).then_with(|| a.1.cmp(b.1)));
        out.into_iter().map(|(_, id)| id.clone()).collect()
    }

    /// The next (or previous) sibling of the selection.
    fn step_sibling(&mut self, forward: bool, count: usize) {
        let siblings = self.siblings();
        let Some(at) = self.selection.as_ref().and_then(|s| siblings.iter().position(|x| x == s)) else { return };
        let next = if forward { (at + count).min(siblings.len() - 1) } else { at.saturating_sub(count) };
        let id = siblings[next].clone();
        self.bring_into_view(&id);
        self.select(Some(id));
    }

    /// What `caller` calls, in the order it calls them (as shown).
    fn calls_of(&self, caller: &ComponentId) -> Vec<ComponentId> {
        let Some(result) = &self.result else { return Vec::new() };
        let mut calls: Vec<(u32, usize, &ComponentId)> = result
            .view
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == RelationKind::Calls && &e.source == caller)
            .map(|(i, e)| (e.order.unwrap_or(u32::MAX), i, &e.target))
            .collect();
        calls.sort();
        let mut seen = BTreeSet::new();
        calls.into_iter().filter(|(_, _, id)| seen.insert(*id)).map(|(_, _, id)| id.clone()).collect()
    }

    /// Steps through a caller's calls in call order (`] e`, `[ e`): the
    /// first step goes to the selection's first (or last) call, later steps
    /// to the call after (or before) it. Says why not, when it can't.
    pub fn step_call(&mut self, forward: bool) -> Result<(), String> {
        let Some(sel) = self.selection.clone() else { return Err("Select a node first.".into()) };
        let walking = self.call_walk.clone().filter(|(caller, at)| self.calls_of(caller).get(*at) == Some(&sel));
        let (caller, next) = match walking {
            Some((caller, at)) => {
                let n = self.calls_of(&caller).len();
                let next = if forward { at + 1 } else { at.wrapping_sub(1) };
                if next >= n {
                    return Err(format!("That was the {} call.", if forward { "last" } else { "first" }));
                }
                (caller, next)
            }
            None => {
                let n = self.calls_of(&sel).len();
                if n == 0 {
                    let name = self.component(&sel).map_or_else(|| sel.to_string(), |c| c.name.clone());
                    let hint = if self.nav.view == "calls" { "" } else { " (the calls view shows calls)" };
                    return Err(format!("{name} calls nothing in this view{hint}."));
                }
                (sel, if forward { 0 } else { n - 1 })
            }
        };
        let target = self.calls_of(&caller)[next].clone();
        self.call_walk = Some((caller, next));
        self.navigate(NavChange::Jump(target));
        Ok(())
    }

    /// Half a page down (or up): the view pans by half its height and the
    /// drawn node nearest to where the selection was on screen is selected.
    fn half_page(&mut self, down: bool, count: usize) {
        if self.result.is_none() {
            return;
        }
        let anchor = self.selected_rect().map(|r| r.center()).filter(|p| self.open_area.contains(*p)).unwrap_or(self.open_area.center());
        let dy = self.open_area.height() / 2.0 * count as f32;
        self.camera.pan.y += if down { -dy } else { dy };
        let Some(result) = &self.result else { return };
        // Cards and closed modules; not the frames of open ones.
        let nearest = result
            .layout
            .rects
            .iter()
            .filter(|(id, _)| self.shown(id) && (!result.layout.containers.contains(*id) || self.closed(id)))
            .map(|(id, r)| (self.to_screen(r).center().distance(anchor), id))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id.clone());
        if let Some(id) = nearest {
            self.select(Some(id));
        }
    }

    fn selected_rect(&self) -> Option<Rect> {
        let r = self.result.as_ref()?.layout.rects.get(self.selection.as_ref()?)?;
        Some(self.to_screen(r))
    }

    /// The `index`th node in view order, or the last (drawn or not: a jump
    /// opens what it lands in).
    fn jump_to_index(&mut self, index: Option<usize>) {
        let Some(result) = &self.result else { return };
        let target = match index {
            Some(i) => result.view.nodes.get(i),
            None => result.view.nodes.last(),
        };
        if let Some(target) = target.map(|n| n.id.clone()) {
            self.navigate(NavChange::Jump(target));
        }
    }

    /// The module a fold acts on: the selection if it's one, else the one
    /// it's in.
    fn fold_target(&self) -> Option<ComponentId> {
        let result = self.result.as_ref()?;
        let sel = self.selection.as_ref()?;
        if result.layout.containers.contains(sel) { Some(sel.clone()) } else { self.container_of(sel).cloned() }
    }

    /// Opens or closes a module by hand; when that's what the zoom would do
    /// anyway, the zoom decides again.
    fn set_fold(&mut self, container: ComponentId, open: bool) {
        let zoom_closed = self.collapsed() && Some(&container) != self.frame.as_ref();
        if open == !zoom_closed {
            self.folds.remove(&container);
        } else {
            self.folds.insert(container.clone(), open);
        }
        // A node the fold hides hands the selection to its module.
        if !open && self.selection.as_ref().is_some_and(|s| !self.shown(s)) {
            self.select(Some(container));
        }
    }

    // ---- semantic zoom ----

    /// How much node cards show at the current zoom.
    pub fn detail(&self) -> Detail {
        let z = self.camera.zoom;
        if z >= self.ui.detail.near {
            Detail::Near
        } else if z >= self.ui.detail.mid {
            Detail::Mid
        } else {
            Detail::Far
        }
    }

    /// The lines a node card shows at `detail`.
    pub fn card_lines(&self, c: &Component, alignment: Alignment, detail: Detail) -> Vec<CardLine> {
        let line = |role, text: String| CardLine { role, text };
        let mut out = vec![line(LineRole::Name, c.name.clone())];
        if detail == Detail::Far {
            return out;
        }
        let location = match (&c.anchor, &c.proposed) {
            (Some(a), _) => format!("{}:{}", a.file, a.span.start_line),
            (None, Some(p)) => format!("{} {}", egui_phosphor::regular::ARROW_RIGHT, p.file),
            (None, None) => "unplaced".to_owned(),
        };
        // Each state with its icon and word, as everywhere else.
        let badge = |name: &str, count: Option<usize>| {
            let (glyph, word, _) = crate::status::look(name);
            match count {
                Some(n) => format!("{glyph} {n} {word}"),
                None => format!("{glyph} {word}"),
            }
        };
        let stage = badge(crate::status::stage(c.stage), None);
        if detail == Detail::Mid {
            let placeholders = if c.placeholders.is_empty() { String::new() } else { format!(" · {}", badge("placeholders", Some(c.placeholders.len()))) };
            out.push(line(LineRole::Meta, format!("{} · {stage}{placeholders}", c.type_name)));
            out.push(line(LineRole::Location, location));
            return out;
        }
        out.push(line(LineRole::Meta, format!("{} · {stage} · {}", c.type_name, badge(crate::status::alignment(alignment), None))));
        if let Some(sig) = &c.signature {
            let params: Vec<String> = sig.params.iter().map(|p| p.ty.as_ref().map_or_else(|| p.name.clone(), |t| format!("{}: {t}", p.name))).collect();
            let returns = sig.returns.as_ref().map(|r| format!(" -> {r}")).unwrap_or_default();
            out.push(line(LineRole::Signature, format!("({}){returns}", params.join(", "))));
        }
        out.push(line(LineRole::Location, location));
        let mut badges = Vec::new();
        if !c.placeholders.is_empty() {
            badges.push(badge("placeholders", Some(c.placeholders.len())));
        }
        if let Some(mark) = self.marks.get(&c.id) {
            if mark.open > 0 {
                badges.push(badge("open_comments", Some(mark.open)));
            }
            if mark.changed {
                badges.push(badge("changed", None));
            }
        }
        if !badges.is_empty() {
            out.push(line(LineRole::Badges, badges.join(" · ")));
        }
        out
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
        let rects: Vec<WorldRect> = result.layout.rects.values().copied().collect();
        self.fit_rects(&rects);
    }

    /// Zooms and pans so `rects` fill the open part of the canvas.
    fn fit_rects(&mut self, rects: &[WorldRect]) {
        if rects.is_empty() {
            return;
        }
        let (min_x, min_y) = rects.iter().fold((f32::MAX, f32::MAX), |(x, y), r| (x.min(r.x), y.min(r.y)));
        let (max_x, max_y) = rects.iter().fold((f32::MIN, f32::MIN), |(x, y), r| (x.max(r.x + r.w), y.max(r.y + r.h)));
        let (w, h) = ((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));
        let pad = 32.0;
        let area = self.open_area;
        let zoom = ((area.width() - pad * 2.0) / w).min((area.height() - pad * 2.0) / h).clamp(0.03, 1.4);
        self.camera.zoom = zoom;
        let content = Vec2::new(w, h) * zoom;
        self.camera.pan = (area.min - self.canvas.min) + (area.size() - content) / 2.0 - Vec2::new(min_x, min_y) * zoom;
    }

    // ---- drawing ----

    pub fn render(&mut self, ui: &mut egui::Ui, feedback: &mut FeedbackInspectorState, slot_idx: usize, pan_held: bool, cx: &mut RunCx<'_>) {
        self.tick(cx.net, cx.ctx);
        self.render_toolbar(ui, cx);
        let area = ui.available_rect_before_wrap();
        let narrow = area.width() < self.ui.chips.narrow_below;
        let has_drawer = self.drawer_open && self.selected().is_some();
        // Wide: the inspector sits beside the canvas. Narrow: it's a sheet
        // over the bottom of the canvas.
        let drawer_w = if has_drawer && !narrow { (area.width() * DRAWER_SHARE).clamp(200.0, 320.0) } else { 0.0 };
        let canvas = Rect::from_min_max(area.min, Pos2::new(area.max.x - drawer_w, area.max.y));
        self.canvas = canvas;
        let sheet_h = if has_drawer && narrow { canvas.height() * SHEET_SHARE } else { 0.0 };
        self.open_area = Rect::from_min_max(canvas.min, Pos2::new(canvas.max.x, canvas.max.y - sheet_h));
        feedback.register_inspectable(ui, &format!("slot_{}:graph:canvas", slot_idx + 1), canvas);
        let response = ui.allocate_rect(canvas, Sense::click_and_drag());
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 0.0, colors().bg);
        self.draw_grid(&painter);
        let time = ui.input(|i| i.time);

        if self.fit && self.result.is_some() {
            self.fit = false;
            self.fit_view();
        }
        if let Some(id) = self.reveal_after_fit.take() {
            self.bring_into_view(&id);
        }
        // Fingers first: while touching, pointer emulation is ignored.
        let touched = self.handle_touch(ui);
        if !touched {
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
        }

        match (&self.result, &self.load) {
            (_, Load::Failed(err)) if self.result.is_none() => {
                painter.text(canvas.center(), Align2::CENTER_CENTER, format!("Couldn't load the view: {err}"), FontId::proportional(crate::theme::look().text_title), colors().error);
            }
            (None, _) => {
                painter.text(canvas.center(), Align2::CENTER_CENTER, "Loading the architecture…", FontId::proportional(crate::theme::look().text_title), colors().muted);
            }
            (Some(result), _) if result.view.nodes.is_empty() => {
                painter.text(
                    canvas.center(),
                    Align2::CENTER_CENTER,
                    "Nothing to show in this view yet. Ingest the code (Space p… or the agent's ingest tool), or pick another view.",
                    FontId::proportional(crate::theme::look().text_title),
                    colors().muted,
                );
            }
            (Some(_), _) => {
                // Moving dashes and the agent's pulse keep drawing.
                if self.draw_graph(&painter, time) {
                    ui.ctx().request_repaint();
                }
            }
        }
        if let Load::Failed(err) = &self.load
            && self.result.is_some()
        {
            painter.text(canvas.left_bottom() + Vec2::new(8.0, -8.0), Align2::LEFT_BOTTOM, format!("refresh failed: {err}"), FontId::proportional(crate::theme::look().text_body), colors().error);
        }

        // Click selects, double click focuses.
        if !touched
            && (response.clicked() || response.double_clicked())
            && let Some(pos) = response.interact_pointer_pos()
        {
            let hit = self.hit(pos);
            if response.double_clicked()
                && let Some(id) = hit.clone()
            {
                self.pointer_jump(NavChange::Focus(id));
            }
            self.select(hit);
        }
        if self.legend_open {
            self.draw_legend(&painter);
        }
        self.render_chips(ui, cx);
        if has_drawer {
            let drawer = if narrow { Rect::from_min_max(Pos2::new(canvas.min.x, canvas.max.y - sheet_h), canvas.max) } else { Rect::from_min_max(Pos2::new(canvas.max.x, area.min.y), area.max) };
            ui.scope_builder(egui::UiBuilder::new().max_rect(drawer.shrink(6.0)), |ui| self.render_drawer(ui));
        }
    }

    /// Feeds this frame's finger events to the gesture recognizer and acts on
    /// what it sees. Returns whether fingers were involved this frame.
    fn handle_touch(&mut self, ui: &egui::Ui) -> bool {
        let (events, time) = ui.input(|i| {
            let touches: Vec<(u64, crate::touch::Phase, Pos2)> = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Touch { id, phase, pos, .. } => Some((id.0, crate::touch::Phase::from(*phase), *pos)),
                    _ => None,
                })
                .collect();
            (touches, i.time)
        });
        let cfg = self.ui.touch.clone();
        let mut gestures = Vec::new();
        for (id, phase, pos) in &events {
            // Fingers that start on a chip belong to the chip.
            if *phase == crate::touch::Phase::Start && self.chip_rects.iter().any(|(_, r)| r.contains(*pos)) {
                continue;
            }
            if *phase == crate::touch::Phase::Start && !self.canvas.contains(*pos) {
                continue;
            }
            gestures.extend(self.gestures.feed(*id, *phase, *pos, time, &cfg));
        }
        gestures.extend(self.gestures.tick(time, &cfg));
        if let Some(due) = self.gestures.long_press_due(&cfg) {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((due - time).max(0.0)));
        }
        let involved = !events.is_empty() || self.gestures.active();
        if involved {
            self.touch_seen = true;
        }
        for gesture in gestures {
            self.on_gesture(gesture);
        }
        involved
    }

    /// What each gesture does to the graph.
    pub fn on_gesture(&mut self, gesture: Gesture) {
        match gesture {
            Gesture::Tap(pos) => {
                self.context = None;
                let hit = self.hit(pos);
                self.select(hit);
            }
            Gesture::DoubleTap(pos) => match self.hit(pos) {
                Some(id) => {
                    self.select(Some(id.clone()));
                    self.pointer_jump(NavChange::Focus(id));
                }
                None => self.fit_view(),
            },
            Gesture::LongPress(pos) => {
                if let Some(id) = self.hit(pos) {
                    self.select(Some(id.clone()));
                    self.context = Some((id, pos));
                }
            }
            Gesture::Pan(delta) => self.camera.pan += delta,
            Gesture::Pinch { factor, center } => self.zoom_by(factor, center),
            // The jump list is the shell's: back and forward go through it.
            Gesture::Swipe(Swipe::Right) => self.requests.run = Some(("nav.back".into(), None)),
            Gesture::Swipe(Swipe::Left) => self.requests.run = Some(("nav.forward".into(), None)),
        }
    }

    /// Whether the on-screen chips show now.
    fn chips_shown(&self) -> bool {
        match self.ui.chips.show {
            ChipMode::Always => true,
            ChipMode::Never => false,
            ChipMode::Auto => self.touch_seen || self.canvas.width() < self.ui.chips.narrow_below,
        }
    }

    /// On-screen chips (view navigation) along the canvas top, and a
    /// long-pressed node's actions next to it. Both run the same actions as
    /// the keys: the graph's own, and the shell's (moves, go-tos, the jump
    /// list).
    fn render_chips(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        self.chip_rects.clear();
        let mut actions = self.actions();
        actions.extend(cx.shell.iter().cloned());
        let size = self.ui.chips.size;
        let chip = |ui: &mut egui::Ui, action: &Action| {
            let text = action.glyph().map_or_else(|| action.label.clone(), str::to_owned);
            let response = ui.add(egui::Button::new(RichText::new(text).size(size * 0.42)).min_size(Vec2::splat(size)).corner_radius(size / 2.0)).on_hover_text(&action.label);
            (response.rect, response.clicked())
        };
        let mut run: Option<Action> = None;
        if self.chips_shown() {
            let bar = Rect::from_min_max(self.canvas.min + Vec2::splat(6.0), Pos2::new(self.canvas.max.x - 6.0, self.canvas.min.y + size * 2.0 + 12.0));
            let wanted: Vec<&Action> = self.ui.chips.actions.iter().filter_map(|id| actions.iter().find(|a| &a.id == id)).collect();
            ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
                ui.horizontal_wrapped(|ui| {
                    for action in wanted {
                        let (rect, clicked) = chip(ui, action);
                        self.chip_rects.push((action.id.clone(), rect));
                        if clicked {
                            run = Some(action.clone());
                        }
                    }
                });
            });
        }
        if let Some((_, at)) = self.context.clone() {
            let wanted: Vec<&Action> = self.ui.context.iter().filter_map(|id| actions.iter().find(|a| &a.id == id)).collect();
            let width = (size + 8.0) * 3.0;
            let origin = Pos2::new(at.x.min(self.canvas.max.x - width).max(self.canvas.min.x), (at.y + 12.0).min(self.canvas.max.y - size * 2.0));
            let panel = Rect::from_min_size(origin, Vec2::new(width, (size + 8.0) * wanted.len().div_ceil(3) as f32));
            ui.scope_builder(egui::UiBuilder::new().max_rect(panel), |ui| {
                egui::Frame::NONE.fill(colors().overlay).corner_radius(crate::theme::look().radius_xl).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for action in wanted {
                            let text = RichText::new(&action.label).size(crate::theme::look().text_label);
                            let response = ui.add(egui::Button::new(text).min_size(Vec2::new(size, size)));
                            self.chip_rects.push((action.id.clone(), response.rect));
                            if response.clicked() {
                                run = Some(action.clone());
                            }
                        }
                    });
                });
            });
        }
        if let Some(action) = run {
            self.context = None;
            self.use_action(&action, None, cx);
        }
    }

    /// Runs an action from a chip or the toolbar: the graph's own right here,
    /// unless it's a jump; jumps and the shell's actions go through the shell
    /// (so the jump list records them).
    fn use_action(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        if action.command::<GraphCommand>().is_some() && !action.jump {
            self.run(action, value, cx);
        } else {
            self.requests.run = Some((action.id.clone(), value));
        }
    }

    /// The legend: what each color, line and outline means, from the style
    /// table. Sized to its text (wrapping on narrow canvases) and kept clear
    /// of the on-screen chips.
    fn draw_legend(&self, painter: &egui::Painter) {
        let theme = colors();
        let entries = style().legend(&theme);
        let (pad, swatch_w, gap) = (10.0, 28.0, 8.0);
        let max_w = (self.canvas.width() - 16.0).clamp(160.0, 380.0);
        let text_w = max_w - pad * 2.0 - swatch_w - gap;
        let font = FontId::proportional(crate::theme::look().text_body);
        let mut rows: Vec<(Option<&str>, Option<&LegendEntry>, std::sync::Arc<egui::Galley>)> = Vec::new();
        let mut section = "";
        for entry in &entries {
            if entry.section != section {
                section = entry.section;
                rows.push((Some(section), None, painter.layout(section.to_owned(), font.clone(), theme.muted, max_w - pad * 2.0)));
            }
            rows.push((None, Some(entry), painter.layout(entry.label.clone(), font.clone(), theme.text, text_w)));
        }
        let width = rows
            .iter()
            .map(|(head, _, g)| if head.is_some() { g.size().x + pad * 2.0 } else { g.size().x + pad * 2.0 + swatch_w + gap })
            .fold(0.0f32, f32::max)
            .min(max_w);
        let height: f32 = rows.iter().map(|(_, _, g)| g.size().y.max(14.0) + 4.0).sum::<f32>() + pad * 2.0;
        // Below any chips that would sit over its corner.
        let mut top = self.canvas.min.y + 8.0;
        let left = self.canvas.max.x - width - 8.0;
        let footprint = Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, height));
        for (_, chip) in &self.chip_rects {
            if chip.intersects(footprint) {
                top = top.max(chip.max.y + 8.0);
            }
        }
        let rect = Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, height)).intersect(self.canvas);
        painter.rect_filled(rect, 6.0, theme.overlay);
        let mut y = rect.min.y + pad;
        for (head, entry, galley) in rows {
            let h = galley.size().y.max(14.0);
            match (head, entry) {
                (Some(_), _) => {
                    painter.galley(Pos2::new(rect.min.x + pad, y), galley, theme.muted);
                }
                (None, Some(entry)) => {
                    let swatch = Rect::from_min_size(Pos2::new(rect.min.x + pad, y + 2.0), Vec2::new(swatch_w, 10.0));
                    match entry.swatch {
                        Swatch::Line(line) => {
                            let points = [swatch.left_center(), swatch.right_center()];
                            if line.dashed {
                                painter.extend(Shape::dashed_line(&points, Stroke::new(line.width, line.color), 5.0, 3.0));
                            } else {
                                painter.line_segment(points, Stroke::new(line.width, line.color));
                            }
                        }
                        Swatch::Outline(line) => {
                            painter.rect_stroke(swatch, 3.0, Stroke::new(line.width, line.color), StrokeKind::Inside);
                        }
                        Swatch::Stripe(color) => {
                            painter.rect_filled(Rect::from_min_size(swatch.min, Vec2::new(5.0, swatch.height())), 1.0, color);
                        }
                    }
                    painter.galley(Pos2::new(swatch.max.x + gap, y), galley, theme.text);
                }
                (None, None) => {}
            }
            y += h + 4.0;
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
        // Back and forward (the shell's jump list) lead the toolbar.
        let mut actions: Vec<Action> = cx
            .shell
            .iter()
            .filter_map(|a| match a.id.as_str() {
                "nav.back" => Some(a.clone().toolbar(ToolbarStyle::Button, "", Some(egui_phosphor::regular::ARROW_LEFT))),
                "nav.forward" => Some(a.clone().toolbar(ToolbarStyle::Button, "", Some(egui_phosphor::regular::ARROW_RIGHT))),
                _ => None,
            })
            .collect();
        actions.extend(self.actions());
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
            let nav = self.nav.clone();
            let focus = if nav.roots.is_empty() {
                "entry points".to_owned()
            } else {
                nav.roots.iter().map(|r| self.component(r).map_or_else(|| r.to_string(), |c| c.name.clone())).collect::<Vec<_>>().join(", ")
            };
            let depth = nav.depth.unwrap_or(self.base_depth());
            let shown = self.result.as_ref().map_or(0, |r| r.view.nodes.len());
            ui.label(RichText::new(format!("{} · from {focus} · depth {depth} · {shown} nodes", nav.plan.as_ref().map_or("code".to_owned(), |p| format!("plan {p}")))).size(crate::theme::look().text_body).color(colors().muted));
            if !self.stale_files.is_empty() {
                let count = self.stale_files.len().to_string();
                crate::status::chip(ui, "stale", Some(&count)).on_hover_text(self.stale_files.join("\n"));
                if ui.small_button("Re-read").on_hover_text("Read the code that changed again (SPC I r)").clicked() {
                    self.requests.reindex = true;
                }
            }
            if let Some(agent) = &self.agent {
                ui.label(RichText::new(format!("{} the agent moved the view: {}", egui_phosphor::regular::ROBOT, agent.what)).size(crate::theme::look().text_body).color(colors().special));
                if let Some(back) = actions.iter().find(|a| a.id == "graph.agent_return")
                    && ui.small_button(format!("{} Return", egui_phosphor::regular::ARROW_U_UP_LEFT)).on_hover_text(&back.label).clicked()
                {
                    used = Some((back.clone(), ToolbarUse::Run(None)));
                }
            }
        });
        ui.separator();
        match used {
            Some((action, ToolbarUse::Run(value))) => self.use_action(&action, value, cx),
            Some((_, ToolbarUse::OpenPalette)) | None => {}
        }
    }

    /// A faint grid under the graph, moving and scaling with it.
    fn draw_grid(&self, painter: &egui::Painter) {
        let look = crate::theme::look();
        if look.grid_step <= 0.0 {
            return;
        }
        let mut step = look.grid_step * self.camera.zoom;
        while step < look.grid_step / 2.0 {
            step *= 2.0;
        }
        let origin = self.canvas.min + self.camera.pan;
        let stroke = Stroke::new(look.line_thin, colors().grid);
        let mut x = self.canvas.left() + (origin.x - self.canvas.left()).rem_euclid(step);
        while x < self.canvas.right() {
            painter.line_segment([Pos2::new(x, self.canvas.top()), Pos2::new(x, self.canvas.bottom())], stroke);
            x += step;
        }
        let mut y = self.canvas.top() + (origin.y - self.canvas.top()).rem_euclid(step);
        while y < self.canvas.bottom() {
            painter.line_segment([Pos2::new(self.canvas.left(), y), Pos2::new(self.canvas.right(), y)], stroke);
            y += step;
        }
    }

    /// Draws the view; returns whether something on it moves (and wants
    /// another frame).
    fn draw_graph(&self, painter: &egui::Painter, time: f64) -> bool {
        let look = crate::theme::look();
        let mut moving = false;
        let Some(result) = &self.result else { return false };
        let z = self.camera.zoom;
        // Containers first, then edges, then nodes on top.
        for id in result.layout.containers.iter().filter(|id| self.shown(id)) {
            let Some(r) = result.layout.rects.get(id) else { continue };
            let rect = self.to_screen(r);
            // A closed module shows the selection hidden inside it.
            let holds_selection = self.closed(id) && self.selection.as_ref().is_some_and(|s| self.ancestors.get(s).is_some_and(|a| a.contains(id)));
            let selected = self.selection.as_ref() == Some(id) || holds_selection;
            painter.rect_filled(rect, 6.0, colors().surface.gamma_multiply(0.6));
            painter.rect_stroke(rect, 6.0, Stroke::new(if selected { crate::theme::look().line_bold } else { crate::theme::look().line_thin }, if selected { colors().focus } else { colors().border_strong }), StrokeKind::Inside);
            let Some(c) = result.components.get(id) else { continue };
            if self.closed(id) {
                // One block per container: its name, as large as fits, and how much is inside.
                let inner = result.layout.rects.iter().filter(|(_, k)| *k != r && r.contains(k)).count();
                let size = (rect.width() / (c.name.len().max(4) as f32 * 0.6)).min(rect.height() / 2.6).clamp(8.0, 20.0);
                // Labels stay inside their block: shortened when they don't fit,
                // the second line only when there's room for it.
                let clip = painter.with_clip_rect(rect);
                let room = rect.width() - 6.0;
                let two_lines = rect.height() >= size * 2.6;
                let name_at = if two_lines { rect.center() - Vec2::new(0.0, size * 0.4) } else { rect.center() };
                if let Some(name) = fit_text(painter, &c.name, &FontId::proportional(size), room) {
                    clip.text(name_at, Align2::CENTER_CENTER, name, FontId::proportional(size), colors().text_strong);
                }
                let small = FontId::proportional((size * 0.7).max(8.0));
                if two_lines && let Some(kind) = fit_text(painter, &format!("{} · {inner}", c.type_name), &small, room) {
                    clip.text(rect.center() + Vec2::new(0.0, size * 0.7), Align2::CENTER_CENTER, kind, small, colors().muted);
                }
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
            // With something selected, lines that don't touch it fade back.
            let touches = self.selection.as_ref().is_none_or(|s| *s == edge.source || *s == edge.target);
            let color = if touches { line.color } else { line.color.gamma_multiply(1.0 - look.fade) };
            let stroke = Stroke::new(line.width, color);
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
                // Planned connections' dashes move along them.
                let period = look.space_sm + look.space_s;
                let offset = if look.dash_speed > 0.0 {
                    moving = true;
                    period - ((time as f32 * look.dash_speed) % period)
                } else {
                    0.0
                };
                painter.extend(Shape::dashed_line_with_offset(&points, stroke, &[look.space_sm], &[look.space_s], offset));
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
            moving |= self.draw_node(painter, self.to_screen(r), c, node.alignment, node.truncated, time);
        }
        moving
    }

    /// Draws one card; returns whether it animates (the agent's pulse).
    fn draw_node(&self, painter: &egui::Painter, rect: Rect, c: &Component, alignment: Alignment, truncated: bool, time: f64) -> bool {
        let z = self.camera.zoom;
        let look = crate::theme::look();
        let selected = self.selection.as_ref() == Some(&c.id);
        let (theme, style) = (colors(), style());
        // The selection glows.
        if selected && look.glow_width > 0.0 {
            let rings = 4;
            for i in 1..=rings {
                let t = i as f32 / rings as f32;
                let alpha = look.glow_alpha * (1.0 - t);
                painter.rect_stroke(rect.expand(look.glow_width * t), look.radius_l + look.glow_width * t, Stroke::new(look.glow_width / rings as f32, theme.focus.gamma_multiply(alpha)), StrokeKind::Outside);
            }
        }
        let outline = style.node_outline(&theme, alignment, selected);
        painter.rect_filled(rect, 6.0, style.node_fill(&theme, selected));
        let stroke = Stroke::new(outline.width, outline.color);
        if outline.dashed {
            let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
            painter.extend(Shape::dashed_line(&corners, stroke, 6.0, 4.0));
        } else {
            painter.rect_stroke(rect, 6.0, stroke, StrokeKind::Inside);
        }
        // The stage as a stripe on the left edge.
        let stripe = (3.0 * z).clamp(2.0, 5.0);
        painter.rect_filled(Rect::from_min_size(rect.min, Vec2::new(stripe, rect.height())), egui::CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, style.stage_color(&theme, c.stage));
        // Where the agent points: a ring, and a pulse going out from it.
        let pointed = self.agent.as_ref().and_then(|a| a.focus.as_ref()) == Some(&c.id);
        if pointed {
            let agent = style.agent_color(&theme);
            painter.rect_stroke(rect.expand(look.space_s), look.radius_xl, Stroke::new(look.line_bold, agent), StrokeKind::Outside);
            if look.pulse_seconds > 0.0 {
                let beat = (time as f32 % look.pulse_seconds) / look.pulse_seconds;
                let reach = look.space_s + look.glow_width * 2.0 * beat;
                painter.rect_stroke(rect.expand(reach), look.radius_xl + reach, Stroke::new(look.line_thin, agent.gamma_multiply(1.0 - beat)), StrokeKind::Outside);
            }
        }
        self.draw_marks(painter, rect, &c.id);
        // Its code changed since it was indexed: the card may be behind.
        if c.anchor.as_ref().is_some_and(|a| self.file_stale(a.file.as_str())) {
            let (glyph, _, color) = crate::status::look("stale");
            let size = (11.0 * z).clamp(9.0, 14.0);
            painter.text(rect.left_top() + Vec2::new(6.0, 4.0) * z.max(0.6), Align2::LEFT_TOP, glyph, FontId::proportional(size), color);
        }
        // Too small to read: the card's color and outline carry it.
        if rect.height() < 9.0 {
            return pointed;
        }
        let clip = painter.with_clip_rect(rect.shrink(2.0));
        let detail = self.detail();
        let lines = self.card_lines(c, alignment, detail);
        // More lines at closer zoom: tighter spacing, smaller type.
        let (step, small) = match detail {
            Detail::Near => (11.0, (8.5 * z).clamp(7.0, 12.0)),
            _ => (16.0, (10.0 * z).clamp(7.0, 13.0)),
        };
        let mut y = 8.0;
        for line in lines {
            let (font, color) = match line.role {
                LineRole::Name => (FontId::proportional((crate::theme::look().text_title * z).clamp(crate::theme::look().text_tiny * 0.66, crate::theme::look().text_display * 1.12).min(rect.height() * 0.45)), colors().text_strong),
                LineRole::Meta => (FontId::proportional(small), colors().muted),
                LineRole::Signature | LineRole::Location => (FontId::monospace(small * 0.95), colors().muted),
                LineRole::Badges => (FontId::proportional(small), colors().warn),
            };
            clip.text(rect.min + Vec2::new(10.0, y) * z, Align2::LEFT_TOP, line.text, font, color);
            y += if line.role == LineRole::Name { step + 4.0 } else { step };
        }
        if truncated {
            painter.text(rect.right_center() + Vec2::new(4.0, 0.0), Align2::LEFT_CENTER, "…", FontId::proportional(crate::theme::look().text_heading * z.max(0.6)), colors().muted);
        }
        pointed
    }

    /// Review badges at the card's top right: a dot when it changed since the
    /// last review, a count of unaddressed comments.
    /// Whether `file` changed since it was indexed.
    fn file_stale(&self, file: &str) -> bool {
        self.stale_files.iter().any(|s| s == file || file.strip_prefix(s.as_str()).is_some_and(|rest| rest.starts_with('/')))
    }

    fn draw_marks(&self, painter: &egui::Painter, rect: Rect, id: &ComponentId) {
        let Some(mark) = self.marks.get(id) else { return };
        let z = self.camera.zoom.max(0.4);
        let mut at = rect.right_top() + Vec2::new(-8.0, 8.0) * z;
        if mark.open > 0 {
            let galley = painter.layout_no_wrap(format!("{}", mark.open), FontId::proportional(crate::theme::look().text_small * z), colors().on_bright);
            let chip = Rect::from_center_size(at, galley.size() + Vec2::new(8.0, 2.0) * z);
            painter.rect_filled(chip, chip.height() / 2.0, colors().warn);
            painter.galley(chip.center() - galley.size() / 2.0, galley, colors().on_bright);
            at.x = chip.left() - 6.0 * z;
        }
        if mark.changed {
            painter.circle_filled(at, 4.0 * z, colors().info);
        }
    }

    /// The compact inspector: a navigable summary of the selection.
    fn render_drawer(&mut self, ui: &mut egui::Ui) {
        let Some(c) = self.selected().cloned() else { return };
        let alignment = self.result.as_ref().and_then(|r| r.view.node(&c.id)).map(|n| n.alignment);
        egui::Frame::NONE.fill(colors().surface).stroke(Stroke::new(crate::theme::look().line_thin, colors().border_strong)).corner_radius(crate::theme::look().radius_l).inner_margin(crate::theme::margin(crate::theme::look().space_m)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt("graph_inspector").auto_shrink([false, false]).show(ui, |ui| {
                // Close first (right-aligned), then the name takes what's left
                // and truncates, so a long name never hides the close button.
                ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(egui_phosphor::regular::X).on_hover_text("Close (x)").clicked() {
                        self.drawer_open = false;
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(&c.name).strong().size(crate::theme::look().text_heading).color(colors().text_strong)).truncate()).on_hover_text(&c.name);
                    });
                });
                ui.label(RichText::new(c.id.as_str()).monospace().size(crate::theme::look().text_small).color(colors().muted));
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("{} · {:?}", c.type_name, c.status)).size(crate::theme::look().text_body).color(colors().muted));
                    crate::status::chip(ui, crate::status::stage(c.stage), None);
                    if let Some(a) = alignment {
                        crate::status::chip(ui, crate::status::alignment(a), None);
                    }
                });
                if let Some(mark) = self.marks.get(&c.id) {
                    ui.horizontal_wrapped(|ui| {
                        if mark.changed {
                            ui.label(RichText::new("changed since review").size(crate::theme::look().text_body).color(colors().info));
                        }
                        if mark.open > 0 {
                            ui.label(RichText::new(format!("{} open review points", mark.open)).size(crate::theme::look().text_body).color(colors().warn));
                        }
                    });
                }
                match (&c.anchor, &c.proposed) {
                    (Some(a), _) => ui.label(RichText::new(format!("{} {}:{}–{}", egui_phosphor::regular::FILE_CODE, a.file, a.span.start_line, a.span.end_line)).monospace().size(crate::theme::look().text_body)),
                    (None, Some(p)) => ui.label(RichText::new(format!("{} planned in {}", egui_phosphor::regular::FILE_DASHED, p.file)).monospace().size(crate::theme::look().text_body).color(colors().warn)),
                    (None, None) => ui.label(RichText::new("not placed yet").size(crate::theme::look().text_body).color(colors().warn)),
                };
                if !c.description.is_empty() {
                    ui.label(RichText::new(&c.description).size(crate::theme::look().text_body));
                }
                let spec = c.spec.as_ref();
                ui.label(
                    RichText::new(format!(
                        "{} logic steps · {} invariants · {} placeholders",
                        spec.map_or(0, |s| s.logic_steps.len()),
                        spec.map_or(0, |s| s.invariants.len()),
                        c.placeholders.len()
                    ))
                    .size(crate::theme::look().text_body)
                    .color(colors().muted),
                );
                ui.horizontal(|ui| {
                    if ui.button(format!("{} Spec view", egui_phosphor::regular::ARTICLE)).on_hover_text("g s").clicked() {
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
                        let (to, from) = (egui_phosphor::regular::ARROW_RIGHT, egui_phosphor::regular::ARROW_LEFT);
                        for (label, list) in [(format!("{} {to}", kind.name()), out), (format!("{from} {}", kind.name()), inc)] {
                            if list.is_empty() {
                                continue;
                            }
                            ui.label(RichText::new(label).size(crate::theme::look().text_small).color(colors().muted));
                            for id in list.into_iter().take(8) {
                                let name = result.components.get(id).map_or(id.to_string(), |x| x.name.clone());
                                if ui.link(RichText::new(name).size(crate::theme::look().text_body)).clicked() {
                                    go = Some(id.clone());
                                }
                            }
                        }
                    }
                }
                if let Some(id) = go {
                    self.pointer_jump(NavChange::Jump(id));
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
    Focus,
    Unfocus,
    Depth(i32),
    SetView,
    NextView,
    SetPlan,
    Zoom(f32),
    Fit,
    CloseInspector,
    Refresh,
    Legend,
    AgentReturn,
    Call { forward: bool },
}

impl ActionProvider for GraphState {
    fn actions(&self) -> Vec<Action> {
        use GraphCommand as G;
        use ToolbarStyle::{Button, Control};
        let mut out = Vec::new();
        let f = |id: &str, label: &str, c: GraphCommand| Action::new(id, label, c).focused();
        if let Some(catalog) = &self.catalog {
            let current = &self.nav.view;
            let choices = catalog.views.iter().map(|(name, v)| Choice::new(name, name).detail(Some(v.description.clone())).current(name == current)).collect();
            out.push(f("graph.view", "Switch view", G::SetView).param(Param::Choice(choices)).toolbar(Control, "View", Some(egui_phosphor::regular::GRAPH)).group(Group::View).jump());
            out.push(f("graph.next_view", "Next view", G::NextView).icon(egui_phosphor::regular::SWAP).group(Group::View).importance(SECONDARY).jump());
        }
        let plan = self.nav.plan.clone();
        let mut plans = vec![Choice::new("", "code only").current(plan.is_none())];
        plans.extend(self.plans.iter().map(|p| Choice::new(&p.0, &p.0).current(plan.as_ref() == Some(p))));
        out.push(f("graph.plan", "Lay a plan over the code", G::SetPlan).param(Param::Choice(plans)).toolbar(Control, "Plan", None).group(Group::View).jump());
        out.push(f("graph.depth_less", "Show fewer hops", G::Depth(-1)).toolbar(Button, "", Some(egui_phosphor::regular::MINUS)).group(Group::View).importance(MINOR).jump());
        out.push(f("graph.depth_more", "Show more hops", G::Depth(1)).toolbar(Button, "", Some(egui_phosphor::regular::PLUS)).group(Group::View).importance(MINOR).jump());
        if !self.nav.roots.is_empty() {
            out.push(f("graph.unfocus", "Back to the view's entry points", G::Unfocus).toolbar(Button, "All", Some(egui_phosphor::regular::ARROWS_OUT)).group(Group::View).jump());
        }
        out.push(f("graph.fit", "Fit to screen", G::Fit).toolbar(Button, "", Some(egui_phosphor::regular::CORNERS_OUT)).group(Group::View).importance(MINOR));
        out.push(f("graph.zoom_in", "Zoom in", G::Zoom(1.2)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_PLUS).group(Group::View).importance(MINOR));
        out.push(f("graph.zoom_out", "Zoom out", G::Zoom(1.0 / 1.2)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_MINUS).group(Group::View).importance(MINOR));
        out.push(f("graph.refresh", "Refresh the view", G::Refresh).group(Group::View).importance(MINOR));
        out.push(f("graph.legend", if self.legend_open { "Hide the legend" } else { "Show the legend" }, G::Legend).toolbar(Button, "", Some(egui_phosphor::regular::INFO)).group(Group::View).importance(MINOR));
        if self.agent.is_some() {
            out.push(f("graph.agent_return", "Return to where you were before the agent moved the view", G::AgentReturn).group(Group::Navigate).importance(PRIMARY).jump());
        }
        if self.selection.is_none() {
            out.push(f("graph.next_call", "Next call, in call order", G::Call { forward: true }).group(Group::Navigate).importance(MINOR).jump());
            out.push(f("graph.prev_call", "Previous call, in call order", G::Call { forward: false }).group(Group::Navigate).importance(MINOR).jump());
        }
        if self.selection.is_some() {
            out.push(f("graph.focus", "Focus on the selection", G::Focus).group(Group::View).jump());
            out.push(f("graph.close_inspector", "Close the inspector", G::CloseInspector).group(Group::View).importance(MINOR));
            // Always offered: when there's nothing to step through, it says why.
            let has_calls = self.selection.as_ref().is_some_and(|s| !self.calls_of(s).is_empty()) || self.call_walk.as_ref().is_some_and(|(caller, at)| self.calls_of(caller).get(*at) == self.selection.as_ref());
            out.push(f("graph.next_call", "Next call, in call order", G::Call { forward: true }).group(Group::Navigate).importance(if has_calls { SECONDARY } else { MINOR }).jump());
            out.push(f("graph.prev_call", "Previous call, in call order", G::Call { forward: false }).group(Group::Navigate).importance(MINOR).jump());
        }
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        let Some(command) = action.command::<GraphCommand>().cloned() else { return };
        let base = self.base_depth();
        match command {
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
                    let at = names.iter().position(|n| **n == self.nav.view).unwrap_or(0);
                    let next = names[(at + 1) % names.len()].clone();
                    self.navigate(NavChange::View(next));
                }
            }
            GraphCommand::SetPlan => {
                let plan = value.filter(|v| !v.is_empty()).and_then(|v| PlanId::new(&v).ok());
                self.navigate(NavChange::Plan(plan));
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
            GraphCommand::Legend => {
                self.legend_open = match value.as_deref() {
                    Some("true") => true,
                    Some("false") => false,
                    _ => !self.legend_open,
                };
            }
            GraphCommand::AgentReturn => {
                if let Some(agent) = self.agent.take() {
                    self.navigate(NavChange::Restore { nav: Box::new(agent.return_to), selection: agent.return_selection });
                }
            }
            GraphCommand::Call { forward } => {
                if let Err(why) = self.step_call(forward) {
                    *cx.note = Some(why);
                }
            }
        }
    }
}

impl Navigable for GraphState {
    fn place(&self) -> Place {
        Place::Graph { nav: self.nav.clone(), selection: self.selection.clone() }
    }

    fn restore(&mut self, place: &Place) {
        if let Place::Graph { nav, selection } = place {
            self.navigate(NavChange::Restore { nav: Box::new(nav.clone()), selection: selection.clone() });
        }
    }

    fn target(&self) -> Option<Target> {
        self.selection.clone().map(Target::Component)
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let dir = match motion {
            Motion::Left => Some(Dir::Left),
            Motion::Down => Some(Dir::Down),
            Motion::Up => Some(Dir::Up),
            Motion::Right => Some(Dir::Right),
            _ => None,
        };
        if let Some(dir) = dir {
            for _ in 0..count {
                self.move_selection(dir);
            }
            return;
        }
        match motion {
            Motion::HalfDown => self.half_page(true, count),
            Motion::HalfUp => self.half_page(false, count),
            Motion::First => self.jump_to_index(Some(0)),
            Motion::Last => self.jump_to_index(None),
            Motion::Line(n) => self.jump_to_index(Some(n.saturating_sub(1))),
            Motion::NextSibling => self.step_sibling(true, count),
            Motion::PrevSibling => self.step_sibling(false, count),
            Motion::Left | Motion::Down | Motion::Up | Motion::Right => {}
        }
    }

    fn enter(&mut self) -> Step {
        match self.selection.clone() {
            Some(id) => {
                self.navigate(NavChange::Focus(id));
                Step::Done
            }
            None => Step::Nothing("Select a node first.".into()),
        }
    }

    fn can_enter(&self) -> bool {
        self.selection.as_ref().is_some_and(|s| self.nav.roots != [s.clone()])
    }

    fn can_leave(&self) -> bool {
        !self.nav.roots.is_empty()
    }

    fn enter_label(&self) -> Option<String> {
        self.selected().map(|c| format!("Focus on {}", c.name))
    }

    fn leave(&mut self) -> Step {
        if self.nav.roots.is_empty() {
            return Step::Nothing("Already showing the whole view.".into());
        }
        self.navigate(NavChange::Unfocus);
        Step::Done
    }

    fn fold(&mut self, fold: Fold) {
        let Some(result) = &self.result else { return };
        let containers: Vec<ComponentId> = result.layout.containers.iter().filter(|c| Some(*c) != self.frame.as_ref()).cloned().collect();
        match fold {
            Fold::OpenAll => {
                for c in containers {
                    self.set_fold(c, true);
                }
            }
            Fold::CloseAll => {
                for c in containers {
                    self.set_fold(c, false);
                }
            }
            Fold::Open | Fold::Close | Fold::Toggle => {
                let Some(c) = self.fold_target() else { return };
                let open = match fold {
                    Fold::Open => true,
                    Fold::Close => false,
                    _ => self.closed(&c),
                };
                // Opened while zoomed out too far to read what's inside:
                // bring the module up to fill the view first.
                if open
                    && self.collapsed()
                    && let Some(rect) = self.result.as_ref().and_then(|r| r.layout.rects.get(&c)).copied()
                {
                    self.fit_rects(&[rect]);
                }
                self.set_fold(c, open);
            }
        }
    }

    fn items(&self) -> Vec<Item> {
        let Some(result) = &self.result else { return Vec::new() };
        result
            .view
            .nodes
            .iter()
            .map(|n| {
                let symbol = n.id.as_str().split_once(':').map_or(n.id.as_str(), |(_, symbol)| symbol);
                let name = result.components.get(&n.id).map(|c| c.name.clone()).unwrap_or_default();
                Item::new(n.id.as_str(), name, symbol)
            })
            .collect()
    }

    /// Drawn nodes on screen, nearest the selection (or the middle) first.
    fn spots(&self) -> Vec<Spot> {
        let Some(result) = &self.result else { return Vec::new() };
        let anchor = self.selected_rect().map(|r| r.center()).unwrap_or(self.canvas.center());
        let mut visible: Vec<(f32, Spot)> = result
            .layout
            .rects
            .iter()
            .filter(|(id, _)| self.shown(id))
            .map(|(id, r)| (self.to_screen(r), id))
            .filter(|(rect, _)| rect.intersects(self.open_area))
            // The part on screen: a label never sits over the inspector.
            .map(|(rect, id)| (rect.center().distance(anchor), Spot { key: id.to_string(), rect: rect.intersect(self.open_area) }))
            .collect();
        visible.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.key.cmp(&b.1.key)));
        visible.into_iter().map(|(_, spot)| spot).collect()
    }

    fn cursor_key(&self) -> Option<String> {
        self.selection.as_ref().map(|s| s.to_string())
    }

    fn save_view(&mut self) {
        self.saved_view = Some((self.camera, self.folds.clone(), self.drawer_open));
    }

    fn restore_view(&mut self) {
        if let Some((camera, folds, drawer_open)) = self.saved_view.take() {
            self.camera = camera;
            self.folds = folds;
            self.drawer_open = drawer_open;
        }
    }

    fn move_to(&mut self, key: &str) {
        let id = ComponentId::new(key);
        if self.component(&id).is_some() {
            self.bring_into_view(&id);
            self.select(Some(id));
        }
    }
}

/// `text` as it fits in `width`: whole, shortened with an ellipsis, or not
/// at all when not even one letter fits.
fn fit_text(painter: &egui::Painter, text: &str, font: &FontId, width: f32) -> Option<String> {
    let fits = |t: &str| painter.layout_no_wrap(t.to_owned(), font.clone(), colors().text).size().x <= width;
    if fits(text) {
        return Some(text.to_owned());
    }
    let chars: Vec<char> = text.chars().collect();
    (1..chars.len()).rev().map(|n| format!("{}…", chars[..n].iter().collect::<String>())).find(|t| fits(t))
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
        let mut g = GraphState { catalog: Some(ViewCatalog::defaults().unwrap()), catalog_requested: true, plans_stale: false, stale: false, ..Default::default() };
        g.set_result(result);
        g
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
        let edge = |a: &str, b: &str| ViewEdge { id: arch_graph::RelationId(format!("calls:{a}->{b}")), kind: RelationKind::Calls, source: id(a), target: id(b), alignment: Alignment::Observed, overlay: false, order: None };
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

    /// The sample inside a module `rust:a` (a container holding all four),
    /// plus a top-level module `rust:b` to its right.
    fn nested_result() -> ViewResult {
        let mut r = sample_result();
        for (id, x) in [("rust:a", -20.0), ("rust:b", 1000.0)] {
            let mut c = r.components[&ComponentId::new("rust:a::run")].clone();
            c.id = ComponentId::new(id);
            c.name = id.trim_start_matches("rust:").into();
            c.type_name = "module".into();
            r.components.insert(c.id.clone(), c);
            r.view.nodes.push(ViewNode { id: ComponentId::new(id), depth: Some(0), truncated: false, alignment: Alignment::Observed });
            let w = if id == "rust:a" { 900.0 } else { 220.0 };
            r.layout.rects.insert(ComponentId::new(id), WorldRect { x, y: -40.0, w, h: 240.0 });
        }
        r.layout.containers.insert(ComponentId::new("rust:a"));
        r
    }

    // req: P9.1
    #[test]
    fn a_new_view_is_fitted_when_it_arrives_not_on_the_old_one() {
        let mut g = showing(sample_result());
        g.fit = false;
        g.navigate(NavChange::Focus(ComponentId::new("rust:a::step")));
        assert!(!g.fit, "the view on screen is still the old one: don't fit it");
        assert!(g.refit && g.stale, "fit once the new view is in");
        g.set_result(sample_result());
        assert!(g.fit && !g.refit, "the new view gets fitted");
        // A refresh of the same view keeps the camera.
        g.fit = false;
        g.on_event(&ArchEvent::Changed { reason: ChangeReason::Ingested, plans: vec![], components: vec![] });
        g.set_result(sample_result());
        assert!(!g.fit, "same view, refreshed: camera stays");
    }

    // req: P9.4, P9.14
    #[test]
    fn moves_searches_and_jumps_only_land_on_what_can_be_seen() {
        let mut g = showing(sample_result());
        g.set_result(nested_result());
        g.camera.zoom = 0.1;
        assert!(g.collapsed() && !g.shown(&ComponentId::new("rust:a::save")), "zoomed out: the module is one block");
        // Moving goes between what's drawn: the module, then the module beside it.
        g.select(Some(ComponentId::new("rust:a")));
        g.move_selection(Dir::Right);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:b", "not into the collapsed module");
        // A focused module (the only box at the top) stays open zoomed out:
        // its parts show as blocks, and moves never jump out to it.
        let mut only_a = nested_result();
        only_a.layout.rects.remove(&ComponentId::new("rust:b"));
        only_a.view.nodes.retain(|n| n.id.as_str() != "rust:b");
        let mut f = showing(only_a);
        f.camera.zoom = 0.1;
        assert!(f.collapsed() && f.shown(&ComponentId::new("rust:a::save")), "the focused module's parts are drawn");
        f.select(Some(ComponentId::new("rust:a::save")));
        for dir in [Dir::Left, Dir::Right, Dir::Up, Dir::Down] {
            f.move_selection(dir);
            assert_ne!(f.selection.as_ref().unwrap().as_str(), "rust:a", "never onto the module it sits in");
        }
        // A search hit inside a collapsed module opens it up.
        Navigable::move_to(&mut g, "rust:a::save");
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::save");
        assert!(g.shown(&ComponentId::new("rust:a::save")), "zoomed in so it's drawn");
        assert!(g.camera.zoom >= GraphState::open_zoom());
        // So does the first/last jump, and the agent's focus once its view is in.
        g.camera.zoom = 0.1;
        g.motion(Motion::First, 1);
        assert!(g.shown(g.selection.as_ref().unwrap()), "{:?}", g.selection);
        g.camera.zoom = 0.1;
        g.reveal(ComponentId::new("rust:a::log"));
        assert!(g.shown(&ComponentId::new("rust:a::log")));
    }

    // req: P9.3
    #[test]
    fn one_reducer_moves_the_view_and_the_selection() {
        let mut g = showing(sample_result());
        g.navigate(NavChange::View("calls".into()));
        assert!(g.stale, "a new view is fetched");
        g.stale = false;
        g.navigate(NavChange::Focus(ComponentId::new("rust:a::run")));
        assert!(g.stale && g.nav.roots == vec![ComponentId::new("rust:a::run")]);
        g.stale = false;
        g.navigate(NavChange::Depth { delta: 1, base: 3 });
        assert_eq!(g.nav.depth, Some(4));
        g.stale = false;
        g.navigate(NavChange::Focus(ComponentId::new("rust:a::run")));
        assert!(!g.stale, "a change to where it already is fetches nothing");
        g.navigate(NavChange::Jump(ComponentId::new("rust:a::step")));
        assert_eq!((g.selection.as_ref().unwrap().as_str(), g.stale), ("rust:a::step", false), "a jump in the same view just selects");
        // Restoring a place (the jump list, marks, the agent's return) is the same reducer.
        let place = g.place();
        g.navigate(NavChange::Plan(Some(PlanId::new("p").unwrap())));
        g.navigate(NavChange::Unfocus);
        g.restore(&place);
        assert_eq!(g.place(), place);
        // Agent requests go through it too.
        g.navigate(NavChange::Request(Box::new(ViewRequest { view: Some("structure".into()), roots: vec![ComponentId::new("rust:b")], ..Default::default() })));
        assert_eq!((g.nav.view.as_str(), g.nav.roots.len()), ("structure", 1));
        g.navigate(NavChange::Depth { delta: -50, base: 3 });
        assert_eq!(g.nav.depth, Some(1), "clamped");
    }

    // req: P9.13, N.8
    #[test]
    fn hint_spots_are_the_drawn_nodes_nearest_first_and_open_focuses() {
        let mut g = GraphState { result: Some(sample_result()), canvas: Rect::from_min_size(Pos2::ZERO, Vec2::new(2000.0, 1000.0)), open_area: Rect::from_min_size(Pos2::ZERO, Vec2::new(2000.0, 1000.0)), ..Default::default() };
        g.set_result(sample_result());
        g.select(Some(ComponentId::new("rust:a::run")));
        let spots = g.spots();
        assert_eq!(spots.len(), 4);
        assert_eq!(spots[0].key, "rust:a::run", "nearest the selection first (it gets the first label)");
        let save = spots.iter().find(|s| s.key == "rust:a::save").unwrap();
        assert_eq!(save.rect, g.to_screen(&g.result.as_ref().unwrap().layout.rects[&ComponentId::new("rust:a::save")]));
        assert_eq!(g.open_item("rust:a::save"), Step::Done);
        assert_eq!((g.selection.as_ref().unwrap().as_str(), g.nav.roots.clone()), ("rust:a::save", vec![ComponentId::new("rust:a::save")]), "F focuses on it");
        // Off screen: no spot.
        g.camera.pan = Vec2::new(5000.0, 5000.0);
        assert!(g.spots().is_empty());
    }

    // req: P9.4, N.10
    #[test]
    fn graph_commands_are_typed_actions_that_move_focus_and_open() {
        let mut g = GraphState { result: Some(sample_result()), ..Default::default() };
        let run = |g: &mut GraphState, id: &str, value: Option<&str>| {
            let action = g.actions().into_iter().find(|a| a.id == id).unwrap_or_else(|| panic!("{id} offered"));
            let net = NetClient::new("http://127.0.0.1:9");
            let ctx = egui::Context::default();
            let mut mode = crate::keymap::InputMode::Normal;
            let mut note = None;
            g.run(&action, value.map(str::to_owned), &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[] });
            note
        };
        g.motion(Motion::Right, 1);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::run", "first move selects a root");
        g.motion(Motion::Right, 1);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::step");
        g.motion(Motion::Down, 1);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::log");
        assert_eq!(g.target(), Some(Target::Component(ComponentId::new("rust:a::log"))));
        // Enter focuses, Backspace goes back out.
        assert_eq!(g.enter(), Step::Done);
        assert_eq!(g.nav.roots[0].as_str(), "rust:a::log");
        assert_eq!(g.leave(), Step::Done);
        assert!(g.nav.roots.is_empty());
        assert!(matches!(g.leave(), Step::Nothing(_)), "already the whole view: says so");
        run(&mut g, "graph.focus", None);
        assert_eq!(g.nav.roots[0].as_str(), "rust:a::log");
        run(&mut g, "graph.depth_more", None);
        assert_eq!(g.nav.depth, Some(4));
        assert!(g.actions().iter().find(|a| a.id == "graph.depth_more").unwrap().jump, "view changes are jumps");
        run(&mut g, "graph.plan", Some("p1"));
        assert_eq!(g.nav.plan.as_ref().unwrap().0, "p1");
        run(&mut g, "graph.plan", Some(""));
        assert!(g.nav.plan.is_none(), "code only");
    }

    /// A layered view of calls in order: run calls step (0), log (1), save (2).
    fn ordered_calls() -> ViewResult {
        let mut r = sample_result();
        let edge = |a: &str, b: &str, order: u32| ViewEdge { id: arch_graph::RelationId(format!("calls:{a}->{b}")), kind: RelationKind::Calls, source: ComponentId::new(a), target: ComponentId::new(b), alignment: Alignment::Observed, overlay: false, order: Some(order) };
        r.view.edges = vec![edge("rust:a::run", "rust:a::save", 2), edge("rust:a::run", "rust:a::step", 0), edge("rust:a::run", "rust:a::log", 1)];
        r
    }

    // req: N.15
    #[test]
    fn calls_step_in_call_order_and_siblings_share_a_module() {
        let mut g = showing(ordered_calls());
        g.select(Some(ComponentId::new("rust:a::run")));
        let sel = |g: &GraphState| g.selection.as_ref().unwrap().to_string();
        assert!(g.step_call(true).is_ok());
        assert_eq!(sel(&g), "rust:a::step", "] e: the first call");
        g.step_call(true).unwrap();
        assert_eq!(sel(&g), "rust:a::log");
        g.step_call(true).unwrap();
        assert_eq!(sel(&g), "rust:a::save");
        assert!(g.step_call(true).unwrap_err().contains("last"), "says when it's the last");
        g.step_call(false).unwrap();
        assert_eq!(sel(&g), "rust:a::log", "[ e: back one");
        g.select(Some(ComponentId::new("rust:a::save")));
        assert!(g.step_call(true).unwrap_err().contains("calls nothing"), "no calls: it says so");
        // ]] / [[: siblings in reading order, in the same module.
        let mut n = showing(nested_result());
        n.camera.zoom = 1.0;
        n.select(Some(ComponentId::new("rust:a::run")));
        n.motion(Motion::NextSibling, 1);
        assert_eq!(n.selection.as_ref().unwrap().as_str(), "rust:a::step");
        n.motion(Motion::NextSibling, 2);
        assert_eq!(n.selection.as_ref().unwrap().as_str(), "rust:a::log", "run, step, save on top; log below");
        n.motion(Motion::NextSibling, 1);
        assert_eq!(n.selection.as_ref().unwrap().as_str(), "rust:a::log", "stays at the last");
        n.select(Some(ComponentId::new("rust:a")));
        n.motion(Motion::NextSibling, 1);
        assert_eq!(n.selection.as_ref().unwrap().as_str(), "rust:b", "modules are siblings of modules");
        // l prefers the box in the same row over a nearer one a row up.
        let mut rows = sample_result();
        rows.layout.rects.insert(ComponentId::new("rust:a::run"), WorldRect { x: 0.0, y: 100.0, w: 220.0, h: 64.0 });
        rows.layout.rects.insert(ComponentId::new("rust:a::step"), WorldRect { x: 260.0, y: 0.0, w: 220.0, h: 60.0 });
        rows.layout.rects.insert(ComponentId::new("rust:a::save"), WorldRect { x: 420.0, y: 110.0, w: 220.0, h: 64.0 });
        rows.layout.rects.insert(ComponentId::new("rust:a::log"), WorldRect { x: 0.0, y: 400.0, w: 220.0, h: 64.0 });
        let mut r = showing(rows);
        r.select(Some(ComponentId::new("rust:a::run")));
        r.motion(Motion::Right, 1);
        assert_eq!(r.selection.as_ref().unwrap().as_str(), "rust:a::save");
    }

    // req: N.5, N.10
    #[test]
    fn moves_go_into_a_module_and_backing_out_keeps_what_held_the_selection() {
        // Focused on the module: its parts are inside it, so moves go in.
        let mut only_a = nested_result();
        only_a.layout.rects.remove(&ComponentId::new("rust:b"));
        only_a.view.nodes.retain(|n| n.id.as_str() != "rust:b");
        let mut g = showing(only_a);
        g.camera.zoom = 1.0;
        g.select(Some(ComponentId::new("rust:a")));
        g.motion(Motion::Right, 1);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::run", "l from the module: its first part");
        // Backing out to a view that doesn't have the selection: what held it.
        let mut nested = nested_result();
        nested.view.edges.push(ViewEdge { id: arch_graph::RelationId("contains:a->log".into()), kind: RelationKind::Contains, source: ComponentId::new("rust:a"), target: ComponentId::new("rust:a::log"), alignment: Alignment::Observed, overlay: false, order: None });
        let mut g = showing(nested);
        g.navigate(NavChange::Focus(ComponentId::new("rust:a")));
        g.set_result(g.result.clone().unwrap());
        g.select(Some(ComponentId::new("rust:a::log")));
        assert_eq!(g.leave(), Step::Done);
        let mut whole = nested_result();
        whole.components.retain(|id, _| !id.as_str().contains("::"));
        whole.view.nodes.retain(|n| !n.id.as_str().contains("::"));
        whole.layout.rects.retain(|id, _| !id.as_str().contains("::"));
        g.set_result(whole);
        assert_eq!(g.selection.as_ref().map(|s| s.as_str()), Some("rust:a"), "the module that held it");
    }

    // req: N.11
    #[test]
    fn modules_fold_in_place_without_moving_the_view() {
        let mut g = showing(nested_result());
        g.camera.zoom = 1.0;
        let nav = g.nav.clone();
        g.select(Some(ComponentId::new("rust:a::step")));
        g.fold(Fold::Close);
        assert!(!g.shown(&ComponentId::new("rust:a::step")), "zc closes the module it's in");
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a", "the selection moves to the module");
        assert_eq!(g.nav, nav, "the view's root didn't change");
        g.fold(Fold::Toggle);
        assert!(g.shown(&ComponentId::new("rust:a::step")), "za opens it again");
        g.camera.zoom = 0.1;
        assert!(!g.shown(&ComponentId::new("rust:a::step")), "zoomed out it closes by itself");
        g.fold(Fold::Open);
        assert!(g.shown(&ComponentId::new("rust:a::step")), "zo opens it even zoomed out");
        assert!(g.camera.zoom >= GraphState::open_zoom(), "and brings it up close enough to read");
        g.fold(Fold::CloseAll);
        assert!(!g.shown(&ComponentId::new("rust:a::save")));
        g.fold(Fold::OpenAll);
        assert!(g.shown(&ComponentId::new("rust:a::save")));
        // A jump into a closed module opens it.
        g.fold(Fold::CloseAll);
        g.camera.zoom = 1.0;
        Navigable::move_to(&mut g, "rust:a::log");
        assert!(g.shown(&ComponentId::new("rust:a::log")));
    }

    // req: N.6
    #[test]
    fn half_a_page_pans_and_selects_the_node_nearest_where_the_cursor_was() {
        let mut r = sample_result();
        // A tall column of nodes.
        for i in 0..20 {
            let id = ComponentId::new(format!("rust:a::n{i}"));
            let mut c = r.components[&ComponentId::new("rust:a::run")].clone();
            c.id = id.clone();
            c.name = format!("n{i}");
            r.components.insert(id.clone(), c);
            r.view.nodes.push(arch_graph::view::ViewNode { id: id.clone(), depth: Some(0), truncated: false, alignment: Alignment::Observed });
            r.layout.rects.insert(id, WorldRect { x: 0.0, y: 200.0 + i as f32 * 100.0, w: 220.0, h: 64.0 });
        }
        let mut g = showing(r);
        g.canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        g.open_area = g.canvas;
        g.camera = Camera { pan: Vec2::new(24.0, 24.0), zoom: 1.0 };
        g.select(Some(ComponentId::new("rust:a::run")));
        g.motion(Motion::HalfDown, 1);
        assert_eq!(g.camera.pan.y, 24.0 - 300.0, "half the viewport");
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::n1", "the node now where run was: {:?}", g.selection);
        g.motion(Motion::HalfUp, 1);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::run");
    }

    // req: P9.2
    #[test]
    fn architecture_events_refresh_and_keep_selection() {
        let mut g = GraphState { result: Some(sample_result()), stale: false, catalog: Some(ViewCatalog::defaults().unwrap()), ..Default::default() };
        g.select(Some(ComponentId::new("rust:a::step")));
        let camera = g.camera;
        // A check result isn't a change: no refetch (the spec view validates
        // on load; refetching on its result would loop).
        g.on_event(&ArchEvent::Validated { plan: arch_graph::PlanId::new("p").unwrap(), component: None, errors: 0, warnings: 0 });
        assert!(!g.stale, "no refetch on a check result");
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
        assert!(g.agent.is_some() && g.stale);
        g.set_result(sample_result());
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::save", "agent focus selected once shown");
        g.navigate(NavChange::Unfocus);
        assert!(g.agent.is_none(), "the user took over");
    }

    /// A graph showing the sample, set up from the default UI config.
    fn configured() -> GraphState {
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        crate::graph_style::set(config.graph_style.clone());
        crate::status::set(config.status.clone());
        let mut g = showing(sample_result());
        g.ui = config.graph;
        g
    }

    /// Draws one frame of the graph alone at `size`, at `time` seconds.
    fn draw(g: &mut GraphState, ctx: &egui::Context, events: Vec<egui::Event>, time: f64, size: Vec2) -> egui::FullOutput {
        let net = NetClient::new("http://127.0.0.1:9");
        let mut mode = crate::keymap::InputMode::Normal;
        let mut feedback = FeedbackInspectorState::default();
        let mut note = None;
        // The shell's actions the chips and toolbar use.
        let shell = crate::app::CockpitApp::new("http://127.0.0.1:9").shell_actions();
        ctx.run(egui::RawInput { events, time: Some(time), screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                g.render(ui, &mut feedback, 0, false, &mut RunCx { net: &net, ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &shell });
            });
        })
    }

    fn texts(output: &egui::FullOutput) -> String {
        fn collect(shape: &Shape, out: &mut String) {
            match shape {
                Shape::Text(t) => {
                    out.push_str(t.galley.text());
                    out.push('\n');
                }
                Shape::Vec(list) => list.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let mut out = String::new();
        output.shapes.iter().for_each(|c| collect(&c.shape, &mut out));
        out
    }

    fn touch(id: u64, phase: egui::TouchPhase, pos: Pos2) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(id), phase, pos, force: None }
    }

    fn click(pos: Pos2, pressed: bool) -> Vec<egui::Event> {
        let button = egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        if pressed { vec![egui::Event::PointerMoved(pos), button] } else { vec![button] }
    }

    fn center_of(g: &GraphState, id: &str) -> Pos2 {
        let r = g.result.as_ref().unwrap().layout.rects[&ComponentId::new(id)];
        g.to_screen(&r).center()
    }

    fn chip(g: &GraphState, action: &str) -> Rect {
        g.chip_rects.iter().find(|(id, _)| id == action).map(|(_, r)| *r).unwrap_or_else(|| panic!("chip {action} shown: {:?}", g.chip_rects))
    }

    // req: P9.6
    #[test]
    fn touch_gestures_and_chips_drive_the_graph_at_phone_size() {
        use egui::TouchPhase::{End, Move, Start};
        let ctx = egui::Context::default();
        let phone = Vec2::new(390.0, 844.0);
        let mut g = configured();
        let mut t = 0.0;
        let mut frame = |g: &mut GraphState, events: Vec<egui::Event>, dt: f64| {
            t += dt;
            draw(g, &ctx, events, t, phone)
        };
        frame(&mut g, vec![], 0.0);
        // Chips show on a narrow screen: big enough to touch, inside the screen.
        let size = g.ui.chips.size;
        assert_eq!(g.chip_rects.len(), g.ui.chips.actions.len(), "{:?}", g.chip_rects);
        for (id, r) in &g.chip_rects {
            assert!(r.height() >= size && r.width() >= size && r.max.x <= phone.x, "{id}: {r:?}");
        }
        // Tap selects.
        let step = center_of(&g, "rust:a::step");
        frame(&mut g, vec![touch(1, Start, step)], 0.1);
        frame(&mut g, vec![touch(1, End, step)], 0.05);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::step");
        // Double tap focuses.
        let save = center_of(&g, "rust:a::save");
        frame(&mut g, vec![touch(2, Start, save)], 1.0);
        frame(&mut g, vec![touch(2, End, save)], 0.05);
        frame(&mut g, vec![touch(3, Start, save)], 0.1);
        frame(&mut g, vec![touch(3, End, save)], 0.05);
        assert_eq!(g.nav.roots, vec![ComponentId::new("rust:a::save")]);
        assert!(g.jumped.take().is_some(), "a double tap is a jump (the shell records it)");
        // Swipe right goes back, left goes forward (the shell's jump list).
        let mid = Pos2::new(120.0, 500.0);
        frame(&mut g, vec![touch(4, Start, mid)], 1.0);
        frame(&mut g, vec![touch(4, Move, mid + Vec2::new(60.0, 0.0))], 0.05);
        frame(&mut g, vec![touch(4, End, mid + Vec2::new(150.0, 4.0))], 0.05);
        assert_eq!(g.requests.run.take(), Some(("nav.back".into(), None)), "back");
        frame(&mut g, vec![touch(5, Start, mid + Vec2::new(150.0, 0.0))], 1.0);
        frame(&mut g, vec![touch(5, End, mid - Vec2::new(10.0, 0.0))], 0.1);
        assert_eq!(g.requests.run.take(), Some(("nav.forward".into(), None)), "forward");
        g.navigate(NavChange::Unfocus);
        g.set_result(sample_result());
        frame(&mut g, vec![], 0.1);
        // Long press opens the node's actions as chips; tapping one runs it.
        let log = center_of(&g, "rust:a::log");
        frame(&mut g, vec![touch(6, Start, log)], 1.0);
        let hold = g.ui.touch.long_press_ms / 1000.0 + 0.05;
        frame(&mut g, vec![], hold);
        assert_eq!(g.context.as_ref().map(|(id, _)| id.as_str()), Some("rust:a::log"));
        frame(&mut g, vec![touch(6, End, log)], 0.1);
        assert!(g.context.is_some(), "lifting doesn't close it");
        let spec = chip(&g, "goto.spec").center();
        frame(&mut g, click(spec, true), 0.1);
        frame(&mut g, click(spec, false), 0.05);
        assert!(g.requests.run.take() == Some(("goto.spec".into(), None)) && g.context.is_none(), "the shell's go-to runs from the chip");
        // Two fingers: pan together, pinch apart to zoom.
        let (zoom, pan) = (g.camera.zoom, g.camera.pan);
        let (a, b) = (Pos2::new(150.0, 400.0), Pos2::new(250.0, 400.0));
        frame(&mut g, vec![touch(7, Start, a), touch(8, Start, b)], 1.0);
        frame(&mut g, vec![touch(7, Move, a + Vec2::new(30.0, 0.0)), touch(8, Move, b + Vec2::new(30.0, 0.0))], 0.05);
        assert!((g.camera.pan.x - pan.x - 30.0).abs() < 0.5 && (g.camera.zoom - zoom).abs() < 1e-4, "two-finger pan: {:?} -> {:?}", pan, g.camera.pan);
        frame(&mut g, vec![touch(7, Move, a - Vec2::new(20.0, 0.0)), touch(8, Move, b + Vec2::new(80.0, 0.0))], 0.05);
        assert!(g.camera.zoom > zoom * 1.5, "pinch out zooms in: {zoom} -> {}", g.camera.zoom);
        frame(&mut g, vec![touch(7, End, a), touch(8, End, b)], 0.05);
        assert_eq!((g.nav.roots.len(), g.requests.run.take()), (0, None), "no tap or swipe from a two-finger gesture");
        // Chips run the same actions as keys.
        let zoomed = g.camera.zoom;
        let zoom_out = chip(&g, "graph.zoom_out").center();
        frame(&mut g, click(zoom_out, true), 0.3);
        frame(&mut g, click(zoom_out, false), 0.05);
        assert!(g.camera.zoom < zoomed);
        let back = chip(&g, "nav.back").center();
        frame(&mut g, click(back, true), 0.3);
        frame(&mut g, click(back, false), 0.05);
        assert_eq!(g.requests.run.take(), Some(("nav.back".into(), None)), "back chip");
        // Wide screens don't show them until a finger touches the graph.
        let mut wide = configured();
        draw(&mut wide, &egui::Context::default(), vec![], 0.0, Vec2::new(1600.0, 900.0));
        assert!(wide.chip_rects.is_empty());
        wide.ui.chips.show = ChipMode::Always;
        draw(&mut wide, &egui::Context::default(), vec![], 0.1, Vec2::new(1600.0, 900.0));
        assert!(!wide.chip_rects.is_empty());
    }

    // req: P9.7
    #[test]
    fn agent_requests_move_the_view_mark_it_and_back_returns() {
        let ctx = egui::Context::default();
        let mut g = configured();
        draw(&mut g, &ctx, vec![], 0.0, Vec2::new(1200.0, 800.0));
        g.select(Some(ComponentId::new("rust:a::step")));
        let before = g.nav.clone();
        let request = |view: &str, focus: &str| ArchEvent::ViewRequested {
            request: Box::new(ViewRequest { view: Some(view.into()), roots: vec![ComponentId::new(focus)], focus: Some(ComponentId::new(focus)), ..Default::default() }),
            by: Requester::Agent,
        };
        g.on_event(&request("calls", "rust:a::save"));
        assert_eq!(g.nav.roots, vec![ComponentId::new("rust:a::save")], "moved through the reducer");
        assert_eq!(g.agent.as_ref().unwrap().what, "calls view · from save · at save");
        g.set_result(sample_result());
        let text = texts(&draw(&mut g, &ctx, vec![], 0.1, Vec2::new(1200.0, 800.0)));
        assert!(text.contains("the agent moved the view: calls view · from save · at save"), "{text}");
        assert!(g.actions().iter().any(|a| a.id == "graph.agent_return"));
        // A second move keeps where the user was.
        g.on_event(&request("structure", "rust:a::log"));
        assert_eq!(g.agent.as_ref().unwrap().return_to.roots, before.roots);
        // Return goes straight back to the user's place, selection included.
        let action = g.actions().into_iter().find(|a| a.id == "graph.agent_return").unwrap();
        let (net, ctx2, mut mode, mut note) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default(), crate::keymap::InputMode::Normal, None);
        g.run(&action, None, &mut RunCx { net: &net, ctx: &ctx2, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[] });
        assert!(g.nav == before && g.agent.is_none());
        g.set_result(sample_result());
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::step");
        // Plain back (ctrl+o, swipe, chip) restores the place the jump list
        // holds, and that clears the mark.
        let place = g.place();
        g.on_event(&request("calls", "rust:a::save"));
        g.restore(&place);
        assert!(g.agent.is_none() && g.nav == before);
        // User requests (e.g. from REST) move without the agent mark.
        g.on_event(&ArchEvent::ViewRequested { request: Box::new(ViewRequest { view: Some("calls".into()), ..Default::default() }), by: Requester::User });
        assert!(g.agent.is_none());
    }

    // req: P9.10
    #[test]
    fn styles_draw_from_the_table_with_a_matching_legend() {
        use crate::graph_style::{LegendEntry, Swatch};
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        let (style, theme) = (&config.graph_style, &config.theme);
        crate::status::set(config.status.clone());
        let legend = style.legend(theme);
        let find = |section: &str, label: &str| legend.iter().find(|e: &&LegendEntry| e.section == section && e.label == label).map(|e| e.swatch);
        for kind in RelationKind::ALL {
            assert_eq!(find("Relations", &kind.name().replace('_', " ")), Some(Swatch::Line(style.edge(theme, kind, arch_graph::Provenance::OBSERVED, Alignment::Observed, false))));
        }
        for alignment in [Alignment::Observed, Alignment::Planned, Alignment::Missing, Alignment::Present] {
            let label = crate::graph_style::alignment_label(alignment);
            let label = label.as_str();
            assert_eq!(find("Plan vs code", label), Some(Swatch::Line(style.edge(theme, RelationKind::Calls, provenance_of(alignment), alignment, false))));
            assert_eq!(find("Nodes", label), Some(Swatch::Outline(style.node_outline(theme, alignment, false))));
        }
        for stage in arch_graph::Stage::ALL {
            assert_eq!(find("Stage", &crate::status::look(crate::status::stage(stage)).1), Some(Swatch::Stripe(style.stage_color(theme, stage))));
        }
        assert!(config.keymap.bind.contains_key("graph.legend") && config.graph.chips.actions.iter().any(|a| a == "graph.legend"), "a key and a chip");
        // Toggling shows exactly the table's entries.
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        let mut g = configured();
        let text = texts(&draw(&mut g, &ctx, vec![], 0.0, Vec2::new(1200.0, 800.0)));
        assert!(!text.contains("planned, missing in code"));
        let toggle = g.actions().into_iter().find(|a| a.id == "graph.legend").unwrap();
        let (net, mut mode, mut note) = (NetClient::new("http://127.0.0.1:9"), crate::keymap::InputMode::Normal, None);
        g.run(&toggle, None, &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[] });
        let out = draw(&mut g, &ctx, vec![], 0.1, Vec2::new(1200.0, 800.0));
        let text = texts(&out);
        for entry in &legend {
            assert!(text.contains(&entry.label), "legend shows {}: {text}", entry.label);
        }
        // Nodes carry their stage stripe in the stage's color.
        let mut fills = Vec::new();
        fn rects(shape: &Shape, out: &mut Vec<egui::Color32>) {
            match shape {
                Shape::Rect(r) => out.push(r.fill),
                Shape::Vec(list) => list.iter().for_each(|s| rects(s, out)),
                _ => {}
            }
        }
        out.shapes.iter().for_each(|c| rects(&c.shape, &mut fills));
        assert!(fills.contains(&style.stage_color(theme, arch_graph::Stage::Implemented)), "implemented stripe drawn");
        g.run(&toggle, None, &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[] });
        assert!(!g.legend_open);
    }

    // req: P9.17
    #[test]
    fn node_cards_show_more_detail_at_closer_zoom_per_config() {
        let mut g = configured();
        let mut result = sample_result();
        let step = ComponentId::new("rust:a::step");
        {
            let c = result.components.get_mut(&step).unwrap();
            c.signature = Some(arch_graph::Signature { params: vec![arch_graph::Param { name: "input".into(), ty: Some("&str".into()), optional: false, variadic: false }], returns: Some("bool".into()), qualifiers: vec![] });
            c.placeholders.push(arch_graph::Span { start_line: 2, end_line: 2, start_byte: 0, end_byte: 1 });
        }
        g.set_result(result);
        g.marks.insert(step.clone(), crate::review::NodeMark { changed: true, open: 2 });
        let c = g.component(&step).unwrap().clone();
        let texts_at = |g: &GraphState, zoom: f32| -> Vec<String> {
            let mut g2 = GraphState { camera: Camera { zoom, ..g.camera }, ..showing(g.result.clone().unwrap()) };
            g2.ui = g.ui.clone();
            g2.marks = g.marks.clone();
            g2.card_lines(&c, Alignment::Present, g2.detail()).into_iter().map(|l| l.text).collect()
        };
        let (mid, near) = (g.ui.detail.mid, g.ui.detail.near);
        assert_eq!(texts_at(&g, mid * 0.9), vec!["step"]);
        // States read as icon and word, from the status vocabulary.
        let state = |name: &str, count: Option<usize>| {
            let (glyph, word, _) = crate::status::look(name);
            count.map_or_else(|| format!("{glyph} {word}"), |n| format!("{glyph} {n} {word}"))
        };
        let implemented = state("stage_implemented", None);
        assert_eq!(texts_at(&g, mid), vec!["step".to_owned(), format!("function · {implemented} · {}", state("placeholders", Some(1))), "unplaced".to_owned()]);
        assert_eq!(
            texts_at(&g, near),
            vec![
                "step".to_owned(),
                format!("function · {implemented} · {}", state("present", None)),
                "(input: &str) -> bool".to_owned(),
                "unplaced".to_owned(),
                format!("{} · {} · {}", state("placeholders", Some(1)), state("open_comments", Some(2)), state("changed", None)),
            ]
        );
        assert!(implemented.contains("implemented") && state("present", None).contains("planned and in code"), "words from config");
        // Thresholds are config (and must make sense).
        assert!(crate::config::UiConfig::with_overrides("[graph.detail]\nmid = 1.0\nnear = 0.5").unwrap_err().contains("graph.detail"));
        g.ui.detail.near = 5.0;
        assert_eq!(texts_at(&g, 1.0).len(), 3, "near now needs zoom 5");
        // What's computed is what's drawn.
        let ctx = egui::Context::default();
        let mut close = configured();
        close.set_result(g.result.clone().unwrap());
        close.marks = g.marks.clone();
        draw(&mut close, &ctx, vec![], 0.0, Vec2::new(1400.0, 800.0));
        close.camera.zoom = close.ui.detail.near + 0.1;
        let text = texts(&draw(&mut close, &ctx, vec![], 0.1, Vec2::new(1400.0, 800.0)));
        assert!(text.contains("(input: &str) -> bool") && text.contains(&format!("2 {}", crate::status::look("open_comments").1)), "{text}");
    }

    // req: V.6
    #[test]
    fn the_graph_has_a_grid_a_glowing_selection_a_pulse_and_moving_dashes() {
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        crate::theme::set_look(config.look.clone());
        let theme = colors();
        let size = Vec2::new(1400.0, 800.0);
        fn shapes(out: &egui::FullOutput) -> Vec<Shape> {
            fn flat(shape: &Shape, into: &mut Vec<Shape>) {
                match shape {
                    Shape::Vec(list) => list.iter().for_each(|s| flat(s, into)),
                    other => into.push(other.clone()),
                }
            }
            let mut all = Vec::new();
            out.shapes.iter().for_each(|c| flat(&c.shape, &mut all));
            all
        }
        let wants_frame = |out: &egui::FullOutput| out.viewport_output.values().any(|v| v.repaint_delay.is_zero());
        let strokes_in = |all: &[Shape], color: egui::Color32| all.iter().filter(|s| matches!(s, Shape::Rect(r) if r.stroke.color == color)).count();
        let mut result = sample_result();
        result.view.edges[2].alignment = Alignment::Planned;
        let mut g = configured();
        g.set_result(result);
        draw(&mut g, &ctx, vec![], 0.0, size);

        // A faint grid, in its own colour, under everything.
        let still = draw(&mut g, &ctx, vec![], 0.1, size);
        let grid = shapes(&still).iter().filter(|s| matches!(s, Shape::LineSegment { stroke, .. } if stroke.color == theme.grid)).count();
        assert!(grid > 10, "grid lines: {grid}");

        // Planned connections' dashes move: they want frames and differ over time.
        assert!(wants_frame(&still), "moving dashes keep drawing");
        let later = draw(&mut g, &ctx, vec![], 0.4, size);
        let dashes = |all: Vec<Shape>| all.into_iter().filter_map(|s| if let Shape::LineSegment { points, stroke } = s { (stroke.color != theme.grid).then_some(points) } else { None }).collect::<Vec<_>>();
        assert_ne!(dashes(shapes(&still)), dashes(shapes(&later)), "the dashes moved");

        // The selection glows; the other lines fade back.
        let look = crate::theme::look();
        let ring = theme.focus.gamma_multiply(look.glow_alpha * (1.0 - 1.0 / 4.0));
        assert_eq!(strokes_in(&shapes(&later), ring), 0, "no glow without a selection");
        g.select(Some(ComponentId::new("rust:a::save")));
        let selected = shapes(&draw(&mut g, &ctx, vec![], 0.5, size));
        assert_eq!(strokes_in(&selected, ring), 1, "the selected card glows");
        let solid = style().edge(&theme, RelationKind::Calls, provenance_of(Alignment::Observed), Alignment::Observed, false).color;
        let lines: Vec<egui::Color32> = selected.iter().filter_map(|s| match s {
            Shape::LineSegment { stroke, .. } if stroke.color != theme.grid => Some(stroke.color),
            Shape::Path(p) => match p.stroke.color {
                egui::epaint::ColorMode::Solid(c) => Some(c),
                _ => None,
            },
            _ => None,
        }).collect();
        assert!(lines.contains(&solid), "the selection's own line in full");
        assert!(lines.contains(&solid.gamma_multiply(1.0 - look.fade)), "a line away from it faded: {lines:?}");

        // Where the agent points: a pulse, which keeps drawing even with still dashes.
        let mut quiet = (*look).clone();
        quiet.dash_speed = 0.0;
        crate::theme::set_look(quiet);
        g.select(None);
        g.agent = Some(AgentMove { what: "reading".into(), focus: Some(ComponentId::new("rust:a::step")), return_to: g.nav.clone(), return_selection: None });
        draw(&mut g, &ctx, vec![], 1.0, size);
        let t = 1.0 + f64::from(look.pulse_seconds) / 4.0;
        let out = draw(&mut g, &ctx, vec![], t, size);
        assert!(wants_frame(&out), "the pulse keeps drawing");
        let agent = style().agent_color(&theme);
        let beat = (t as f32 % look.pulse_seconds) / look.pulse_seconds;
        let width_of = |all: &[Shape], color: egui::Color32| all.iter().find_map(|s| match s {
            Shape::Rect(r) if r.stroke.color == color => Some(r.rect.width()),
            _ => None,
        });
        let all = shapes(&out);
        let (ring, pulse) = (width_of(&all, agent).expect("the agent's ring"), width_of(&all, agent.gamma_multiply(1.0 - beat)).expect("the pulse"));
        assert!(pulse > ring, "the pulse goes out from the card: {pulse} > {ring}");
        g.agent = None;
        draw(&mut g, &ctx, vec![], 2.0, size);
        let calm = draw(&mut g, &ctx, vec![], 2.1, size);
        assert!(!wants_frame(&calm), "nothing moving, nothing redrawn");
        crate::theme::set_look((*look).clone());
    }
}
