//! The graph: one of three views (Contains, Calls, Data flow) as a tree the
//! server builds and lays out; this side only asks for it and draws it.
//!
//! - `request`: what was asked (view, source, root, way, depth, the
//!   branches opened and closed) and the back trail; the one way it changes.
//! - `cards`: the cards held, as the server sends them and changes them.
//! - `draw`: the canvas (cards, lines, the pills on the selection's lines),
//!   only what's on screen.
//! - `select`: the selection, what's tied to it, and the universal moves
//!   between cards (go in, back, folds, search and hints over cards).
//! - `details`: the details card for the selection, fetched when selected.
//! - `source`: the Code/plan picker and its "show in full code" switch.
//! - `actions`: the graph's actions (its keys live in the keymap).
//! - this file: the state, and the one place answers are asked for and
//!   taken in (`tick`).
//!
//! Nothing changes on screen unless the user (or the agent) did something.

pub(crate) mod actions;
mod cards;
mod details;
mod draw;
pub mod request;
mod select;
mod source;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use arch_graph::tree::{Card, Tree, TreeConfig, ViewKind};
use arch_graph::{ArchEvent, ChangeReason, Component, ComponentDetail, ComponentId, PlanId, Requester, Source, ViewRequest};
use egui::{Pos2, Rect, Vec2};

pub use request::{Nav, NavChange};

use crate::graph_config::GraphUi;
use crate::net::{NetClient, NetResult};

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
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Camera {
    pub(crate) pan: Vec2,
    pub(crate) zoom: f32,
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
    /// What opening a card adds (for the tree of that generation).
    Expanded(u64, Box<NetResult<arch_graph::tree::Expanded>>),
    Card(String, Source, Box<NetResult<ComponentDetail>>),
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

pub struct GraphState {
    /// The views' settings and layout sizes (from the server).
    pub(crate) config: Option<TreeConfig>,
    config_requested: bool,
    /// The plans and walkthroughs there are.
    plans: Vec<(PlanId, arch_graph::PlanKind)>,
    plans_stale: bool,
    /// A tree under this plan wasn't found: the plans are asked for again,
    /// and only if it's not among them does the view say it's gone.
    plan_unsure: Option<PlanId>,
    pub nav: Nav,
    /// Where going in started, latest last (back pops it).
    trail: Vec<request::Step>,
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
    pub(crate) details: BTreeMap<(String, Source), Detail>,
    /// Files changed since they were indexed (their cards are marked).
    pub stale_files: Vec<String>,
    /// Review badges: changed since the last review, unaddressed comments.
    pub marks: BTreeMap<ComponentId, crate::review::NodeMark>,
    /// What the desk's comparison changed, by component: a badge on each;
    /// the only-changed filter shows those changed themselves (and what
    /// holds them).
    changed: BTreeMap<ComponentId, harness_contracts::changes::ComponentChanges>,
    /// Which read of the changes' components that is, and what was
    /// compared (find looks only in its files while only changed show).
    changed_read: u64,
    comparison: Option<harness_contracts::changes::Comparison>,
    load: Load,
    stale: bool,
    /// A card opened whose branch is to be fetched and merged in, and the
    /// request the tree held answers.
    expanding: Option<(String, arch_graph::tree::TreeRequest)>,
    generation: u64,
    camera: Camera,
    /// Fit the camera to the tree once it arrives (after the user moved).
    fit: bool,
    refit: bool,
    /// Show this card (in view, or in the middle) once the frame's camera
    /// is settled.
    reveal_after_fit: Option<(String, crate::viewport::Show)>,
    /// A key's move of the camera under way: from, to, since when (set
    /// on the first frame it's drawn).
    glide: Option<(Camera, Camera, Option<f64>)>,
    /// Moved by hand (a drag, the wheel, fingers) since the last key: the
    /// next key starts from what's on the screen.
    by_hand: bool,
    /// Opened to go to its first card: go there once it shows.
    first_child_of: Option<String>,
    /// The agent moved the view (shown until the user moves).
    pub agent: Option<AgentMove>,
    pub requests: GraphRequests,
    canvas: Rect,
    inbox: Arc<Mutex<Vec<Reply>>>,
    /// Behavior settings (chips, zoom), from the UI config.
    pub ui: GraphUi,
    /// A finger touched it (its chips show from then on).
    touch_seen: bool,
    /// A flick gliding on (points a second), slowing as a list's does.
    fling: Vec2,
    /// Two fingers took part in what the pointer is doing (lifting them
    /// isn't a flick).
    pinching: bool,
    /// The "+N" counts drawn this frame (click one to open its card).
    pub(crate) more_rects: Vec<(String, Rect)>,
    /// The camera before a search started previewing matches.
    saved_view: Option<Camera>,
    pub legend_open: bool,
    /// Something the graph has to say (a branch that didn't fit, a plan gone).
    pub note: Option<String>,
    /// Finding anything in the code (typed in the palette).
    pub(crate) find: actions::Find,
    /// A toolbar button whose action needs input: open the palette at it.
    /// Card texts as they fit their cards, at the camera's zoom.
    fitted: std::cell::RefCell<draw::Fitted>,
}

impl Default for GraphState {
    fn default() -> Self {
        Self {
            config: None,
            config_requested: false,
            plans: Vec::new(),
            plans_stale: true,
            plan_unsure: None,
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
            changed: BTreeMap::new(),
            changed_read: 0,
            comparison: None,
            load: Load::Idle,
            stale: true,
            expanding: None,
            generation: 0,
            camera: Camera::default(),
            fit: true,
            refit: false,
            reveal_after_fit: None,
            glide: None,
            by_hand: false,
            first_child_of: None,
            agent: None,
            requests: GraphRequests::default(),
            canvas: Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)),
            inbox: Arc::default(),
            ui: GraphUi::default(),
            touch_seen: false,
            fling: Vec2::ZERO,
            pinching: false,
            more_rects: Vec::new(),
            saved_view: None,
            legend_open: false,
            note: None,
            find: actions::Find::default(),
            fitted: Default::default(),
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
        match self.details.get(&(id.0.clone(), self.nav.source.clone())) {
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
        match self.details.get(&(key.clone(), self.nav.source.clone())) {
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
                        self.reveal_after_fit = Some((select.clone(), crate::viewport::Show::Middle));
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

    /// What the desk's comparison changed, by component; showing only
    /// those, the tree is asked for again when they change.
    pub fn set_changed(&mut self, changed: BTreeMap<ComponentId, harness_contracts::changes::ComponentChanges>, comparison: &harness_contracts::changes::Comparison, read: u64) {
        self.changed_read = read;
        self.comparison = Some(comparison.clone());
        if changed != self.changed {
            self.changed = changed;
            self.stale |= self.nav.only_changed;
        }
    }

    /// Which read of the changes' components it has.
    pub fn changed_read(&self) -> u64 {
        self.changed_read
    }

    /// Lines added and removed in a component, by the desk's comparison.
    pub fn changed(&self, id: &ComponentId) -> Option<(u32, u32)> {
        self.changed.get(id).map(|c| (c.added, c.removed))
    }

    /// The components changed themselves.
    fn changed_directly(&self) -> impl Iterator<Item = &ComponentId> {
        self.changed.iter().filter(|(_, c)| c.direct).map(|(id, _)| id)
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
                self.reveal_after_fit = Some((pending, crate::viewport::Show::Middle));
            } else {
                self.remembered = Some(pending);
            }
        }
        // Opened to go to its first card (none came: it says so).
        if let Some(parent) = self.first_child_of.take() {
            match select::children(&tree, &parent).first() {
                Some(first) => {
                    self.selection = Some(first.clone());
                    self.reveal_after_fit = Some((first.clone(), crate::viewport::Show::InView));
                }
                None => self.note = Some(format!("Opened {}: nothing more came to show.", tree.card(&parent).map_or(parent.as_str(), |c| c.label.as_str()))),
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

    /// Asks the server for each kind (symbols, files, lines) at once.
    fn send_find(&mut self, net: &NetClient, ctx: &egui::Context) {
        if !std::mem::take(&mut self.find.send) {
            return;
        }
        let limits = &self.ui.find;
        use arch_search::SearchKind;
        for (kind, limit) in [(SearchKind::Symbols, limits.symbols), (SearchKind::Files, limits.files), (SearchKind::Text, limits.text)] {
            let (inbox, asked) = (self.inbox.clone(), self.find.asked);
            let changed = self.comparison.as_ref().filter(|_| self.nav.only_changed);
            net.search(&self.find.query, kind, &self.nav.source, limit, changed, ctx.clone(), move |r| push(&inbox, Reply::Found(asked, kind, Box::new(r))));
        }
    }

    /// Fetches what's missing or stale and applies replies. Call every frame.
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::Config(Ok(config)) => self.config = Some(config),
                Reply::Config(Err(e)) => {
                    self.config_requested = false;
                    self.load = Load::Failed(e.into());
                }
                Reply::Plans(Ok(plans)) => self.plans_arrived(plans.into_iter().map(|p| (p.id, p.kind)).collect()),
                Reply::Plans(Err(_)) => {}
                Reply::Card(key, source, detail) => {
                    let detail = match *detail {
                        Ok(d) => Detail::Ready(Box::new(d)),
                        Err(e) => Detail::Failed(e.into()),
                    };
                    self.details.insert((key, source), detail);
                }
                Reply::Found(asked, kind, found) => self.found(asked, kind, *found),
                Reply::Expanded(generation, change) => {
                    if generation != self.generation {
                        continue;
                    }
                    match *change {
                        Ok(change) => self.take_change(change),
                        // Couldn't be had alone: the whole tree, again.
                        Err(_) => self.stale = true,
                    }
                }
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
            net.graph_tree(&self.nav.asking(self.changed_directly()), ctx.clone(), move |r| push(&inbox, Reply::Tree(generation, Box::new(r))));
        }
        if let Some((card, held)) = self.expanding.take()
            && self.tree.is_some()
        {
            let (inbox, generation) = (self.inbox.clone(), self.generation);
            net.graph_expand(&held, &card, 1, ctx.clone(), move |r| push(&inbox, Reply::Expanded(generation, Box::new(r))));
        }
        self.want_details(net, ctx);
        self.send_find(net, ctx);
    }

    /// The answer shown, when the server cut it or took longer than its
    /// limit: a few words for the status line, and more on hover.
    pub fn answer_status(&self) -> Option<(String, String)> {
        let tree = self.tree.as_ref()?;
        let mut words = Vec::new();
        let mut more = Vec::new();
        if let Some(cut) = &tree.cut {
            let why = if cut.reason == arch_graph::tree::CutReason::Time { "out of time" } else { "too big" };
            words.push(format!("cut ({why})"));
            more.push(format!("The graph shows {} of {} levels ({} more left out). Go into a card, or show fewer levels.", cut.shown, cut.asked, cut.hidden));
        }
        if let Some(ms) = tree.slow_ms {
            words.push(format!("slow: {ms} ms"));
            more.push("The server took longer than its limit to build it. Each stage's time is at /api/perf.".to_owned());
        }
        (!words.is_empty()).then(|| (format!("graph {}", words.join(", ")), more.join("\n")))
    }

    /// A tree that couldn't be had: a root that isn't there shows the
    /// view's starting points and says so. Otherwise, under a plan, the
    /// plans are asked for again (whether it's gone is theirs to say); the
    /// error shows meanwhile.
    fn failed(&mut self, error: crate::net::NetError) {
        // The server's 404: there's no such thing (not a word in its message).
        let missing = error.not_found();
        let error = String::from(error);
        if missing && let Some(root) = self.nav.root.clone() {
            self.navigate(NavChange::Top);
            self.note = Some(format!("{root} isn't in this view: showing where it starts."));
            return;
        }
        if missing && let Source::Plan(plan) = self.nav.source.clone() {
            self.plans_stale = true;
            self.plan_unsure = Some(plan);
        }
        self.load = Load::Failed(error);
    }
}

#[cfg(test)]
pub(crate) mod tests;
