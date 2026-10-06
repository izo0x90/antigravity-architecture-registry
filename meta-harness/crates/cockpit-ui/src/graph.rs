//! The architecture graph: server views drawn with pan and zoom, a navigation
//! history (view, plan, focus, depth and the node jumped to), vim-style
//! commands with counts, search, marks and hint targeting, touch gestures and
//! on-screen chips, a legend, semantic zoom and a compact inspector.
//! Everything shown comes from the server's views; the graph refreshes itself
//! when the architecture changes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use arch_graph::view::Rect as WorldRect;
use arch_graph::{Alignment, ArchEvent, ChangeReason, Component, ComponentId, PlanId, RelationKind, Requester, ViewCatalog, ViewRequest, ViewResult};
use egui::{Align2, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Vec2};

use crate::actions::{Action, ActionProvider, Choice, Param, RunCx, ToolbarStyle, ToolbarUse, toolbar_control};
use crate::feedback::FeedbackInspectorState;
use crate::graph_config::{ChipMode, GraphUi};
use crate::graph_style::{LegendEntry, Swatch, alignment_label, provenance_of, style};
use crate::keymap::InputMode;
use crate::net::{NetClient, NetResult};
use crate::theme::colors;
use crate::touch::{Gesture, Gestures, Swipe};

/// Where the graph is looking: which view, laid over which plan, rooted where,
/// how deep. Every change is a step in the history.
#[derive(Clone, Debug, PartialEq)]
pub struct Nav {
    pub view: String,
    pub plan: Option<PlanId>,
    pub roots: Vec<ComponentId>,
    /// None means the view's own depth.
    pub depth: Option<u32>,
    /// The node this step is at (where a jump landed, or what was selected
    /// when the next step was taken), so back returns to it.
    pub cursor: Option<ComponentId>,
}

impl Nav {
    /// Same view of the graph (a fetch would return the same thing).
    pub fn same_view(&self, other: &Nav) -> bool {
        self.view == other.view && self.plan == other.plan && self.roots == other.roots && self.depth == other.depth
    }
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
    /// Go to a node in the same view (search, marks, first/last).
    Jump(ComponentId),
    /// Return to a recorded place (a mark, or where the agent took over).
    Restore(Box<Nav>),
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
        Self { history: vec![Nav { view: view.to_owned(), plan: None, roots: Vec::new(), depth: None, cursor: None }], cursor: 0 }
    }

    /// Records the node the current step is at (before leaving it).
    pub fn remember(&mut self, at: Option<ComponentId>) {
        self.history[self.cursor].cursor = at;
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
            NavChange::Focus(id) => {
                next.roots = vec![id.clone()];
                next.cursor = Some(id);
            }
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
                if request.focus.is_some() {
                    next.cursor = request.focus;
                }
            }
            NavChange::Jump(id) => next.cursor = Some(id),
            NavChange::Restore(nav) => next = *nav,
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

/// The agent moved the view: what it showed, and where the user was.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentMove {
    pub what: String,
    pub focus: Option<ComponentId>,
    pub return_to: Nav,
}

/// What typed keys mean while the graph captures the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Capture {
    Search,
    MarkSet,
    MarkJump,
}

/// A search being typed: the query, and the selection to go back to.
#[derive(Clone, Debug, PartialEq)]
pub struct Search {
    pub query: String,
    before: Option<ComponentId>,
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
    pub nav: Navigator,
    pub result: Option<ViewResult>,
    /// Ids drawn inside some container (hidden when containers collapse).
    nested: BTreeSet<ComponentId>,
    /// The one container everything else sits in (a focused module): it
    /// stays open when zoomed out, so its parts show as blocks.
    frame: Option<ComponentId>,
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
    pub hint: Option<Hint>,
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
    capture: Option<Capture>,
    pub search: Option<Search>,
    last_query: String,
    /// Marks by letter: a place in the history to jump back to.
    bookmarks: BTreeMap<char, Nav>,
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
            frame: None,
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
            hint: None,
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
            capture: None,
            search: None,
            last_query: String::new(),
            bookmarks: BTreeMap::new(),
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

    /// Navigates through the history reducer: the view is fetched again when
    /// it changed, and the selection follows the step's node.
    pub fn navigate(&mut self, change: NavChange) {
        let user = !matches!(change, NavChange::Request(_));
        let before = self.nav.current().clone();
        self.nav.remember(self.selection.clone());
        if self.nav.apply(change) {
            let now = self.nav.current().clone();
            let refetch = !now.same_view(&before);
            if refetch {
                self.stale = true;
                self.refit = true;
            }
            if let Some(at) = now.cursor {
                if !refetch && self.component(&at).is_some() {
                    self.select(Some(at.clone()));
                    self.bring_into_view(&at);
                } else {
                    self.pending_select = Some(at);
                }
            }
        }
        if user {
            self.agent = None;
        }
    }

    /// The least zoom at which containers open (their contents are drawn).
    fn open_zoom() -> f32 {
        11.0 / arch_graph::view::layout::HEADER * 1.05
    }

    /// Makes `id` visible: zooms in when it's inside a collapsed container,
    /// and pans so it's on screen (centered) when it isn't.
    fn bring_into_view(&mut self, id: &ComponentId) {
        let Some(r) = self.result.as_ref().and_then(|r| r.layout.rects.get(id)).copied() else { return };
        if !self.shown(id) {
            self.camera.zoom = Self::open_zoom().max(self.camera.zoom);
            let center = Vec2::new(r.x + r.w / 2.0, r.y + r.h / 2.0) * self.camera.zoom;
            self.camera.pan = (self.open_area.center() - self.canvas.min) - center;
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
    }

    /// Selects `id`, refocusing the view on it when it isn't shown.
    pub fn reveal(&mut self, id: ComponentId) {
        if self.component(&id).is_some() {
            self.bring_into_view(&id);
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
            // A check result changes no shape on screen (and refetching on it
            // would echo the spec view's own validation forever).
            ArchEvent::Validated { .. } => {}
            // Stages show on nodes; the queue panel follows the rest.
            ArchEvent::QueueUpdated { .. } => {}
            ArchEvent::ViewRequested { request, by } => {
                if *by == Requester::Agent {
                    let mut return_to = self.agent.as_ref().map(|a| a.return_to.clone()).unwrap_or_else(|| self.nav.current().clone());
                    if self.agent.is_none() {
                        return_to.cursor = self.selection.clone();
                    }
                    self.agent = Some(AgentMove { what: self.describe(request), focus: request.focus.clone(), return_to });
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
        let mut parts = vec![format!("{} view", request.view.as_deref().unwrap_or(&self.nav.current().view))];
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
        if let Some(sel) = &self.selection
            && !result.components.contains_key(sel)
        {
            self.selection = None;
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
        self.nested = result
            .layout
            .containers
            .iter()
            .filter(|c| Some(*c) != self.frame.as_ref())
            .filter_map(|c| result.layout.rects.get(c))
            .flat_map(|outer| result.layout.rects.iter().filter(move |(_, r)| *r != outer && outer.contains(r)).map(|(id, _)| id.clone()))
            .collect();
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
                        // The plan this step shows is gone (deleted): show the code instead.
                        Err(e) if self.nav.current().plan.as_ref().is_some_and(|p| !self.plans.contains(p)) && e.to_lowercase().contains("not found") => {
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
                (along > 1.0 && across.abs() <= along * 2.0).then_some((along + 2.0 * across.abs(), id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id.clone());
        if let Some(best) = best {
            self.bring_into_view(&best);
            self.select(Some(best));
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
        self.bring_into_view(&target);
        self.select(Some(target.clone()));
        match action {
            HintAction::Select => {}
            HintAction::Focus => self.navigate(NavChange::Focus(target)),
            HintAction::OpenCode => self.request_code(),
            HintAction::OpenSpec => self.requests.open_spec = true,
        }
    }

    // ---- search, marks, first/last ----

    /// Nodes in the view whose name or symbol (the id without its language)
    /// contains `query` (any case), in view order.
    pub fn matches(&self, query: &str) -> Vec<ComponentId> {
        let (Some(result), false) = (&self.result, query.is_empty()) else { return Vec::new() };
        let q = query.to_lowercase();
        // Best first: exact name, then name starting with it, then anywhere.
        let mut hits: Vec<(u8, ComponentId)> = result
            .view
            .nodes
            .iter()
            .filter_map(|n| {
                let symbol = n.id.as_str().split_once(':').map_or(n.id.as_str(), |(_, symbol)| symbol).to_lowercase();
                let name = result.components.get(&n.id).map(|c| c.name.to_lowercase()).unwrap_or_default();
                let rank = if name == q {
                    0
                } else if name.starts_with(&q) {
                    1
                } else if name.contains(&q) || symbol.contains(&q) {
                    2
                } else {
                    return None;
                };
                Some((rank, n.id.clone()))
            })
            .collect();
        // Ties keep the view's order.
        hits.sort_by_key(|(rank, _)| *rank);
        hits.into_iter().map(|(_, id)| id).collect()
    }

    /// Opens the search prompt (the keyboard goes to it).
    pub fn start_search(&mut self) {
        self.capture = Some(Capture::Search);
        self.search = Some(Search { query: String::new(), before: self.selection.clone() });
    }

    /// Typed keys for the search prompt: Enter jumps to the match shown,
    /// Backspace deletes, Escape gives up. Returns whether it's still open.
    fn search_input(&mut self, typed: &[char], back: bool) -> bool {
        let Some(mut search) = self.search.take() else { return false };
        if back {
            self.selection = search.before;
            self.capture = None;
            return false;
        }
        for c in typed {
            match c {
                '\n' | '\r' => {
                    let target = self.selection.clone();
                    self.selection = search.before.clone();
                    self.last_query = search.query;
                    self.capture = None;
                    if let Some(target) = target.filter(|t| Some(t) != search.before.as_ref()) {
                        self.navigate(NavChange::Jump(target));
                    }
                    return false;
                }
                '\u{8}' => {
                    search.query.pop();
                }
                c if !c.is_control() => search.query.push(*c),
                _ => {}
            }
        }
        // Incremental: show the first match while typing.
        match self.matches(&search.query).into_iter().next() {
            Some(first) => {
                self.selection = Some(first.clone());
                self.bring_into_view(&first);
            }
            None => self.selection = search.before.clone(),
        }
        self.search = Some(search);
        true
    }

    /// Jumps to the next (or previous) match of the last search.
    pub fn search_step(&mut self, forward: bool) {
        let found = self.matches(&self.last_query);
        if found.is_empty() {
            return;
        }
        let n = found.len();
        let next = match self.selection.as_ref().and_then(|s| found.iter().position(|f| f == s)) {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        };
        self.navigate(NavChange::Jump(found[next].clone()));
    }

    /// Where the selection is among the last search's matches: (index, count).
    pub fn search_position(&self) -> Option<(usize, usize)> {
        let found = self.matches(&self.last_query);
        let at = self.selection.as_ref().and_then(|s| found.iter().position(|f| f == s))?;
        Some((at + 1, found.len()))
    }

    /// Records the current place under `letter`.
    pub fn set_mark(&mut self, letter: char) {
        let mut here = self.nav.current().clone();
        here.cursor = self.selection.clone();
        self.bookmarks.insert(letter, here);
    }

    /// Goes back to the place marked `letter`, if any.
    pub fn jump_to_mark(&mut self, letter: char) {
        if let Some(place) = self.bookmarks.get(&letter).cloned() {
            self.navigate(NavChange::Restore(Box::new(place)));
        }
    }

    /// The first (or last) node in view order.
    pub fn jump_to_end(&mut self, last: bool) {
        let Some(result) = &self.result else { return };
        let target = if last { result.view.nodes.last() } else { result.view.nodes.first() };
        if let Some(target) = target.map(|n| n.id.clone()) {
            self.navigate(NavChange::Jump(target));
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
        if detail == Detail::Mid {
            let placeholders = if c.placeholders.is_empty() { String::new() } else { format!(" · {} placeholders", c.placeholders.len()) };
            out.push(line(LineRole::Meta, format!("{} · {:?}{placeholders}", c.type_name, c.stage)));
            out.push(line(LineRole::Location, location));
            return out;
        }
        out.push(line(LineRole::Meta, format!("{} · {:?} · {}", c.type_name, c.stage, alignment_label(alignment))));
        if let Some(sig) = &c.signature {
            let params: Vec<String> = sig.params.iter().map(|p| p.ty.as_ref().map_or_else(|| p.name.clone(), |t| format!("{}: {t}", p.name))).collect();
            let returns = sig.returns.as_ref().map(|r| format!(" -> {r}")).unwrap_or_default();
            out.push(line(LineRole::Signature, format!("({}){returns}", params.join(", "))));
        }
        out.push(line(LineRole::Location, location));
        let mut badges = Vec::new();
        if !c.placeholders.is_empty() {
            badges.push(format!("{} placeholders", c.placeholders.len()));
        }
        if let Some(mark) = self.marks.get(&c.id) {
            if mark.open > 0 {
                badges.push(format!("{} open review points", mark.open));
            }
            if mark.changed {
                badges.push("changed since review".to_owned());
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
        let rects: Vec<&WorldRect> = result.layout.rects.values().collect();
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

        if self.fit && self.result.is_some() {
            self.fit = false;
            self.fit_view();
        }
        if let Some(id) = self.reveal_after_fit.take() {
            self.bring_into_view(&id);
        }
        // Fingers first: while touching, pointer emulation is ignored.
        let touched = self.handle_touch(ui, cx);
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
        if !touched
            && (response.clicked() || response.double_clicked())
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
        if self.legend_open {
            self.draw_legend(&painter);
        }
        self.render_chips(ui, cx);
        if has_drawer {
            let drawer = if narrow { Rect::from_min_max(Pos2::new(canvas.min.x, canvas.max.y - sheet_h), canvas.max) } else { Rect::from_min_max(Pos2::new(canvas.max.x, area.min.y), area.max) };
            ui.scope_builder(egui::UiBuilder::new().max_rect(drawer.shrink(6.0)), |ui| self.render_drawer(ui));
        }
        let capturing = self.hint.is_some() || self.capture.is_some();
        if *cx.input_mode == (InputMode::Capture { slot: cx.slot }) && !capturing {
            *cx.input_mode = InputMode::Normal;
        }
    }

    /// Feeds this frame's finger events to the gesture recognizer and acts on
    /// what it sees. Returns whether fingers were involved this frame.
    fn handle_touch(&mut self, ui: &egui::Ui, cx: &mut RunCx<'_>) -> bool {
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
            self.on_gesture(gesture, cx);
        }
        involved
    }

    /// What each gesture does to the graph.
    pub fn on_gesture(&mut self, gesture: Gesture, cx: &mut RunCx<'_>) {
        match gesture {
            Gesture::Tap(pos) => {
                self.context = None;
                let hit = self.hit(pos);
                match (&self.hint, hit) {
                    // A hint on a phone: tapping a labelled node completes it.
                    (Some(hint), Some(id)) => {
                        let action = hint.action;
                        self.hint = None;
                        *cx.input_mode = InputMode::Normal;
                        self.hint_hit(action, id);
                    }
                    (_, hit) => self.select(hit),
                }
            }
            Gesture::DoubleTap(pos) => match self.hit(pos) {
                Some(id) => {
                    self.select(Some(id.clone()));
                    self.navigate(NavChange::Focus(id));
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
            Gesture::Swipe(Swipe::Right) => self.navigate(NavChange::Back),
            Gesture::Swipe(Swipe::Left) => self.navigate(NavChange::Forward),
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
    /// the keys.
    fn render_chips(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        self.chip_rects.clear();
        let actions = self.actions();
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
                egui::Frame::NONE.fill(colors().overlay).corner_radius(8).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for action in wanted {
                            let text = RichText::new(&action.label).size(12.0);
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
            self.run(&action, None, cx);
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
        let font = FontId::proportional(11.0);
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
            if let Some(agent) = &self.agent {
                ui.label(RichText::new(format!("{} the agent moved the view: {}", egui_phosphor::regular::ROBOT, agent.what)).size(11.0).color(colors().special));
                if let Some(back) = actions.iter().find(|a| a.id == "graph.agent_return")
                    && ui.small_button(format!("{} Return", egui_phosphor::regular::ARROW_U_UP_LEFT)).on_hover_text(&back.label).clicked()
                {
                    used = Some((back.clone(), ToolbarUse::Run(None)));
                }
            }
            match (&self.search, self.capture) {
                (Some(search), _) => {
                    let found = self.matches(&search.query).len();
                    ui.label(RichText::new(format!("/{}| {found} matches", search.query)).monospace().size(11.0).color(colors().key));
                }
                (None, Some(Capture::MarkSet)) => {
                    ui.label(RichText::new("mark: press a letter").size(11.0).color(colors().key));
                }
                (None, Some(Capture::MarkJump)) => {
                    ui.label(RichText::new("jump to mark: press its letter").size(11.0).color(colors().key));
                }
                _ => {
                    if let Some((at, of)) = (!self.last_query.is_empty()).then(|| self.search_position()).flatten() {
                        ui.label(RichText::new(format!("/{} {at}/{of}", self.last_query)).monospace().size(11.0).color(colors().muted));
                    }
                }
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
            if collapsed && Some(id) != self.frame.as_ref() {
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
        if self.agent.as_ref().and_then(|a| a.focus.as_ref()) == Some(&c.id) {
            painter.rect_stroke(rect.expand(4.0), 8.0, Stroke::new(2.0, style.agent_color(&theme)), StrokeKind::Outside);
        }
        self.draw_marks(painter, rect, &c.id);
        // Too small to read: the card's color and outline carry it.
        if rect.height() < 9.0 {
            return;
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
                LineRole::Name => (FontId::proportional((13.0 * z).clamp(6.0, 18.0).min(rect.height() * 0.45)), colors().text_strong),
                LineRole::Meta => (FontId::proportional(small), colors().muted),
                LineRole::Signature | LineRole::Location => (FontId::monospace(small * 0.95), colors().muted),
                LineRole::Badges => (FontId::proportional(small), colors().warn),
            };
            clip.text(rect.min + Vec2::new(10.0, y) * z, Align2::LEFT_TOP, line.text, font, color);
            y += if line.role == LineRole::Name { step + 4.0 } else { step };
        }
        if truncated {
            painter.text(rect.right_center() + Vec2::new(4.0, 0.0), Align2::LEFT_CENTER, "…", FontId::proportional(14.0 * z.max(0.6)), colors().muted);
        }
    }

    /// Review badges at the card's top right: a dot when it changed since the
    /// last review, a count of unaddressed comments.
    fn draw_marks(&self, painter: &egui::Painter, rect: Rect, id: &ComponentId) {
        let Some(mark) = self.marks.get(id) else { return };
        let z = self.camera.zoom.max(0.4);
        let mut at = rect.right_top() + Vec2::new(-8.0, 8.0) * z;
        if mark.open > 0 {
            let galley = painter.layout_no_wrap(format!("{}", mark.open), FontId::proportional(10.0 * z), colors().on_bright);
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
        egui::Frame::NONE.fill(colors().surface).stroke(Stroke::new(1.0, colors().border_strong)).corner_radius(6).inner_margin(8).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // Close first (right-aligned), then the name takes what's left
                // and truncates, so a long name never hides the close button.
                ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(egui_phosphor::regular::X).on_hover_text("Close (x)").clicked() {
                        self.drawer_open = false;
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(&c.name).strong().size(14.0).color(colors().text_strong)).truncate()).on_hover_text(&c.name);
                    });
                });
                ui.label(RichText::new(c.id.as_str()).monospace().size(10.0).color(colors().muted));
                ui.label(RichText::new(format!("{} · {:?} · {:?}{}", c.type_name, c.status, c.stage, alignment.map(|a| format!(" · {a:?}")).unwrap_or_default())).size(11.0));
                if let Some(mark) = self.marks.get(&c.id) {
                    ui.horizontal_wrapped(|ui| {
                        if mark.changed {
                            ui.label(RichText::new("changed since review").size(11.0).color(colors().info));
                        }
                        if mark.open > 0 {
                            ui.label(RichText::new(format!("{} open review points", mark.open)).size(11.0).color(colors().warn));
                        }
                    });
                }
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
                        let (to, from) = (egui_phosphor::regular::ARROW_RIGHT, egui_phosphor::regular::ARROW_LEFT);
                        for (label, list) in [(format!("{} {to}", kind.name()), out), (format!("{from} {}", kind.name()), inc)] {
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
                    self.navigate(NavChange::Jump(id));
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
    Search,
    SearchStep { forward: bool },
    MarkSet,
    MarkJump,
    End { last: bool },
    Legend,
    AgentReturn,
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
            out.push(f("graph.next_view", "Next view", G::NextView).icon(egui_phosphor::regular::SWAP));
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
        out.push(f("graph.zoom_in", "Zoom in", G::Zoom(1.2)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_PLUS));
        out.push(f("graph.zoom_out", "Zoom out", G::Zoom(1.0 / 1.2)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_MINUS));
        out.push(f("graph.refresh", "Refresh the view", G::Refresh));
        out.push(f("graph.legend", if self.legend_open { "Hide the legend" } else { "Show the legend" }, G::Legend).toolbar(Button, "", Some(egui_phosphor::regular::INFO)));
        out.push(f("graph.search", "Search the view", G::Search).icon(egui_phosphor::regular::MAGNIFYING_GLASS));
        out.push(f("graph.search_next", "Next match", G::SearchStep { forward: true }));
        out.push(f("graph.search_prev", "Previous match", G::SearchStep { forward: false }));
        out.push(f("graph.mark_set", "Set a mark here", G::MarkSet).icon(egui_phosphor::regular::BOOKMARK_SIMPLE));
        out.push(f("graph.mark_jump", "Jump to a mark", G::MarkJump));
        out.push(f("graph.first", "First node", G::End { last: false }));
        out.push(f("graph.last", "Last node", G::End { last: true }));
        if self.agent.is_some() {
            out.push(f("graph.agent_return", "Return to where you were before the agent moved the view", G::AgentReturn));
        }
        for (id, label, dir) in [("graph.left", "Select left", Dir::Left), ("graph.down", "Select below", Dir::Down), ("graph.up", "Select above", Dir::Up), ("graph.right", "Select right", Dir::Right)] {
            out.push(f(id, label, G::Move(dir)));
        }
        out.push(f("graph.hint", "Pick a node by hint", G::Hint(HintAction::Select)));
        out.push(f("graph.hint_focus", "Focus a node by hint", G::Hint(HintAction::Focus)).icon(egui_phosphor::regular::CURSOR_CLICK));
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
                    // Through the history, so going back undoes it.
                    self.navigate(NavChange::Jump(t));
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
            GraphCommand::Search => {
                self.start_search();
                *cx.input_mode = InputMode::Capture { slot: cx.slot };
            }
            GraphCommand::SearchStep { forward } => self.search_step(forward),
            GraphCommand::MarkSet | GraphCommand::MarkJump => {
                self.capture = Some(if command == GraphCommand::MarkSet { Capture::MarkSet } else { Capture::MarkJump });
                *cx.input_mode = InputMode::Capture { slot: cx.slot };
            }
            GraphCommand::End { last } => self.jump_to_end(last),
            GraphCommand::Legend => {
                self.legend_open = match value.as_deref() {
                    Some("true") => true,
                    Some("false") => false,
                    _ => !self.legend_open,
                };
            }
            GraphCommand::AgentReturn => {
                if let Some(agent) = self.agent.take() {
                    self.navigate(NavChange::Restore(Box::new(agent.return_to)));
                }
            }
        }
    }

    fn on_capture(&mut self, typed: &[char], back: bool, _cx: &mut RunCx<'_>) -> bool {
        if self.hint.is_some() {
            return self.hint_input(typed, back);
        }
        match self.capture {
            Some(Capture::Search) => self.search_input(typed, back),
            Some(mark @ (Capture::MarkSet | Capture::MarkJump)) => {
                // Waits for a letter; anything else (or Escape) gives up.
                if !back && typed.is_empty() {
                    return true;
                }
                self.capture = None;
                if let Some(letter) = typed.iter().copied().find(|c| c.is_alphanumeric()).filter(|_| !back) {
                    if mark == Capture::MarkSet {
                        self.set_mark(letter);
                    } else {
                        self.jump_to_mark(letter);
                    }
                }
                false
            }
            None => false,
        }
    }

    fn takes_counts(&self) -> bool {
        true
    }
}

/// `text` as it fits in `width`: whole, shortened with an ellipsis, or not
/// at all when not even one letter fits.
fn fit_text(painter: &egui::Painter, text: &str, font: &FontId, width: f32) -> Option<String> {
    let fits = |t: &str| painter.layout_no_wrap(t.to_owned(), font.clone(), egui::Color32::WHITE).size().x <= width;
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
        g.last_query = "save".into();
        g.search_step(true);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::save");
        assert!(g.shown(&ComponentId::new("rust:a::save")), "zoomed in so it's drawn");
        assert!(g.camera.zoom >= GraphState::open_zoom());
        // So does the first/last jump, and the agent's focus once its view is in.
        g.camera.zoom = 0.1;
        g.jump_to_end(false);
        assert!(g.shown(g.selection.as_ref().unwrap()), "{:?}", g.selection);
        g.camera.zoom = 0.1;
        g.reveal(ComponentId::new("rust:a::log"));
        assert!(g.shown(&ComponentId::new("rust:a::log")));
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
        g.navigate(NavChange::Back);
        assert!(g.agent.is_none(), "the user took over");
    }

    /// A graph showing the sample, set up from the default UI config.
    fn configured() -> GraphState {
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        crate::graph_style::set(config.graph_style.clone());
        let mut g = showing(sample_result());
        g.ui = config.graph;
        g
    }

    /// Draws one frame of the graph alone at `size`, at `time` seconds.
    fn draw(g: &mut GraphState, ctx: &egui::Context, events: Vec<egui::Event>, time: f64, size: Vec2) -> (egui::FullOutput, InputMode) {
        let net = NetClient::new("http://127.0.0.1:9");
        let mut mode = InputMode::Normal;
        let mut feedback = FeedbackInspectorState::default();
        let out = ctx.run(egui::RawInput { events, time: Some(time), screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                g.render(ui, &mut feedback, 0, false, &mut RunCx { net: &net, ctx, input_mode: &mut mode, slot: 0, hint_chars: "asdf" });
            });
        });
        (out, mode)
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
        assert_eq!(g.nav.current().roots, vec![ComponentId::new("rust:a::save")]);
        // Swipe right goes back, left goes forward.
        let mid = Pos2::new(120.0, 500.0);
        frame(&mut g, vec![touch(4, Start, mid)], 1.0);
        frame(&mut g, vec![touch(4, Move, mid + Vec2::new(60.0, 0.0))], 0.05);
        frame(&mut g, vec![touch(4, End, mid + Vec2::new(150.0, 4.0))], 0.05);
        assert!(g.nav.current().roots.is_empty(), "back");
        frame(&mut g, vec![touch(5, Start, mid + Vec2::new(150.0, 0.0))], 1.0);
        frame(&mut g, vec![touch(5, End, mid - Vec2::new(10.0, 0.0))], 0.1);
        assert_eq!(g.nav.current().roots.len(), 1, "forward");
        g.navigate(NavChange::Back);
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
        let spec = chip(&g, "graph.open_spec").center();
        frame(&mut g, click(spec, true), 0.1);
        frame(&mut g, click(spec, false), 0.05);
        assert!(g.requests.open_spec && g.context.is_none());
        // Two fingers: pan together, pinch apart to zoom.
        let (zoom, pan) = (g.camera.zoom, g.camera.pan);
        let (a, b) = (Pos2::new(150.0, 400.0), Pos2::new(250.0, 400.0));
        frame(&mut g, vec![touch(7, Start, a), touch(8, Start, b)], 1.0);
        frame(&mut g, vec![touch(7, Move, a + Vec2::new(30.0, 0.0)), touch(8, Move, b + Vec2::new(30.0, 0.0))], 0.05);
        assert!((g.camera.pan.x - pan.x - 30.0).abs() < 0.5 && (g.camera.zoom - zoom).abs() < 1e-4, "two-finger pan: {:?} -> {:?}", pan, g.camera.pan);
        frame(&mut g, vec![touch(7, Move, a - Vec2::new(20.0, 0.0)), touch(8, Move, b + Vec2::new(80.0, 0.0))], 0.05);
        assert!(g.camera.zoom > zoom * 1.5, "pinch out zooms in: {zoom} -> {}", g.camera.zoom);
        frame(&mut g, vec![touch(7, End, a), touch(8, End, b)], 0.05);
        assert_eq!(g.nav.current().roots.len(), 0, "no tap or swipe from a two-finger gesture");
        // Chips run the same actions as keys.
        let zoomed = g.camera.zoom;
        let zoom_out = chip(&g, "graph.zoom_out").center();
        frame(&mut g, click(zoom_out, true), 0.3);
        frame(&mut g, click(zoom_out, false), 0.05);
        assert!(g.camera.zoom < zoomed);
        g.navigate(NavChange::Focus(ComponentId::new("rust:a::run")));
        let back = chip(&g, "graph.back").center();
        frame(&mut g, click(back, true), 0.3);
        frame(&mut g, click(back, false), 0.05);
        assert!(g.nav.current().roots.is_empty(), "back chip");
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
        let before = g.nav.current().clone();
        let request = |view: &str, focus: &str| ArchEvent::ViewRequested {
            request: Box::new(ViewRequest { view: Some(view.into()), roots: vec![ComponentId::new(focus)], focus: Some(ComponentId::new(focus)), ..Default::default() }),
            by: Requester::Agent,
        };
        g.on_event(&request("calls", "rust:a::save"));
        assert_eq!(g.nav.current().roots, vec![ComponentId::new("rust:a::save")], "moved through the reducer");
        assert_eq!(g.agent.as_ref().unwrap().what, "calls view · from save · at save");
        g.set_result(sample_result());
        let text = texts(&draw(&mut g, &ctx, vec![], 0.1, Vec2::new(1200.0, 800.0)).0);
        assert!(text.contains("the agent moved the view: calls view · from save · at save"), "{text}");
        assert!(g.actions().iter().any(|a| a.id == "graph.agent_return"));
        // A second move keeps where the user was.
        g.on_event(&request("structure", "rust:a::log"));
        assert_eq!(g.agent.as_ref().unwrap().return_to.roots, before.roots);
        // Return goes straight back to the user's place, selection included.
        let action = g.actions().into_iter().find(|a| a.id == "graph.agent_return").unwrap();
        let (net, ctx2, mut mode) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default(), InputMode::Normal);
        g.run(&action, None, &mut RunCx { net: &net, ctx: &ctx2, input_mode: &mut mode, slot: 0, hint_chars: "asdf" });
        assert!(g.nav.current().same_view(&before) && g.agent.is_none());
        g.set_result(sample_result());
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::step");
        // Plain back (ctrl+o, swipe, chip) steps back one move and clears the mark.
        g.on_event(&request("calls", "rust:a::save"));
        g.navigate(NavChange::Back);
        assert!(g.agent.is_none() && g.nav.current().same_view(&before));
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
        let legend = style.legend(theme);
        let find = |section: &str, label: &str| legend.iter().find(|e: &&LegendEntry| e.section == section && e.label == label).map(|e| e.swatch);
        for kind in RelationKind::ALL {
            assert_eq!(find("Relations", &kind.name().replace('_', " ")), Some(Swatch::Line(style.edge(theme, kind, arch_graph::Provenance::OBSERVED, Alignment::Observed, false))));
        }
        for alignment in [Alignment::Observed, Alignment::Planned, Alignment::Missing, Alignment::Present] {
            let label = alignment_label(alignment);
            assert_eq!(find("Plan vs code", label), Some(Swatch::Line(style.edge(theme, RelationKind::Calls, provenance_of(alignment), alignment, false))));
            assert_eq!(find("Nodes", label), Some(Swatch::Outline(style.node_outline(theme, alignment, false))));
        }
        for stage in arch_graph::Stage::ALL {
            assert_eq!(find("Stage", &format!("{stage:?}")), Some(Swatch::Stripe(style.stage_color(theme, stage))));
        }
        assert!(config.keymap.bind.contains_key("graph.legend") && config.graph.chips.actions.iter().any(|a| a == "graph.legend"), "a key and a chip");
        // Toggling shows exactly the table's entries.
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        let mut g = configured();
        let text = texts(&draw(&mut g, &ctx, vec![], 0.0, Vec2::new(1200.0, 800.0)).0);
        assert!(!text.contains("planned, missing in code"));
        let toggle = g.actions().into_iter().find(|a| a.id == "graph.legend").unwrap();
        let (net, mut mode) = (NetClient::new("http://127.0.0.1:9"), InputMode::Normal);
        g.run(&toggle, None, &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, hint_chars: "asdf" });
        let out = draw(&mut g, &ctx, vec![], 0.1, Vec2::new(1200.0, 800.0)).0;
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
        g.run(&toggle, None, &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, hint_chars: "asdf" });
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
        assert_eq!(texts_at(&g, mid), vec!["step", "function · Implemented · 1 placeholders", "unplaced"]);
        assert_eq!(
            texts_at(&g, near),
            vec!["step", "function · Implemented · planned and in code", "(input: &str) -> bool", "unplaced", "1 placeholders · 2 open review points · changed since review"]
        );
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
        let text = texts(&draw(&mut close, &ctx, vec![], 0.1, Vec2::new(1400.0, 800.0)).0);
        assert!(text.contains("(input: &str) -> bool") && text.contains("2 open review points"), "{text}");
    }
}
