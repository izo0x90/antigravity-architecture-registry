//! The architecture graph: server views drawn as scenes (cards sized to their
//! text, arranged by what depends on what, lines routed round them), one
//! reducer for where it looks (view, plan, focus, depth), levels you zoom
//! into and out of, modules opened in place, touch gestures and on-screen
//! chips, a legend, semantic zoom and a compact inspector. The universal keys
//! (moves, search, hints, marks, the jump list) drive it through `Navigable`.
//! Everything shown comes from the server's views; the graph refreshes itself
//! when the architecture changes.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use arch_graph::view::scene::{self, End, Measure, Rect as WorldRect, Scene};
use arch_graph::view::LayoutKind;
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
    /// What a step takes and gives (data-flow views).
    Shape,
    Signature,
    Location,
    Badges,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CardLine {
    pub role: LineRole,
    pub text: String,
}

/// Cards moving to new places (after a refresh or a module opening): where
/// they were, and when the move started (the first frame that draws it).
#[derive(Clone, Debug, Default, PartialEq)]
struct Moving {
    from: BTreeMap<ComponentId, WorldRect>,
    started: Option<f64>,
}

/// The camera gliding from one place to another (into a level or out).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Glide {
    from: Camera,
    to: Camera,
    started: Option<f64>,
}

/// Going into a module, or up out of a level, by zooming: where it was on
/// screen, so the new level grows out of it (or shrinks back into it).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Going {
    In(Rect),
    Out(Rect),
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

/// How far out and in the camera zooms.
const MIN_ZOOM: f32 = 0.03;
const MAX_ZOOM: f32 = 3.0;

/// The views that tell calls as a story, forwards and backwards.
const STORY: &str = "story";
/// The finest level: the parts themselves.
const PARTS: &str = "parts";
/// The view that shows one value's whole path.
const DATA_PATH: &str = "data_path";
const REACHED_BY: &str = "reached_by";

/// The hint key of the "+N more" block.
const MORE: &str = "more";

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
    /// The view as drawn: the server's, seen at the chosen level.
    pub result: Option<ViewResult>,
    /// The view as the server sent it (the level is applied to a copy).
    raw: Option<ViewResult>,
    /// The level relations are seen at (a level's name; none: the parts).
    pub grain: Option<String>,
    /// How the view is drawn at this level: cards, open modules, blocks and
    /// routed lines (rebuilt when the view, the open modules or the
    /// selection's neighbourhood change).
    pub scene: Scene,
    /// Names as drawn here (short, with the part above when two shown
    /// components share one).
    names: BTreeMap<ComponentId, String>,
    /// Modules opened in place (the rest show shut).
    open: BTreeSet<ComponentId>,
    /// Every part of a crowded level shown (not just the neighbourhood).
    show_all: bool,
    /// Story steps collapsed by hand (their branch summed up).
    shut: BTreeSet<ComponentId>,
    /// Following one value through the data flow (its name): what doesn't
    /// carry it fades.
    pub trace: Option<String>,
    moving: Option<Moving>,
    glide: Option<Glide>,
    going: Option<Going>,
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
    /// The camera and open modules before a search started previewing matches.
    saved_view: Option<(Camera, BTreeSet<ComponentId>, bool)>,
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
    /// Where the long-press chips start in `chip_rects` (after the bar's).
    context_chips: usize,
    /// Markers at the screen's edge for the selection's lines that go off
    /// it: what's at the other end, and where the marker is (click to go).
    pub(crate) beyond: Vec<(ComponentId, Rect)>,
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
            raw: None,
            grain: None,
            scene: Scene::default(),
            names: BTreeMap::new(),
            open: BTreeSet::new(),
            show_all: false,
            shut: BTreeSet::new(),
            trace: None,
            moving: None,
            glide: None,
            going: None,
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
            context_chips: 0,
            beyond: Vec::new(),
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
                if let Some(source) = request.source {
                    self.nav.plan = source.plan().cloned();
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
            self.show_all = false;
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

    /// Makes `id` visible: opens the shut modules around it in place (or
    /// shows every part of a crowded level, when "+N more" holds it), and
    /// pans so it's on screen when it isn't.
    fn bring_into_view(&mut self, id: &ComponentId) {
        if !self.shown(id) && self.is_story() {
            // Open the collapsed branches hiding it, one at a time.
            for branch in self.shut.clone() {
                self.shut.remove(&branch);
                self.rebuild(true);
                if self.shown(id) {
                    break;
                }
            }
        }
        if !self.shown(id) && self.scene.outside.get(id).is_none() {
            if self.scene.more.as_ref().is_some_and(|(_, held)| held.iter().any(|h| h == id || self.scene.ancestors(id).contains(h))) {
                self.show_all = true;
            }
            for outer in self.scene.ancestors(id) {
                if Some(&outer) != self.scene.frame.as_ref() {
                    self.open.insert(outer);
                }
            }
            self.rebuild(true);
        }
        let Some(r) = self.box_of(id) else { return };
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
        if let Some(source) = &request.source {
            parts.push(source.to_string());
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
        self.raw = Some(result.clone());
        let result = self.at_level(result);
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
        // The same level again (a refresh): things keep their places and glide
        // to new ones.
        let same_level = self.result.as_ref().is_some_and(|old| old.view.spec.name == result.view.spec.name && old.view.roots == result.view.roots);
        self.open.retain(|c| result.view.edges.iter().any(|e| e.kind == RelationKind::Contains && &e.source == c));
        self.shut.retain(|c| result.view.node(c).is_some());
        self.result = Some(result);
        self.rebuild(same_level);
        // Focused on one thing that holds nothing (a function, in a view of
        // what's in what): show it where it lives, inside its module, selected.
        if let Some(result) = &self.raw
            && result.view.spec.layout == LayoutKind::Nested
            && let [root] = result.view.roots.as_slice()
            && !result.view.edges.iter().any(|e| e.kind == RelationKind::Contains && &e.source == root)
            && let Some(holder) = result.view.trail.last().cloned()
        {
            let root = root.clone();
            self.navigate(NavChange::Focus(holder));
            self.pending_select = Some(root);
        }
        self.load = Load::Idle;
        if std::mem::take(&mut self.refit) {
            self.fit = true;
        }
    }

    /// Lays the view out again: the same level keeps its places (and cards
    /// glide to new ones), a new level starts fresh.
    fn rebuild(&mut self, keep: bool) {
        let Some(result) = &self.result else {
            self.scene = Scene::default();
            return;
        };
        let previous = keep.then(|| std::mem::take(&mut self.scene));
        let cards = Cards::new(result, &self.ui.arrange);
        self.names = cards.names.clone();
        let options = scene::Options { open: &self.open, selection: self.selection.as_ref(), max_cards: self.ui.levels.max_cards, show_all: self.show_all, shut: &self.shut };
        self.scene = scene::build(&result.view, &options, &cards, &self.ui.arrange, previous.as_ref());
        if let Some(before) = previous {
            let from: BTreeMap<ComponentId, WorldRect> = before.rects.into_iter().filter(|(id, r)| self.scene.rects.get(id).is_some_and(|now| now != r)).collect();
            if !from.is_empty() && crate::theme::look().move_seconds > 0.0 {
                self.moving = Some(Moving { from, started: None });
            }
        }
    }

    /// The levels this view can be seen at (those that group some of it).
    fn levels(&self) -> Vec<arch_graph::view::grain::GrainSpec> {
        let (Some(raw), Some(catalog)) = (&self.raw, &self.catalog) else { return Vec::new() };
        if matches!(raw.view.spec.layout, LayoutKind::Nested | LayoutKind::Story) {
            return Vec::new();
        }
        arch_graph::view::grain::levels(&raw.view, &raw.components, &catalog.grains).into_iter().cloned().collect()
    }

    /// `result` seen at the chosen level (as it is, at the parts' level).
    fn at_level(&self, mut result: ViewResult) -> ViewResult {
        let Some(name) = &self.grain else { return result };
        let Some(catalog) = &self.catalog else { return result };
        if matches!(result.view.spec.layout, LayoutKind::Nested | LayoutKind::Story) {
            return result;
        }
        let levels = arch_graph::view::grain::levels(&result.view, &result.components, &catalog.grains);
        if let Some(at) = levels.iter().position(|g| &g.name == name) {
            result.view = arch_graph::view::grain::regroup(&result.view, &result.components, &levels, at);
        }
        result
    }

    /// Sees the view at another level (none: the parts themselves). What
    /// was selected stays selected, or what holds it now does.
    fn set_grain(&mut self, grain: Option<String>) {
        if self.grain == grain {
            return;
        }
        self.grain = grain;
        self.open.clear();
        if let Some(raw) = self.raw.clone() {
            self.set_result(raw);
            self.moving = None;
            self.fit = true;
            if let Some(sel) = self.selection.clone()
                && !self.shown(&sel)
            {
                let holder = self.scene.ancestors(&sel).into_iter().rev().find(|a| self.shown(a));
                self.select(holder);
            }
        }
    }

    /// One level coarser (`up`) or finer.
    fn step_grain(&mut self, up: bool) -> Result<(), String> {
        let names: Vec<String> = self.levels().into_iter().map(|g| g.name).collect();
        if names.is_empty() {
            return Err("This view has no levels to step through.".into());
        }
        let at = self.grain.as_ref().and_then(|g| names.iter().position(|n| n == g));
        let next = match (at, up) {
            (None, true) => Some(Some(names[0].clone())),
            (Some(i), true) if i + 1 < names.len() => Some(Some(names[i + 1].clone())),
            (Some(0), false) => Some(None),
            (Some(i), false) => Some(Some(names[i - 1].clone())),
            _ => None,
        };
        match next {
            Some(grain) => {
                self.set_grain(grain);
                Ok(())
            }
            None => Err(if up { "Already at the widest level." } else { "Already showing every part." }.into()),
        }
    }

    /// Whether the view is told as a story.
    fn is_story(&self) -> bool {
        self.result.as_ref().is_some_and(|r| r.view.spec.layout == LayoutKind::Story)
    }

    /// Story steps that lead somewhere (a branch that can collapse).
    fn branches(&self) -> Vec<ComponentId> {
        let Some(result) = &self.result else { return Vec::new() };
        let outward = result.view.spec.direction != arch_graph::view::Direction::In;
        let ends: BTreeSet<&ComponentId> = result.view.edges.iter().filter(|e| e.kind == RelationKind::Calls).map(|e| if outward { &e.source } else { &e.target }).collect();
        ends.into_iter().cloned().collect()
    }

    /// Whether `id` holds parts in this view (a module that opens in place).
    fn is_module(&self, id: &ComponentId) -> bool {
        self.result.as_ref().is_some_and(|r| r.view.edges.iter().any(|e| e.kind == RelationKind::Contains && &e.source == id)) && self.scene.frame.as_ref() != Some(id)
    }

    /// Whether a module shows shut (as one card).
    fn closed(&self, container: &ComponentId) -> bool {
        !self.scene.containers.contains(container)
    }

    /// Whether a node is drawn (not inside a shut module, not held in "+N more").
    fn shown(&self, id: &ComponentId) -> bool {
        self.scene.rects.contains_key(id)
    }

    /// Everything that can be selected, with where it is: cards, open
    /// modules, and the blocks for what's outside the focus.
    fn boxes(&self) -> impl Iterator<Item = (&ComponentId, &WorldRect)> {
        self.scene.rects.iter().chain(self.scene.outside.iter().map(|(id, (r, _))| (id, r)))
    }

    fn box_of(&self, id: &ComponentId) -> Option<WorldRect> {
        self.boxes().find(|(b, _)| *b == id).map(|(_, r)| *r)
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
        let Some(current) = self.selection.as_ref().and_then(|s| self.box_of(s)) else {
            let first = result.view.roots.first().or(result.view.nodes.first().map(|n| &n.id)).cloned();
            if let Some(first) = first {
                self.bring_into_view(&first);
                self.select(Some(first));
            }
            return;
        };
        let center = |r: &WorldRect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let current = &current;
        let (cx, cy) = center(current);
        let best = self
            .boxes()
            // Not what it sits in, nor what sits in it: moves go between neighbours.
            .filter(|(id, r)| Some(*id) != self.selection.as_ref() && !r.contains(current) && !current.contains(r))
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
            let inside: Vec<(&ComponentId, &WorldRect)> = self.boxes().filter(|(_, r)| current.contains(r) && *r != current).collect();
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

    /// The innermost open module a node is drawn in (None: the top level).
    fn container_of(&self, id: &ComponentId) -> Option<&ComponentId> {
        self.scene.parent.get(id).filter(|p| self.scene.containers.contains(*p))
    }

    /// Drawn nodes in the same module as the selection, in reading order
    /// (top to bottom, left to right).
    fn siblings(&self) -> Vec<ComponentId> {
        let (Some(result), Some(sel)) = (&self.result, &self.selection) else { return Vec::new() };
        let parent = self.container_of(sel).cloned();
        let _ = result;
        let mut out: Vec<(&WorldRect, &ComponentId)> = self.boxes().filter(|(id, _)| self.container_of(id).cloned() == parent).map(|(id, r)| (r, id)).collect();
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
        // Cards, shut modules and blocks; not the frames of open ones.
        let nearest = self
            .boxes()
            .filter(|(id, _)| !self.scene.containers.contains(*id))
            .map(|(id, r)| (self.to_screen(r).center().distance(anchor), id))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id.clone());
        if let Some(id) = nearest {
            self.select(Some(id));
        }
    }

    fn selected_rect(&self) -> Option<Rect> {
        let r = self.box_of(self.selection.as_ref()?)?;
        Some(self.to_screen(&r))
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
        let sel = self.selection.as_ref()?;
        if self.is_module(sel) { Some(sel.clone()) } else { self.container_of(sel).cloned() }
    }

    /// Opens or shuts modules in place. The rest keep their places (and
    /// glide to new ones); the module stays where it is on screen.
    fn set_folds(&mut self, modules: &[ComponentId], open: bool) {
        let anchor = modules.first().and_then(|m| self.box_of(m)).map(|r| self.to_screen(&r).min);
        for m in modules {
            if open {
                self.open.insert(m.clone());
            } else {
                self.open.remove(m);
            }
        }
        self.rebuild(true);
        if let (Some(before), Some(now)) = (anchor, modules.first().and_then(|m| self.box_of(m)).map(|r| self.to_screen(&r).min)) {
            self.camera.pan += before - now;
        }
        // A node a module shut over hands the selection to the module.
        if !open
            && let Some(sel) = self.selection.clone()
            && !self.shown(&sel)
            && self.scene.outside.get(&sel).is_none()
        {
            let holder = self.scene.ancestors(&sel).into_iter().rev().find(|a| self.shown(a));
            self.select(holder);
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

    pub fn card_lines(&self, c: &Component, alignment: Alignment, detail: Detail) -> Vec<CardLine> {
        card_lines(c, alignment, detail, self.marks.get(&c.id))
    }

    /// What a card says at the current zoom, under the name it's drawn with
    /// (a shut module says how many parts it holds).
    fn card_text(&self, c: &Component, alignment: Alignment, closed: bool) -> Vec<CardLine> {
        let detail = self.detail();
        let mut lines = if closed {
            let members = self.result.as_ref().and_then(|r| r.view.node(&c.id)).map_or(0, |n| n.members);
            let mut lines = vec![CardLine { role: LineRole::Name, text: c.name.clone() }, CardLine { role: LineRole::Meta, text: module_meta(c, members) }];
            if detail == Detail::Near {
                lines.extend(card_lines(c, alignment, detail, self.marks.get(&c.id)).into_iter().filter(|l| l.role == LineRole::Badges));
            }
            lines
        } else {
            card_lines(c, alignment, detail, self.marks.get(&c.id))
        };
        if let Some(name) = self.names.get(&c.id) {
            lines[0].text = name.clone();
        }
        if self.is_flow() && detail != Detail::Far
            && let Some(shape) = shape_of(c)
        {
            lines.insert(1.min(lines.len()), CardLine { role: LineRole::Shape, text: shape });
        }
        lines
    }

    /// With something selected: it, what it's linked to (either way), what
    /// holds it and, for an open module, its parts. The rest dims.
    fn related(&self) -> Option<BTreeSet<ComponentId>> {
        let sel = self.selection.as_ref()?;
        let mut out: BTreeSet<ComponentId> = BTreeSet::from([sel.clone()]);
        for line in &self.scene.lines {
            let ends = [&line.from, &line.to];
            if ends.iter().any(|e| matches!(e, End::Node(id) | End::Outside(id) if id == sel)) {
                for e in ends {
                    if let End::Node(id) | End::Outside(id) = e {
                        out.insert(id.clone());
                    }
                }
            }
        }
        out.extend(self.scene.ancestors(sel));
        if self.scene.containers.contains(sel) {
            out.extend(self.scene.parent.iter().filter(|(_, p)| *p == sel).map(|(c, _)| c.clone()));
        }
        Some(out)
    }

    /// Whether the view shows data flow (steps say their shape).
    fn is_flow(&self) -> bool {
        self.result.as_ref().is_some_and(|r| r.view.spec.kinds.contains(&RelationKind::DataFlow))
    }

    /// Whether a line carries the value being followed.
    fn on_trace(&self, carries: &[String]) -> bool {
        self.trace.as_ref().is_some_and(|t| carries.iter().any(|c| mentions(c, t)))
    }

    /// What the selection could be followed by: its own type, what it
    /// takes and gives, and what its flows carry.
    fn follow_choices(&self) -> Vec<String> {
        let Some(c) = self.selected() else { return Vec::new() };
        let mut out: Vec<String> = Vec::new();
        let mut push = |v: String| {
            if !v.is_empty() && !out.contains(&v) {
                out.push(v);
            }
        };
        if c.signature.is_none() && c.type_name != "function" {
            push(c.name.clone());
        }
        if let Some(sig) = &c.signature {
            sig.returns.iter().flat_map(|r| type_names(r)).for_each(&mut push);
            sig.params.iter().filter_map(|p| p.ty.as_deref()).flat_map(type_names).for_each(&mut push);
        }
        for line in &self.scene.lines {
            if [&line.from, &line.to].iter().any(|e| matches!(e, End::Node(id) if *id == c.id)) {
                line.carries.iter().cloned().for_each(&mut push);
            }
        }
        out
    }

    // ---- levels ----

    /// Up a level: to the module above the focus (or the whole view), with
    /// the one you were in selected. Views without levels go back to their
    /// entry points.
    fn up_level(&mut self) {
        let left = self.nav.roots.first().cloned();
        let nested = self.result.as_ref().is_some_and(|r| r.view.spec.layout == LayoutKind::Nested);
        let above = self.result.as_ref().and_then(|r| r.view.trail.last().cloned());
        match (nested, above) {
            (true, Some(parent)) => self.navigate(NavChange::Focus(parent)),
            _ => self.navigate(NavChange::Unfocus),
        }
        if let Some(left) = left {
            self.pending_select = Some(left);
        }
    }

    /// Zooming moves between levels: far enough into a shut module goes
    /// into it (it grows into the new level); far enough out of a level goes
    /// up to the one above (it shrinks back into its card).
    fn level_by_zoom(&mut self, around: Pos2, factor: f32) {
        let nested = self.result.as_ref().is_some_and(|r| r.view.spec.layout == LayoutKind::Nested);
        if !nested || self.load == Load::Loading || self.stale {
            return;
        }
        let levels = self.ui.levels.clone();
        let area = self.open_area;
        let fills = |r: Rect, share: f32| r.width() >= area.width() * share || r.height() >= area.height() * share;
        let shut = |g: &Self, id: &ComponentId| g.shown(id) && g.closed(id) && g.result.as_ref().and_then(|r| r.view.node(id)).is_some_and(|n| n.members > 0);
        let under = self.hit(around).filter(|id| shut(self, id)).or_else(|| self.selection.clone().filter(|id| shut(self, id)));
        // In: it fills enough of the view, or the zoom can't go closer.
        let at_limit = factor > 1.0 && self.camera.zoom >= MAX_ZOOM;
        if let Some(id) = under
            && let Some(r) = self.box_of(&id).map(|r| self.to_screen(&r))
            && (fills(r, levels.enter_fill) || at_limit)
        {
            self.going = Some(Going::In(r));
            self.pointer_jump(NavChange::Focus(id));
            return;
        }
        if factor < 1.0
            && !self.nav.roots.is_empty()
            && let Some(b) = self.scene.bounds().map(|b| self.to_screen(&b))
            && !fills(b, levels.leave_fill)
        {
            self.going = Some(Going::Out(b));
            if self.jumped.is_none() {
                self.jumped = Some(self.place());
            }
            self.up_level();
        }
    }

    /// Zooms as the user asked (wheel, pinch, keys, chips), which can move
    /// between levels.
    fn zoom_by_user(&mut self, factor: f32, around: Pos2) {
        // Mid-way into (or out of) a level, the zoom waits for it to arrive.
        if self.glide.is_some() || self.going.is_some() {
            return;
        }
        self.zoom_by(factor, around);
        self.level_by_zoom(around, factor);
    }

    fn request_code(&mut self) {
        if let Some(c) = self.selected() {
            self.requests.open_code = match (&c.anchor, &c.proposed) {
                (Some(a), _) if crate::status::is_folder(a) => None,
                (Some(a), _) => Some((a.file.to_string(), a.span.start_line, a.span.end_line)),
                (None, Some(p)) => Some((p.file.to_string(), 1, 1)),
                (None, None) => None,
            };
        }
    }

    fn zoom_by(&mut self, factor: f32, around: Pos2) {
        let old = self.camera.zoom;
        let new = (old * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let world = (around - self.canvas.min - self.camera.pan) / old;
        self.camera.zoom = new;
        self.camera.pan = around - self.canvas.min - world * new;
    }

    fn fit_view(&mut self) {
        let Some(bounds) = self.scene.bounds() else { return };
        if self.is_story() {
            // A story reads like a page: as wide as the view (never past
            // full size), from the top.
            let pad = crate::theme::look().space_xxxl;
            let area = self.open_area;
            let zoom = ((area.width() - pad * 2.0) / bounds.w.max(1.0)).clamp(MIN_ZOOM, 1.0);
            self.camera.zoom = zoom;
            self.camera.pan = (area.min - self.canvas.min) + Vec2::splat(pad) - Vec2::new(bounds.x, bounds.y) * zoom;
            return;
        }
        // With a selection, frame it and what it's linked to; else the whole.
        let focus: Vec<WorldRect> = self.related().map(|r| r.iter().filter_map(|id| self.box_of(id)).collect()).unwrap_or_default();
        self.fit_rects(if focus.is_empty() { std::slice::from_ref(&bounds) } else { &focus });
        // Never smaller than readable: what doesn't fit opens at reading
        // size around the selection (or where it starts), and pans from there.
        let readable = self.ui.detail.mid;
        if self.camera.zoom < readable {
            let area = self.open_area;
            self.camera.zoom = readable;
            match self.selection.as_ref().and_then(|s| self.box_of(s)) {
                // Toward what it's linked to, as far as keeps it on screen.
                Some(sel) => {
                    let (min_x, min_y) = focus.iter().fold((f32::MAX, f32::MAX), |(x, y), r| (x.min(r.x), y.min(r.y)));
                    let (max_x, max_y) = focus.iter().fold((f32::MIN, f32::MIN), |(x, y), r| (x.max(r.right()), y.max(r.bottom())));
                    let margin = self.ui.arrange.gap_x / 2.0;
                    let half = Vec2::new(area.width(), area.height()) / readable / 2.0;
                    let toward = |lo: f32, hi: f32, s0: f32, s1: f32, half: f32| ((lo + hi) / 2.0).clamp(s1 + margin - half, s0 - margin + half);
                    let center = Vec2::new(toward(min_x, max_x, sel.x, sel.right(), half.x), toward(min_y, max_y, sel.y, sel.bottom(), half.y));
                    self.camera.pan = (area.center() - self.canvas.min) - center * readable;
                }
                None => {
                    let anchor = Vec2::new(bounds.x, bounds.y) + Vec2::new(area.width(), area.height()) / readable / 2.0;
                    self.camera.pan = (area.center() - self.canvas.min) - anchor * readable;
                }
            }
        }
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
        let zoom = ((area.width() - pad * 2.0) / w).min((area.height() - pad * 2.0) / h).clamp(MIN_ZOOM, 1.4);
        self.camera.zoom = zoom;
        let content = Vec2::new(w, h) * zoom;
        self.camera.pan = (area.min - self.canvas.min) + (area.size() - content) / 2.0 - Vec2::new(min_x, min_y) * zoom;
    }

    /// Arriving at a level by zooming: it grows out of the card it was (or
    /// the level left shrinks back into its card), then settles where the
    /// fit put it.
    fn glide_in(&mut self, time: f64) {
        let Some(going) = self.going.take() else { return };
        let target = self.camera;
        let (world, screen) = match going {
            Going::In(card) => (self.scene.bounds(), card),
            Going::Out(level) => (self.pending_select.as_ref().or(self.selection.as_ref()).and_then(|id| self.box_of(id)), level),
        };
        let Some(world) = world.filter(|w| w.w > 0.0 && w.h > 0.0) else { return };
        let zoom = (screen.width() / world.w).min(screen.height() / world.h).max(f32::EPSILON);
        let pan = (screen.center() - self.canvas.min) - Vec2::new(world.x + world.w / 2.0, world.y + world.h / 2.0) * zoom;
        let from = Camera { zoom, pan };
        if crate::theme::look().move_seconds > 0.0 {
            self.camera = from;
            self.glide = Some(Glide { from, to: target, started: Some(time) });
        }
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
            self.glide_in(time);
        }
        // Moves and glides start on the first frame that draws them.
        if let Some(m) = &mut self.moving {
            let started = *m.started.get_or_insert(time);
            if move_t(Some(started), time).is_none_or(|t| t >= 1.0) {
                self.moving = None;
            }
        }
        if let Some(g) = &mut self.glide {
            let started = *g.started.get_or_insert(time);
            match move_t(Some(started), time) {
                Some(t) if t < 1.0 => {
                    self.camera = Camera { zoom: g.from.zoom * (g.to.zoom / g.from.zoom).powf(t), pan: g.from.pan + (g.to.pan - g.from.pan) * t };
                }
                _ => {
                    self.camera = g.to;
                    self.glide = None;
                }
            }
        }
        if let Some(id) = self.reveal_after_fit.take() {
            self.bring_into_view(&id);
        }
        // Fingers first: while touching, pointer emulation is ignored.
        let touched = self.handle_touch(ui);
        if !touched {
            // Pan (drag, or hold the leader key) and zoom (wheel, pinch).
            if response.dragged() {
                self.glide = None;
                self.camera.pan += response.drag_delta();
            }
            if pan_held && response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            if response.hovered() {
                let (scroll, pinch, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
                let around = pointer.unwrap_or(canvas.center());
                if pinch != 1.0 {
                    self.zoom_by_user(pinch, around);
                } else if scroll != 0.0 {
                    self.zoom_by_user((1.0 + scroll * 0.0015).clamp(0.8, 1.25), around);
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
                // Moving dashes, gliding cards and the agent's pulse keep drawing.
                let hover = response.hover_pos();
                if self.draw_graph(&painter, time, hover) {
                    ui.ctx().request_repaint();
                }
                self.beyond = self.edge_markers(&painter);
            }
        }
        if let Load::Failed(err) = &self.load
            && self.result.is_some()
        {
            painter.text(canvas.left_bottom() + Vec2::new(8.0, -8.0), Align2::LEFT_BOTTOM, format!("refresh failed: {err}"), FontId::proportional(crate::theme::look().text_body), colors().error);
        }

        // Click selects, double click focuses ("+N more" shows everything).
        if !touched
            && (response.clicked() || response.double_clicked())
            && let Some(pos) = response.interact_pointer_pos()
        {
            if let Some(id) = self.beyond.iter().find(|(_, r)| r.contains(pos)).map(|(id, _)| id.clone()) {
                self.pointer_jump(NavChange::Jump(id));
                return;
            }
            if self.hit_more(pos) {
                self.show_everything();
                return;
            }
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
                self.close_context();
                if let Some(id) = self.beyond.iter().find(|(_, r)| r.contains(pos)).map(|(id, _)| id.clone()) {
                    self.pointer_jump(NavChange::Jump(id));
                    return;
                }
                if self.hit_more(pos) {
                    self.show_everything();
                    return;
                }
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
            Gesture::Pinch { factor, center } => self.zoom_by_user(factor, center),
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
        self.context_chips = self.chip_rects.len();
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
            self.close_context();
            self.use_action(&action, None, cx);
        }
    }

    /// Closes the long-press chips (and forgets where they were, so the next
    /// touch there is the graph's).
    fn close_context(&mut self) {
        self.context = None;
        self.chip_rects.truncate(self.context_chips);
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
        let entries: Vec<LegendEntry> = style().legend(&theme).into_iter().filter(|e| self.on_screen(e)).collect();
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

    /// Whether a legend entry explains something drawn now.
    fn on_screen(&self, entry: &LegendEntry) -> bool {
        let Some(result) = &self.result else { return false };
        let lines = &self.scene.lines;
        let drawn = || self.scene.rects.keys().filter_map(|id| result.view.node(id));
        match entry.section {
            "Relations" => lines.iter().any(|l| l.kinds.iter().any(|k| k.name().replace('_', " ") == entry.label)),
            "Plan vs code" => {
                if entry.label == crate::status::look("unconfirmed").1 {
                    lines.iter().any(|l| l.unconfirmed)
                } else if entry.label.starts_with("overlay") {
                    lines.iter().any(|l| l.overlay)
                } else {
                    lines.iter().any(|l| crate::graph_style::alignment_label(l.alignment) == entry.label)
                }
            }
            "Nodes" => match entry.label.as_str() {
                "selected" => self.selection.is_some(),
                "the agent pointed here" => self.agent.as_ref().is_some_and(|a| a.focus.is_some()),
                label => drawn().any(|n| crate::graph_style::alignment_label(n.alignment) == label),
            },
            "Stage" => self.scene.rects.keys().filter_map(|id| result.components.get(id)).any(|c| crate::status::look(crate::status::stage(c.stage)).1 == entry.label),
            _ => true,
        }
    }

    fn hit(&self, pos: Pos2) -> Option<ComponentId> {
        // Innermost first: smallest box containing the point.
        self.boxes()
            .map(|(id, r)| (self.to_screen(r), id))
            .filter(|(rect, _)| rect.contains(pos))
            .min_by(|a, b| a.0.area().total_cmp(&b.0.area()))
            .map(|(_, id)| id.clone())
    }

    /// Whether `pos` is on the "+N more" block.
    fn hit_more(&self, pos: Pos2) -> bool {
        self.scene.more.as_ref().is_some_and(|(r, _)| self.to_screen(r).contains(pos))
    }

    /// Shows every part of a crowded level.
    fn show_everything(&mut self) {
        self.show_all = true;
        self.rebuild(true);
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
        // Where we are: plan, the level's trail (each step a link), depth;
        // and whether the agent moved us.
        let mut crumb: Option<Option<ComponentId>> = None;
        ui.horizontal_wrapped(|ui| {
            let nav = self.nav.clone();
            let look = crate::theme::look();
            let muted = |t: String| RichText::new(t).size(look.text_body).color(colors().muted);
            ui.label(muted(format!("{} ·", nav.plan.as_ref().map_or("code".to_owned(), |p| format!("plan {p}")))));
            let name = |id: &ComponentId| self.component(id).map_or_else(|| id.to_string(), |c| c.name.clone());
            let trail = self.result.as_ref().map(|r| r.view.trail.clone()).unwrap_or_default();
            match nav.roots.as_slice() {
                [] => {
                    ui.label(muted("from the entry points".into()));
                }
                [root] => {
                    if ui.link(RichText::new("All").size(look.text_body)).on_hover_text("The whole view").clicked() {
                        crumb = Some(None);
                    }
                    for step in &trail {
                        ui.label(muted("›".into()));
                        if ui.link(RichText::new(name(step)).size(look.text_body)).clicked() {
                            crumb = Some(Some(step.clone()));
                        }
                    }
                    ui.label(muted("›".into()));
                    ui.label(RichText::new(name(root)).size(look.text_body).strong().color(colors().text_strong));
                }
                roots => {
                    ui.label(muted(format!("from {}", roots.iter().map(name).collect::<Vec<_>>().join(", "))));
                }
            }
            let depth = nav.depth.unwrap_or(self.base_depth());
            let shown = self.result.as_ref().map_or(0, |r| r.view.nodes.len());
            ui.label(muted(format!("· depth {depth} · {shown} nodes")));
            if let Some(level) = &self.grain {
                ui.label(RichText::new(format!("{} by {level}", egui_phosphor::regular::STACK)).size(look.text_body).strong().color(colors().focus));
            }
            if let Some(value) = &self.trace {
                let (glyph, word, color) = crate::status::look("following");
                ui.label(RichText::new(format!("{glyph} {word} {value}")).size(look.text_body).strong().color(color));
            }
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
        match crumb {
            Some(Some(id)) => self.pointer_jump(NavChange::Focus(id)),
            Some(None) => self.pointer_jump(NavChange::Unfocus),
            None => {}
        }
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

    fn draw_graph(&self, painter: &egui::Painter, time: f64, hover: Option<Pos2>) -> bool {
        let look = crate::theme::look();
        let theme = colors();
        let Some(result) = &self.result else { return false };
        let mut moving = self.moving.is_some() || self.glide.is_some();
        // Lines settle in once the cards have moved.
        let settle = self.moving.as_ref().map_or(1.0, |m| move_t(m.started, time).unwrap_or(0.0));
        let mut unlabelled: Vec<Unlabelled> = Vec::new();
        let related = self.related();
        // A story's module bands, behind everything: each run of steps in one module.
        for (i, band) in self.scene.bands.iter().enumerate() {
            let rect = self.to_screen(&band.rect);
            // Alternate bands differ, so a change of module shows.
            let fill = if i % 2 == 0 { theme.raised.gamma_multiply(look.fade) } else { theme.surface.gamma_multiply(look.fade) };
            painter.rect_filled(rect, look.radius_m, fill);
            if rect.height() >= look.text_small * self.ui.arrange.line_height * 2.0 {
                // Named at its right end, kept in view.
                let name = result.components.get(&band.module).map_or_else(|| band.module.to_string(), |c| c.name.clone());
                let right = rect.right().min(self.open_area.right()) - look.space_m;
                painter.text(Pos2::new(right, rect.top() + look.space_s), Align2::RIGHT_TOP, name, FontId::proportional(look.text_small), theme.muted);
            }
        }
        // Open modules, outermost first: a box with the name in its title band.
        let mut open: Vec<&ComponentId> = self.scene.containers.iter().collect();
        open.sort_by_key(|c| self.scene.ancestors(c).len());
        for id in open {
            let Some(r) = self.scene.rects.get(id) else { continue };
            let rect = self.drawn(id, r, time);
            let selected = self.selection.as_ref() == Some(id);
            let fill = if self.scene.frame.as_ref() == Some(id) { theme.surface.gamma_multiply(0.35) } else { theme.surface.gamma_multiply(0.6) };
            painter.rect_filled(rect, look.radius_l, fill);
            let stroke = if selected { Stroke::new(look.line_bold, theme.focus) } else { Stroke::new(look.line_thin, theme.border_strong) };
            painter.rect_stroke(rect, look.radius_l, stroke, StrokeKind::Inside);
            let name = self.names.get(id).cloned().or_else(|| result.components.get(id).map(|c| c.name.clone())).unwrap_or_default();
            let band = self.ui.arrange.header * self.camera.zoom;
            let font = FontId::proportional(look.text_label);
            let pad = (self.ui.arrange.pad * self.camera.zoom).min(self.ui.arrange.pad);
            match (band >= look.text_label * self.ui.arrange.line_height).then(|| whole_or_full(painter, &name, &font, rect.width() - pad * 2.0, self.camera.zoom >= 1.0)).flatten() {
                Some(text) => {
                    painter.text(Pos2::new(rect.left() + pad, rect.top() + band / 2.0), Align2::LEFT_CENTER, text, font, theme.muted);
                }
                None => unlabelled.push(Unlabelled { id: id.clone(), rect, text: name, open: true }),
            }
        }
        for line in &self.scene.lines {
            moving |= self.draw_line(painter, line, time, settle);
        }
        for (id, r) in self.scene.cards() {
            let Some(c) = result.components.get(id) else { continue };
            let node = result.view.node(id);
            let rect = self.drawn(id, r, time);
            // A card that holds parts is a module, shown shut, whether or not its parts are here.
            let shut = node.is_some_and(|n| n.members > 0);
            let (animates, named) = self.draw_node(painter, rect, c, node.map_or(Alignment::Observed, |n| n.alignment), node.is_some_and(|n| n.truncated) && !shut, shut, time);
            moving |= animates;
            // With something selected, what isn't related dims; what is gets a ring.
            if let Some(related) = &related {
                if !related.contains(id) {
                    painter.rect_filled(rect.expand(look.space_xs), look.radius_l, theme.bg.gamma_multiply(look.dim));
                } else if self.selection.as_ref() != Some(id) {
                    painter.rect_stroke(rect, look.radius_l, Stroke::new(look.line_bold, theme.focus.gamma_multiply(look.glow_alpha * 2.0)), StrokeKind::Outside);
                }
            }
            // Following a value: what it doesn't pass through fades back.
            if self.trace.is_some() && !self.scene.lines.iter().any(|l| self.on_trace(&l.carries) && [&l.from, &l.to].iter().any(|e| matches!(e, End::Node(n) if n == id))) {
                painter.rect_filled(rect.expand(look.space_xs), look.radius_l, theme.bg.gamma_multiply(look.fade));
            }
            if !named {
                unlabelled.push(Unlabelled { id: id.clone(), rect, text: self.names.get(id).cloned().unwrap_or_else(|| c.name.clone()), open: false });
            }
        }
        // What a story says beside its steps.
        let outward = result.view.spec.direction != arch_graph::view::Direction::In;
        for (id, note) in &self.scene.notes {
            let Some(r) = self.scene.rects.get(id) else { continue };
            let rect = self.drawn(id, r, time);
            // Said only where the step is big enough to read beside.
            if rect.height() < look.text_small * self.ui.arrange.line_height * 2.0 {
                continue;
            }
            let mut said: Vec<(String, egui::Color32)> = Vec::new();
            if note.inside > 0 {
                let what = if outward { "calls inside" } else { "callers further out" };
                said.push((format!("{} {} {what}", egui_phosphor::regular::CARET_RIGHT, note.inside), theme.muted));
            }
            if note.also > 0 {
                let what = if outward { "also called from" } else { "also reached from" };
                said.push((format!("{} {what} {} more", egui_phosphor::regular::ARROWS_MERGE, note.also), theme.info));
            }
            for back in &note.loops {
                let name = self.names.get(back).cloned().unwrap_or_else(|| back.to_string());
                said.push((format!("{} back to {name}", egui_phosphor::regular::ARROW_U_UP_LEFT), theme.warn));
            }
            let font = FontId::proportional(look.text_small);
            let mut x = rect.right() + look.space_m;
            for (text, color) in said {
                let galley = painter.layout_no_wrap(text, font.clone(), color);
                let pill = Rect::from_min_size(Pos2::new(x, rect.center().y - galley.size().y / 2.0 - look.space_hair), galley.size() + Vec2::new(look.space_m * 2.0, look.space_hair * 2.0));
                painter.rect_filled(pill, look.radius_pill, theme.bg);
                painter.rect_stroke(pill, look.radius_pill, Stroke::new(look.line_thin, color.gamma_multiply(0.6)), StrokeKind::Inside);
                painter.galley(pill.min + Vec2::new(look.space_m, look.space_hair), galley, color);
                x = pill.right() + look.space_s;
            }
        }
        // What's reached outside the focus, and what a crowded level holds back.
        for (group, (r, reached)) in &self.scene.outside {
            let rect = self.to_screen(r);
            let selected = self.selection.as_ref() == Some(group);
            painter.rect_filled(rect, look.radius_l, theme.special_bg);
            painter.rect_stroke(rect, look.radius_l, Stroke::new(if selected { look.line_bold } else { look.line_thin }, if selected { theme.focus } else { theme.special }), StrokeKind::Inside);
            let text = outside_text(result, group, *reached);
            if !self.block_text(painter, rect, &text, theme.special) {
                unlabelled.push(Unlabelled { id: group.clone(), rect, text, open: false });
            }
            if related.as_ref().is_some_and(|r| !r.contains(group)) {
                painter.rect_filled(rect.expand(look.space_xs), look.radius_l, theme.bg.gamma_multiply(look.dim));
            }
        }
        if let Some((r, held)) = &self.scene.more {
            let rect = self.to_screen(r);
            painter.rect_filled(rect, look.radius_l, theme.surface);
            let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
            painter.extend(Shape::dashed_line(&corners, Stroke::new(look.line_thin, theme.border_strong), look.space_sm, look.space_s));
            self.block_text(painter, rect, &more_text(held.len()), theme.muted);
        }
        // Names that don't fit their cards float over them.
        let font = FontId::proportional(look.text_label);
        let sized: Vec<(ComponentId, Rect, Vec2)> = unlabelled.iter().map(|u| (u.id.clone(), u.rect, painter.layout_no_wrap(u.text.clone(), font.clone(), theme.text).size())).collect();
        for (id, at) in self.label_plan(&sized, hover) {
            let Some(u) = unlabelled.iter().find(|u| u.id == id) else { continue };
            painter.rect_filled(at, look.radius_pill, theme.overlay);
            painter.rect_stroke(at, look.radius_pill, Stroke::new(look.line_thin, if self.selection.as_ref() == Some(&id) { theme.focus } else { theme.border }), StrokeKind::Inside);
            painter.text(at.center(), Align2::CENTER_CENTER, &u.text, font.clone(), if u.open { theme.muted } else { theme.text_strong });
        }
        moving
    }

    /// Where the selection's lines leave the screen: a marker at the edge
    /// naming what's at the other end (an arrow for which way), placed where
    /// the line goes out and kept clear of the others. Returns them for clicks.
    fn edge_markers(&self, painter: &egui::Painter) -> Vec<(ComponentId, Rect)> {
        let Some(sel) = self.selection.clone() else { return Vec::new() };
        let look = crate::theme::look();
        let theme = colors();
        let area = self.open_area.shrink(look.space_m);
        let font = FontId::proportional(look.text_small);
        let mut placed: Vec<(ComponentId, Rect)> = Vec::new();
        let mut hidden = 0usize;
        // Heaviest lines first: they get a marker when there's no room for all.
        let mut lines: Vec<&scene::Line> = self.scene.lines.iter().collect();
        lines.sort_by(|a, b| b.count.cmp(&a.count));
        for line in lines {
            let is = |e: &End| matches!(e, End::Node(id) | End::Outside(id) if *id == sel);
            let (other, outgoing) = if is(&line.from) { (&line.to, true) } else if is(&line.to) { (&line.from, false) } else { continue };
            let (End::Node(other_id) | End::Outside(other_id)) = other else { continue };
            let Some(far) = self.scene.rect_of(other).map(|r| self.to_screen(&r)) else { continue };
            if area.intersects(far) || placed.iter().any(|(id, _)| id == other_id) {
                continue;
            }
            let mut points: Vec<Pos2> = line.points.iter().map(|p| self.world_to_screen(*p)).collect();
            if !outgoing {
                points.reverse();
            }
            let Some(out) = points.iter().position(|p| !area.contains(*p)) else { continue };
            let exit = area.clamp(points[out]);
            let name = self.names.get(other_id).cloned().or_else(|| self.component(other_id).map(|c| c.name.clone())).unwrap_or_else(|| other_id.to_string());
            let arrow = match (line.both_ways, outgoing) {
                (true, _) => egui_phosphor::regular::ARROWS_LEFT_RIGHT,
                (false, true) => egui_phosphor::regular::ARROW_RIGHT,
                (false, false) => egui_phosphor::regular::ARROW_LEFT,
            };
            let galley = painter.layout_no_wrap(format!("{arrow} {name}"), font.clone(), theme.text_strong);
            let size = galley.size() + Vec2::new(look.space_m * 2.0, look.space_xs * 2.0);
            let mut rect = Rect::from_center_size(exit, size);
            rect = rect.translate(Vec2::new((area.left() - rect.left()).max(0.0) + (area.right() - rect.right()).min(0.0), (area.top() - rect.top()).max(0.0) + (area.bottom() - rect.bottom()).min(0.0)));
            if placed.len() >= self.ui.levels.max_markers {
                hidden += 1;
                continue;
            }
            // Along the edge it sits on, clear of the markers already there;
            // past the edge's end, a column (or row) further in.
            let on_side = (exit.x - area.left()).abs() < 1.0 || (exit.x - area.right()).abs() < 1.0;
            let inward = if (exit.x - area.right()).abs() < 1.0 || (exit.y - area.bottom()).abs() < 1.0 { -1.0 } else { 1.0 };
            let mut tries = 0;
            while placed.iter().any(|(_, r)| r.expand(look.space_hair).intersects(rect)) && tries < 256 {
                rect = rect.translate(if on_side { Vec2::new(0.0, size.y + look.space_xs) } else { Vec2::new(size.x + look.space_xs, 0.0) });
                if !area.contains_rect(rect) {
                    let wrap = placed.iter().map(|(_, r)| if on_side { r.width() } else { r.height() }).fold(0.0f32, f32::max) + look.space_s;
                    rect = if on_side {
                        Rect::from_min_size(Pos2::new(rect.min.x + inward * wrap, area.top()), rect.size())
                    } else {
                        Rect::from_min_size(Pos2::new(area.left(), rect.min.y + inward * wrap), rect.size())
                    };
                }
                tries += 1;
            }
            let st = style().edge(&theme, line.kind, provenance_of(line.alignment), line.alignment, line.overlay);
            painter.rect_filled(rect, look.radius_pill, theme.overlay);
            painter.rect_stroke(rect, look.radius_pill, Stroke::new(look.line_thin, st.color), StrokeKind::Inside);
            painter.galley(rect.min + Vec2::new(look.space_m, look.space_xs), galley, theme.text_strong);
            placed.push((other_id.clone(), rect));
        }
        if hidden > 0 {
            let galley = painter.layout_no_wrap(format!("+{hidden} more off screen"), font.clone(), theme.muted);
            let size = galley.size() + Vec2::new(look.space_m * 2.0, look.space_xs * 2.0);
            let rect = Rect::from_min_size(Pos2::new(area.right() - size.x, area.bottom() - size.y), size);
            painter.rect_filled(rect, look.radius_pill, theme.overlay);
            painter.galley(rect.min + Vec2::new(look.space_m, look.space_xs), galley, theme.muted);
        }
        placed
    }

    /// A block's one line, centred, when it fits; says whether it did.
    fn block_text(&self, painter: &egui::Painter, rect: Rect, text: &str, color: egui::Color32) -> bool {
        let look = crate::theme::look();
        let font = FontId::proportional(look.text_body);
        if rect.height() < look.text_body * self.ui.arrange.line_height {
            return false;
        }
        let pad = (self.ui.arrange.pad * self.camera.zoom).min(self.ui.arrange.pad);
        match whole_or_full(painter, text, &font, rect.width() - pad * 2.0, self.camera.zoom >= 1.0) {
            Some(shown) => {
                painter.text(rect.center(), Align2::CENTER_CENTER, shown, font, color);
                true
            }
            None => false,
        }
    }

    /// Where the names that don't fit their cards go: over the card (an open
    /// module's at its top), most important first (the selection, what it's
    /// tied to, open modules, shut modules, the most tied), each only where
    /// it covers no other; the one under the pointer always.
    pub(crate) fn label_plan(&self, items: &[(ComponentId, Rect, Vec2)], hover: Option<Pos2>) -> Vec<(ComponentId, Rect)> {
        let look = crate::theme::look();
        let sel = self.selection.as_ref();
        let mut degree: BTreeMap<&ComponentId, u32> = BTreeMap::new();
        let mut near: BTreeSet<&ComponentId> = BTreeSet::new();
        for line in &self.scene.lines {
            for (end, other) in [(&line.from, &line.to), (&line.to, &line.from)] {
                if let End::Node(id) | End::Outside(id) = end {
                    *degree.entry(id).or_default() += line.count;
                    if let End::Node(o) | End::Outside(o) = other
                        && Some(o) == sel
                    {
                        near.insert(id);
                    }
                }
            }
        }
        let rank = |id: &ComponentId| {
            let class = if Some(id) == sel {
                0
            } else if near.contains(id) {
                1
            } else if self.scene.containers.contains(id) {
                2
            } else if self.is_module(id) {
                3
            } else {
                4
            };
            (class, Reverse(degree.get(id).copied().unwrap_or(0)), id.clone())
        };
        let mut order: Vec<&(ComponentId, Rect, Vec2)> = items.iter().collect();
        order.sort_by_key(|(id, _, _)| rank(id));
        let pill = |(id, card, size): &(ComponentId, Rect, Vec2)| {
            let size = *size + Vec2::new(look.space_m * 2.0, look.space_xs * 2.0);
            let at = if self.scene.containers.contains(id) { Rect::from_min_size(card.min + Vec2::splat(look.space_s), size) } else { Rect::from_center_size(card.center(), size) };
            (id.clone(), at)
        };
        let mut placed: Vec<(ComponentId, Rect)> = Vec::new();
        for item in &order {
            let (id, at) = pill(item);
            if !placed.iter().any(|(_, r)| r.expand(look.space_hair).intersects(at)) && at.intersects(self.canvas) {
                placed.push((id, at));
            }
        }
        // Under the pointer: shown whatever it covers (drawn last, on top).
        if let Some(pos) = hover
            && let Some(item) = order.iter().filter(|(_, card, _)| card.contains(pos)).min_by(|a, b| a.1.area().total_cmp(&b.1.area()))
            && !placed.iter().any(|(id, _)| *id == item.0)
        {
            placed.push(pill(item));
        }
        placed
    }

    /// Where a node is drawn now: gliding from its old place while cards move.
    fn drawn(&self, id: &ComponentId, r: &WorldRect, time: f64) -> Rect {
        let to = self.to_screen(r);
        let Some(m) = &self.moving else { return to };
        let (Some(from), Some(t)) = (m.from.get(id), move_t(m.started, time)) else { return to };
        let from = self.to_screen(from);
        Rect::from_min_max(from.min.lerp(to.min, t), from.max.lerp(to.max, t))
    }

    fn world_to_screen(&self, (x, y): (f32, f32)) -> Pos2 {
        self.canvas.min + self.camera.pan + Vec2::new(x, y) * self.camera.zoom
    }

    /// One merged line: its corners rounded, as thick as how many relations
    /// it stands for, its count where it's worth saying, and a stub with the
    /// count where it leaves the level. Returns whether it moves (dashes).
    fn draw_line(&self, painter: &egui::Painter, line: &scene::Line, time: f64, settle: f32) -> bool {
        if line.points.len() < 2 {
            return false;
        }
        let look = crate::theme::look();
        let theme = colors();
        let st = if line.unconfirmed { style().unconfirmed(&theme) } else { style().edge(&theme, line.kind, provenance_of(line.alignment), line.alignment, line.overlay) };
        // With something selected, lines that don't touch it fade back;
        // following a value, lines that don't carry it fade further.
        let touches = self.selection.as_ref().is_none_or(|s| [&line.from, &line.to].iter().any(|e| matches!(e, End::Node(id) | End::Outside(id) if id == s)));
        let traced = self.trace.is_none() || self.on_trace(&line.carries);
        let lit = self.selection.is_some() && touches;
        let color = if traced && touches { st.color } else if traced { st.color.gamma_multiply(1.0 - look.dim) } else { st.color.gamma_multiply((1.0 - look.dim) * (1.0 - look.fade)) }.gamma_multiply(settle);
        let width = st.width * (1.0 + (line.count as f32).log2() * look.line_count_gain).min(look.line_count_max) + if lit { look.line_thin } else { 0.0 };
        let stroke = Stroke::new(width, color);
        let mut corners: Vec<Pos2> = line.points.iter().map(|p| self.world_to_screen(*p)).collect();
        // A line leaving the level is a short stub on its card's edge (with
        // its count), drawn whole only when either end is selected.
        let outward = matches!((&line.from, &line.to), (End::Outside(_), _) | (_, End::Outside(_)));
        if outward && !lit {
            let inside_first = matches!(line.from, End::Node(_));
            if !inside_first {
                corners.reverse();
            }
            let reach = self.ui.arrange.gap_x * self.camera.zoom / 2.0;
            let mut stub = vec![corners[0]];
            let mut left = reach;
            for pair in corners.windows(2) {
                let step = (pair[1] - pair[0]).length();
                if step >= left {
                    stub.push(pair[0] + (pair[1] - pair[0]) * (left / step.max(f32::EPSILON)));
                    break;
                }
                stub.push(pair[1]);
                left -= step;
            }
            let tip = stub[stub.len() - 1];
            let (glyph, color) = (if inside_first { egui_phosphor::regular::ARROW_RIGHT } else { egui_phosphor::regular::ARROW_LEFT }, st.color.gamma_multiply(settle));
            painter.add(Shape::line(stub, Stroke::new(st.width, color.gamma_multiply(if self.selection.is_some() { 1.0 - look.dim } else { 1.0 }))));
            if self.selection.is_none() {
                line_pill(painter, tip, &format!("{glyph} {}", line.count), color);
            }
            return false;
        }
        let points = rounded(&corners, look.radius_l);
        let mut moving = false;
        if st.dashed {
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
            painter.add(Shape::line(points, stroke));
        }
        // Arrow heads: where it goes, and where it comes back to (both ways).
        let n = corners.len();
        let mut heads = vec![(corners[n - 1], corners[n - 2])];
        if line.both_ways {
            heads.push((corners[0], corners[1]));
        }
        for (tip, before) in heads {
            let dir = (tip - before).normalized();
            let side = Vec2::new(-dir.y, dir.x) * look.space_s;
            painter.add(Shape::convex_polygon(vec![tip, tip - dir * look.space_m + side, tip - dir * look.space_m - side], color, Stroke::NONE));
        }
        // What it carries and how many relations it stands for, on its
        // longest stretch; and the count where it leaves the level.
        let said = {
            let mut parts: Vec<String> = line.carries.iter().take(2).cloned().collect();
            if line.carries.len() > 2 {
                parts.push(format!("+{}", line.carries.len() - 2));
            }
            let what = parts.join(", ");
            match (line.count > 1, what.is_empty()) {
                (true, true) => line.count.to_string(),
                (true, false) => format!("{} · {what}", line.count),
                (false, _) => what,
            }
        };
        // Said only for the selection's own lines (or the value followed).
        let legible = lit || (self.trace.is_some() && traced);
        if !said.is_empty() && legible {
            let (a, b) = corners.windows(2).map(|w| (w[0], w[1])).max_by(|x, y| (x.1 - x.0).length().total_cmp(&(y.1 - y.0).length())).unwrap_or((corners[0], corners[1]));
            line_pill(painter, a.lerp(b, 0.5), &said, color);
        }
        if let Some(exit) = line.exit
            && legible
        {
            line_pill(painter, self.world_to_screen(exit), &line.count.to_string(), color);
        }
        moving
    }

    /// Draws one card; returns whether it animates (the agent's pulse) and
    /// whether its name fit on it.
    #[allow(clippy::too_many_arguments)]
    fn draw_node(&self, painter: &egui::Painter, rect: Rect, c: &Component, alignment: Alignment, truncated: bool, closed: bool, time: f64) -> (bool, bool) {
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
        painter.rect_filled(rect, look.radius_l, style.node_fill(&theme, selected));
        let stroke = Stroke::new(outline.width, outline.color);
        if outline.dashed {
            let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
            painter.extend(Shape::dashed_line(&corners, stroke, look.space_sm, look.space_s));
        } else {
            painter.rect_stroke(rect, look.radius_l, stroke, StrokeKind::Inside);
        }
        // A shut module: a second edge behind, like a stack.
        if closed {
            painter.rect_stroke(rect.translate(Vec2::splat(look.space_xs)), look.radius_l, Stroke::new(look.line_thin, theme.border), StrokeKind::Outside);
        }
        // The stage as a stripe on the left edge.
        let stripe = look.space_s.min(rect.width() / 4.0);
        let r = look.radius_l as u8;
        painter.rect_filled(Rect::from_min_size(rect.min, Vec2::new(stripe, rect.height())), egui::CornerRadius { nw: r, sw: r, ne: 0, se: 0 }, style.stage_color(&theme, c.stage));
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
            painter.text(rect.left_top() + Vec2::new(stripe + look.space_xs, look.space_xs), Align2::LEFT_TOP, glyph, FontId::proportional(look.text_small), color);
        }
        // Text keeps its size: the lines that fit, the name first.
        let pad = (self.ui.arrange.pad * self.camera.zoom).min(self.ui.arrange.pad);
        let left = rect.left() + stripe + pad;
        let room = rect.right() - pad - left;
        let mut y = rect.top() + pad * 0.75;
        let mut named = false;
        for (k, line) in self.card_text(c, alignment, closed).into_iter().enumerate() {
            let (font, color) = match line.role {
                LineRole::Name => (FontId::proportional(look.text_title), theme.text_strong),
                LineRole::Meta => (FontId::proportional(look.text_small), theme.muted),
                LineRole::Shape => (FontId::monospace(look.text_small), theme.info),
                LineRole::Signature | LineRole::Location => (FontId::monospace(look.text_small), theme.muted),
                LineRole::Badges => (FontId::proportional(look.text_small), theme.warn),
            };
            let h = font.size * self.ui.arrange.line_height;
            if y + h > rect.bottom() - pad * 0.5 {
                break;
            }
            // The name whole, or not here (it floats); other lines may shorten.
            let fitted = if k == 0 { whole_or_full(painter, &line.text, &font, room, self.camera.zoom >= 1.0) } else { fit_text(painter, &line.text, &font, room) };
            let Some(text) = fitted else { break };
            painter.with_clip_rect(rect).text(Pos2::new(left, y), Align2::LEFT_TOP, text, font, color);
            named |= k == 0;
            y += h;
        }
        if truncated {
            painter.text(rect.right_center() + Vec2::new(look.space_s, 0.0), Align2::LEFT_CENTER, "…", FontId::proportional(look.text_heading), theme.muted);
        }
        (pointed, named)
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
                    (Some(a), _) => ui.label(RichText::new(format!("{} {}", if crate::status::is_folder(a) { egui_phosphor::regular::FOLDER } else { egui_phosphor::regular::FILE_CODE }, crate::status::place(a))).monospace().size(crate::theme::look().text_body)),
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
    Up,
    ShowAll,
    /// The selection's story in the named view (forwards or backwards).
    Story(&'static str),
    /// Follow a value through the data flow (or stop).
    Follow,
    Unfollow,
    /// See the relations at a level (by name; empty: the parts), or a level
    /// coarser or finer.
    Level,
    Coarser,
    Finer,
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
        let levels = self.levels();
        if !levels.is_empty() {
            let mut choices = vec![Choice::new("", PARTS).current(self.grain.is_none())];
            choices.extend(levels.iter().map(|g| Choice::new(&g.name, &g.name).current(self.grain.as_deref() == Some(g.name.as_str()))));
            out.push(f("graph.level", "See it by level (functions, types, modules, packages, folders)", G::Level).param(Param::Choice(choices)).toolbar(Control, "Level", Some(egui_phosphor::regular::STACK)).group(Group::View));
            out.push(f("graph.coarser", "A level up: group into what holds them", G::Coarser).icon(egui_phosphor::regular::ARROW_LINE_UP).group(Group::View).importance(SECONDARY));
            out.push(f("graph.finer", "A level down: open into the parts", G::Finer).icon(egui_phosphor::regular::ARROW_LINE_DOWN).group(Group::View).importance(SECONDARY));
        }
        out.push(f("graph.depth_less", "Show fewer hops", G::Depth(-1)).toolbar(Button, "", Some(egui_phosphor::regular::MINUS)).group(Group::View).importance(MINOR).jump());
        out.push(f("graph.depth_more", "Show more hops", G::Depth(1)).toolbar(Button, "", Some(egui_phosphor::regular::PLUS)).group(Group::View).importance(MINOR).jump());
        if !self.nav.roots.is_empty() {
            if self.result.as_ref().is_some_and(|r| r.view.spec.layout == LayoutKind::Nested && !r.view.trail.is_empty()) {
                out.push(f("graph.up", "Up a level", G::Up).toolbar(Button, "", Some(egui_phosphor::regular::ARROW_UP)).group(Group::View).jump());
            }
            out.push(f("graph.unfocus", "Back to the view's entry points", G::Unfocus).toolbar(Button, "All", Some(egui_phosphor::regular::ARROWS_OUT)).group(Group::View).jump());
        }
        if let Some((_, held)) = &self.scene.more {
            out.push(f("graph.show_all", &format!("Show every part ({} more)", held.len()), G::ShowAll).group(Group::View).importance(SECONDARY));
        }
        out.push(f("graph.fit", "Fit to screen", G::Fit).toolbar(Button, "", Some(egui_phosphor::regular::CORNERS_OUT)).group(Group::View).importance(MINOR));
        out.push(f("graph.zoom_in", "Zoom in", G::Zoom(1.2)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_PLUS).group(Group::View).importance(MINOR));
        out.push(f("graph.zoom_out", "Zoom out", G::Zoom(1.0 / 1.2)).icon(egui_phosphor::regular::MAGNIFYING_GLASS_MINUS).group(Group::View).importance(MINOR));
        out.push(f("graph.refresh", "Refresh the view", G::Refresh).group(Group::View).importance(MINOR));
        out.push(f("graph.legend", if self.legend_open { "Hide the legend" } else { "Show the legend" }, G::Legend).toolbar(Button, "", Some(egui_phosphor::regular::INFO)).group(Group::View).importance(MINOR));
        if self.trace.is_some() {
            out.push(f("graph.unfollow", "Stop following", G::Unfollow).toolbar(Button, "", Some(egui_phosphor::regular::X)).group(Group::View).importance(SECONDARY));
        }
        if self.agent.is_some() {
            out.push(f("graph.agent_return", "Return to where you were before the agent moved the view", G::AgentReturn).group(Group::Navigate).importance(PRIMARY).jump());
        }
        if self.selection.is_none() {
            out.push(f("graph.next_call", "Next call, in call order", G::Call { forward: true }).group(Group::Navigate).importance(MINOR).jump());
            out.push(f("graph.prev_call", "Previous call, in call order", G::Call { forward: false }).group(Group::Navigate).importance(MINOR).jump());
        }
        if self.selection.is_some() {
            let has_story = |name: &str| self.catalog.as_ref().is_some_and(|c| c.get(name).is_some());
            if has_story(STORY) {
                out.push(f("graph.story", "Tell its story (what it calls, in order)", G::Story(STORY)).icon(egui_phosphor::regular::LIST_NUMBERS).group(Group::View).importance(SECONDARY).jump());
            }
            if has_story(REACHED_BY) {
                out.push(f("graph.reached_by", "What reaches it (its callers, outwards)", G::Story(REACHED_BY)).icon(egui_phosphor::regular::ARROW_BEND_UP_LEFT).group(Group::View).importance(SECONDARY).jump());
            }
            let values = self.follow_choices();
            if !values.is_empty() && self.catalog.as_ref().is_some_and(|c| c.get(DATA_PATH).is_some()) {
                let choices = values.iter().map(|v| Choice::new(v, v).current(self.trace.as_deref() == Some(v.as_str()))).collect();
                out.push(f("graph.follow", "Follow this data through the code", G::Follow).param(Param::Choice(choices)).icon(egui_phosphor::regular::PATH).group(Group::View).importance(SECONDARY).jump());
            }
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
                let center = self.selected_rect().map_or(self.canvas.center(), |r| r.center());
                self.zoom_by_user(factor, center);
            }
            GraphCommand::Up => self.up_level(),
            GraphCommand::Follow => {
                let value = value.or_else(|| self.follow_choices().into_iter().next());
                if let (Some(value), Some(id)) = (value, self.selection.clone()) {
                    self.trace = Some(value);
                    // Its whole path: where it comes from and where it goes.
                    self.navigate(NavChange::View(DATA_PATH.to_owned()));
                    self.navigate(NavChange::Focus(id));
                }
            }
            GraphCommand::Unfollow => self.trace = None,
            GraphCommand::Level => self.set_grain(value.filter(|v| !v.is_empty())),
            GraphCommand::Coarser | GraphCommand::Finer => {
                if let Err(why) = self.step_grain(matches!(command, GraphCommand::Coarser)) {
                    *cx.note = Some(why);
                }
            }
            GraphCommand::Story(view) => {
                if let Some(id) = self.selection.clone() {
                    self.navigate(NavChange::View(view.to_owned()));
                    self.navigate(NavChange::Focus(id));
                }
            }
            GraphCommand::ShowAll => self.show_everything(),
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
        let nested = self.result.as_ref().is_some_and(|r| r.view.spec.layout == LayoutKind::Nested);
        let holds = |id: &ComponentId| self.result.as_ref().and_then(|r| r.view.node(id)).is_some_and(|n| n.members > 0) || self.scene.outside.contains_key(id);
        self.selected().map(|c| if nested && holds(&c.id) { format!("Into {}", c.name) } else { format!("Focus on {}", c.name) })
    }

    fn leave_label(&self) -> Option<String> {
        let result = self.result.as_ref()?;
        if result.view.spec.layout != LayoutKind::Nested || self.nav.roots.is_empty() {
            return None;
        }
        Some(match result.view.trail.last() {
            Some(up) => format!("Up to {}", self.component(up).map_or_else(|| up.to_string(), |c| c.name.clone())),
            None => "Up to the whole view".to_owned(),
        })
    }

    fn leave(&mut self) -> Step {
        if self.nav.roots.is_empty() {
            return Step::Nothing("Already showing the whole view.".into());
        }
        self.up_level();
        Step::Done
    }

    fn fold(&mut self, fold: Fold) {
        // A story's branches collapse and open in place.
        if self.is_story() {
            match fold {
                Fold::OpenAll => self.shut.clear(),
                Fold::CloseAll => self.shut = self.branches().into_iter().collect(),
                Fold::Open | Fold::Close | Fold::Toggle => {
                    let Some(step) = self.selection.clone() else { return };
                    let close = match fold {
                        Fold::Open => false,
                        Fold::Close => true,
                        _ => !self.shut.contains(&step),
                    };
                    if close { self.shut.insert(step) } else { self.shut.remove(&step) };
                }
            }
            self.rebuild(true);
            if let Some(sel) = self.selection.clone()
                && !self.shown(&sel)
            {
                let holder = self.shut.iter().find(|b| self.shown(b)).cloned();
                self.select(holder);
            }
            return;
        }
        let Some(result) = &self.result else { return };
        let modules: BTreeSet<ComponentId> = result.view.edges.iter().filter(|e| e.kind == RelationKind::Contains && self.is_module(&e.source)).map(|e| e.source.clone()).collect();
        match fold {
            Fold::OpenAll => {
                let all: Vec<ComponentId> = modules.into_iter().collect();
                self.set_folds(&all, true);
            }
            Fold::CloseAll => {
                let all: Vec<ComponentId> = modules.into_iter().collect();
                self.set_folds(&all, false);
            }
            Fold::Open | Fold::Close | Fold::Toggle => {
                let Some(c) = self.fold_target() else { return };
                let open = match fold {
                    Fold::Open => true,
                    Fold::Close => false,
                    _ => self.closed(&c),
                };
                self.set_folds(&[c], open);
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

    fn spots(&self) -> Vec<Spot> {
        let anchor = self.selected_rect().map(|r| r.center()).unwrap_or(self.canvas.center());
        let more = self.scene.more.iter().map(|(r, _)| (MORE.to_owned(), r));
        let mut visible: Vec<(f32, Spot)> = self
            .boxes()
            .map(|(id, r)| (id.to_string(), r))
            .chain(more)
            .map(|(key, r)| (self.to_screen(r), key))
            .filter(|(rect, _)| rect.intersects(self.open_area))
            // The part on screen: a label never sits over the inspector.
            .map(|(rect, key)| (rect.center().distance(anchor), Spot { key, rect: rect.intersect(self.open_area) }))
            .collect();
        visible.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.key.cmp(&b.1.key)));
        visible.into_iter().map(|(_, spot)| spot).collect()
    }

    fn cursor_key(&self) -> Option<String> {
        self.selection.as_ref().map(|s| s.to_string())
    }

    fn save_view(&mut self) {
        self.saved_view = Some((self.camera, self.open.clone(), self.drawer_open));
    }

    fn restore_view(&mut self) {
        if let Some((camera, open, drawer_open)) = self.saved_view.take() {
            self.open = open;
            self.rebuild(true);
            self.moving = None;
            self.camera = camera;
            self.drawer_open = drawer_open;
        }
    }

    fn move_to(&mut self, key: &str) {
        if key == MORE {
            self.show_everything();
            return;
        }
        let id = ComponentId::new(key);
        if self.component(&id).is_some() {
            self.bring_into_view(&id);
            self.select(Some(id));
        }
    }
}

/// The lines a node card shows at `detail` (badges from its review `mark`).
pub fn card_lines(c: &Component, alignment: Alignment, detail: Detail, mark: Option<&crate::review::NodeMark>) -> Vec<CardLine> {
    let line = |role, text: String| CardLine { role, text };
    let mut out = vec![line(LineRole::Name, c.name.clone())];
    if detail == Detail::Far {
        return out;
    }
    let location = match (&c.anchor, &c.proposed) {
        (Some(a), _) if crate::status::is_folder(a) => crate::status::place(a),
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
    if let Some(mark) = mark {
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

/// A name drawn off its card, waiting for a place.
struct Unlabelled {
    id: ComponentId,
    rect: Rect,
    text: String,
    open: bool,
}

/// How far along a move is (eased), from when it started; None before it
/// has (or with moves turned off).
fn move_t(started: Option<f64>, time: f64) -> Option<f32> {
    let secs = crate::theme::look().move_seconds;
    if secs <= 0.0 {
        return None;
    }
    let t = ((time - started?) as f32 / secs).clamp(0.0, 1.0);
    Some(1.0 - (1.0 - t).powi(3))
}

/// A polyline with its corners rounded.
fn rounded(points: &[Pos2], radius: f32) -> Vec<Pos2> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    for w in points.windows(3) {
        let (a, b, c) = (w[0], w[1], w[2]);
        let r = radius.min((b - a).length() / 2.0).min((c - b).length() / 2.0);
        let (p0, p1) = (b + (a - b).normalized() * r, b + (c - b).normalized() * r);
        for k in 0..=4 {
            let t = k as f32 / 4.0;
            let u = 1.0 - t;
            out.push(Pos2::new(u * u * p0.x + 2.0 * u * t * b.x + t * t * p1.x, u * u * p0.y + 2.0 * u * t * b.y + t * t * p1.y));
        }
    }
    out.push(points[points.len() - 1]);
    out
}

/// A small label on a line (a count, what it carries).
fn line_pill(painter: &egui::Painter, at: Pos2, text: &str, color: egui::Color32) {
    let look = crate::theme::look();
    let theme = colors();
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(look.text_tiny), color);
    let rect = Rect::from_center_size(at, galley.size() + Vec2::new(look.space_s * 2.0, look.space_hair * 2.0));
    painter.rect_filled(rect, look.radius_pill, theme.bg);
    painter.rect_stroke(rect, look.radius_pill, Stroke::new(look.line_thin, color), StrokeKind::Inside);
    painter.galley(rect.center() - galley.size() / 2.0, galley, color);
}

/// What a step takes and gives, by type (`Plan, &str -> Report`), marked
/// when what comes out isn't one of what goes in (the data changes shape).
fn shape_of(c: &Component) -> Option<String> {
    let sig = c.signature.as_ref()?;
    let ins: Vec<String> = sig.params.iter().filter_map(|p| p.ty.clone()).collect();
    let out = sig.returns.clone()?;
    let mut text = format!("{} -> {out}", if ins.is_empty() { "()".to_owned() } else { ins.join(", ") });
    let took: BTreeSet<String> = ins.iter().flat_map(|t| type_names(t)).collect();
    let gives = type_names(&out);
    if !ins.is_empty() && !gives.is_empty() && gives.iter().all(|g| !took.contains(g)) {
        let (glyph, word, _) = crate::status::look("reshapes");
        text = format!("{text}  {glyph} {word}");
    }
    Some(text)
}

/// The type names in a written type (`Result<Report, Error>` -> Report,
/// Error): capitalised words, wrappers and all.
fn type_names(written: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in written.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')) {
        if word.chars().next().is_some_and(char::is_uppercase) && !out.iter().any(|w| w == word) {
            out.push(word.to_owned());
        }
    }
    out
}

/// Whether `text` names `value` as a whole word (`Store.count` names Store).
fn mentions(text: &str, value: &str) -> bool {
    text.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')).any(|w| w == value) || text == value
}

/// A shut module's second line: what it is and how much is inside.
fn module_meta(c: &Component, members: u32) -> String {
    format!("{} · {members} parts", c.type_name)
}

/// The block for what's reached outside the focus.
fn outside_text(result: &ViewResult, group: &ComponentId, reached: u32) -> String {
    let name = result.components.get(group).map_or_else(|| group.to_string(), |c| c.name.clone());
    format!("{reached} more in {name}")
}

fn more_text(hidden: usize) -> String {
    format!("+{hidden} more")
}

/// What cards say, sized for the scene (the text keeps its size at every
/// zoom, so cards are as big as their text at zoom 1).
struct Cards<'a> {
    result: &'a ViewResult,
    metrics: &'a scene::Metrics,
    names: BTreeMap<ComponentId, String>,
}

impl<'a> Cards<'a> {
    fn new(result: &'a ViewResult, metrics: &'a scene::Metrics) -> Self {
        let mut count: BTreeMap<&str, usize> = BTreeMap::new();
        for n in &result.view.nodes {
            if let Some(c) = result.components.get(&n.id) {
                *count.entry(c.name.as_str()).or_default() += 1;
            }
        }
        // A name two shown components share comes with the part above it.
        let names = result
            .view
            .nodes
            .iter()
            .filter_map(|n| result.components.get(&n.id))
            .map(|c| {
                let name = if count.get(c.name.as_str()).copied().unwrap_or(0) > 1 { qualified(c) } else { c.name.clone() };
                (c.id.clone(), name)
            })
            .collect();
        Self { result, metrics, names }
    }

    fn lines(&self, id: &ComponentId, closed: bool) -> Vec<(String, f32)> {
        let look = crate::theme::look();
        let Some(c) = self.result.components.get(id) else { return vec![(id.to_string(), look.text_title)] };
        let name = self.names.get(id).cloned().unwrap_or_else(|| c.name.clone());
        if closed || self.result.view.node(id).is_some_and(|n| n.members > 0) {
            let members = self.result.view.node(id).map_or(0, |n| n.members);
            return vec![(name, look.text_title), (module_meta(c, members), look.text_small)];
        }
        let alignment = self.result.view.node(id).map_or(Alignment::Observed, |n| n.alignment);
        let mut lines: Vec<(String, f32)> = card_lines(c, alignment, Detail::Mid, None).into_iter().map(|l| (l.text, look.text_small)).collect();
        lines[0] = (name, look.text_title);
        if self.result.view.spec.kinds.contains(&RelationKind::DataFlow)
            && let Some(shape) = shape_of(c)
        {
            lines.insert(1, (shape, look.text_small));
        }
        lines
    }
}

impl Measure for Cards<'_> {
    fn card(&self, id: &ComponentId, closed: bool) -> (f32, f32) {
        let lines = self.lines(id, closed);
        let refs: Vec<(&str, f32)> = lines.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        self.metrics.card(&refs)
    }
    fn outside(&self, group: &ComponentId, reached: u32) -> (f32, f32) {
        self.metrics.card(&[(&outside_text(self.result, group, reached), crate::theme::look().text_body)])
    }
    fn more(&self, hidden: usize) -> (f32, f32) {
        self.metrics.card(&[(&more_text(hidden), crate::theme::look().text_body)])
    }
}

/// A name with the part of the id above it (`app.helpers`, `tests::common`).
fn qualified(c: &Component) -> String {
    let symbol = c.id.as_str().split_once(':').map_or(c.id.as_str(), |(_, s)| s);
    let cut = symbol.strip_suffix(c.name.as_str()).unwrap_or(symbol);
    let sep: String = cut.chars().rev().take_while(|ch| !ch.is_alphanumeric() && *ch != '_').collect::<Vec<_>>().into_iter().rev().collect();
    let above = cut[..cut.len() - sep.len()].rsplit(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-').next().unwrap_or_default();
    if above.is_empty() { c.name.clone() } else { format!("{above}{sep}{}", c.name) }
}

/// Text for a card or block: whole when it fits; shortened only when the
/// card is at full size (`full`), since a longer name than any card is wide
/// can't fit anywhere; otherwise none (the name floats over it instead).
fn whole_or_full(painter: &egui::Painter, text: &str, font: &FontId, width: f32, full: bool) -> Option<String> {
    let shown = fit_text(painter, text, font, width)?;
    (shown == text || full).then_some(shown)
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
    use arch_graph::view::{ViewEdge, ViewNode};

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
        let edge = |a: &str, b: &str| ViewEdge { id: arch_graph::RelationId(format!("calls:{a}->{b}")), kind: RelationKind::Calls, source: id(a), target: id(b), alignment: Alignment::Observed, overlay: false, order: None, carries: None, unconfirmed: None };
        ViewResult {
            view: View {
                spec: ViewSpec { name: "calls".into(), description: String::new(), kinds: vec![RelationKind::Calls], direction: Default::default(), depth: 3, layout: Default::default(), overlays: vec![], include_ancestors: false, links: vec![], roots: vec![], filters: Default::default() },
                roots: vec![id("rust:a::run")],
                nodes: ids.iter().map(|s| ViewNode { id: id(s), depth: Some(0), truncated: false, alignment: Alignment::Observed, members: 0, ancestors: vec![] }).collect(),
                edges: vec![edge("rust:a::run", "rust:a::step"), edge("rust:a::step", "rust:a::save"), edge("rust:a::run", "rust:a::log")],
                links: vec![],
                trail: vec![],
                outside: vec![],
            },
            text: String::new(),
            components: ids.iter().map(|s| (id(s), component(s))).collect(),
        }
    }

    /// The sample inside a module `rust:a` (holding all four), plus a
    /// top-level module `rust:b`, as a structure view (modules nest).
    pub(crate) fn nested_result() -> ViewResult {
        let mut r = sample_result();
        r.view.spec.name = "structure".into();
        r.view.spec.layout = arch_graph::view::LayoutKind::Nested;
        r.view.roots.clear();
        for id in ["rust:a", "rust:b"] {
            let mut c = r.components[&ComponentId::new("rust:a::run")].clone();
            c.id = ComponentId::new(id);
            c.name = id.trim_start_matches("rust:").into();
            c.type_name = "module".into();
            r.components.insert(c.id.clone(), c);
            let members = if id == "rust:a" { 4 } else { 0 };
            r.view.nodes.push(ViewNode { id: ComponentId::new(id), depth: Some(0), truncated: false, alignment: Alignment::Observed, members, ancestors: vec![] });
        }
        for part in ["rust:a::run", "rust:a::step", "rust:a::save", "rust:a::log"] {
            r.view.edges.push(ViewEdge { id: arch_graph::RelationId(format!("contains:rust:a->{part}")), kind: RelationKind::Contains, source: ComponentId::new("rust:a"), target: ComponentId::new(part), alignment: Alignment::Observed, overlay: false, order: None, carries: None, unconfirmed: None });
        }
        r
    }

    /// The sample's functions inside modules `a` (all four) and `b` (one
    /// more, called from two of them): a calls view with a level to see it at.
    pub(crate) fn leveled_result() -> ViewResult {
        let mut r = sample_result();
        for id in ["rust:a", "rust:b"] {
            let mut c = r.components[&ComponentId::new("rust:a::run")].clone();
            c.id = ComponentId::new(id);
            c.name = id.trim_start_matches("rust:").into();
            c.type_name = "module".into();
            r.components.insert(c.id.clone(), c);
        }
        r.components.get_mut(&ComponentId::new("rust:a::step")).unwrap().signature = Some(arch_graph::Signature { params: vec![], returns: Some("Report".into()), qualifiers: vec![] });
        let mut extra = r.components[&ComponentId::new("rust:a::run")].clone();
        extra.id = ComponentId::new("rust:b::keep");
        extra.name = "keep".into();
        r.components.insert(extra.id.clone(), extra);
        r.view.nodes.push(ViewNode { id: ComponentId::new("rust:b::keep"), depth: Some(2), truncated: false, alignment: Alignment::Observed, members: 0, ancestors: vec![ComponentId::new("rust:b")] });
        for n in r.view.nodes.iter_mut().filter(|n| n.id.as_str().starts_with("rust:a::")) {
            n.ancestors = vec![ComponentId::new("rust:a")];
        }
        for (from, to) in [("rust:a::save", "rust:b::keep"), ("rust:a::log", "rust:b::keep")] {
            r.view.edges.push(ViewEdge { id: arch_graph::RelationId(format!("calls:{from}->{to}")), kind: RelationKind::Calls, source: ComponentId::new(from), target: ComponentId::new(to), alignment: Alignment::Observed, overlay: false, order: None, carries: None, unconfirmed: None });
        }
        r
    }

    /// `nested_result` with module `a` open in place.
    pub(crate) fn opened() -> GraphState {
        let mut g = showing(nested_result());
        g.set_folds(&[ComponentId::new("rust:a")], true);
        g.moving = None;
        g
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
        assert!(g.closed(&ComponentId::new("rust:a")) && !g.shown(&ComponentId::new("rust:a::save")), "a module shows shut: one card");
        // Moving goes between what's drawn: the module, then the module beside it.
        g.select(Some(ComponentId::new("rust:a")));
        g.move_selection(Dir::Right);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:b", "not into the shut module");
        // A focused module (the only one at the top) is the level, open: its
        // parts are drawn, and moves never jump out to it.
        let mut only_a = nested_result();
        only_a.view.nodes.retain(|n| n.id.as_str() != "rust:b");
        let mut f = showing(only_a);
        assert!(f.shown(&ComponentId::new("rust:a::save")), "the focused module's parts are drawn");
        f.select(Some(ComponentId::new("rust:a::save")));
        for dir in [Dir::Left, Dir::Right, Dir::Up, Dir::Down] {
            f.move_selection(dir);
            assert_ne!(f.selection.as_ref().unwrap().as_str(), "rust:a", "never onto the module it sits in");
        }
        // A search hit inside a shut module opens it.
        Navigable::move_to(&mut g, "rust:a::save");
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::save");
        assert!(g.shown(&ComponentId::new("rust:a::save")), "opened so it's drawn");
        // So does the first/last jump, and the agent's focus once its view is in.
        g.fold(Fold::CloseAll);
        g.motion(Motion::First, 1);
        assert!(g.shown(g.selection.as_ref().unwrap()), "{:?}", g.selection);
        g.fold(Fold::CloseAll);
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
        assert_eq!(save.rect, g.to_screen(&g.scene.rects[&ComponentId::new("rust:a::save")]));
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
        let edge = |a: &str, b: &str, order: u32| ViewEdge { id: arch_graph::RelationId(format!("calls:{a}->{b}")), kind: RelationKind::Calls, source: ComponentId::new(a), target: ComponentId::new(b), alignment: Alignment::Observed, overlay: false, order: Some(order), carries: None, unconfirmed: None };
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
        let mut n = opened();
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
        let mut r = showing(sample_result());
        r.scene.rects.insert(ComponentId::new("rust:a::run"), WorldRect { x: 0.0, y: 100.0, w: 220.0, h: 64.0 });
        r.scene.rects.insert(ComponentId::new("rust:a::step"), WorldRect { x: 260.0, y: 0.0, w: 220.0, h: 60.0 });
        r.scene.rects.insert(ComponentId::new("rust:a::save"), WorldRect { x: 420.0, y: 110.0, w: 220.0, h: 64.0 });
        r.scene.rects.insert(ComponentId::new("rust:a::log"), WorldRect { x: 0.0, y: 400.0, w: 220.0, h: 64.0 });
        r.select(Some(ComponentId::new("rust:a::run")));
        r.motion(Motion::Right, 1);
        assert_eq!(r.selection.as_ref().unwrap().as_str(), "rust:a::save");
    }

    // req: N.5, N.10
    #[test]
    fn moves_go_into_a_module_and_backing_out_keeps_what_held_the_selection() {
        // Focused on the module: its parts are inside it, so moves go in.
        let mut only_a = nested_result();
        only_a.view.nodes.retain(|n| n.id.as_str() != "rust:b");
        let mut g = showing(only_a);
        g.select(Some(ComponentId::new("rust:a")));
        g.motion(Motion::Right, 1);
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a::run", "l from the module: its first part");
        // Backing out to a view that doesn't have the selection: what held it.
        let mut nested = nested_result();
        nested.view.edges.push(ViewEdge { id: arch_graph::RelationId("contains:a->log".into()), kind: RelationKind::Contains, source: ComponentId::new("rust:a"), target: ComponentId::new("rust:a::log"), alignment: Alignment::Observed, overlay: false, order: None, carries: None, unconfirmed: None });
        let mut g = showing(nested);
        g.navigate(NavChange::Focus(ComponentId::new("rust:a")));
        g.set_result(g.result.clone().unwrap());
        g.select(Some(ComponentId::new("rust:a::log")));
        assert_eq!(g.leave(), Step::Done);
        let mut whole = nested_result();
        whole.components.retain(|id, _| !id.as_str().contains("::"));
        whole.view.nodes.retain(|n| !n.id.as_str().contains("::"));
        whole.view.edges.clear();
        g.set_result(whole);
        assert_eq!(g.selection.as_ref().map(|s| s.as_str()), Some("rust:a"), "the module that held it");
    }

    // req: N.11
    #[test]
    fn modules_fold_in_place_without_moving_the_view() {
        let mut g = opened();
        let nav = g.nav.clone();
        let a = ComponentId::new("rust:a");
        let on_screen = |g: &GraphState| g.to_screen(&g.scene.rects[&ComponentId::new("rust:a")]).min;
        g.select(Some(ComponentId::new("rust:a::step")));
        let before = on_screen(&g);
        g.fold(Fold::Close);
        assert!(!g.shown(&ComponentId::new("rust:a::step")), "zc shuts the module it's in");
        assert_eq!(g.selection.as_ref().unwrap().as_str(), "rust:a", "the selection moves to the module");
        assert_eq!(g.nav, nav, "the view's root didn't change");
        assert_eq!(on_screen(&g), before, "the module stays where it was on screen");
        g.fold(Fold::Toggle);
        assert!(g.shown(&ComponentId::new("rust:a::step")), "za opens it again");
        assert_eq!(on_screen(&g), before);
        assert!(g.scene.rects[&a].contains(&g.scene.rects[&ComponentId::new("rust:a::step")]), "its parts inside it");
        g.fold(Fold::CloseAll);
        assert!(!g.shown(&ComponentId::new("rust:a::save")));
        g.fold(Fold::OpenAll);
        assert!(g.shown(&ComponentId::new("rust:a::save")));
        // A jump into a shut module opens it.
        g.fold(Fold::CloseAll);
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
            r.view.nodes.push(arch_graph::view::ViewNode { id: id.clone(), depth: Some(0), truncated: false, alignment: Alignment::Observed, members: 0, ancestors: vec![] });
        }
        let mut g = showing(r);
        // Laid out by hand: run at the top of a tall column, the rest to its right.
        g.scene.rects.clear();
        g.scene.lines.clear();
        for (id, x, y) in [("rust:a::run", 0.0, 0.0), ("rust:a::step", 300.0, 0.0), ("rust:a::save", 600.0, 0.0), ("rust:a::log", 300.0, 100.0)] {
            g.scene.rects.insert(ComponentId::new(id), WorldRect { x, y, w: 220.0, h: 64.0 });
        }
        for i in 0..20 {
            g.scene.rects.insert(ComponentId::new(format!("rust:a::n{i}")), WorldRect { x: 0.0, y: 200.0 + i as f32 * 100.0, w: 220.0, h: 64.0 });
        }
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
        let r = g.scene.rects[&ComponentId::new(id)];
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
        // Double tap focuses (on the card, not on a marker or chip over it).
        let card = g.to_screen(&g.scene.rects[&ComponentId::new("rust:a::step")]).intersect(g.open_area);
        let save = [card.center(), card.left_center() + Vec2::new(4.0, 0.0), card.right_center() - Vec2::new(4.0, 0.0), card.center_top() + Vec2::new(0.0, 4.0), card.center_bottom() - Vec2::new(0.0, 4.0)]
            .into_iter()
            .find(|p| !g.beyond.iter().map(|(_, r)| r).chain(g.chip_rects.iter().map(|(_, r)| r)).any(|r| r.contains(*p)))
            .expect("some of the card is free");
        frame(&mut g, vec![touch(2, Start, save)], 1.0);
        frame(&mut g, vec![touch(2, End, save)], 0.05);
        frame(&mut g, vec![touch(3, Start, save)], 0.1);
        frame(&mut g, vec![touch(3, End, save)], 0.05);
        assert_eq!(g.nav.roots, vec![ComponentId::new("rust:a::step")]);
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
        // Toggling shows the table's entries for what's on screen, and only those.
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
        let shown: Vec<&LegendEntry> = legend.iter().filter(|e| g.on_screen(e)).collect();
        for entry in &shown {
            assert!(text.contains(&entry.label), "legend shows {}: {text}", entry.label);
        }
        assert!(shown.iter().any(|e| e.label == "calls") && !text.contains("\ncontains\n") && !text.contains("data flow"), "only calls are drawn here: {text}");
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
        assert!(lines.contains(&solid.gamma_multiply(1.0 - look.dim)), "a line away from it dimmed: {lines:?}");

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

    /// Every text a frame draws, with its size.
    fn sized_texts(out: &egui::FullOutput) -> Vec<(String, f32)> {
        fn collect(shape: &Shape, into: &mut Vec<(String, f32)>) {
            match shape {
                Shape::Text(t) => into.push((t.galley.text().to_owned(), t.galley.job.sections.first().map_or(0.0, |s| s.format.font_id.size))),
                Shape::Vec(list) => list.iter().for_each(|s| collect(s, into)),
                _ => {}
            }
        }
        let mut all = Vec::new();
        out.shapes.iter().for_each(|c| collect(&c.shape, &mut all));
        all
    }

    /// `nested_result` focused on module `a` (the level a zoom goes into).
    fn inside_a() -> ViewResult {
        let mut r = nested_result();
        r.view.roots = vec![ComponentId::new("rust:a")];
        r.view.nodes.retain(|n| n.id.as_str() != "rust:b");
        r
    }

    // req: G.1, G.3
    #[test]
    fn zooming_goes_into_a_module_and_out_of_it_with_the_way_shown() {
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        crate::theme::set_look(config.look.clone());
        let size = Vec2::new(1400.0, 800.0);
        let mut g = configured();
        g.set_result(nested_result());
        draw(&mut g, &ctx, vec![], 0.0, size);
        let a = ComponentId::new("rust:a");
        assert!(g.closed(&a) && g.shown(&a), "a shows shut at the top");
        assert_eq!(g.leave_label(), None, "at the top there's nowhere up");
        // Zooming in on it: past the share of the view it goes in.
        let card = g.to_screen(&g.scene.rects[&a]);
        for _ in 0..60 {
            if !g.nav.roots.is_empty() {
                break;
            }
            g.zoom_by_user(1.15, card.center());
        }
        assert_eq!(g.nav.roots, vec![a.clone()], "zoomed into a");
        assert!(g.jumped.take().is_some(), "a jump (the jump list has it)");
        let zoom_then = g.camera.zoom;
        g.zoom_by_user(1.15, card.center());
        assert_eq!(g.camera.zoom, zoom_then, "on the way in, more zoom waits");
        // The level arrives and grows out of where the card was.
        g.set_result(inside_a());
        let t0 = 1.0;
        draw(&mut g, &ctx, vec![], t0, size);
        let target = g.glide.expect("it glides").to;
        assert_ne!(g.camera, target, "it starts from the card");
        draw(&mut g, &ctx, vec![], t0 + 5.0, size);
        assert_eq!((g.camera, g.glide), (target, None), "and settles where the fit puts it");
        assert!(g.shown(&ComponentId::new("rust:a::save")) && g.scene.frame == Some(a.clone()), "inside, its parts are the level");
        assert_eq!(g.leave_label().as_deref(), Some("Up to the whole view"));
        // The trail says where we are, and each step is a link.
        let text = texts(&draw(&mut g, &ctx, vec![], t0 + 10.0, size));
        assert!(text.contains("All\n") && text.contains("a\n"), "{text}");
        // Zooming out of it: below the share it goes up, a selected.
        let middle = g.open_area.center();
        for _ in 0..60 {
            if g.nav.roots.is_empty() {
                break;
            }
            g.zoom_by_user(1.0 / 1.15, middle);
        }
        assert!(g.nav.roots.is_empty(), "back up");
        g.set_result(nested_result());
        draw(&mut g, &ctx, vec![], t0 + 20.0, size);
        assert_eq!(g.selection, Some(a.clone()), "the module it came out of is selected");
        assert!(g.glide.is_some(), "and the level shrinks back into its card");
        // Two shown things with one name say which is which.
        let mut twins = nested_result();
        let mut other = twins.components[&ComponentId::new("rust:a::log")].clone();
        other.id = ComponentId::new("rust:b::log");
        twins.components.insert(other.id.clone(), other);
        twins.view.nodes.push(ViewNode { id: ComponentId::new("rust:b::log"), depth: Some(1), truncated: false, alignment: Alignment::Observed, members: 0, ancestors: vec![] });
        g.set_result(twins);
        assert_eq!((g.names[&ComponentId::new("rust:a::log")].as_str(), g.names[&ComponentId::new("rust:b::log")].as_str()), ("a::log", "b::log"));
        assert_eq!(g.names[&ComponentId::new("rust:a::run")], "run", "the rest keep their short names");
    }

    // req: G.2
    #[test]
    fn text_keeps_its_size_and_names_that_dont_fit_float_without_covering_each_other() {
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        crate::theme::set_look(config.look.clone());
        let look = crate::theme::look();
        let size = Vec2::new(1400.0, 800.0);
        let mut g = configured();
        draw(&mut g, &ctx, vec![], 0.0, size);
        let name_size = |g: &mut GraphState, zoom: f32| {
            g.camera.zoom = zoom;
            sized_texts(&draw(g, &ctx, vec![], 0.1, size)).into_iter().find(|(t, _)| t == "step").map(|(_, s)| s)
        };
        assert_eq!(name_size(&mut g, 1.0), Some(look.text_title), "on its card");
        assert_eq!(name_size(&mut g, 2.5), Some(look.text_title), "the same size close up");
        assert_eq!(name_size(&mut g, 0.2), Some(look.text_label), "too small for its card: it floats, still readable");
        // Floating names: the selection's first, never over each other.
        g.select(Some(ComponentId::new("rust:a::log")));
        let card = Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(20.0, 10.0));
        let crowd: Vec<(ComponentId, Rect, Vec2)> = ["rust:a::run", "rust:a::step", "rust:a::save", "rust:a::log"].iter().map(|id| (ComponentId::new(*id), card, Vec2::new(60.0, 14.0))).collect();
        let plan = g.label_plan(&crowd, None);
        assert_eq!(plan.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["rust:a::log"], "all in one place: only the most important shows");
        let spread: Vec<(ComponentId, Rect, Vec2)> = crowd.iter().enumerate().map(|(i, (id, r, s))| (id.clone(), r.translate(Vec2::new(i as f32 * 40.0, 0.0)), *s)).collect();
        let plan = g.label_plan(&spread, None);
        assert_eq!(plan[0].0.as_str(), "rust:a::log");
        for (i, (a, ra)) in plan.iter().enumerate() {
            for (b, rb) in plan.iter().skip(i + 1) {
                assert!(!ra.intersects(*rb), "{a} and {b} overlap");
            }
        }
        assert!(plan.len() < 4, "some wait for the pointer");
        let missing = spread.iter().find(|(id, _, _)| !plan.iter().any(|(p, _)| p == id)).unwrap();
        let hovered = g.label_plan(&spread, Some(missing.1.center()));
        assert!(hovered.iter().any(|(id, _)| *id == missing.0), "under the pointer it shows");
    }

    // req: G.6, G.8
    #[test]
    fn merged_lines_show_their_counts_and_cards_glide_to_new_places() {
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        crate::theme::set_look(config.look.clone());
        let size = Vec2::new(1400.0, 800.0);
        // b calls into a's parts three times over: one line, b -> a, counted.
        let mut r = nested_result();
        for (i, part) in ["rust:a::run", "rust:a::step", "rust:a::save"].iter().enumerate() {
            r.view.links.push(arch_graph::view::ViewLink { source: ComponentId::new("rust:b"), target: ComponentId::new(*part), kind: RelationKind::Calls, count: i as u32 + 1 });
        }
        let mut g = configured();
        g.set_result(r.clone());
        let line = g.scene.lines.iter().find(|l| l.kind == RelationKind::Calls && l.from == End::Node(ComponentId::new("rust:b"))).expect("merged").clone();
        assert_eq!((line.to.clone(), line.count), (End::Node(ComponentId::new("rust:a")), 6));
        let text = texts(&draw(&mut g, &ctx, vec![], 0.0, size));
        assert!(!text.lines().any(|l| l == "6"), "nothing selected: lines say nothing: {text}");
        g.select(Some(ComponentId::new("rust:b")));
        let text = texts(&draw(&mut g, &ctx, vec![], 0.05, size));
        assert!(text.lines().any(|l| l == "6"), "selected: its line says how many it stands for: {text}");
        g.select(None);
        // Opened in place: the lines split onto the parts, and the cards
        // that moved glide there.
        let before = g.scene.rects.clone();
        g.set_folds(&[ComponentId::new("rust:a")], true);
        let moved = g.moving.clone().expect("cards move");
        assert!(moved.from.iter().all(|(id, r)| before.get(id) == Some(r)));
        let b = ComponentId::new("rust:b");
        let (from, to) = (g.to_screen(&before[&b]), g.to_screen(&g.scene.rects[&b]));
        if from != to {
            draw(&mut g, &ctx, vec![], 1.0, size);
            let half = g.drawn(&b, &g.scene.rects[&b].clone(), 1.0 + f64::from(crate::theme::look().move_seconds) / 2.0);
            assert!(half != from && half != to, "between the two halfway");
        }
        draw(&mut g, &ctx, vec![], 5.0, size);
        draw(&mut g, &ctx, vec![], 10.0, size);
        assert!(g.moving.is_none(), "and stop");
        assert_eq!(g.scene.lines.iter().filter(|l| l.from == End::Node(b.clone()) && l.kind == RelationKind::Calls).count(), 3);
        // A refresh of the same level keeps everything where it is.
        let places = g.scene.rects.clone();
        g.set_result(r);
        assert_eq!(g.scene.rects, places);
    }

    // req: C.1, C.2, C.3
    #[test]
    fn a_story_folds_in_place_and_says_what_it_hides() {
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        crate::theme::set_look(config.look.clone());
        let size = Vec2::new(1400.0, 800.0);
        // run calls step, then log; step calls save and log; all in module a.
        let mut r = sample_result();
        r.view.spec.name = "story".into();
        r.view.spec.layout = arch_graph::view::LayoutKind::Story;
        let call = |a: &str, b: &str, order: u32| ViewEdge { id: arch_graph::RelationId(format!("calls:{a}->{b}")), kind: RelationKind::Calls, source: ComponentId::new(a), target: ComponentId::new(b), alignment: Alignment::Observed, overlay: false, order: Some(order), carries: None, unconfirmed: None };
        r.view.edges = vec![call("rust:a::run", "rust:a::step", 0), call("rust:a::run", "rust:a::log", 1), call("rust:a::step", "rust:a::save", 0), call("rust:a::step", "rust:a::log", 1)];
        let mut module = r.components[&ComponentId::new("rust:a::run")].clone();
        module.id = ComponentId::new("rust:a");
        module.name = "a".into();
        r.components.insert(module.id.clone(), module);
        for n in &mut r.view.nodes {
            n.ancestors = vec![ComponentId::new("rust:a")];
        }
        let mut g = configured();
        g.set_result(r);
        let y = |g: &GraphState, id: &str| g.scene.rects[&ComponentId::new(id)].y;
        assert!(y(&g, "rust:a::run") < y(&g, "rust:a::step") && y(&g, "rust:a::step") < y(&g, "rust:a::save") && y(&g, "rust:a::save") < y(&g, "rust:a::log"), "top to bottom in the order things happen");
        let text = texts(&draw(&mut g, &ctx, vec![], 0.0, size));
        assert!(text.contains("also called from 1 more"), "log is told once: {text}");
        assert!(text.contains("a\n"), "the module's band is named");
        // zc on step: its branch collapses in place and says how much it hides.
        g.select(Some(ComponentId::new("rust:a::step")));
        g.fold(Fold::Close);
        assert!(!g.shown(&ComponentId::new("rust:a::save")) && g.shown(&ComponentId::new("rust:a::step")));
        let text = texts(&draw(&mut g, &ctx, vec![], 1.0, size));
        assert!(text.contains("2 calls inside"), "{text}");
        // A jump to something it hides opens it again.
        Navigable::move_to(&mut g, "rust:a::save");
        assert!(g.shown(&ComponentId::new("rust:a::save")) && g.selection.as_ref().map(|s| s.as_str()) == Some("rust:a::save"));
        g.fold(Fold::CloseAll);
        assert!(!g.shown(&ComponentId::new("rust:a::step")), "everything folds into the entry");
        g.fold(Fold::OpenAll);
        assert!(g.shown(&ComponentId::new("rust:a::save")));
        // From any view, a selected function's story (and what reaches it) is one action.
        let mut c = showing(sample_result());
        c.select(Some(ComponentId::new("rust:a::step")));
        let story = c.actions().into_iter().find(|a| a.id == "graph.reached_by").expect("offered");
        let (net, mut mode, mut note) = (NetClient::new("http://127.0.0.1:9"), crate::keymap::InputMode::Normal, None);
        c.run(&story, None, &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[] });
        assert_eq!((c.nav.view.as_str(), c.nav.roots.clone()), ("reached_by", vec![ComponentId::new("rust:a::step")]));
    }

    // req: D.2, D.3, D.4
    #[test]
    fn flow_steps_say_their_shape_lines_say_what_flows_and_one_value_can_be_followed() {
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        crate::theme::set_look(config.look.clone());
        let size = Vec2::new(1400.0, 800.0);
        // load -> parse (a Text), parse -> check (a Report), and an unconfirmed log.
        let mut r = sample_result();
        r.view.spec.name = "data_flow".into();
        r.view.spec.kinds = vec![RelationKind::DataFlow];
        let sig = |ins: &[&str], out: &str| Some(arch_graph::Signature { params: ins.iter().enumerate().map(|(i, t)| arch_graph::Param { name: format!("p{i}"), ty: Some((*t).into()), optional: false, variadic: false }).collect(), returns: Some(out.into()), qualifiers: vec![] });
        r.components.get_mut(&ComponentId::new("rust:a::run")).unwrap().signature = sig(&["&Path"], "Text");
        r.components.get_mut(&ComponentId::new("rust:a::step")).unwrap().signature = sig(&["Text"], "Result<Report, Error>");
        r.components.get_mut(&ComponentId::new("rust:a::save")).unwrap().signature = sig(&["Report"], "Report");
        let flow = |a: &str, b: &str, carries: &str| ViewEdge { id: arch_graph::RelationId(format!("data_flow:{a}->{b}")), kind: RelationKind::DataFlow, source: ComponentId::new(a), target: ComponentId::new(b), alignment: Alignment::Observed, overlay: false, order: None, carries: Some(carries.into()), unconfirmed: None };
        let mut unsure = flow("rust:a::run", "rust:a::log", "");
        unsure.carries = None;
        unsure.unconfirmed = Some("log declares no parameter types".into());
        r.view.edges = vec![flow("rust:a::run", "rust:a::step", "Text"), flow("rust:a::step", "rust:a::save", "Result<Report, Error>"), unsure];
        let mut g = configured();
        g.set_result(r);
        g.camera.zoom = 1.4;
        let text = texts(&draw(&mut g, &ctx, vec![], 0.0, size));
        // D.2: each step's shape; what flows on each line.
        assert!(text.contains("Text -> Result<Report, Error>"), "{text}");
        g.select(Some(ComponentId::new("rust:a::step")));
        let text = texts(&draw(&mut g, &ctx, vec![], 0.05, size));
        assert!(text.lines().any(|l| l == "Text"), "the selection's lines say what flows: {text}");
        g.select(None);
        // D.3: steps where the data changes shape are marked; one that keeps it isn't.
        let (_, reshapes, _) = crate::status::look("reshapes");
        let step = shape_of(g.component(&ComponentId::new("rust:a::step")).unwrap()).unwrap();
        let save = shape_of(g.component(&ComponentId::new("rust:a::save")).unwrap()).unwrap();
        assert!(step.contains(&reshapes) && !save.contains(&reshapes), "{step} / {save}");
        assert!(g.scene.lines.iter().any(|l| l.unconfirmed), "the unconfirmed flow is drawn as one");
        // D.4: follow Report from save: its whole path, the rest faded.
        g.select(Some(ComponentId::new("rust:a::save")));
        let follow = g.actions().into_iter().find(|a| a.id == "graph.follow").expect("offered");
        let crate::actions::Param::Choice(choices) = follow.param.clone() else { panic!("a choice") };
        assert!(choices.iter().any(|c| c.value == "Report"), "{choices:?}");
        let (net, mut mode, mut note) = (NetClient::new("http://127.0.0.1:9"), crate::keymap::InputMode::Normal, None);
        g.run(&follow, Some("Report".into()), &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[] });
        assert_eq!((g.trace.as_deref(), g.nav.view.as_str(), g.nav.roots.clone()), (Some("Report"), "data_path", vec![ComponentId::new("rust:a::save")]));
        let carrying = g.scene.lines.iter().find(|l| l.carries.iter().any(|c| c.contains("Report"))).unwrap().clone();
        let other = g.scene.lines.iter().find(|l| l.carries == vec!["Text".to_owned()]).unwrap().clone();
        assert!(g.on_trace(&carrying.carries) && !g.on_trace(&other.carries), "Result<Report, Error> carries Report; Text doesn't");
        let text = texts(&draw(&mut g, &ctx, vec![], 1.0, size));
        assert!(text.contains("following Report"), "{text}");
        let unfollow = g.actions().into_iter().find(|a| a.id == "graph.unfollow").expect("can stop");
        g.run(&unfollow, None, &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[] });
        assert!(g.trace.is_none());
    }

    // req: G.10, G.11, G.12
    #[test]
    fn selecting_lights_up_whats_related_views_open_readable_and_go_tos_land_in_context() {
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        crate::theme::set_look(config.look.clone());
        let (look, theme) = (crate::theme::look(), colors());
        let size = Vec2::new(1400.0, 800.0);
        // G.10: step is linked to run (its caller) and save (its callee); log isn't.
        let mut g = configured();
        draw(&mut g, &ctx, vec![], 0.0, size);
        assert!(g.related().is_none(), "nothing selected: nothing dims");
        g.select(Some(ComponentId::new("rust:a::step")));
        let related = g.related().unwrap();
        let ids = |names: &[&str]| names.iter().map(|n| ComponentId::new(*n)).collect::<BTreeSet<_>>();
        assert_eq!(related, ids(&["rust:a::run", "rust:a::step", "rust:a::save"]));
        let out = draw(&mut g, &ctx, vec![], 0.1, size);
        let dim = theme.bg.gamma_multiply(look.dim);
        fn fills(shape: &Shape, into: &mut Vec<(egui::Color32, Rect)>) {
            match shape {
                Shape::Rect(r) => into.push((r.fill, r.rect)),
                Shape::Vec(list) => list.iter().for_each(|s| fills(s, into)),
                _ => {}
            }
        }
        let mut drawn = Vec::new();
        out.shapes.iter().for_each(|c| fills(&c.shape, &mut drawn));
        let log = g.to_screen(&g.scene.rects[&ComponentId::new("rust:a::log")]);
        let veiled = |card: Rect| drawn.iter().any(|(fill, r)| *fill == dim && r.contains_rect(card));
        assert!(veiled(log), "log isn't related: it dims");
        assert!(!veiled(g.to_screen(&g.scene.rects[&ComponentId::new("rust:a::save")])), "save is: it stays bright");

        // G.11: a wide view opens at reading size around the selection, not squeezed.
        let mut wide = sample_result();
        for i in 0..30 {
            let id = ComponentId::new(format!("rust:a::n{i:02}"));
            let mut c = wide.components[&ComponentId::new("rust:a::run")].clone();
            c.id = id.clone();
            c.name = format!("n{i:02}");
            wide.components.insert(id.clone(), c);
            wide.view.nodes.push(ViewNode { id: id.clone(), depth: Some(1), truncated: false, alignment: Alignment::Observed, members: 0, ancestors: vec![] });
            let from = if i == 0 { "rust:a::save".to_owned() } else { format!("rust:a::n{:02}", i - 1) };
            wide.view.edges.push(ViewEdge { id: arch_graph::RelationId(format!("calls:{from}->{id}")), kind: RelationKind::Calls, source: ComponentId::new(&from), target: id.clone(), alignment: Alignment::Observed, overlay: false, order: None, carries: None, unconfirmed: None });
        }
        let mut w = configured();
        w.set_result(wide);
        w.select(Some(ComponentId::new("rust:a::n15")));
        draw(&mut w, &ctx, vec![], 0.0, size);
        w.fit_view();
        assert!(w.camera.zoom >= w.ui.detail.mid, "readable: {}", w.camera.zoom);
        let at = w.to_screen(&w.scene.rects[&ComponentId::new("rust:a::n15")]).center();
        assert!(at.distance(w.open_area.center()) < 1.0, "around the selection");
        // Lines: one per pair, never on top of each other.
        assert!(scene::problems(&w.scene).is_empty(), "{:?}", scene::problems(&w.scene));
        // What a selected line reaches off screen is named at the edge; a click goes there.
        let mut far = w.result.clone().unwrap();
        far.view.edges.push(ViewEdge { id: arch_graph::RelationId("calls:n15->n29".into()), kind: RelationKind::Calls, source: ComponentId::new("rust:a::n15"), target: ComponentId::new("rust:a::n29"), alignment: Alignment::Observed, overlay: false, order: None, carries: None, unconfirmed: None });
        w.set_result(far);
        w.fit_view();
        let text = texts(&draw(&mut w, &ctx, vec![], 1.0, size));
        let marker = w.beyond.iter().find(|(id, _)| id.as_str() == "rust:a::n29").map(|(_, r)| *r).expect("n29 is off screen: marked at the edge");
        assert!(text.contains("n29"), "{text}");
        assert!(w.open_area.contains_rect(marker), "on screen");
        draw(&mut w, &ctx, click(marker.center(), true), 1.1, size);
        draw(&mut w, &ctx, click(marker.center(), false), 1.15, size);
        assert_eq!(w.selection.as_ref().map(|s| s.as_str()), Some("rust:a::n29"), "clicking it goes there");

        // G.12: going to a function (it holds nothing) opens it in its module.
        let mut n = configured();
        n.set_result(nested_result());
        let mut alone = nested_result();
        alone.view.roots = vec![ComponentId::new("rust:a::step")];
        alone.view.nodes.retain(|x| x.id.as_str() == "rust:a::step");
        alone.view.edges.clear();
        alone.view.trail = vec![ComponentId::new("rust:a")];
        n.reveal(ComponentId::new("rust:zz::unshown"));
        n.set_result(alone);
        assert_eq!(n.nav.roots, vec![ComponentId::new("rust:a")], "to the module that holds it");
        let mut module = nested_result();
        module.view.roots = vec![ComponentId::new("rust:a")];
        module.view.nodes.retain(|x| x.id.as_str() != "rust:b");
        n.set_result(module);
        assert_eq!(n.selection.as_ref().map(|s| s.as_str()), Some("rust:a::step"), "selected there, among its neighbours");
        assert!(n.shown(&ComponentId::new("rust:a::run")) && n.shown(&ComponentId::new("rust:a::save")));
    }

    // req: G.13, G.14
    #[test]
    fn relations_step_through_levels_open_in_place_and_markers_stay_on_screen() {
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        crate::theme::set_look(config.look.clone());
        let size = Vec2::new(1400.0, 800.0);
        // The sample's functions sit in module a; a second module b holds one more.
        let mut r = sample_result();
        let mut module = |id: &str| {
            let mut c = r.components[&ComponentId::new("rust:a::run")].clone();
            c.id = ComponentId::new(id);
            c.name = id.trim_start_matches("rust:").into();
            c.type_name = "module".into();
            r.components.insert(c.id.clone(), c);
        };
        module("rust:a");
        module("rust:b");
        let mut extra = r.components[&ComponentId::new("rust:a::run")].clone();
        extra.id = ComponentId::new("rust:b::keep");
        extra.name = "keep".into();
        r.components.insert(extra.id.clone(), extra);
        r.view.nodes.push(ViewNode { id: ComponentId::new("rust:b::keep"), depth: Some(2), truncated: false, alignment: Alignment::Observed, members: 0, ancestors: vec![ComponentId::new("rust:b")] });
        for n in r.view.nodes.iter_mut().filter(|n| n.id.as_str().starts_with("rust:a::")) {
            n.ancestors = vec![ComponentId::new("rust:a")];
        }
        for (from, to) in [("rust:a::save", "rust:b::keep"), ("rust:a::log", "rust:b::keep")] {
            r.view.edges.push(ViewEdge { id: arch_graph::RelationId(format!("calls:{from}->{to}")), kind: RelationKind::Calls, source: ComponentId::new(from), target: ComponentId::new(to), alignment: Alignment::Observed, overlay: false, order: None, carries: None, unconfirmed: None });
        }
        let mut g = configured();
        g.set_result(r);
        assert!(g.actions().iter().any(|a| a.id == "graph.level"), "a level control");
        assert_eq!(g.levels().iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["modules"], "packages would group the same: not offered");
        g.select(Some(ComponentId::new("rust:a::save")));
        // A level up: two modules, one line between them counting both calls.
        g.step_grain(true).unwrap();
        assert_eq!(g.grain.as_deref(), Some("modules"));
        assert_eq!(g.scene.rects.keys().map(|k| k.as_str()).collect::<Vec<_>>(), ["rust:a", "rust:b"]);
        assert_eq!(g.scene.lines.len(), 1);
        assert_eq!(g.scene.lines[0].count, 2);
        assert_eq!(g.selection.as_ref().map(|s| s.as_str()), Some("rust:a"), "the selection's holder is selected");
        let text = texts(&draw(&mut g, &ctx, vec![], 0.0, size));
        assert!(text.contains("by modules"), "the toolbar says the level: {text}");
        // Opened in place: a's functions inside it, the rest as it was.
        g.fold(Fold::Open);
        assert!(g.shown(&ComponentId::new("rust:a::save")) && !g.shown(&ComponentId::new("rust:b::keep")));
        assert!(g.scene.rects[&ComponentId::new("rust:a")].contains(&g.scene.rects[&ComponentId::new("rust:a::run")]));
        g.fold(Fold::Close);
        assert!(!g.shown(&ComponentId::new("rust:a::save")));
        // And back down to the parts.
        g.step_grain(false).unwrap();
        assert!(g.grain.is_none() && g.shown(&ComponentId::new("rust:a::save")));
        assert!(g.step_grain(false).is_err(), "the parts are the finest");
        // Focused on one function in a view of calls, a level up keeps the
        // view where it is (it doesn't jump to the function's module).
        let nav = g.nav.clone();
        g.nav.roots = vec![ComponentId::new("rust:a::run")];
        let mut rooted = g.raw.clone().unwrap();
        rooted.view.roots = vec![ComponentId::new("rust:a::run")];
        rooted.view.trail = vec![ComponentId::new("rust:a")];
        g.set_result(rooted);
        g.step_grain(true).unwrap();
        assert_eq!(g.nav.roots, vec![ComponentId::new("rust:a::run")], "still rooted at run");
        assert!(g.scene.rects.len() >= 2, "{:?}", g.scene.rects.keys().collect::<Vec<_>>());
        g.nav = nav;

        // G.14: many lines off screen: markers on screen, apart, the rest summed up.
        let mut wide = sample_result();
        let hub = ComponentId::new("rust:a::run");
        for i in 0..40 {
            let id = ComponentId::new(format!("rust:a::far{i:02}"));
            let mut c = wide.components[&hub].clone();
            c.id = id.clone();
            c.name = format!("far{i:02}");
            wide.components.insert(id.clone(), c);
            wide.view.nodes.push(ViewNode { id: id.clone(), depth: Some(1), truncated: false, alignment: Alignment::Observed, members: 0, ancestors: vec![] });
            wide.view.edges.push(ViewEdge { id: arch_graph::RelationId(format!("calls:run->{id}")), kind: RelationKind::Calls, source: hub.clone(), target: id, alignment: Alignment::Observed, overlay: false, order: None, carries: None, unconfirmed: None });
        }
        let mut w = configured();
        w.set_result(wide);
        w.select(Some(hub.clone()));
        draw(&mut w, &ctx, vec![], 0.0, Vec2::new(900.0, 500.0));
        w.camera.zoom = 2.5;
        w.camera.pan = (w.open_area.center() - w.canvas.min) - {
            let (x, y) = w.scene.rects[&hub].center();
            Vec2::new(x, y) * 2.5
        };
        let text = texts(&draw(&mut w, &ctx, vec![], 0.1, Vec2::new(900.0, 500.0)));
        assert!(!w.beyond.is_empty() && w.beyond.len() <= w.ui.levels.max_markers, "{}", w.beyond.len());
        for (i, (_, a)) in w.beyond.iter().enumerate() {
            assert!(w.open_area.contains_rect(*a), "on screen: {a:?}");
            for (_, b) in w.beyond.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "apart: {a:?} {b:?}");
            }
        }
        assert!(text.contains("more off screen"), "the rest summed up: {text}");
    }
}
