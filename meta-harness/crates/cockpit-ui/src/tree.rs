//! The outline: the Contains view as a collapsible list (what contains
//! what), driven by keys like every view: a cursor row, folds, a filter that
//! `/` types into. Enter goes in (unfolds the row, then into its first
//! part), Backspace out (to what holds it); showing a row elsewhere is a
//! go-to, as anywhere. It holds what the server sent (the view's depth, plus
//! the rows opened further): a row is folded when it's shut or its parts
//! aren't here yet, and unfolding it loads them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use arch_graph::tree::{Card, Opened, Tree, TreeRequest, ViewKind};
use arch_graph::{ComponentId, Source};
use egui::RichText;

use crate::actions::{Action, ActionProvider, RunCx};
use crate::feedback::FeedbackInspectorState;
use crate::nav::{Fold, Item, Motion, Move, Navigable, Place, SearchEvent, Spot, Step, Target};
use crate::viewport::{Go, Show, Stop, Viewport};
use crate::net::{NetClient, NetResult};
use crate::theme::colors;

/// One drawn row: a component at a depth in the outline.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: ComponentId,
    pub depth: usize,
}

#[derive(Default)]
pub struct ArchitectureTreeState {
    /// Rows shown: those matching it, and what contains them.
    pub search_filter: String,
    /// The filter before a search started typing into it (Escape returns it).
    filter_before: Option<String>,
    /// Rows the search being typed opened (closed again as it changes).
    opened_by_search: BTreeSet<ComponentId>,
    source: Source,
    result: Option<Tree>,
    /// Cards by key in what was sent.
    cards: BTreeMap<ComponentId, Card>,
    /// Rows unfolded past the view's depth (their parts loaded), oldest first.
    opened: Vec<String>,
    stale: bool,
    loaded_once: bool,
    error: Option<String>,
    /// What it has to say about the last answer (rows it couldn't open).
    note: Option<String>,
    /// The number of the last request sent, and of the last answered: only
    /// the answer to the last one is taken (an older one arriving late
    /// would undo what was asked since).
    asked: u64,
    answered: u64,
    inbox: Arc<Mutex<Vec<Answer>>>,
    children: BTreeMap<ComponentId, Vec<ComponentId>>,
    parent: BTreeMap<ComponentId, ComponentId>,
    roots: Vec<ComponentId>,
    /// Rows folded shut (everything else is open).
    closed: BTreeSet<ComponentId>,
    pub cursor: Option<ComponentId>,
    /// How to show the cursor once the rows are laid out.
    show: Option<Show>,
    /// Where it's looking (the shared rules move it), the time last drawn,
    /// and a row's height with the space under it.
    viewport: Viewport,
    clock: f64,
    pitch: f32,
    row_h: f32,
    /// Rows drawn last frame, and where.
    spots: Vec<Spot>,
}

const ROW_H: f32 = 20.0;

/// An answer, with the number of the request it answers.
type Answer = (u64, NetResult<Tree>);

impl ArchitectureTreeState {
    fn set_source(&mut self, source: &Source) {
        if &self.source != source {
            self.source = source.clone();
            self.opened.clear();
            self.stale = true;
        }
    }

    /// Whether a row's parts are here.
    fn loaded(&self, id: &ComponentId) -> bool {
        self.children.get(id).is_some_and(|k| !k.is_empty())
    }

    /// Whether a row holds parts (here, or still to load).
    fn has_children(&self, id: &ComponentId) -> bool {
        self.loaded(id) || self.cards.get(id).is_some_and(|c| c.more > 0)
    }

    /// A row with parts that doesn't show them: shut by hand, or its parts
    /// not loaded yet. The one meaning of folded (the arrow, every key).
    fn folded(&self, id: &ComponentId) -> bool {
        self.has_children(id) && (self.closed.contains(id) || !self.loaded(id))
    }

    /// Asks for the rows' parts (every time: an answer that didn't bring
    /// them is never the last word).
    fn load(&mut self, ids: &[ComponentId]) {
        for id in ids {
            if !self.opened.iter().any(|o| o == id.as_str()) {
                self.opened.push(id.0.clone());
            }
        }
        self.stale |= !ids.is_empty();
    }

    /// Unfolds a folded row (loading its parts when they aren't here).
    /// Returns whether it was folded.
    fn unfold(&mut self, id: &ComponentId) -> bool {
        if !self.folded(id) {
            return false;
        }
        self.closed.remove(id);
        if !self.loaded(id) {
            self.load(std::slice::from_ref(id));
        }
        true
    }

    /// Folds an open row. Returns whether it was open.
    fn fold_row(&mut self, id: &ComponentId) -> bool {
        if !self.has_children(id) || self.folded(id) {
            return false;
        }
        self.closed.insert(id.clone());
        true
    }

    /// Takes a fetched Contains tree: who contains whom, in tree order.
    pub fn set_result(&mut self, result: Tree) {
        self.children.clear();
        self.parent.clear();
        self.opened.retain(|o| !result.dropped.contains(o));
        self.cards = result.cards.iter().map(|c| (ComponentId::new(c.key.clone()), c.clone())).collect();
        self.note = (!result.dropped.is_empty()).then(|| {
            let names: Vec<String> = result.dropped.iter().map(|k| self.name(&ComponentId::new(k.clone()))).collect();
            format!("Couldn't open {}: it goes over what one view holds.", names.join(", "))
        });
        for c in &result.cards {
            if let Some(p) = &c.parent
                && p != &c.key
            {
                let (id, p) = (ComponentId::new(c.key.clone()), ComponentId::new(p.clone()));
                self.parent.insert(id.clone(), p.clone());
                self.children.entry(p).or_default().push(id);
            }
        }
        // Containment loops: whatever can't reach a root becomes one.
        let reaches_root = |id: &ComponentId| {
            let mut seen = BTreeSet::new();
            let mut at = id;
            while let Some(p) = self.parent.get(at) {
                if !seen.insert(at.clone()) {
                    return false;
                }
                at = p;
            }
            true
        };
        let looped: Vec<ComponentId> = self.cards.keys().filter(|id| !reaches_root(id)).cloned().collect();
        for id in looped {
            if let Some(p) = self.parent.remove(&id)
                && let Some(kids) = self.children.get_mut(&p)
            {
                kids.retain(|k| k != &id);
            }
        }
        self.roots = result.cards.iter().map(|c| ComponentId::new(c.key.clone())).filter(|id| !self.parent.contains_key(id)).collect();
        self.closed.retain(|id| self.cards.contains_key(id));
        if self.cursor.as_ref().is_some_and(|c| !self.cards.contains_key(c)) {
            self.cursor = None;
        }
        self.result = Some(result);
        self.error = None;
    }

    /// Takes the answer to request `number`, when it's the last one asked.
    fn answer(&mut self, number: u64, reply: NetResult<Tree>) {
        if number != self.asked {
            return;
        }
        self.answered = number;
        match reply {
            Ok(result) => self.set_result(result),
            Err(e) => self.error = Some(e.into()),
        }
    }

    fn name(&self, id: &ComponentId) -> String {
        self.cards.get(id).map_or_else(|| id.to_string(), |c| c.label.clone())
    }

    fn matches(&self, id: &ComponentId) -> bool {
        let filter = self.search_filter.to_lowercase();
        filter.is_empty() || self.name(id).to_lowercase().contains(&filter) || id.as_str().to_lowercase().contains(&filter)
    }

    /// The rows on screen, in order: open rows' children under them; with a
    /// filter, only matches and what contains them (opened up).
    pub fn rows(&self) -> Vec<Row> {
        fn walk(tree: &ArchitectureTreeState, id: &ComponentId, depth: usize, keep: &dyn Fn(&ComponentId) -> bool, out: &mut Vec<Row>, seen: &mut BTreeSet<ComponentId>) {
            if !seen.insert(id.clone()) || !keep(id) {
                return;
            }
            out.push(Row { id: id.clone(), depth });
            if !tree.closed.contains(id) {
                for kid in tree.children.get(id).into_iter().flatten() {
                    walk(tree, kid, depth + 1, keep, out, seen);
                }
            }
        }
        let mut keep_ids: Option<BTreeSet<ComponentId>> = None;
        if !self.search_filter.is_empty() {
            let mut keep = BTreeSet::new();
            for id in self.all_ids() {
                if self.matches(&id) {
                    let mut at = Some(id);
                    while let Some(x) = at {
                        at = self.parent.get(&x).cloned();
                        keep.insert(x);
                    }
                }
            }
            keep_ids = Some(keep);
        }
        let keep = |id: &ComponentId| keep_ids.as_ref().is_none_or(|k| k.contains(id));
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for root in &self.roots {
            walk(self, root, 0, &keep, &mut out, &mut seen);
        }
        out
    }

    /// Every component in outline order (open or not).
    fn all_ids(&self) -> Vec<ComponentId> {
        fn walk(tree: &ArchitectureTreeState, id: &ComponentId, out: &mut Vec<ComponentId>, seen: &mut BTreeSet<ComponentId>) {
            if !seen.insert(id.clone()) {
                return;
            }
            out.push(id.clone());
            for kid in tree.children.get(id).into_iter().flatten() {
                walk(tree, kid, out, seen);
            }
        }
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for root in &self.roots {
            walk(self, root, &mut out, &mut seen);
        }
        out
    }

    /// Opens what contains `id`, so its row shows.
    fn open_to(&mut self, id: &ComponentId) {
        let mut at = self.parent.get(id).cloned();
        while let Some(p) = at {
            self.closed.remove(&p);
            at = self.parent.get(&p).cloned();
        }
    }

    /// Into an open row's first part. Returns whether there was one.
    fn go_in(&mut self, id: &ComponentId) -> bool {
        let first = (!self.folded(id)).then(|| self.children.get(id).and_then(|k| k.first()).cloned()).flatten();
        first.inspect(|first| self.reveal(first, Show::InView)).is_some()
    }

    /// Out to what holds the row. Returns whether something does.
    fn go_out(&mut self, id: &ComponentId) -> bool {
        self.parent.get(id).cloned().inspect(|p| self.reveal(p, Show::InView)).is_some()
    }

    /// Puts the cursor on `id` (opening what contains it) and shows it.
    pub fn reveal(&mut self, id: &ComponentId, how: Show) {
        if self.cards.contains_key(id) || self.parent.contains_key(id) || self.roots.contains(id) {
            self.open_to(id);
            if !self.rows().iter().any(|r| &r.id == id) {
                // Filtered out: the filter goes so the row can show.
                self.search_filter.clear();
            }
            self.cursor = Some(id.clone());
            self.show = Some(how);
        }
    }

    fn put_cursor(&mut self, index: usize, how: Show) {
        let rows = self.rows();
        if let Some(row) = rows.get(index.min(rows.len().saturating_sub(1))) {
            self.cursor = Some(row.id.clone());
            self.show = Some(how);
        }
    }

    /// Moves by the shared rules from where the cursor is: a row each.
    fn go(&mut self, go: Go) {
        // A key's move: what was asked before it is done with.
        self.show = None;
        let rows = self.rows();
        let pitch = if self.pitch > 0.0 { self.pitch } else { ROW_H };
        let stops: Vec<Stop> = (0..rows.len()).map(|i| Stop::new(i as f32 * pitch, pitch)).collect();
        if let Some(moved) = self.viewport.go(&stops, self.cursor_index(&rows), go) {
            self.cursor = Some(rows[moved.cursor].id.clone());
            self.viewport.take(&moved, self.clock, true);
        }
    }

    fn cursor_index(&self, rows: &[Row]) -> Option<usize> {
        self.cursor.as_ref().and_then(|c| rows.iter().position(|r| &r.id == c))
    }

    pub(crate) fn draw(&mut self, ui: &mut egui::Ui, feedback: &mut FeedbackInspectorState, cx: &mut RunCx<'_>) {
        let panel = cx.panel;
        let header = ui.horizontal(|ui| ui.add(egui::TextEdit::singleline(&mut self.search_filter).hint_text("filter…").desired_width(f32::INFINITY)));
        feedback.register_inspectable(ui, &format!("panel_{}:tree:header", panel + 1), header.response.rect);
        ui.separator();
        if self.result.is_none() {
            ui.label(RichText::new(self.error.clone().unwrap_or_else(|| "Loading…".into())).color(colors().muted));
            return;
        }
        let rows = self.rows();
        if self.cursor.is_none() && !rows.is_empty() {
            self.cursor = Some(rows[0].id.clone());
        }
        let row_h = ui.text_style_height(&egui::TextStyle::Body).max(ROW_H).max(self.row_h);
        let pitch = row_h + ui.spacing().item_spacing.y;
        (self.pitch, self.viewport.margin, self.clock) = (pitch, 2.0 * pitch, ui.input(|i| i.time));
        // Asked before the rows were laid out: shown now (once the window's
        // size is known).
        if self.viewport.window.y > 0.0
            && let (Some(show), Some(at)) = (self.show, self.cursor_index(&rows))
        {
            self.show = None;
            let stops: Vec<Stop> = (0..rows.len()).map(|i| Stop::new(i as f32 * pitch, pitch)).collect();
            if let Some(moved) = self.viewport.go(&stops, Some(at), Go::To(at, show)) {
                self.viewport.take(&moved, self.clock, false);
            }
        }
        let mut area = egui::ScrollArea::vertical().id_salt("tree_rows").auto_shrink([false, false]);
        if let Some(offset) = self.viewport.wanted(self.clock) {
            area = area.vertical_scroll_offset(offset.y);
        }
        let mut spots = Vec::new();
        let mut measured: f32 = 0.0;
        let output = area.show_rows(ui, row_h, rows.len(), |ui, range| {
            for row in &rows[range] {
                let on_cursor = self.cursor.as_ref() == Some(&row.id);
                let response = ui
                    .horizontal(|ui| {
                        ui.set_min_height(row_h);
                        ui.add_space(row.depth as f32 * crate::theme::look().space_xl);
                        if self.has_children(&row.id) {
                            let icon = if self.folded(&row.id) { egui_phosphor::regular::CARET_RIGHT } else { egui_phosphor::regular::CARET_DOWN };
                            ui.label(RichText::new(icon).size(crate::theme::look().text_body).color(colors().muted));
                        } else {
                            ui.add_space(crate::theme::look().space_xl);
                        }
                        let c = self.cards.get(&row.id);
                        let (name, kind) = c.map_or_else(|| (row.id.to_string(), String::new()), |c| (c.label.clone(), c.kind.clone()));
                        let color = if self.matches(&row.id) { colors().text } else { colors().faint };
                        let text = RichText::new(format!("{name} ")).size(crate::theme::look().text_label).color(if on_cursor { colors().text_strong } else { color });
                        let label = ui.label(if on_cursor { text.background_color(colors().selection) } else { text });
                        ui.label(RichText::new(kind).size(crate::theme::look().text_small).color(colors().muted));
                        label.rect
                    });
                measured = measured.max(response.response.rect.height());
                spots.push(Spot { key: row.id.to_string(), rect: response.inner });
            }
        });
        if measured > self.row_h + 0.5 {
            self.row_h = measured;
            ui.ctx().request_repaint();
        }
        self.viewport.drawn(output.state.offset, output.inner_rect.size(), output.content_size);
        if self.viewport.gliding() || self.show.is_some() {
            ui.ctx().request_repaint();
        }
        self.spots = spots.into_iter().filter(|s| s.rect.intersects(output.inner_rect)).collect();
    }
}

/// What the outline's own actions do.
enum TreeCommand {
    ClearFilter,
}

impl ArchitectureTreeState {
    /// The shared moves where the cursor is, in the outline's words:
    /// through the rows, into and out of a row (unfolding and folding it),
    /// between siblings, folding rows.
    fn moves(&self) -> Vec<Action> {
        use crate::actions::SECONDARY;
        let mut out = crate::nav::list_moves();
        out.push(Move::Motion(Motion::Left).offer("Fold, or up to what holds it").importance(SECONDARY));
        out.push(Move::Motion(Motion::Right).offer("Unfold, or into its first child").importance(SECONDARY));
        out.push(Move::Motion(Motion::NextSibling).offer("Next sibling row"));
        out.push(Move::Motion(Motion::PrevSibling).offer("Previous sibling row"));
        let id = self.cursor.as_ref();
        out.push(match id {
            Some(id) if self.folded(id) => Move::Enter.offer(format!("Unfold {}", self.name(id))),
            Some(id) => match self.children.get(id).and_then(|k| k.first()) {
                Some(first) => Move::Enter.offer(format!("Into {}", self.name(first))),
                None => Move::Enter.offer_if(false, format!("{} holds nothing", self.name(id))),
            },
            None => Move::Enter.offer_if(false, "Unfold, or into its first part"),
        });
        out.push(match id.and_then(|id| self.parent.get(id)) {
            Some(p) => Move::Leave.offer(format!("Out to {}", self.name(p))),
            None => Move::Leave.offer_if(false, "Out to what holds it"),
        });
        // On a leaf, the fold is the row that holds it.
        let row = id.and_then(|id| if self.has_children(id) { Some(id) } else { self.parent.get(id) });
        let name = row.map(|r| self.name(r));
        out.extend(crate::nav::fold_here(row.zip(name.as_deref()).map(|(r, n)| (n, self.folded(r)))));
        let ids = self.all_ids();
        let (any_folded, any_open) = (ids.iter().any(|id| self.folded(id)), ids.iter().any(|id| self.has_children(id) && !self.folded(id)));
        out.extend(crate::nav::fold_all((any_folded, "Unfold everything"), (any_open, "Fold everything")));
        out
    }
}

impl ActionProvider for ArchitectureTreeState {
    fn actions(&self) -> Vec<Action> {
        let mut out = self.moves();
        if !self.search_filter.is_empty() {
            out.push(Action::new("tree.clear_filter", "Clear the filter", TreeCommand::ClearFilter).focused().importance(crate::actions::MINOR).header(crate::actions::Style::Button, "", Some(egui_phosphor::regular::X)));
        }
        out
    }

    fn run(&mut self, action: &Action, _value: Option<String>, _cx: &mut RunCx<'_>) {
        if let Some(TreeCommand::ClearFilter) = action.command::<TreeCommand>() {
            self.search_filter.clear();
        }
    }
}

impl crate::buffer::Buffer for ArchitectureTreeState {
    fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, drawing: &mut crate::buffer::Drawing<'_>) {
        self.draw(ui, drawing.feedback, cx);
    }

    fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for (number, reply) in replies {
            self.answer(number, reply);
        }
        if self.stale || !self.loaded_once {
            self.stale = false;
            self.loaded_once = true;
            self.asked += 1;
            let (inbox, number) = (self.inbox.clone(), self.asked);
            let request = TreeRequest {
                view: ViewKind::Contains,
                source: self.source.clone(),
                opened: self.opened.iter().map(|card| Opened { card: card.clone(), levels: 1 }).collect(),
                // With a plan: the plan inside the whole codebase.
                full_code: true,
                ..TreeRequest::default()
            };
            net.graph_tree(&request, ctx.clone(), move |r| {
                if let Ok(mut i) = inbox.lock() {
                    i.push((number, r));
                }
            });
        }
    }


    /// Changed code or plans: the outline read again.
    fn on_event(&mut self, event: &arch_graph::ArchEvent) {
        if matches!(event, arch_graph::ArchEvent::Changed { .. }) {
            crate::buffer::Buffer::invalidate(self);
        }
    }

    fn invalidate(&mut self) {
        self.stale = true;
    }


    /// A row's name.
    fn knows(&self, id: &ComponentId) -> Option<crate::buffer::Known> {
        self.cards.get(id).map(|c| crate::buffer::Known { name: Some(c.label.clone()), ..Default::default() })
    }

    fn describe(&self) -> String {
        self.cursor.as_ref().map_or_else(|| "the outline".to_owned(), |id| format!("at {}", self.name(id)))
    }
}

impl crate::hosted::Hosted for ArchitectureTreeState {
    fn title(&self) -> crate::hosted::Status {
        crate::hosted::Status::new(format!("{} Outline", egui_phosphor::regular::TREE_STRUCTURE), crate::hosted::Tone::Info)
    }

    fn busy(&self) -> Option<String> {
        (self.asked != self.answered).then(|| "Loading the outline".into())
    }

    /// What the server left out of the last answer, and what it couldn't open.
    fn status(&self) -> Vec<crate::hosted::Status> {
        use crate::hosted::{Status, Tone};
        let mut out = Vec::new();
        if let Some(cut) = self.result.as_ref().and_then(|t| t.cut.as_ref()) {
            let why = if cut.reason == arch_graph::tree::CutReason::Time { "out of time" } else { "too big" };
            let mut more = format!("It shows {} of {} levels; {} parts left out. Unfold a row to load what's in it.", cut.shown, cut.asked, cut.hidden);
            if cut.starts_left_out > 0 {
                more.push_str(&format!(" {} top-level parts aren't shown.", cut.starts_left_out));
            }
            out.push(Status::new(format!("cut ({why})"), Tone::Warn).detail(more));
        }
        if let Some(note) = &self.note {
            out.push(Status::new(note.clone(), Tone::Warn));
        }
        out
    }
}

impl Navigable for ArchitectureTreeState {
    /// The source in use: its outline.
    fn share(&mut self, shared: &crate::shared::Shared) {
        self.set_source(shared.source.get());
    }

    fn place(&self) -> Place {
        Place::Tree { cursor: self.cursor.clone() }
    }

    fn restore(&mut self, place: &Place) {
        if let Place::Tree { cursor: Some(id) } = place {
            self.reveal(&id.clone(), Show::Middle);
        }
    }

    fn target(&self) -> Option<Target> {
        self.cursor.clone().map(Target::Component)
    }

    /// Any component, not the one under the cursor.
    fn can_show(&self, target: &Target) -> bool {
        target.in_component() && target.elsewhere(self.target().as_ref())
    }

    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        let id = target.component().ok_or("Not a component.")?;
        self.reveal(id, Show::Middle);
        Ok(())
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let at = self.cursor_index(&rows);
        match motion {
            Motion::Down if at.is_none() => self.go(Go::First),
            Motion::Down => self.go(Go::Down(count)),
            Motion::Up => self.go(Go::Up(count)),
            Motion::HalfDown => self.go(Go::HalfDown(count)),
            Motion::HalfUp => self.go(Go::HalfUp(count)),
            Motion::First => self.go(Go::First),
            Motion::Last => self.go(Go::Last),
            Motion::Line(n) => self.put_cursor(n.saturating_sub(1), Show::Middle),
            // h: fold the row, or step out to what contains it.
            Motion::Left => {
                for _ in 0..count {
                    let Some(id) = self.cursor.clone() else { return };
                    if !self.fold_row(&id) {
                        self.go_out(&id);
                    }
                }
            }
            // l: unfold the row, or step into its first part.
            Motion::Right => {
                for _ in 0..count {
                    let Some(id) = self.cursor.clone() else { return };
                    if !self.unfold(&id) {
                        self.go_in(&id);
                    }
                }
            }
            Motion::NextSibling | Motion::PrevSibling => {
                let Some(id) = self.cursor.clone() else { return };
                let siblings: Vec<ComponentId> = match self.parent.get(&id) {
                    Some(p) => self.children.get(p).cloned().unwrap_or_default(),
                    None => self.roots.clone(),
                };
                let shown: Vec<&ComponentId> = siblings.iter().filter(|s| rows.iter().any(|r| &r.id == *s)).collect();
                if let Some(i) = shown.iter().position(|s| **s == id) {
                    let next = if motion == Motion::NextSibling { (i + count).min(shown.len() - 1) } else { i.saturating_sub(count) };
                    let target = shown[next].clone();
                    self.reveal(&target, Show::InView);
                }
            }
            Motion::HalfLeft | Motion::HalfRight | Motion::FewLeft | Motion::FewRight => {}
        }
    }

    /// In: a folded row unfolds; an open one, into its first part.
    fn enter(&mut self) -> Step {
        let Some(id) = self.cursor.clone() else { return Step::Nothing("The outline is empty.".into()) };
        if self.unfold(&id) || self.go_in(&id) {
            return Step::Done;
        }
        Step::Nothing(format!("{} holds nothing.", self.name(&id)))
    }

    /// Out: to what holds the row.
    fn leave(&mut self) -> Step {
        let Some(id) = self.cursor.clone() else { return Step::Nothing("The outline is empty.".into()) };
        if self.go_out(&id) {
            return Step::Done;
        }
        Step::Nothing("Already at the top.".into())
    }

    fn fold(&mut self, fold: Fold) {
        let cursor = self.cursor.clone();
        match fold {
            // Every folded row, one level: shut ones open, the rest load.
            Fold::OpenAll => {
                self.closed.clear();
                let unloaded: Vec<ComponentId> = self.all_ids().into_iter().filter(|id| self.folded(id)).collect();
                self.load(&unloaded);
            }
            Fold::CloseAll => {
                self.closed = self.children.iter().filter(|(_, k)| !k.is_empty()).map(|(id, _)| id.clone()).collect();
                // The cursor's row may be inside a folded one now: the top-level row it's in.
                if let Some(mut at) = cursor {
                    while let Some(p) = self.parent.get(&at) {
                        at = p.clone();
                    }
                    self.cursor = Some(at);
                    self.show = Some(Show::InView);
                }
            }
            Fold::Open | Fold::Close | Fold::Toggle => {
                let Some(id) = cursor else { return };
                // On a leaf, the fold is the row that holds it.
                let row = if self.has_children(&id) { id } else { match self.parent.get(&id).cloned() { Some(p) => p, None => return } };
                let open = match fold {
                    Fold::Open => true,
                    Fold::Close => false,
                    _ => self.folded(&row),
                };
                if open {
                    self.unfold(&row);
                } else if self.fold_row(&row) && self.cursor.as_ref() != Some(&row) {
                    self.cursor = Some(row);
                    self.show = Some(Show::InView);
                }
            }
        }
    }

    fn items(&self) -> Vec<Item> {
        self.all_ids().into_iter().map(|id| Item::new(id.as_str(), self.name(&id), id.as_str())).collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.cursor.as_ref().map(|c| c.to_string())
    }

    fn move_to(&mut self, key: &str) {
        self.reveal(&ComponentId::new(key), Show::Middle);
    }

    /// `/` types into the filter; Escape puts back the one there was.
    fn on_search(&mut self, event: SearchEvent<'_>) {
        match event {
            SearchEvent::Typing(query) => {
                if self.filter_before.is_none() {
                    self.filter_before = Some(self.search_filter.clone());
                }
                self.search_filter = query.to_owned();
                // What matches shows: open the rows on the way to it (only
                // those: what an earlier, shorter query opened shuts again).
                let opened = std::mem::take(&mut self.opened_by_search);
                self.closed.extend(opened);
                if !query.is_empty() {
                    let found: Vec<ComponentId> = self.all_ids().into_iter().filter(|id| self.matches(id)).collect();
                    for id in found {
                        let mut at = self.parent.get(&id).cloned();
                        while let Some(p) = at {
                            if self.closed.remove(&p) {
                                self.opened_by_search.insert(p.clone());
                            }
                            at = self.parent.get(&p).cloned();
                        }
                    }
                }
            }
            SearchEvent::Confirmed => {
                self.filter_before = None;
                self.opened_by_search.clear();
            }
            SearchEvent::Cancelled => {
                if let Some(before) = self.filter_before.take() {
                    self.search_filter = before;
                }
                let opened = std::mem::take(&mut self.opened_by_search);
                self.closed.extend(opened);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::nav::rank_matches;
    use arch_graph::tree::{TreeRequest, ViewKind};
    use arch_graph::{Graph, Provenance, Relation, RelationKind};

    /// Two modules: `a` holds run, step and save (and run holds log); `b` is empty.
    pub(crate) fn outline() -> Tree {
        outline_to(3)
    }

    /// The outline sent `depth` levels deep.
    fn outline_to(depth: u32) -> Tree {
        let sample = crate::graph::tests::sample_graph();
        let mut g = Graph::default();
        for id in ["rust:a", "rust:b", "rust:a::run", "rust:a::step", "rust:a::save", "rust:a::log"] {
            g.upsert_component(sample.component(&ComponentId::new(id)).unwrap().clone(), Provenance::OBSERVED);
        }
        for (a, b) in [("rust:a", "rust:a::run"), ("rust:a", "rust:a::step"), ("rust:a", "rust:a::save"), ("rust:a::run", "rust:a::log")] {
            g.upsert_relation(Relation::new(RelationKind::Contains, ComponentId::new(a), ComponentId::new(b), Provenance::OBSERVED));
        }
        crate::graph::tests::tree_of(&g, None, TreeRequest { view: ViewKind::Contains, depth: Some(depth), ..TreeRequest::default() })
    }

    fn ids(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| format!("{}{}", "  ".repeat(r.depth), r.id.as_str().trim_start_matches("rust:"))).collect()
    }

    fn cursor(t: &ArchitectureTreeState) -> &str {
        t.cursor.as_ref().map_or("", |c| c.as_str().trim_start_matches("rust:"))
    }

    #[test]
    fn the_outline_is_driven_by_keys() {
        let mut t = ArchitectureTreeState::default();
        t.set_result(outline());
        assert_eq!(ids(&t.rows()), ["a", "  a::run", "    a::log", "  a::save", "  a::step", "b"], "parts by name");
        // j/k with counts, gg/G.
        t.motion(Motion::Down, 1);
        assert_eq!(cursor(&t), "a");
        t.motion(Motion::Down, 2);
        assert_eq!(cursor(&t), "a::log");
        t.motion(Motion::Last, 1);
        assert_eq!(cursor(&t), "b");
        t.motion(Motion::Up, 3);
        assert_eq!(cursor(&t), "a::log");
        t.motion(Motion::First, 1);
        assert_eq!(cursor(&t), "a");
        // l: into the first child; h: fold, then up.
        t.motion(Motion::Right, 1);
        assert_eq!(cursor(&t), "a::run");
        t.motion(Motion::Left, 1);
        assert_eq!(cursor(&t), "a::run", "h on an open row folds it");
        assert_eq!(ids(&t.rows()), ["a", "  a::run", "  a::save", "  a::step", "b"]);
        t.motion(Motion::Left, 1);
        assert_eq!(cursor(&t), "a", "h on a folded row goes up");
        t.motion(Motion::Right, 1);
        assert_eq!(cursor(&t), "a::run", "l steps in");
        t.motion(Motion::Right, 1);
        assert_eq!(cursor(&t), "a::run", "l on a folded row unfolds it");
        assert_eq!(t.rows().len(), 6);
        // ]] / [[: siblings.
        t.motion(Motion::NextSibling, 1);
        assert_eq!(cursor(&t), "a::save");
        t.motion(Motion::PrevSibling, 5);
        assert_eq!(cursor(&t), "a::run");
        // Enter goes in, Backspace out.
        assert_eq!(t.enter(), Step::Done);
        assert_eq!(cursor(&t), "a::log", "Enter on an open row: into its first part");
        assert_eq!(t.enter(), Step::Nothing("log holds nothing.".into()));
        assert_eq!(t.leave(), Step::Done);
        assert_eq!((cursor(&t), t.rows().len()), ("a::run", 6), "Backspace goes out without folding");
        t.motion(Motion::Left, 1);
        assert!(t.folded(&ComponentId::new("rust:a::run")));
        assert_eq!(t.enter(), Step::Done, "Enter on a folded row unfolds it");
        assert_eq!((cursor(&t), t.rows().len()), ("a::run", 6));
        t.leave();
        assert_eq!(t.leave(), Step::Nothing("Already at the top.".into()));
        // zM / zR / za.
        t.fold(Fold::CloseAll);
        assert_eq!(ids(&t.rows()), ["a", "b"]);
        assert_eq!(cursor(&t), "a", "the cursor goes to the row holding it");
        t.fold(Fold::OpenAll);
        assert_eq!(t.rows().len(), 6);
        t.move_to("rust:a::step");
        t.fold(Fold::Toggle);
        assert_eq!((ids(&t.rows()).len(), cursor(&t)), (2, "a"), "za on a leaf folds the row holding it");
        // / filters as it's typed; Escape puts the filter back; n/N find rows by name.
        t.fold(Fold::OpenAll);
        t.on_search(SearchEvent::Typing("lo"));
        assert_eq!(ids(&t.rows()), ["a", "  a::run", "    a::log"], "matches and what holds them");
        t.move_to("rust:a::run");
        t.fold(Fold::Close);
        assert_eq!(ids(&t.rows()), ["a", "  a::run"], "folds hold while filtered");
        t.on_search(SearchEvent::Typing("log"));
        assert_eq!(ids(&t.rows()), ["a", "  a::run", "    a::log"], "typing opens the way to matches");
        t.on_search(SearchEvent::Cancelled);
        assert_eq!(t.rows().len(), 5, "giving up the search folds back what it opened");
        let label = |t: &ArchitectureTreeState, id: &str| t.actions().into_iter().find(|a| a.id == id).map(|a| a.label);
        assert_eq!(label(&t, "nav.enter").as_deref(), Some("Unfold run"), "Enter says what it does");
        t.fold(Fold::OpenAll);
        assert_eq!(t.rows().len(), 6);
        assert_eq!(label(&t, "nav.enter").as_deref(), Some("Into log"));
        assert_eq!(label(&t, "nav.leave").as_deref(), Some("Out to a"));
        let items = t.items();
        assert_eq!(items[rank_matches(&items, "step")[0]].key, "rust:a::step");
        // Restoring a place opens the way to it.
        t.fold(Fold::CloseAll);
        t.restore(&Place::Tree { cursor: Some(ComponentId::new("rust:a::log")) });
        assert_eq!(cursor(&t), "a::log");
        assert!(t.rows().iter().any(|r| r.id.as_str() == "rust:a::log"));
    }

    #[test]
    fn a_row_whose_parts_arent_here_is_folded_and_unfolding_it_loads_them() {
        let run = ComponentId::new("rust:a::run");
        let mut t = ArchitectureTreeState { loaded_once: true, ..Default::default() };
        t.set_result(outline_to(1));
        assert_eq!(ids(&t.rows()), ["a", "  a::run", "  a::save", "  a::step", "b"]);
        assert!(t.folded(&run) && t.has_children(&run), "run holds log, not sent yet: folded like a shut row");
        t.move_to("rust:a::run");
        let label = |t: &ArchitectureTreeState, id: &str| t.actions().into_iter().find(|a| a.id == id).map(|a| a.label);
        assert_eq!(label(&t, "nav.enter").as_deref(), Some("Unfold run"));
        // h on it goes out (it's folded); l and Enter ask for its parts.
        t.motion(Motion::Left, 1);
        assert_eq!(cursor(&t), "a");
        t.move_to("rust:a::run");
        assert_eq!(t.enter(), Step::Done);
        assert!(t.stale && t.opened == ["rust:a::run"], "asked for");
        // An answer that didn't bring them: asking again asks again.
        t.stale = false;
        t.motion(Motion::Right, 1);
        assert!(t.stale && t.opened.len() == 1, "asked again, once in the list");
        // Answers: only the last request's is taken.
        t.asked = 2;
        t.answer(1, Ok(outline_to(1)));
        assert_eq!((t.answered, t.rows().len()), (0, 5), "an older answer arriving late is ignored");
        assert!(crate::hosted::Hosted::busy(&t).is_some(), "still waiting");
        t.answer(2, Ok(outline_to(3)));
        assert_eq!(ids(&t.rows()), ["a", "  a::run", "    a::log", "  a::save", "  a::step", "b"]);
        assert!(crate::hosted::Hosted::busy(&t).is_none() && !t.folded(&run));
        assert_eq!(t.enter(), Step::Done);
        assert_eq!(cursor(&t), "a::log");
        // Unfold everything loads every row not loaded.
        let mut all = ArchitectureTreeState::default();
        all.set_result(outline_to(1));
        all.fold(Fold::OpenAll);
        assert_eq!(all.opened, ["rust:a::run"]);
    }

    #[test]
    fn a_row_that_didnt_fit_says_so() {
        let mut t = ArchitectureTreeState::default();
        t.set_result(outline_to(1));
        t.load(&[ComponentId::new("rust:a::run")]);
        let mut answer = outline_to(1);
        answer.dropped = vec!["rust:a::run".into()];
        answer.cut = Some(arch_graph::tree::Cut { reason: arch_graph::tree::CutReason::Budget, starts_left_out: 0, shown: 1, asked: 2, hidden: 1 });
        t.set_result(answer);
        let said: Vec<String> = crate::hosted::Hosted::status(&t).into_iter().map(|s| s.text).collect();
        assert_eq!(said, ["cut (too big)", "Couldn't open run: it goes over what one view holds."]);
        assert!(t.opened.is_empty() && t.folded(&ComponentId::new("rust:a::run")), "it stays folded, and asking again asks again");
        t.set_result(outline_to(3));
        assert!(crate::hosted::Hosted::status(&t).is_empty(), "said until the next answer");
    }

    #[test]
    fn drawn_rows_take_hints_and_half_pages_follow_the_viewport() {
        let ctx = egui::Context::default();
        let net = NetClient::new("http://127.0.0.1:9");
        let mut feedback = FeedbackInspectorState::default();
        let mut t = ArchitectureTreeState { loaded_once: true, ..Default::default() };
        t.set_result(outline());
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 160.0))), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let (mut mode, mut note, mut asked) = (crate::keymap::InputMode::Normal, None, None);
                    t.draw(ui, &mut feedback, &mut RunCx { net: &net, ctx, input_mode: &mut mode, panel: 0, note: &mut note, asked: &mut asked });
                });
            });
        }
        let spots = t.spots();
        assert!(!spots.is_empty() && spots.len() < 6, "only what fits is labelled: {}", spots.len());
        assert_eq!(spots[0].key, "rust:a");
        // Half a window moves the view and the cursor together.
        t.motion(Motion::First, 1);
        t.motion(Motion::HalfDown, 1);
        let at = t.cursor_index(&t.rows()).unwrap();
        let top = at as f32 * t.pitch;
        assert!(at > 0 && t.viewport.offset.y > 0.0, "both moved: row {at}, offset {}", t.viewport.offset.y);
        assert!(top >= t.viewport.offset.y && top < t.viewport.offset.y + t.viewport.window.y, "the cursor stays on screen");
    }

    #[test]
    fn as_a_destination_it_shows_any_component_but_the_one_under_the_cursor() {
        let mut t = ArchitectureTreeState::default();
        t.set_result(outline());
        t.motion(Motion::Down, 1);
        let here = t.target().unwrap();
        let other = Target::Component(ComponentId::new(format!("{}::step", here.component().unwrap().as_str())));
        assert!(!t.can_show(&here) && t.can_show(&other));
        assert_eq!(t.show_target(&other), Ok(()));
        assert_eq!(t.target(), Some(other));
    }

}
