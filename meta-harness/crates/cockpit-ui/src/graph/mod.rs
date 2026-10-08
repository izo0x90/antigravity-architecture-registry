//! The graph: one of three views (Contains, Calls, Data flow) as a tree the
//! server builds and lays out; this side only asks for it and draws it.
//!
//! - `request`: where it looks, and the one reducer that changes it.
//! - `draw`: the canvas (cards, lines, the pills on the selection's lines),
//!   only what's on screen.
//! - `select`: the selection, what's tied to it, and moving between cards.
//! - `details`: the details card for the selection, fetched when selected.
//! - `actions`: the graph's actions (its keys live in the keymap).
//! - `navigable`: the universal keys (moves, go in, back, folds, search,
//!   hints, the jump list).
//!
//! Nothing changes on screen unless the user (or the agent) did something.

pub(crate) mod actions;
mod details;
mod draw;
mod find;
mod navigable;
pub mod request;
mod select;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use arch_graph::tree::{Card, Tree, TreeConfig, ViewKind};
use arch_graph::{ArchEvent, ChangeReason, Component, ComponentDetail, ComponentId, PlanId, Requester, ViewRequest};
use egui::{Pos2, Rect, Vec2};

pub use request::{Nav, NavChange};

use crate::graph_config::GraphUi;
use crate::nav::{Navigable, Place};
use crate::net::{NetClient, NetResult};
use crate::touch::Gestures;

/// The agent moved the view: what it showed, and where the user was.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentMove {
    pub what: String,
    /// The card it pointed at.
    pub select: Option<String>,
    pub return_to: Nav,
    pub return_selection: Option<String>,
}

/// Things the graph asks other parts of the cockpit to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphRequests {
    /// Open this file at these lines in the code view.
    pub open_code: Option<(String, u32, u32)>,
    pub open_spec: bool,
    /// Run this action (by id, with its value) the way keys do: from a chip
    /// or the toolbar, for the shell's actions.
    pub run: Option<(String, Option<String>)>,
    /// Read the code that changed again.
    pub reindex: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Camera {
    pan: Vec2,
    zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { pan: Vec2::new(24.0, 24.0), zoom: 1.0 }
    }
}

enum Reply {
    Config(NetResult<TreeConfig>),
    Plans(NetResult<Vec<arch_graph::Plan>>),
    Tree(u64, Box<NetResult<Tree>>),
    Card(String, Option<PlanId>, Box<NetResult<ComponentDetail>>),
    Found(u64, arch_search::SearchKind, Box<NetResult<arch_search::Found>>),
}

#[derive(Clone, Debug, PartialEq)]
enum Load {
    Idle,
    Loading,
    Failed(String),
}

/// A card's details as fetched (for one source).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Detail {
    Loading,
    Ready(Box<ComponentDetail>),
    Failed(String),
}

/// Where the user was before going into a card: back returns there.
#[derive(Clone, Debug, PartialEq)]
struct Step {
    nav: Nav,
    selection: Option<String>,
}

pub struct GraphState {
    /// The views' settings and layout sizes (from the server).
    pub(crate) config: Option<TreeConfig>,
    config_requested: bool,
    plans: Vec<PlanId>,
    plans_stale: bool,
    pub nav: Nav,
    /// Where going in started, latest last (back pops it).
    trail: Vec<Step>,
    /// The tree on screen (the only graph data this side holds).
    pub tree: Option<Tree>,
    /// Card index by key.
    at: BTreeMap<String, usize>,
    /// The selected card's key.
    pub selection: Option<String>,
    /// Select this once it shows up (the agent's card, a reveal).
    pending_select: Option<String>,
    /// A selection the tree on screen doesn't have (it comes back with one that does).
    remembered: Option<String>,
    /// Details by card and plan, fetched for the selection.
    details: BTreeMap<(String, Option<PlanId>), Detail>,
    /// Files changed since they were indexed (their cards are marked).
    pub stale_files: Vec<String>,
    /// Review badges: changed since the last review, unaddressed comments.
    pub marks: BTreeMap<ComponentId, crate::review::NodeMark>,
    load: Load,
    stale: bool,
    generation: u64,
    camera: Camera,
    /// Fit the camera to the tree once it arrives (after the user moved).
    fit: bool,
    refit: bool,
    /// Bring this card into view once the frame's camera is settled.
    reveal_after_fit: Option<String>,
    pub drawer_open: bool,
    /// The agent moved the view (shown until the user moves).
    pub agent: Option<AgentMove>,
    /// Where the graph was before a jump made with the pointer or a finger
    /// (the shell records it in the jump list).
    pub jumped: Option<Place>,
    pub requests: GraphRequests,
    canvas: Rect,
    /// The part of the canvas the details card doesn't cover.
    open_area: Rect,
    inbox: Arc<Mutex<Vec<Reply>>>,
    /// Behavior settings (touch, chips), from the UI config.
    pub ui: GraphUi,
    gestures: Gestures,
    touch_seen: bool,
    /// Long-pressed card and where: its actions show as chips there.
    pub context: Option<(String, Pos2)>,
    pub(crate) chip_rects: Vec<(String, Rect)>,
    context_chips: usize,
    /// The "+N" counts drawn this frame (click one to open its card).
    pub(crate) more_rects: Vec<(String, Rect)>,
    /// The camera before a search started previewing matches.
    saved_view: Option<(Camera, bool)>,
    pub legend_open: bool,
    /// Something the graph has to say (a branch that didn't fit, a plan gone).
    pub note: Option<String>,
    /// Finding anything in the code (typed in the palette).
    pub(crate) find: find::Find,
    /// A toolbar button whose action needs input: open the palette at it.
    palette_request: Option<String>,
}

impl Default for GraphState {
    fn default() -> Self {
        Self {
            config: None,
            config_requested: false,
            plans: Vec::new(),
            plans_stale: true,
            nav: Nav::default(),
            trail: Vec::new(),
            tree: None,
            at: BTreeMap::new(),
            selection: None,
            pending_select: None,
            remembered: None,
            details: BTreeMap::new(),
            stale_files: Vec::new(),
            marks: BTreeMap::new(),
            load: Load::Idle,
            stale: true,
            generation: 0,
            camera: Camera::default(),
            fit: true,
            refit: false,
            reveal_after_fit: None,
            drawer_open: true,
            agent: None,
            jumped: None,
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
            more_rects: Vec::new(),
            saved_view: None,
            legend_open: false,
            note: None,
            find: find::Find::default(),
            palette_request: None,
        }
    }
}

fn push(inbox: &Arc<Mutex<Vec<Reply>>>, reply: Reply) {
    if let Ok(mut i) = inbox.lock() {
        i.push(reply);
    }
}

impl GraphState {
    /// A card on screen.
    pub fn card(&self, key: &str) -> Option<&Card> {
        let tree = self.tree.as_ref()?;
        self.at.get(key).and_then(|i| tree.cards.get(*i))
    }

    pub fn selected_card(&self) -> Option<&Card> {
        self.card(self.selection.as_deref()?)
    }

    /// A component this side knows in full (fetched for a selection).
    pub fn component(&self, id: &ComponentId) -> Option<&Component> {
        match self.details.get(&(id.0.clone(), self.nav.plan.clone())) {
            Some(Detail::Ready(d)) if d.component.id == *id => Some(&d.component),
            _ => self.details.values().find_map(|d| match d {
                Detail::Ready(d) if d.component.id == *id => Some(&d.component),
                _ => None,
            }),
        }
    }

    /// The selection's component, once its details are in.
    pub fn selected(&self) -> Option<&Component> {
        let key = self.selection.as_ref()?;
        match self.details.get(&(key.clone(), self.nav.plan.clone())) {
            Some(Detail::Ready(d)) => Some(&d.component),
            _ => None,
        }
    }

    /// What the user has selected (its component): the selection, or one the
    /// tree on screen doesn't have (the spec keeps showing it).
    pub fn current(&self) -> Option<ComponentId> {
        match self.selected_card() {
            Some(card) => card.component.clone(),
            None => self.remembered.as_ref().map(|k| ComponentId::new(k.clone())),
        }
    }

    /// The depth shown now (the view's own when none was picked).
    pub fn depth(&self) -> u32 {
        self.nav.depth.or_else(|| self.tree.as_ref().and_then(|t| t.request.depth)).or_else(|| self.config.as_ref().map(|c| c.view(self.nav.view).depth)).unwrap_or(0)
    }

    /// The one way where the graph looks changes: applies the change and
    /// fetches the tree again if it changed. A user's move ends the agent's.
    pub fn navigate(&mut self, change: NavChange) {
        let user = !matches!(change, NavChange::Request(_));
        let before = self.nav.clone();
        if let NavChange::GoIn(card) = &change
            && self.nav.root.as_ref() != Some(card)
        {
            self.trail.push(Step { nav: self.nav.clone(), selection: self.selection.clone() });
            self.pending_select = Some(card.clone());
        }
        request::apply(&mut self.nav, change);
        if self.nav != before {
            self.stale = true;
            self.refit = true;
            self.note = None;
        }
        if user {
            self.agent = None;
        }
    }

    /// Back: to where the last "go in" started, else out to what holds the
    /// root (or the view's starting points). False when there's nowhere back.
    pub fn back(&mut self) -> bool {
        if let Some(step) = self.trail.pop() {
            self.nav = step.nav;
            self.pending_select = step.selection;
            self.stale = true;
            self.refit = true;
            self.agent = None;
            return true;
        }
        let Some(root) = self.nav.root.clone() else { return false };
        let holder = self.tree.as_ref().and_then(|t| t.path.last()).map(|c| c.key.clone());
        let change = match (self.nav.view, holder) {
            (ViewKind::Contains, Some(holder)) => NavChange::GoIn(holder),
            _ => NavChange::Top,
        };
        self.navigate(change);
        self.trail.clear();
        self.pending_select = Some(root);
        true
    }

    /// The roots gone through to get here, oldest first (each one a way back).
    pub fn trail_roots(&self) -> Vec<Option<String>> {
        self.trail.iter().map(|s| s.nav.root.clone()).collect()
    }

    /// Back to the `index`th step of the trail.
    pub fn back_to(&mut self, index: usize) {
        if index >= self.trail.len() {
            return;
        }
        let step = self.trail[index].clone();
        self.trail.truncate(index);
        self.nav = step.nav;
        self.pending_select = step.selection;
        self.stale = true;
        self.refit = true;
        self.agent = None;
    }

    /// A jump made with the pointer or a finger: remembers where it left
    /// from for the jump list, then navigates.
    fn pointer_jump(&mut self, change: NavChange) {
        if self.jumped.is_none() {
            self.jumped = Some(self.place());
        }
        self.navigate(change);
    }

    pub fn select(&mut self, key: Option<String>) {
        if key.is_some() {
            self.drawer_open = true;
        }
        self.selection = key;
        self.remembered = None;
    }

    /// Selects a component: here if the tree has it; otherwise the view
    /// goes into it (data flow shows calls for it).
    pub fn reveal(&mut self, id: ComponentId) {
        let key = id.0.clone();
        if self.card(&key).is_some() {
            self.select(Some(key.clone()));
            self.reveal_after_fit = Some(key);
            return;
        }
        if self.nav.view == ViewKind::DataFlow {
            self.navigate(NavChange::View { view: ViewKind::Calls, root: None });
        }
        self.navigate(NavChange::GoIn(key.clone()));
        self.pending_select = Some(key);
    }

    pub fn on_event(&mut self, event: &ArchEvent) {
        match event {
            // Changed code or plans: the same tree again (the camera stays).
            ArchEvent::Changed { reason, .. } => {
                self.stale = true;
                self.details.clear();
                if matches!(reason, ChangeReason::PlanEdited) {
                    self.plans_stale = true;
                }
            }
            ArchEvent::Validated { .. } | ArchEvent::QueueUpdated { .. } | ArchEvent::Index { .. } => {}
            ArchEvent::ViewRequested { request, by } => {
                if *by == Requester::Agent {
                    let (return_to, return_selection) = match &self.agent {
                        Some(a) => (a.return_to.clone(), a.return_selection.clone()),
                        None => (self.nav.clone(), self.selection.clone()),
                    };
                    self.agent = Some(AgentMove { what: self.describe(request), select: request.select.clone(), return_to, return_selection });
                }
                self.navigate(NavChange::Request(request.clone()));
                if let Some(select) = &request.select {
                    if self.card(select).is_some() && !self.stale {
                        self.select(Some(select.clone()));
                        self.reveal_after_fit = Some(select.clone());
                    } else {
                        self.pending_select = Some(select.clone());
                    }
                }
            }
        }
    }

    /// What a view request shows, in words.
    fn describe(&self, request: &ViewRequest) -> String {
        let name = |key: &str| self.card(key).map_or_else(|| key.rsplit([':', '.']).next().unwrap_or(key).to_owned(), |c| c.label.clone());
        let mut parts = vec![format!("{} view", request.view.unwrap_or(self.nav.view).name().replace('_', " "))];
        if let Some(source) = &request.source {
            parts.push(source.to_string());
        }
        if let Some(root) = &request.root {
            parts.push(format!("from {}", name(root)));
        }
        if let Some(select) = &request.select {
            parts.push(format!("at {}", name(select)));
        }
        parts.join(" · ")
    }

    /// Fetch everything again (events may have been missed).
    pub fn invalidate(&mut self) {
        self.stale = true;
        self.plans_stale = true;
        self.details.clear();
    }

    /// Takes a tree the server sent: keeps the selection when it's still
    /// there, selects what was waiting to show, and says what didn't fit.
    pub fn set_tree(&mut self, tree: Tree) {
        request::forget_dropped(&mut self.nav, &tree.dropped);
        if !tree.dropped.is_empty() {
            let names: Vec<String> = tree.dropped.iter().map(|k| tree.card(k).map_or_else(|| k.clone(), |c| c.label.clone())).collect();
            self.note = Some(format!("Closed {} again: opening it went over what one view holds.", names.join(", ")));
        }
        self.at = tree.cards.iter().enumerate().map(|(i, c)| (c.key.clone(), i)).collect();
        if let Some(sel) = self.selection.clone()
            && !self.at.contains_key(&sel)
        {
            self.selection = None;
            self.remembered = Some(sel);
        }
        if self.selection.is_none()
            && let Some(back) = self.remembered.take()
        {
            if self.at.contains_key(&back) {
                self.selection = Some(back);
            } else {
                self.remembered = Some(back);
            }
        }
        if let Some(pending) = self.pending_select.take() {
            if self.at.contains_key(&pending) {
                self.selection = Some(pending.clone());
                self.drawer_open = true;
                self.reveal_after_fit = Some(pending);
            } else {
                self.remembered = Some(pending);
            }
        }
        // Gone into something that holds nothing (Contains): show it where
        // it lives, inside its box, selected.
        let lone_leaf = tree.request.view == ViewKind::Contains && tree.cards.len() == 1 && tree.cards[0].more == 0;
        let holder = tree.path.last().map(|c| c.key.clone());
        let leaf = tree.roots.first().cloned();
        self.tree = Some(tree);
        self.load = Load::Idle;
        if std::mem::take(&mut self.refit) {
            self.fit = true;
        }
        if lone_leaf && let (Some(holder), Some(leaf)) = (holder, leaf) {
            self.trail.pop();
            self.navigate(NavChange::GoIn(holder));
            self.pending_select = Some(leaf);
        }
    }

    /// Asks for the selection's details (for each source it's compared in).
    fn want_details(&mut self, net: &NetClient, ctx: &egui::Context) {
        let Some(card) = self.selected_card().cloned() else { return };
        if card.component.is_none() {
            return;
        }
        let mut wanted = vec![self.nav.plan.clone()];
        // A planned change: the code as it is too, to show both.
        if self.nav.plan.is_some() && matches!(card.mark, arch_graph::tree::Mark::Changed | arch_graph::tree::Mark::Mismatch) {
            wanted.push(None);
        }
        for plan in wanted {
            let key = (card.key.clone(), plan.clone());
            if self.details.contains_key(&key) {
                continue;
            }
            if self.details.len() >= self.ui.details_kept.max(1) {
                self.details.clear();
            }
            self.details.insert(key, Detail::Loading);
            let (inbox, k, asked) = (self.inbox.clone(), card.key.clone(), plan.clone());
            net.graph_card(&card.key, asked.as_ref(), ctx.clone(), move |r| push(&inbox, Reply::Card(k, plan, Box::new(r))));
        }
    }

    pub(crate) fn detail(&self, key: &str, plan: Option<&PlanId>) -> Option<&Detail> {
        self.details.get(&(key.to_owned(), plan.cloned()))
    }

    /// Fetches what's missing or stale and applies replies. Call every frame.
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::Config(Ok(config)) => self.config = Some(config),
                Reply::Config(Err(e)) => {
                    self.config_requested = false;
                    self.load = Load::Failed(e);
                }
                Reply::Plans(Ok(plans)) => self.plans = plans.into_iter().map(|p| p.id).collect(),
                Reply::Plans(Err(_)) => {}
                Reply::Card(key, plan, detail) => {
                    let detail = match *detail {
                        Ok(d) => Detail::Ready(Box::new(d)),
                        Err(e) => Detail::Failed(e),
                    };
                    self.details.insert((key, plan), detail);
                }
                Reply::Found(asked, kind, found) => self.found(asked, kind, *found),
                Reply::Tree(generation, tree) => {
                    if generation != self.generation {
                        continue;
                    }
                    match *tree {
                        Ok(tree) => self.set_tree(tree),
                        Err(e) => self.failed(e),
                    }
                }
            }
        }
        if !self.config_requested {
            self.config_requested = true;
            let inbox = self.inbox.clone();
            net.graph_config(ctx.clone(), move |r| push(&inbox, Reply::Config(r)));
        }
        if self.plans_stale {
            self.plans_stale = false;
            let inbox = self.inbox.clone();
            net.arch_plans(ctx.clone(), move |r| push(&inbox, Reply::Plans(r)));
        }
        if self.stale {
            self.stale = false;
            self.generation += 1;
            self.load = Load::Loading;
            let (inbox, generation) = (self.inbox.clone(), self.generation);
            net.graph_tree(&self.nav.request(), ctx.clone(), move |r| push(&inbox, Reply::Tree(generation, Box::new(r))));
        }
        self.want_details(net, ctx);
        self.send_find(net, ctx);
    }

    /// A tree that couldn't be had: a plan that's gone shows the code, a
    /// root that isn't there shows the view's starting points; both say so.
    fn failed(&mut self, error: String) {
        let missing = error.to_lowercase().contains("not found");
        if missing && let Some(plan) = self.nav.plan.clone() {
            self.plans_stale = true;
            self.navigate(NavChange::Plan(None));
            self.note = Some(format!("Plan {plan} isn't there any more: showing the code."));
            return;
        }
        if missing && let Some(root) = self.nav.root.clone() {
            self.navigate(NavChange::Top);
            self.note = Some(format!("{root} isn't in this view: showing where it starts."));
            return;
        }
        self.load = Load::Failed(error);
    }
}

#[cfg(test)]
pub(crate) mod tests;
