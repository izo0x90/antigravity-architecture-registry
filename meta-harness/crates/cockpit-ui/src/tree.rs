//! The outline: the structure view as a collapsible tree (what contains
//! what), driven by keys like every view: a cursor row, folds, a filter that
//! `/` types into. Enter (or a click) selects the row's component in the
//! graph.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use arch_graph::{ComponentId, PlanId, RelationKind, ViewResult};
use egui::{RichText, Sense};

use crate::actions::{Action, ActionProvider, RunCx};
use crate::feedback::FeedbackInspectorState;
use crate::nav::{Fold, Item, Motion, Navigable, Place, Scroll, SearchEvent, Spot, Step, Target, scroll_offset};
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
    plan: Option<PlanId>,
    result: Option<ViewResult>,
    stale: bool,
    loaded_once: bool,
    error: Option<String>,
    /// Select this in the graph.
    pub select: Option<ComponentId>,
    inbox: Arc<Mutex<Vec<NetResult<ViewResult>>>>,
    children: BTreeMap<ComponentId, Vec<ComponentId>>,
    parent: BTreeMap<ComponentId, ComponentId>,
    roots: Vec<ComponentId>,
    /// Rows folded shut (everything else is open).
    closed: BTreeSet<ComponentId>,
    pub cursor: Option<ComponentId>,
    scroll: Option<Scroll>,
    offset_y: f32,
    view_h: f32,
    row_h: f32,
    /// Rows drawn last frame, and where.
    spots: Vec<Spot>,
}

const ROW_H: f32 = 20.0;

impl ArchitectureTreeState {
    pub fn set_plan(&mut self, plan: Option<&PlanId>) {
        if self.plan.as_ref() != plan {
            self.plan = plan.cloned();
            self.stale = true;
        }
    }

    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Takes a fetched structure view: who contains whom, in view order.
    pub fn set_result(&mut self, result: ViewResult) {
        self.children.clear();
        self.parent.clear();
        for e in result.view.edges.iter().filter(|e| e.kind == RelationKind::Contains) {
            if !self.parent.contains_key(&e.target) && e.source != e.target {
                self.parent.insert(e.target.clone(), e.source.clone());
                self.children.entry(e.source.clone()).or_default().push(e.target.clone());
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
        let looped: Vec<ComponentId> = result.view.nodes.iter().map(|n| n.id.clone()).filter(|id| !reaches_root(id)).collect();
        for id in looped {
            if let Some(p) = self.parent.remove(&id)
                && let Some(kids) = self.children.get_mut(&p)
            {
                kids.retain(|k| k != &id);
            }
        }
        self.roots = result.view.nodes.iter().map(|n| n.id.clone()).filter(|id| !self.parent.contains_key(id)).collect();
        self.closed.retain(|id| result.components.contains_key(id));
        if self.cursor.as_ref().is_some_and(|c| !result.components.contains_key(c)) {
            self.cursor = None;
        }
        self.result = Some(result);
        self.error = None;
    }

    fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Ok(result) => self.set_result(result),
                Err(e) => self.error = Some(e),
            }
        }
        if self.stale || !self.loaded_once {
            self.stale = false;
            self.loaded_once = true;
            let inbox = self.inbox.clone();
            net.arch_view("structure", self.plan.as_ref(), &[], Some(12), ctx.clone(), move |r| {
                if let Ok(mut i) = inbox.lock() {
                    i.push(r);
                }
            });
        }
    }

    fn name(&self, id: &ComponentId) -> String {
        self.result.as_ref().and_then(|r| r.components.get(id)).map_or_else(|| id.to_string(), |c| c.name.clone())
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

    fn has_children(&self, id: &ComponentId) -> bool {
        self.children.get(id).is_some_and(|k| !k.is_empty())
    }

    /// Opens what contains `id`, so its row shows.
    fn open_to(&mut self, id: &ComponentId) {
        let mut at = self.parent.get(id).cloned();
        while let Some(p) = at {
            self.closed.remove(&p);
            at = self.parent.get(&p).cloned();
        }
    }

    /// Puts the cursor on `id` (opening what contains it) and scrolls to it.
    pub fn reveal(&mut self, id: &ComponentId, how: Scroll) {
        if self.result.as_ref().is_some_and(|r| r.components.contains_key(id)) || self.parent.contains_key(id) || self.roots.contains(id) {
            self.open_to(id);
            if !self.rows().iter().any(|r| &r.id == id) {
                // Filtered out: the filter goes so the row can show.
                self.search_filter.clear();
            }
            self.cursor = Some(id.clone());
            self.scroll = Some(how);
        }
    }

    fn put_cursor(&mut self, index: usize, how: Scroll) {
        let rows = self.rows();
        if let Some(row) = rows.get(index.min(rows.len().saturating_sub(1))) {
            self.cursor = Some(row.id.clone());
            self.scroll = Some(how);
        }
    }

    fn cursor_index(&self, rows: &[Row]) -> Option<usize> {
        self.cursor.as_ref().and_then(|c| rows.iter().position(|r| &r.id == c))
    }

    /// Rows that fit in half the viewport.
    fn half_page_rows(&self) -> usize {
        let pitch = if self.row_h > 0.0 { self.row_h } else { ROW_H };
        if self.view_h > 0.0 { ((self.view_h / pitch) / 2.0).floor().max(1.0) as usize } else { 10 }
    }

    pub fn render(&mut self, ui: &mut egui::Ui, net: &NetClient, feedback: &mut FeedbackInspectorState, slot_idx: usize) {
        self.tick(net, ui.ctx());
        let header = ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} Outline", egui_phosphor::regular::TREE_STRUCTURE)).strong().color(colors().info));
            ui.add(egui::TextEdit::singleline(&mut self.search_filter).hint_text("filter… (/)").desired_width(160.0));
            if !self.search_filter.is_empty() && ui.small_button(egui_phosphor::regular::X).on_hover_text("Clear the filter").clicked() {
                self.search_filter.clear();
            }
        });
        feedback.register_inspectable(ui, &format!("slot_{}:tree:header", slot_idx + 1), header.response.rect);
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
        let mut area = egui::ScrollArea::vertical().id_salt("tree_rows").auto_shrink([false, false]);
        if let (Some(how), Some(at)) = (self.scroll, self.cursor_index(&rows)) {
            let view_h = if self.view_h > 0.0 { self.view_h } else { ui.available_height() };
            area = area.vertical_scroll_offset(scroll_offset(how, at, pitch, self.offset_y, view_h));
            self.scroll = None;
        }
        let (mut toggled, mut picked) = (None, None);
        let mut spots = Vec::new();
        let mut measured: f32 = 0.0;
        let output = area.show_rows(ui, row_h, rows.len(), |ui, range| {
            for row in &rows[range] {
                let on_cursor = self.cursor.as_ref() == Some(&row.id);
                let response = ui
                    .horizontal(|ui| {
                        ui.set_min_height(row_h);
                        ui.add_space(row.depth as f32 * 14.0);
                        if self.has_children(&row.id) {
                            let open = !self.closed.contains(&row.id);
                            let icon = if open { egui_phosphor::regular::CARET_DOWN } else { egui_phosphor::regular::CARET_RIGHT };
                            if ui.add(egui::Label::new(RichText::new(icon).size(11.0).color(colors().muted)).sense(Sense::click())).clicked() {
                                toggled = Some(row.id.clone());
                            }
                        } else {
                            ui.add_space(14.0);
                        }
                        let c = self.result.as_ref().and_then(|r| r.components.get(&row.id));
                        let (name, kind) = c.map_or_else(|| (row.id.to_string(), String::new()), |c| (c.name.clone(), c.type_name.clone()));
                        let color = if self.matches(&row.id) { colors().text } else { colors().faint };
                        let text = RichText::new(format!("{name} ")).size(12.0).color(if on_cursor { colors().text_strong } else { color });
                        let label = ui.selectable_label(on_cursor, text);
                        ui.label(RichText::new(kind).size(10.0).color(colors().muted));
                        if label.clicked() {
                            picked = Some(row.id.clone());
                        }
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
        self.offset_y = output.state.offset.y;
        self.view_h = output.inner_rect.height();
        self.spots = spots.into_iter().filter(|s| s.rect.intersects(output.inner_rect)).collect();
        if let Some(id) = toggled
            && !self.closed.remove(&id)
        {
            self.closed.insert(id);
        }
        if let Some(id) = picked {
            self.cursor = Some(id.clone());
            self.select = Some(id);
        }
    }
}

impl ActionProvider for ArchitectureTreeState {
    fn actions(&self) -> Vec<Action> {
        Vec::new()
    }

    fn run(&mut self, _action: &Action, _value: Option<String>, _cx: &mut RunCx<'_>) {}
}

impl Navigable for ArchitectureTreeState {
    fn place(&self) -> Place {
        Place::Tree { cursor: self.cursor.clone() }
    }

    fn restore(&mut self, place: &Place) {
        if let Place::Tree { cursor: Some(id) } = place {
            self.reveal(&id.clone(), Scroll::Center);
        }
    }

    fn target(&self) -> Option<Target> {
        self.cursor.clone().map(Target::Component)
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let at = self.cursor_index(&rows);
        let here = at.unwrap_or(0);
        match motion {
            Motion::Down => self.put_cursor(at.map_or(0, |i| i + count), Scroll::KeepVisible),
            Motion::Up => self.put_cursor(here.saturating_sub(count), Scroll::KeepVisible),
            Motion::HalfDown => self.put_cursor(here + self.half_page_rows() * count, Scroll::KeepVisible),
            Motion::HalfUp => self.put_cursor(here.saturating_sub(self.half_page_rows() * count), Scroll::KeepVisible),
            Motion::First => self.put_cursor(0, Scroll::KeepVisible),
            Motion::Last => self.put_cursor(rows.len() - 1, Scroll::KeepVisible),
            Motion::Line(n) => self.put_cursor(n.saturating_sub(1), Scroll::Center),
            // h: fold the row, or step out to what contains it.
            Motion::Left => {
                for _ in 0..count {
                    let Some(id) = self.cursor.clone() else { return };
                    if self.has_children(&id) && !self.closed.contains(&id) {
                        self.closed.insert(id);
                    } else if let Some(p) = self.parent.get(&id).cloned() {
                        self.reveal(&p, Scroll::KeepVisible);
                    }
                }
            }
            // l: unfold the row, or step into its first child.
            Motion::Right => {
                for _ in 0..count {
                    let Some(id) = self.cursor.clone() else { return };
                    if self.closed.remove(&id) {
                        continue;
                    }
                    if let Some(first) = self.children.get(&id).and_then(|k| k.first()).cloned() {
                        self.reveal(&first, Scroll::KeepVisible);
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
                    self.reveal(&target, Scroll::KeepVisible);
                }
            }
        }
    }

    /// Unfolds a folded row; otherwise selects it in the graph.
    fn enter(&mut self) -> Step {
        let Some(id) = self.cursor.clone() else { return Step::Nothing("The outline is empty.".into()) };
        if self.closed.remove(&id) {
            return Step::Done;
        }
        self.select = Some(id);
        Step::Done
    }

    fn can_enter(&self) -> bool {
        self.cursor.is_some()
    }

    fn enter_label(&self) -> Option<String> {
        let id = self.cursor.as_ref()?;
        Some(if self.closed.contains(id) { "Unfold".to_owned() } else { format!("Select {} in the graph", self.name(id)) })
    }

    fn can_leave(&self) -> bool {
        self.cursor.as_ref().is_some_and(|id| (self.has_children(id) && !self.closed.contains(id)) || self.parent.contains_key(id))
    }

    /// Folds an open row; otherwise goes to what contains it.
    fn leave(&mut self) -> Step {
        let Some(id) = self.cursor.clone() else { return Step::Nothing("The outline is empty.".into()) };
        if self.has_children(&id) && !self.closed.contains(&id) {
            self.closed.insert(id);
            return Step::Done;
        }
        match self.parent.get(&id).cloned() {
            Some(p) => {
                self.reveal(&p, Scroll::KeepVisible);
                Step::Done
            }
            None => Step::Nothing("Already at the top.".into()),
        }
    }

    fn fold(&mut self, fold: Fold) {
        let cursor = self.cursor.clone();
        match fold {
            Fold::OpenAll => self.closed.clear(),
            Fold::CloseAll => {
                self.closed = self.children.iter().filter(|(_, k)| !k.is_empty()).map(|(id, _)| id.clone()).collect();
                // The cursor's row may be inside a folded one now: the top-level row it's in.
                if let Some(mut at) = cursor {
                    while let Some(p) = self.parent.get(&at) {
                        at = p.clone();
                    }
                    self.cursor = Some(at);
                    self.scroll = Some(Scroll::KeepVisible);
                }
            }
            Fold::Open | Fold::Close | Fold::Toggle => {
                let Some(id) = cursor else { return };
                // On a leaf, the fold is the row that holds it.
                let row = if self.has_children(&id) { id } else { match self.parent.get(&id).cloned() { Some(p) => p, None => return } };
                let open = match fold {
                    Fold::Open => true,
                    Fold::Close => false,
                    _ => self.closed.contains(&row),
                };
                if open {
                    self.closed.remove(&row);
                } else {
                    self.closed.insert(row.clone());
                    if self.cursor.as_ref() != Some(&row) {
                        self.cursor = Some(row);
                        self.scroll = Some(Scroll::KeepVisible);
                    }
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
        self.reveal(&ComponentId::new(key), Scroll::Center);
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
    use arch_graph::view::ViewEdge;
    use arch_graph::{Alignment, RelationId};

    /// Two modules: `a` holds run, step and save (and run holds log); `b` is empty.
    pub(crate) fn outline() -> ViewResult {
        let mut r = crate::graph::tests::sample_result();
        for module in ["rust:a", "rust:b"] {
            let mut c = r.components[&ComponentId::new("rust:a::run")].clone();
            c.id = ComponentId::new(module);
            c.name = module.trim_start_matches("rust:").into();
            c.type_name = "module".into();
            r.components.insert(c.id.clone(), c);
        }
        let contains = |a: &str, b: &str| ViewEdge { id: RelationId(format!("contains:{a}->{b}")), kind: RelationKind::Contains, source: ComponentId::new(a), target: ComponentId::new(b), alignment: Alignment::Observed, overlay: false, order: None };
        r.view.nodes.insert(0, arch_graph::view::ViewNode { id: ComponentId::new("rust:a"), depth: Some(0), truncated: false, alignment: Alignment::Observed });
        r.view.nodes.push(arch_graph::view::ViewNode { id: ComponentId::new("rust:b"), depth: Some(0), truncated: false, alignment: Alignment::Observed });
        r.view.edges = vec![contains("rust:a", "rust:a::run"), contains("rust:a", "rust:a::step"), contains("rust:a", "rust:a::save"), contains("rust:a::run", "rust:a::log")];
        r
    }

    fn ids(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| format!("{}{}", "  ".repeat(r.depth), r.id.as_str().trim_start_matches("rust:"))).collect()
    }

    fn cursor(t: &ArchitectureTreeState) -> &str {
        t.cursor.as_ref().map_or("", |c| c.as_str().trim_start_matches("rust:"))
    }

    // req: N.17, N.11
    #[test]
    fn the_outline_is_driven_by_keys() {
        let mut t = ArchitectureTreeState::default();
        t.set_result(outline());
        assert_eq!(ids(&t.rows()), ["a", "  a::run", "    a::log", "  a::step", "  a::save", "b"]);
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
        assert_eq!(ids(&t.rows()), ["a", "  a::run", "  a::step", "  a::save", "b"]);
        t.motion(Motion::Left, 1);
        assert_eq!(cursor(&t), "a", "h on a folded row goes up");
        t.motion(Motion::Right, 1);
        assert_eq!(cursor(&t), "a::run", "l steps in");
        t.motion(Motion::Right, 1);
        assert_eq!(cursor(&t), "a::run", "l on a folded row unfolds it");
        assert_eq!(t.rows().len(), 6);
        // ]] / [[: siblings.
        t.motion(Motion::NextSibling, 1);
        assert_eq!(cursor(&t), "a::step");
        t.motion(Motion::PrevSibling, 5);
        assert_eq!(cursor(&t), "a::run");
        // Enter selects in the graph; Backspace goes up (folding first).
        assert_eq!(t.enter(), Step::Done);
        assert_eq!(t.select.take(), Some(ComponentId::new("rust:a::run")));
        assert_eq!(t.leave(), Step::Done);
        assert!(t.closed.contains(&ComponentId::new("rust:a::run")));
        assert_eq!(t.enter(), Step::Done, "Enter on a folded row unfolds it");
        assert!(t.select.is_none());
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
        assert_eq!(t.enter_label().as_deref(), Some("Unfold"), "Enter says what it does");
        t.fold(Fold::OpenAll);
        assert_eq!(t.rows().len(), 6);
        assert_eq!(t.enter_label().as_deref(), Some("Select run in the graph"));
        let items = t.items();
        assert_eq!(items[rank_matches(&items, "step")[0]].key, "rust:a::step");
        // Restoring a place opens the way to it.
        t.fold(Fold::CloseAll);
        t.restore(&Place::Tree { cursor: Some(ComponentId::new("rust:a::log")) });
        assert_eq!(cursor(&t), "a::log");
        assert!(t.rows().iter().any(|r| r.id.as_str() == "rust:a::log"));
    }

    // req: N.8, N.6
    #[test]
    fn drawn_rows_take_hints_and_half_pages_follow_the_viewport() {
        let ctx = egui::Context::default();
        let net = NetClient::new("http://127.0.0.1:9");
        let mut feedback = FeedbackInspectorState::default();
        let mut t = ArchitectureTreeState { loaded_once: true, ..Default::default() };
        t.set_result(outline());
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 160.0))), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| t.render(ui, &net, &mut feedback, 0));
            });
        }
        let spots = t.spots();
        assert!(!spots.is_empty() && spots.len() < 6, "only what fits is labelled: {}", spots.len());
        assert_eq!(spots[0].key, "rust:a");
        let half = t.half_page_rows();
        assert!((1..=4).contains(&half), "{half}");
        t.motion(Motion::First, 1);
        t.motion(Motion::HalfDown, 1);
        assert_eq!(t.cursor_index(&t.rows()), Some(half));
    }
}
