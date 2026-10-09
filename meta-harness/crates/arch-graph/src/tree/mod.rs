//! The three views as trees: Contains (boxes in boxes), Calls (functions in
//! call order, down or up) and Data flow (data types, with the functions
//! that turn one into another on the lines). Each tree starts from a root,
//! goes to a depth, and is cut to a card budget, saying what it cut. Pure
//! logic over [`GraphRead`]: the server builds and lays trees out; the
//! cockpit only draws them.

mod layout;
mod lens;
mod read;
mod text;
mod walk;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use layout::{Point, Rect, card_lines, count_text, problems};
pub use read::GraphRead;
pub use text::render;

use crate::{ComponentId, ModelError, Plan, PlanId, RelationKind, Source};

const DEFAULT_CONFIG: &str = include_str!("../../config/trees.toml");

/// Which of the three views.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    #[default]
    Contains,
    Calls,
    DataFlow,
}

impl ViewKind {
    pub const ALL: [ViewKind; 3] = [ViewKind::Contains, ViewKind::Calls, ViewKind::DataFlow];

    pub fn name(self) -> &'static str {
        match self {
            ViewKind::Contains => "contains",
            ViewKind::Calls => "calls",
            ViewKind::DataFlow => "data_flow",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.name() == name)
    }
}

/// Down: what it calls, contains, or turns into. Up: what calls it, holds
/// it, or it came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Way {
    #[default]
    Down,
    Up,
}

impl Way {
    pub fn flipped(self) -> Self {
        match self {
            Way::Down => Way::Up,
            Way::Up => Way::Down,
        }
    }
}

/// A card opened by hand some levels further than the depth.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opened {
    pub card: String,
    #[serde(default = "one")]
    pub levels: u32,
}

fn one() -> u32 {
    1
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// What to show.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TreeRequest {
    #[serde(default)]
    pub view: ViewKind,
    #[serde(default)]
    pub source: Source,
    /// The card to start from; none: the view's natural starting points
    /// (with a plan, the plan's own top-level items).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default)]
    pub way: Way,
    /// Levels below the root; none: the view's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// Cards opened by hand, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opened: Vec<Opened>,
    /// Cards closed by hand (their children hidden).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<String>,
    /// With a plan: show it inside the whole codebase instead of only the
    /// plan and the code it plugs into.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub full_code: bool,
}

/// How a planned thing and the code line up, as shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    /// Plain code the plan doesn't touch.
    #[default]
    Code,
    /// Planned, not in the code yet.
    New,
    /// In the code, and the plan changes it.
    Changed,
    /// Planned, and the code now matches.
    Done,
    /// The code differs from what the plan says.
    Mismatch,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Card {
    /// A component's id, or `type:<name>` for a data type, `start:` /
    /// `end:` plus a function's id where data starts or stops.
    pub key: String,
    pub label: String,
    /// The component's type, or `data`, `start`, `end`.
    pub kind: String,
    /// A second line: where it is, or its status.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// The card it hangs from in the tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub level: u32,
    /// How many children it has that aren't shown.
    #[serde(default)]
    pub more: u32,
    /// Its children are shown.
    #[serde(default)]
    pub open: bool,
    /// Lines from it to cards shown elsewhere that weren't drawn (over the
    /// answer's line limit).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub links_left_out: u32,
    #[serde(default)]
    pub mark: Mark,
    /// Other plans that also change it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_plans: Vec<PlanId>,
    /// It can be called (a function, a method): what Calls can start from.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub callable: bool,
    /// The component behind it (for details), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentId>,
    /// With a plan in focus: code the plan plugs into, not part of it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outside_plan: bool,
    pub rect: Rect,
}

/// How a line relates to the tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    /// From a card to its child.
    #[default]
    Tree,
    /// To a card already shown elsewhere (drawn once, pointed at).
    Repeat,
    /// Back up to a card above it on the same path.
    Loop,
}

/// What sits on a line: data going in or coming back (Calls), or the
/// function that turns one data type into the next (Data flow).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PillRole {
    In,
    Out,
    Step,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pill {
    pub text: String,
    pub role: PillRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub from: String,
    pub to: String,
    pub kind: LineKind,
    pub relation: RelationKind,
    /// Calls: position among the caller's calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
    #[serde(default)]
    pub mark: Mark,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pills: Vec<Pill>,
    /// Where it runs, once laid out (what's always drawn).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<Point>,
    /// A line to a card drawn further away: its whole way there, drawn
    /// while either end is selected (`path` is then only a stub).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reach: Vec<Point>,
    /// A call found in the code: how it's known (none for structure and
    /// for what only a plan says).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
}

/// How a link found in the code is known.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// The language's own analyzer confirmed it (its name and version).
    Proven(String),
    /// Only matched by name: likely, not confirmed.
    Named,
}

impl Evidence {
    /// How a relation is known, when it was found in the code.
    pub fn of(relation: &crate::Relation) -> Option<Self> {
        relation.provenance.observed.then(|| relation.proven_by.clone().map_or(Evidence::Named, Evidence::Proven))
    }
}

/// Why a tree was cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutReason {
    /// More cards than one answer holds.
    Budget,
    /// Building it took too long.
    Time,
}

/// What a tree didn't show.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cut {
    pub reason: CutReason,
    /// Starting points not shown (no root picked, more than the view shows).
    #[serde(default)]
    pub starts_left_out: u32,
    /// Levels shown below the root, of the levels asked for.
    pub shown: u32,
    pub asked: u32,
    /// Cards left out (counted on the cards they hang from).
    pub hidden: u32,
}

/// One step of where the root sits (what contains it, outermost first).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Crumb {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tree {
    /// What was asked, with the depth filled in.
    pub request: TreeRequest,
    /// The cards at the top (one with a root; the starting points without).
    pub roots: Vec<String>,
    pub cards: Vec<Card>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<Line>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut: Option<Cut>,
    /// Where the root sits, outermost first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<Crumb>,
    /// Cards opened by hand that didn't fit and were closed again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<String>,
    /// The laid-out drawing's size.
    pub size: (f32, f32),
    /// Set when building it took longer than the server's slow limit: how
    /// long it took, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_ms: Option<u64>,
}

impl Tree {
    pub fn card(&self, key: &str) -> Option<&Card> {
        self.cards.iter().find(|c| c.key == key)
    }
}

/// One view's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewConfig {
    pub description: String,
    pub depth: u32,
    /// Starting points shown when no root is picked.
    pub starts: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntryConfig {
    pub skip_paths: Vec<String>,
    pub skip_modules: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataConfig {
    pub ignore: Vec<String>,
    pub strip: Vec<String>,
}

/// Sizes for laying trees out (the cockpit draws with the same).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutConfig {
    pub char_width: f32,
    pub line_height: f32,
    pub name_size: f32,
    pub small_size: f32,
    pub pad: f32,
    pub card_min_w: f32,
    pub card_max_w: f32,
    pub gap_x: f32,
    pub gap_y: f32,
    pub header: f32,
    pub box_gap: f32,
    pub row_aspect: f32,
    pub curve_steps: usize,
}

impl LayoutConfig {
    /// How wide `text` is at `size`.
    pub fn text_width(&self, text: &str, size: f32) -> f32 {
        text.chars().count() as f32 * size * self.char_width
    }
}

/// The views, where Calls starts, what data is, and layout sizes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TreeConfig {
    pub views: BTreeMap<ViewKind, ViewConfig>,
    pub entries: EntryConfig,
    pub data: DataConfig,
    pub layout: LayoutConfig,
}

impl TreeConfig {
    pub fn defaults() -> Result<Self, ModelError> {
        toml::from_str(DEFAULT_CONFIG).map_err(|e| ModelError::Config(format!("trees: {e}")))
    }

    /// The defaults with a project's overrides (only the keys it gives).
    pub fn with_overrides(text: &str) -> Result<Self, ModelError> {
        let config = |e: &dyn std::fmt::Display| ModelError::Config(format!("trees: {e}"));
        let mut base: toml::Table = toml::from_str(DEFAULT_CONFIG).map_err(|e| config(&e))?;
        let overrides: toml::Table = toml::from_str(text).map_err(|e| config(&e))?;
        merge(&mut base, overrides);
        toml::Value::Table(base).try_into().map_err(|e| config(&e))
    }

    pub fn view(&self, view: ViewKind) -> &ViewConfig {
        // Every view is in the built-in config.
        &self.views[&view]
    }
}

fn merge(base: &mut toml::Table, overrides: toml::Table) {
    for (key, value) in overrides {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(inner)), toml::Value::Table(update)) => merge(inner, update),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// What a tree is built from besides the request.
pub struct Input<'a, G: GraphRead> {
    /// The code, or the code with the request's plan laid over it.
    pub graph: &'a G,
    /// The request's plan, if it shows one.
    pub plan: Option<&'a Plan>,
    /// For each component, the other plans that change it.
    pub other_plans: &'a BTreeMap<ComponentId, Vec<PlanId>>,
    pub config: &'a TreeConfig,
    /// What each component type allows (data types, callables).
    pub types: &'a crate::TypeSystem,
    /// Cards one answer may hold.
    pub budget: usize,
    /// Lines to cards shown elsewhere (repeats, loops) one answer may hold.
    pub link_budget: usize,
    /// Asked between levels: true when building has taken too long.
    pub out_of_time: &'a dyn Fn() -> bool,
}

/// Builds the tree a request asks for, cut to the budget, and lays it out.
pub fn build<G: GraphRead>(input: &Input<G>, request: &TreeRequest) -> Tree {
    let mut tree = walk_tree(input, request);
    lay_out(&mut tree, &input.config.layout);
    tree
}

/// The cards and lines, not yet placed (`build` without the layout, so the
/// two can be timed apart).
pub fn walk_tree<G: GraphRead>(input: &Input<G>, request: &TreeRequest) -> Tree {
    let lens = lens::Lens::new(input, request);
    walk::walk(&lens, input, request)
}

/// Places a walked tree's cards and lines.
pub fn lay_out(tree: &mut Tree, config: &LayoutConfig) {
    let view = tree.request.view;
    layout::lay_out(tree, view, config);
}

/// The cards `card` adds when opened `levels` further: only the new ones
/// (what the tree already shows isn't sent again).
pub fn expand<G: GraphRead>(input: &Input<G>, request: &TreeRequest, card: &str, levels: u32) -> Tree {
    let before = build(input, request);
    let mut wider = request.clone();
    wider.closed.retain(|c| c != card);
    wider.opened.push(Opened { card: card.to_owned(), levels });
    let mut after = build(input, &wider);
    let known: std::collections::BTreeSet<&str> = before.cards.iter().map(|c| c.key.as_str()).collect();
    after.cards.retain(|c| !known.contains(c.key.as_str()));
    let added: std::collections::BTreeSet<String> = after.cards.iter().map(|c| c.key.clone()).collect();
    after.lines.retain(|l| added.contains(&l.to) || added.contains(&l.from));
    after
}

/// Where a card sits: what holds it, what else is in there, and the card's
/// own lines to those (for going out).
pub fn context<G: GraphRead>(input: &Input<G>, request: &TreeRequest, card: &str) -> Tree {
    let lens = lens::Lens::new(input, request);
    let mut tree = walk::context(&lens, input, request, card);
    layout::lay_out(&mut tree, request.view, &input.config.layout);
    tree
}

#[cfg(test)]
mod tests;
