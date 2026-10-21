//! The spec view: everything about one component, in full. It follows the
//! graph's selection (or stays pinned). A cursor walks every item in it
//! (section titles, relations, steps, invariants, findings, discussions
//! and what's said in them), so every control has a key.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use arch_graph::{ComponentDetail, ComponentId, Finding, PlanDiff, RelationKind, Report, Severity, Source};
use egui::RichText;
use harness_contracts::changes::{Comparison, FileChanges, Hunk};
use harness_contracts::{Discussion, DiscussionId, DiscussionStatus, Subject};

use crate::actions::{Action, ActionProvider, Choice, Group, MINOR, PRIMARY, Param, RunCx, SECONDARY, Style, When};
use crate::card::Section;
use crate::hosted::{Status, Tone};
use crate::nav::{Ask, Fold, GoTo, Item, Motion, Move, Navigable, Place, Spot, Step, Target};
use crate::net::{NetClient, NetResult};
use crate::theme::colors;
use crate::viewport::{Go, Measured, Show};

/// A file's changes, by the comparison and file they were asked for.
type ChangesReply = (Comparison, String, NetResult<FileChanges>);

/// One navigable item in the spec view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecItem {
    /// The section's title row.
    Header,
    Relation { kind: RelationKind, other: ComponentId, outgoing: bool },
    Step(usize),
    Invariant(usize),
    Placeholder(usize),
    Finding(usize),
    /// The component's source (the code section's body).
    Source,
    /// One of the changes in it (the changes section).
    Hunk(usize),
    /// A discussion (what it asks).
    Comment(DiscussionId),
    /// What's said in it after (from 0: its second message on).
    Reply(DiscussionId, usize),
}

/// The section a discussion shows in, when it's on `id`'s card: one on the
/// component (or the whole plan) in the overview, on a relation in
/// relations, on a section in its own.
pub fn section_of(on: &Subject, id: &ComponentId) -> Option<Section> {
    match on {
        Subject::PlanComponent { id: on, .. } if on == id => Some(Section::Overview),
        Subject::PlanSection { id: on, section, .. } if on == id => Some(Section::from_key(section).unwrap_or(Section::Overview)),
        Subject::PlanRelation { source, target, .. } if source == id || target == id => Some(Section::Relations),
        // The whole plan's: on every card (it holds every approval).
        Subject::Plan { .. } => Some(Section::Overview),
        _ => None,
    }
}

enum Reply {
    Detail(ComponentId, Box<NetResult<ComponentDetail>>),
    Report(ComponentId, NetResult<Report>),
    /// A review write finished.
    Posted(Result<(), String>),
}

/// One cursor stop: an item in a section.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub section: Section,
    pub item: SpecItem,
}

impl Entry {
    /// Stable across frames (search results and marks hold it).
    pub fn key(&self) -> String {
        format!("{}#{:?}", self.section.key(), self.item)
    }
}

/// A plan's item shown as a card (instead of a component's): the plan as
/// the desk last read it (`Shared::review`; none: it reads another now).
pub struct ItemCard {
    pub plan: arch_graph::PlanId,
    pub id: arch_graph::ItemId,
    read: Option<Result<arch_graph::Plan, String>>,
}

impl ItemCard {
    fn key(&self) -> String {
        format!("item#{}", self.id)
    }
}

pub struct SpecState {
    /// What it shows: the graph selection unless pinned.
    pub target: Option<ComponentId>,
    /// Or a plan's item (then there's no component).
    pub item: Option<ItemCard>,
    /// What it's read from: the code, or the code with a plan (whose parts
    /// are discussed, and whose checks run).
    pub source: Source,
    pub pinned: bool,
    /// Shown in the quick look (held on its card there, so no pinning).
    pub floating: bool,
    pub detail: Option<ComponentDetail>,
    pub findings: Vec<Finding>,
    /// The section the cursor is in.
    pub current: Section,
    /// The cursor: an index into `entries()`.
    pub cursor: usize,
    pub folded: BTreeSet<Section>,
    /// Where it's looking (the shared rules move it, an item a stop).
    scrolling: Measured,
    pub(crate) loaded: Option<(ComponentId, Source)>,
    error: Option<String>,
    /// What it asked of the rest of the cockpit, not taken yet.
    asks: Vec<Ask>,
    /// The component the user was on last (shown once it's unpinned).
    followed: Option<ComponentId>,
    /// What it took last from what the buffers share.
    seen: crate::shared::Seen,
    /// The cockpit user (who reviews).
    pub author: String,
    /// The discussions on the plan's parts (all of them) and its diff since
    /// the last review.
    pub discussions: Vec<Discussion>,
    pub diff: PlanDiff,
    /// What's said in its discussions, drawn (an agent's as markdown).
    markdown: egui_commonmark::CommonMarkCache,
    /// The review point `] c` is on.
    pub point: Option<DiscussionId>,
    note: Option<String>,
    /// The component's code changed since it was indexed (set by the app).
    pub out_of_date: bool,
    /// The component types (what each requires), from the server.
    pub types: Option<arch_graph::TypeSystem>,
    /// What the desk's comparison changed in its code: its file's changes
    /// that touch its lines; what they were asked for (the comparison, the
    /// file, the list as read); and the answer's way back.
    changes: Vec<Hunk>,
    changes_asked: Option<(Comparison, String, u64)>,
    /// The desk's comparison, the read of its list, and the lines of
    /// context around a change (what it shows).
    comparing: (Comparison, u64, u32),
    changes_inbox: Arc<Mutex<Option<ChangesReply>>>,
    /// The item a restored place is on, until the component's detail is in.
    restore_item: Option<String>,
    /// Items drawn last frame, and where.
    spots: Vec<Spot>,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

impl Default for SpecState {
    fn default() -> Self {
        Self {
            target: None,
            source: Source::Code,
            pinned: false,
            floating: false,
            detail: None,
            findings: Vec::new(),
            current: Section::Overview,
            cursor: 0,
            folded: BTreeSet::new(),
            scrolling: Measured::default(),
            loaded: None,
            error: None,
            asks: Vec::new(),
            followed: None,
            seen: Default::default(),
            author: String::new(),
            discussions: Vec::new(),
            diff: PlanDiff::default(),
            markdown: Default::default(),
            point: None,
            note: None,
            out_of_date: false,
            types: None,
            changes: Vec::new(),
            changes_asked: None,
            comparing: Default::default(),
            changes_inbox: Arc::default(),
            restore_item: None,
            item: None,
            spots: Vec::new(),
            inbox: Arc::default(),
        }
    }
}

impl SpecState {
    /// Reads components from `source` (unless pinned).
    fn follow_source(&mut self, source: &Source) {
        if !self.pinned && &self.source != source {
            self.source = source.clone();
        }
    }

    /// Moving to another component: the cursor and the review point
    /// belonged to the old one.
    fn leave_component(&mut self) {
        self.changes.clear();
        self.changes_asked = None;
        self.cursor = 0;
        self.current = Section::Overview;
        self.point = None;
    }

    /// Its code's lines, from the index: the file and first and last line.
    fn code_lines(&self) -> Option<(String, u32, u32)> {
        let anchor = self.detail.as_ref()?.component.anchor.as_ref()?;
        Some((anchor.file.as_str().to_owned(), anchor.span.start_line, anchor.span.end_line.max(anchor.span.start_line)))
    }

    /// Shows what the desk's comparison changed in its code (with lines of
    /// context around each change): asked for when the component, the
    /// comparison or its list (the times it was read) changes.
    fn follow_changes(&mut self, net: &NetClient, ctx: &egui::Context) {
        let (comparison, read, context) = (&self.comparing.0, self.comparing.1, self.comparing.2);
        let on_files = comparison.on_files();
        let lines = self.code_lines();
        if let Some((asked, path, result)) = self.changes_inbox.lock().ok().and_then(|mut r| r.take())
            && asked == on_files
            && let Some((file, first, last)) = &lines
            && path == *file
        {
            let touches = |h: &Hunk| h.new_start <= *last && h.new_start + h.new_lines.max(1) > *first;
            self.changes = result.map(|c| c.hunks.into_iter().filter(touches).collect()).unwrap_or_default();
        }
        let Some((file, ..)) = lines else {
            self.changes.clear();
            return;
        };
        let want = (on_files, file, read);
        if self.changes_asked.as_ref() == Some(&want) {
            return;
        }
        let inbox = self.changes_inbox.clone();
        let (asked, path) = (want.0.clone(), want.1.clone());
        net.change_file(&want.0, &want.1, harness_contracts::changes::Group::Changes, context, ctx.clone(), move |r| {
            if let Ok(mut i) = inbox.lock() {
                *i = Some((asked, path, r));
            }
        });
        self.changes_asked = Some(want);
    }

    /// Shows `id` even while pinned (a go-to asked for it).
    /// Shows `id` where the user went: not pinned, it's what the user is on
    /// now (what follows that moves to it too).
    fn land_on(&mut self, id: ComponentId) {
        if !self.pinned {
            self.asks.push(Ask::Follow(id.clone()));
        }
        self.show(id);
    }

    pub fn show(&mut self, id: ComponentId) {
        if self.target.as_ref() != Some(&id) {
            self.leave_component();
        }
        self.item = None;
        self.target = Some(id);
    }

    /// Shows a plan's item's card (no component's).
    fn show_item(&mut self, plan: &arch_graph::PlanId, id: &arch_graph::ItemId) {
        if self.item.as_ref().is_some_and(|c| c.plan == *plan && c.id == *id) {
            return;
        }
        self.leave_component();
        self.target = None;
        self.detail = None;
        self.item = Some(ItemCard { plan: plan.clone(), id: id.clone(), read: None });
        // What's shared is taken again: the desk's plan, for its card.
        self.seen.review = u64::MAX;
    }

    /// Draws a plan's item's card: its title and state, its text, the item
    /// it's under and those under it.
    fn draw_item(&mut self, ui: &mut egui::Ui) {
        // Its discussions, drawn as every conversation is.
        let mut markdown = std::mem::take(&mut self.markdown);
        let on: Vec<Discussion> = match &self.item {
            Some(card) => {
                let item = Subject::PlanItem { plan: card.plan.clone(), item: card.id.0.clone() };
                self.discussions.iter().filter(|d| d.on.subject == item).cloned().collect()
            }
            None => Vec::new(),
        };
        self.draw_item_with(ui, &on, &mut markdown);
        self.markdown = markdown;
    }

    fn draw_item_with(&mut self, ui: &mut egui::Ui, on: &[Discussion], markdown: &mut egui_commonmark::CommonMarkCache) {
        let Some(card) = &self.item else { return };
        let look = crate::theme::look();
        let key = card.key();
        let plan = match &card.read {
            None => {
                ui.label(RichText::new("Loading…").color(colors().muted));
                return;
            }
            Some(Err(e)) => {
                ui.label(RichText::new(format!("Couldn't read plan {}: {e}", card.plan)).color(colors().muted));
                return;
            }
            Some(Ok(plan)) => plan,
        };
        let Some(item) = plan.item(&card.id) else {
            ui.label(RichText::new(format!("Plan {} has no item {} any more.", card.plan, card.id)).color(colors().muted));
            self.spots.clear();
            return;
        };
        let response = ui
            .vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&item.title).size(look.text_title).strong());
                    ui.label(RichText::new(item.state.name()).color(colors().focus));
                });
                ui.label(RichText::new(format!("{} · plan {}", item.id, plan.id)).size(look.text_small).color(colors().muted));
                if let Some(parent) = item.parent.as_ref().and_then(|p| plan.item(p)) {
                    ui.label(RichText::new(format!("Under {} {}", parent.id, parent.title)).color(colors().muted));
                }
                ui.add_space(look.space_s);
                if item.text.is_empty() {
                    ui.label(RichText::new("No text.").color(colors().muted));
                } else {
                    ui.label(&item.text);
                }
                let under: Vec<&arch_graph::Item> = plan.items.iter().filter(|i| i.parent.as_ref() == Some(&item.id)).collect();
                if !under.is_empty() {
                    ui.add_space(look.space_s);
                    ui.label(RichText::new("Under it").strong());
                    for i in under {
                        ui.label(format!("{} {} ({})", i.id, i.title, i.state.name()));
                    }
                }
                for d in on {
                    ui.add_space(look.space_s);
                    egui::Frame::NONE.fill(colors().surface).stroke(egui::Stroke::new(look.line_thin, colors().border)).corner_radius(look.radius_m).inner_margin(crate::theme::margin(look.space_sm)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        crate::conversation::heading(ui, d);
                        for said in &d.said {
                            crate::conversation::message(ui, markdown, &crate::conversation::said(d, said));
                        }
                    });
                }
            })
            .response;
        self.spots = vec![Spot { key, rect: response.rect }];
    }

    /// The relations shown, in order: by kind, outgoing then incoming.
    fn relations(&self) -> Vec<(RelationKind, ComponentId, bool)> {
        let Some(detail) = &self.detail else { return Vec::new() };
        let mut out = Vec::new();
        for kind in RelationKind::ALL {
            for (list, outgoing) in [(&detail.outgoing, true), (&detail.incoming, false)] {
                for r in list.iter().filter(|r| r.kind == kind) {
                    out.push((kind, if outgoing { r.target.clone() } else { r.source.clone() }, outgoing));
                }
            }
        }
        out
    }

    /// Every item in a section, in the order it's drawn (title first).
    fn section_items(&self, section: Section) -> Vec<SpecItem> {
        let mut out = vec![SpecItem::Header];
        let spec = self.detail.as_ref().and_then(|d| d.component.spec.as_ref());
        match section {
            Section::Logic => out.extend((0..spec.map_or(0, |s| s.logic_steps.len())).map(SpecItem::Step)),
            Section::Invariants => out.extend((0..spec.map_or(0, |s| s.invariants.len())).map(SpecItem::Invariant)),
            Section::Relations => out.extend(self.relations().into_iter().map(|(kind, other, outgoing)| SpecItem::Relation { kind, other, outgoing })),
            Section::Code if self.detail.as_ref().is_some_and(|d| d.code.is_some()) => out.push(SpecItem::Source),
            Section::Changes => out.extend((0..self.changes.len()).map(SpecItem::Hunk)),
            Section::Placeholders => out.extend((0..self.detail.as_ref().map_or(0, |d| d.component.placeholders.len())).map(SpecItem::Placeholder)),
            Section::Findings => out.extend((0..self.findings.len()).map(SpecItem::Finding)),
            _ => {}
        }
        for c in self.comments_in(Some(section)) {
            out.push(SpecItem::Comment(c.id.clone()));
            out.extend((0..c.said.len().saturating_sub(1)).map(|i| SpecItem::Reply(c.id.clone(), i)));
        }
        out
    }

    /// Every item (folded sections included).
    fn all_entries(&self) -> Vec<Entry> {
        Section::ALL.into_iter().flat_map(|section| self.section_items(section).into_iter().map(move |item| Entry { section, item })).collect()
    }

    /// The cursor's stops: titles, and the items of open sections.
    pub fn entries(&self) -> Vec<Entry> {
        self.all_entries().into_iter().filter(|e| e.item == SpecItem::Header || !self.folded.contains(&e.section)).collect()
    }

    pub fn entry(&self) -> Option<Entry> {
        self.entries().get(self.cursor).cloned()
    }

    fn put_cursor(&mut self, index: usize) {
        let entries = self.entries();
        if entries.is_empty() {
            return;
        }
        self.cursor = index.min(entries.len() - 1);
        self.current = entries[self.cursor].section;
        self.scrolling.show(self.cursor, Show::InView);
    }

    /// Moves by the shared rules.
    fn go(&mut self, go: Go) {
        let entries = self.entries();
        if let Some(at) = self.scrolling.go(entries.len(), Some(self.cursor), go) {
            self.cursor = at;
            self.current = entries[at].section;
        }
    }

    /// Puts the cursor on an entry (opening its section).
    fn put_on(&mut self, entry: &Entry) {
        self.folded.remove(&entry.section);
        if let Some(i) = self.entries().iter().position(|e| e == entry) {
            self.put_cursor(i);
        }
    }

    pub fn step_section(&mut self, delta: i32) {
        let at = Section::ALL.iter().position(|s| *s == self.current).unwrap_or(0) as i32;
        let next = Section::ALL[(at + delta).clamp(0, Section::ALL.len() as i32 - 1) as usize];
        self.put_on(&Entry { section: next, item: SpecItem::Header });
    }

    /// The discussions on the target, in a section (or all of them), oldest first.
    pub fn comments_in(&self, section: Option<Section>) -> Vec<&Discussion> {
        let Some(target) = &self.target else { return Vec::new() };
        let mut out: Vec<&Discussion> = self.discussions.iter().filter(|c| section_of(&c.on.subject, target).is_some_and(|s| section.is_none_or(|want| want == s))).collect();
        out.sort_by_key(|c| c.said.first().map_or(0, |s| s.at_ms));
        out
    }

    /// Points on the target not settled.
    pub fn open_points(&self) -> Vec<&Discussion> {
        self.comments_in(None).into_iter().filter(|c| c.status != DiscussionStatus::Settled).collect()
    }

    /// Sections of the target that changed since the last review.
    pub fn changed_sections(&self) -> BTreeSet<Section> {
        match &self.target {
            Some(id) => self.diff.fields(id).iter().map(|f| Section::of_field(f)).collect(),
            None => BTreeSet::new(),
        }
    }

    /// The details the component's type requires that it doesn't have yet.
    pub fn missing_required(&self) -> Vec<arch_graph::RequiredField> {
        let (Some(detail), Some(types)) = (&self.detail, &self.types) else { return Vec::new() };
        types.resolve(&detail.component.type_name).map(|ty| ty.missing(&detail.component)).unwrap_or_default()
    }

    /// The items that changed since the last review (by entry key).
    pub fn changed_items(&self) -> BTreeSet<String> {
        let Some(target) = &self.target else { return BTreeSet::new() };
        let fields = self.diff.fields(target);
        let mut out: BTreeSet<String> = BTreeSet::new();
        for f in &fields {
            let item = |prefix: &str| f.strip_prefix(prefix).and_then(|i| i.parse::<usize>().ok());
            if let Some(i) = item("spec.logic_steps.") {
                out.insert(Entry { section: Section::Logic, item: SpecItem::Step(i) }.key());
            }
            if let Some(i) = item("spec.invariants.") {
                out.insert(Entry { section: Section::Invariants, item: SpecItem::Invariant(i) }.key());
            }
        }
        for change in &self.diff.changes {
            if let arch_graph::Change::RelationAdded { kind, source, target: to } | arch_graph::Change::RelationChanged { kind, source, target: to, .. } = change {
                let (other, outgoing) = if source == target { (to.clone(), true) } else if to == target { (source.clone(), false) } else { continue };
                out.insert(Entry { section: Section::Relations, item: SpecItem::Relation { kind: *kind, other, outgoing } }.key());
            }
        }
        out
    }

    /// The header card: what it is, where, how far along, and what needs looking at.
    fn render_header(&self, ui: &mut egui::Ui, detail: &ComponentDetail) {
        let c = &detail.component;
        let look = crate::theme::look();
        egui::Frame::NONE.fill(colors().surface).stroke(egui::Stroke::new(look.line_thin, colors().border_strong)).corner_radius(look.radius_xl).inner_margin(crate::theme::margin(look.space_l)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&c.name).size(look.text_display).strong().color(colors().text_strong));
                ui.label(RichText::new(&c.type_name).size(look.text_label).color(colors().muted));
            });
            ui.label(RichText::new(c.id.as_str()).monospace().size(look.text_small).color(colors().faint));
            let location = match (&c.anchor, &c.proposed) {
                (Some(a), _) => Some(crate::status::place(a)),
                (None, Some(p)) => Some(format!("{} (planned)", p.file)),
                (None, None) => None,
            };
            match location {
                Some(text) => {
                    ui.label(RichText::new(format!("{} {text}", egui_phosphor::regular::FILE_CODE)).monospace().size(look.text_small));
                }
                None => {
                    ui.label(crate::status::text("planned").size(look.text_small));
                }
            }
            ui.add_space(look.space_s);
            ui.horizontal_wrapped(|ui| {
                crate::status::chip(ui, crate::status::alignment(detail.alignment), None);
                let open = self.open_points().len();
                if open > 0 {
                    crate::status::chip(ui, "open_discussions", Some(&open.to_string()));
                }
                if !self.changed_sections().is_empty() {
                    crate::status::chip(ui, "changed", None);
                }
                if !c.placeholders.is_empty() {
                    crate::status::chip(ui, "placeholders", Some(&c.placeholders.len().to_string()));
                }
                let missing = self.missing_required();
                if !missing.is_empty() {
                    crate::status::chip(ui, "required_missing", Some(&missing.len().to_string())).on_hover_text(missing.iter().map(|f| f.name()).collect::<Vec<_>>().join(", "));
                }
                if self.out_of_date {
                    crate::status::chip(ui, "stale", None);
                }
            });
            ui.add_space(look.space_s);
            let stages = [
                arch_graph::Stage::Declared,
                arch_graph::Stage::ArchApproved,
                arch_graph::Stage::PlanApproved,
                arch_graph::Stage::Skeleton,
                arch_graph::Stage::Implemented,
            ];
            let words: Vec<(&str, String)> = stages.iter().map(|s| (crate::status::stage(*s), crate::status::look(crate::status::stage(*s)).1)).collect();
            let steps: Vec<(&str, &str)> = words.iter().map(|(n, w)| (*n, w.as_str())).collect();
            crate::status::track(ui, &steps, stages.iter().position(|s| *s == c.stage));
        });
        ui.add_space(look.space_m);
    }

    /// Moves to the next (or previous) unaddressed point, cursor and all.
    pub fn step_point(&mut self, forward: bool) {
        let points: Vec<(DiscussionId, Option<Section>)> = {
            let target = self.target.clone();
            self.open_points().into_iter().map(|c| (c.id.clone(), target.as_ref().and_then(|t| section_of(&c.on.subject, t)))).collect()
        };
        if points.is_empty() {
            self.point = None;
            return;
        }
        let n = points.len();
        let at = self.point.as_ref().and_then(|p| points.iter().position(|(id, _)| id == p)).map_or(if forward { 0 } else { n - 1 }, |i| if forward { (i + 1) % n } else { (i + n - 1) % n });
        let (id, section) = points[at].clone();
        self.point = Some(id.clone());
        if let Some(section) = section {
            self.put_on(&Entry { section, item: SpecItem::Comment(id) });
        }
    }

    /// The discussion the cursor is on (or on what's said in it).
    fn comment_here(&self) -> Option<&Discussion> {
        let id = match self.entry()?.item {
            SpecItem::Comment(id) | SpecItem::Reply(id, _) => id,
            _ => return None,
        };
        self.discussions.iter().find(|c| c.id == id)
    }

    fn posted<T: Send + 'static>(&self) -> impl FnOnce(NetResult<T>) + Send + 'static {
        let inbox = self.inbox.clone();
        move |r: NetResult<T>| push(&inbox, Reply::Posted(r.map(|_| ()).map_err(String::from)))
    }

    fn code_location(&self) -> Option<(String, u32, u32)> {
        crate::buffer::code_of(&self.detail.as_ref()?.component)
    }

    /// Draws it.
    pub(crate) fn draw(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        if self.item.is_some() {
            self.draw_item(ui);
            return;
        }
        let _ = cx;
        let Some(detail) = self.detail.clone() else {
            let msg = match (&self.target, &self.error) {
                (_, Some(e)) => format!("Couldn't load it: {e}"),
                (None, _) => "Select a component in the graph to see its spec.".to_owned(),
                (Some(_), None) => "Loading…".to_owned(),
            };
            ui.label(RichText::new(msg).color(colors().muted));
            self.spots.clear();
            return;
        };
        let entries = self.entries();
        if self.cursor >= entries.len() {
            self.cursor = entries.len().saturating_sub(1);
        }
        let cursor_key = entries.get(self.cursor).map(Entry::key);
        let c = &detail.component;
        let changed = self.changed_sections();
        let mut spots: Vec<Spot> = Vec::new();
        let relations = self.relations();
        let changed_items = self.changed_items();
        let missing = self.missing_required();
        let type_name = c.type_name.clone();
        let area = self.scrolling.area(egui::ScrollArea::vertical().id_salt("spec_items").auto_shrink([false, false]), ui.input(|i| i.time));
        let output = area.show(ui, |ui| {
            // One item: highlighted under the cursor, scrolled to when it moved there.
            let mut item = |ui: &mut egui::Ui, entry: Entry, add: &mut dyn FnMut(&mut egui::Ui)| -> egui::Response {
                let on = cursor_key.as_deref() == Some(entry.key().as_str());
                let fill = if on { colors().selection } else { egui::Color32::TRANSPARENT };
                let response = egui::Frame::NONE.fill(fill).corner_radius(crate::theme::look().radius_s).inner_margin(crate::theme::margin2(crate::theme::look().space_s, crate::theme::look().space_hair)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    add(ui);
                }).response;
                // Changed since the review: a bar at its left edge.
                if changed_items.contains(&entry.key()) {
                    let (_, word, color) = crate::status::look("changed");
                    let bar = egui::Rect::from_min_size(response.rect.left_top() - egui::vec2(crate::theme::look().space_s, 0.0), egui::vec2(crate::theme::look().line_bold, response.rect.height()));
                    ui.painter().rect_filled(bar, 0.0, color);
                    response.clone().on_hover_text(word);
                }
                spots.push(Spot { key: entry.key(), rect: response.rect });
                response
            };
            self.render_header(ui, &detail);
            for section in Section::ALL {
                let open = !self.folded.contains(&section);
                let mut title = section.title().to_owned();
                if changed.contains(&section) {
                    title.push_str(&format!("  {} changed", egui_phosphor::regular::DOT));
                }
                let open_here = self.comments_in(Some(section)).iter().filter(|c| c.status != DiscussionStatus::Settled).count();
                if open_here > 0 {
                    title.push_str(&format!("  · {open_here} open"));
                }
                let required = missing.iter().filter(|f| Section::of_required(**f) == section).count();
                if required > 0 {
                    let (glyph, word, _) = crate::status::look("required_missing");
                    title.push_str(&format!("  {glyph} {required} {word}"));
                }
                let color = if self.current == section { colors().focus } else if changed.contains(&section) { colors().info } else { colors().text_strong };
                let caret = if open { egui_phosphor::regular::CARET_DOWN } else { egui_phosphor::regular::CARET_RIGHT };
                item(ui, Entry { section, item: SpecItem::Header }, &mut |ui| {
                    ui.label(RichText::new(format!("{caret} {title}")).strong().size(crate::theme::look().text_title).color(color));
                });
                if !open {
                    continue;
                }
                let required_here: Vec<&arch_graph::RequiredField> = missing.iter().filter(|f| Section::of_required(**f) == section).collect();
                ui.indent(("spec", section as u8), |ui| {
                    // What this type needs here and doesn't have yet.
                    for field in &required_here {
                        ui.horizontal_wrapped(|ui| {
                            crate::status::chip(ui, "required_missing", None);
                            ui.label(RichText::new(format!("{}: none yet, required for a {type_name}.", field.name().replace('_', " "))).size(crate::theme::look().text_body).color(colors().text));
                        });
                    }
                    match section {
                        Section::Overview => {
                            ui.label(RichText::new(&c.name).size(crate::theme::look().text_display).strong());
                            ui.label(RichText::new(c.id.as_str()).monospace().size(crate::theme::look().text_body).color(colors().muted));
                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!("{} · {:?}", c.type_name, c.status));
                                crate::status::chip(ui, crate::status::stage(c.stage), None);
                                crate::status::chip(ui, crate::status::alignment(detail.alignment), None);
                            });
                            match (&c.anchor, &c.proposed) {
                                (Some(a), _) => ui.label(RichText::new(format!("{} ({})", a.symbol, crate::status::place(a))).monospace()),
                                (None, Some(p)) => ui.label(RichText::new(format!("planned as {} in {}", p.symbol, p.file)).monospace().color(colors().warn)),
                                (None, None) => ui.label(RichText::new("not placed yet").color(colors().warn)),
                            };
                            if c.description.is_empty() {
                                ui.label(RichText::new("No description yet.").italics().color(colors().muted));
                            } else {
                                ui.label(&c.description);
                            }
                            if let Some(pattern) = c.spec.as_ref().and_then(|s| s.pattern.as_ref()) {
                                ui.label(format!("Pattern: {pattern}"));
                            }
                        }
                        Section::Signature => match &c.signature {
                            Some(sig) => {
                                for p in &sig.params {
                                    ui.label(RichText::new(format!("{}: {}", p.name, p.ty.as_deref().unwrap_or("?"))).monospace());
                                }
                                ui.label(RichText::new(format!("{} {}", egui_phosphor::regular::ARROW_RIGHT, sig.returns.as_deref().unwrap_or("()"))).monospace());
                            }
                            None => {
                                ui.label(RichText::new("No signature.").color(colors().muted));
                            }
                        },
                        Section::Logic => {
                            let steps = c.spec.as_ref().map(|s| s.logic_steps.clone()).unwrap_or_default();
                            if steps.is_empty() {
                                ui.label(RichText::new("No logic steps written yet.").color(colors().muted));
                            }
                            for (i, step) in steps.iter().enumerate() {
                                item(ui, Entry { section, item: SpecItem::Step(i) }, &mut |ui| {
                                    ui.label(RichText::new(format!("{}. {}", step.sequence, step.name)).strong());
                                    if !step.description.is_empty() {
                                        ui.label(&step.description);
                                    }
                                });
                            }
                        }
                        Section::Invariants => {
                            let invariants = c.spec.as_ref().map(|s| s.invariants.clone()).unwrap_or_default();
                            if invariants.is_empty() {
                                ui.label(RichText::new("No invariants declared.").color(colors().muted));
                            }
                            for (i, inv) in invariants.iter().enumerate() {
                                item(ui, Entry { section, item: SpecItem::Invariant(i) }, &mut |ui| {
                                    ui.label(RichText::new(format!("{} ({})", inv.name, inv.kind)).strong());
                                    if !inv.description.is_empty() {
                                        ui.label(&inv.description);
                                    }
                                });
                            }
                        }
                        Section::Relations => {
                            let mut heading: Option<(RelationKind, bool)> = None;
                            for (kind, other, outgoing) in &relations {
                                if heading != Some((*kind, *outgoing)) {
                                    heading = Some((*kind, *outgoing));
                                    let label = if *outgoing { format!("{} {}", kind.name(), egui_phosphor::regular::ARROW_RIGHT) } else { format!("{} {}", egui_phosphor::regular::ARROW_LEFT, kind.name()) };
                                    ui.label(RichText::new(label).size(crate::theme::look().text_body).color(colors().muted));
                                }
                                // The component's own name; its full id on hover.
                                let symbol = other.as_str().split_once(':').map_or(other.as_str(), |(_, s)| s);
                                let name = symbol.rsplit([':', '.']).next().unwrap_or(symbol).to_owned();
                                let entry = Entry { section, item: SpecItem::Relation { kind: *kind, other: other.clone(), outgoing: *outgoing } };
                                let list = if *outgoing { &detail.outgoing } else { &detail.incoming };
                                let found = list.iter().find(|r| r.kind == *kind && (if *outgoing { &r.target } else { &r.source }) == other);
                                let (text, how) = found.map_or_else(|| (name.clone(), String::new()), |r| how_known(r, &name));
                                item(ui, entry, &mut |ui| {
                                    ui.label(RichText::new(&text).monospace().size(crate::theme::look().text_body).color(colors().info)).on_hover_text(format!("{}{how}", other.as_str()));
                                });
                            }
                        }
                        Section::Code => match &detail.code {
                            Some(code) => {
                                if self.out_of_date {
                                    ui.horizontal_wrapped(|ui| {
                                        crate::status::chip(ui, "stale", None);
                                        ui.label(RichText::new("This code changed since it was indexed; the excerpt may be old.").size(crate::theme::look().text_body).color(colors().muted));
                                    });
                                }
                                item(ui, Entry { section, item: SpecItem::Source }, &mut |ui| {
                                    egui::Frame::NONE.fill(colors().bg).inner_margin(crate::theme::margin(crate::theme::look().space_sm)).corner_radius(crate::theme::look().radius_m).show(ui, |ui| {
                                        ui.label(RichText::new(code).monospace().size(crate::theme::look().text_body));
                                    });
                                });
                            }
                            None => {
                                ui.label(RichText::new("Not in the code yet.").color(colors().muted));
                            }
                        },
                        Section::Changes => {
                            if self.changes.is_empty() {
                                ui.label(RichText::new("None in the changes compared.").color(colors().muted));
                            }
                            for (i, hunk) in self.changes.iter().enumerate() {
                                item(ui, Entry { section, item: SpecItem::Hunk(i) }, &mut |ui| {
                                    crate::diff::draw_hunk(ui, hunk, false);
                                });
                            }
                        }
                        Section::Placeholders => {
                            if c.placeholders.is_empty() {
                                ui.label(RichText::new("None.").color(colors().muted));
                            }
                            for (i, span) in c.placeholders.iter().enumerate() {
                                item(ui, Entry { section, item: SpecItem::Placeholder(i) }, &mut |ui| {
                                    ui.label(RichText::new(format!("placeholder body at line {}", span.start_line)).color(colors().warn));
                                });
                            }
                        }
                        Section::Findings => {
                            if self.source.plan().is_none() {
                                ui.label(RichText::new("Pick a plan in the graph to see its checks.").color(colors().muted));
                            } else if self.findings.is_empty() {
                                ui.label(RichText::new("No findings.").color(colors().ok));
                            }
                            // Worst first.
                            let mut order: Vec<usize> = (0..self.findings.len()).collect();
                            order.sort_by_key(|i| match self.findings[*i].severity {
                                Severity::Error => 0,
                                Severity::Warning => 1,
                                Severity::Info => 2,
                            });
                            for i in order {
                                let f = &self.findings[i];
                                let (_, _, color) = crate::status::look(crate::status::severity(f.severity));
                                item(ui, Entry { section, item: SpecItem::Finding(i) }, &mut |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        crate::status::chip(ui, crate::status::severity(f.severity), None);
                                        ui.label(RichText::new(&f.message).color(color));
                                    });
                                    if let Some(s) = &f.suggestion {
                                        ui.label(RichText::new(s).size(crate::theme::look().text_body).color(colors().muted));
                                    }
                                });
                            }
                        }
                    }
                    let here: Vec<Discussion> = self.comments_in(Some(section)).into_iter().cloned().collect();
                    let point = self.point.clone();
                    for discussion in &here {
                        render_thread(ui, discussion, point.as_ref(), section, &mut item, &mut self.markdown);
                    }
                });
            }
        });
        // Each item's stop, in the cursor's order.
        for entry in &entries {
            if let Some(s) = spots.iter().find(|s| s.key == entry.key()) {
                self.scrolling.item(s.rect);
            }
        }
        self.scrolling.drawn(&output);
        if self.scrolling.busy() {
            ui.ctx().request_repaint();
        }
        self.spots = spots.into_iter().filter(|s| s.rect.intersects(output.inner_rect)).collect();
    }

}

/// Draws one cursor stop (`Entry`), its contents drawn by the closure given.
type ItemDrawer<'a> = dyn FnMut(&mut egui::Ui, Entry, &mut dyn FnMut(&mut egui::Ui)) -> egui::Response + 'a;

/// One discussion's thread, as every conversation draws one: what it asks
/// and what's said after are cursor stops.
fn render_thread(ui: &mut egui::Ui, c: &Discussion, point: Option<&DiscussionId>, section: Section, item: &mut ItemDrawer, markdown: &mut egui_commonmark::CommonMarkCache) {
    let stroke = if point == Some(&c.id) { colors().focus } else { colors().border };
    egui::Frame::NONE.fill(colors().surface).stroke(egui::Stroke::new(crate::theme::look().line_thin, stroke)).corner_radius(crate::theme::look().radius_m).inner_margin(crate::theme::margin(crate::theme::look().space_sm)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        for (i, said) in c.said.iter().enumerate() {
            let stop = if i == 0 { SpecItem::Comment(c.id.clone()) } else { SpecItem::Reply(c.id.clone(), i - 1) };
            item(ui, Entry { section, item: stop }, &mut |ui| {
                if i == 0 {
                    crate::conversation::heading(ui, c);
                }
                crate::conversation::message(ui, markdown, &crate::conversation::said(c, said));
            });
        }
    });
}

/// A relation's name as listed, and how it's known (for its hover): a
/// planned one the code doesn't have says so; one found in the code says
/// what found it, and one only matched by name that nothing confirmed it.
fn how_known(r: &arch_graph::Relation, name: &str) -> (String, String) {
    use arch_graph::tree::Evidence;
    if r.provenance.planned && !r.provenance.observed {
        return (format!("{name} (planned)"), String::new());
    }
    match Evidence::of(r) {
        Some(e @ Evidence::Named) => (format!("{name} (matched by name)"), format!("\n{}: not confirmed by the language's own analyzer.", capitalized(&e.describe()))),
        Some(e) => (name.to_owned(), format!("\n{}.", capitalized(&e.describe()))),
        None => (name.to_owned(), String::new()),
    }
}

/// `text` with its first letter a capital.
pub(crate) fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

fn push(inbox: &Arc<Mutex<Vec<Reply>>>, reply: Reply) {
    if let Ok(mut i) = inbox.lock() {
        i.push(reply);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SpecCommand {
    /// Read the component's code again (it changed since it was read).
    Reread,
    NextSection,
    PrevSection,
    /// Go to the section (its index in `Section::ALL`).
    Section,
    Pin,
    MarkReviewed,
    Point { forward: bool },
}

impl ActionProvider for SpecState {
    fn actions(&self) -> Vec<Action> {
        use SpecCommand as S;
        let f = |id: &str, label: &str, detail: &str, c: SpecCommand| Action::new(id, label, detail, c).focused();
        // An item's card: one stop, nothing of a component's card (its
        // state is the shell's, on the item).
        if self.item.is_some() {
            let mut out = crate::nav::list_moves();
            if !self.floating {
                out.push(
                    f("spec.pin", if self.pinned { "Unpin (follow what you're on)" } else { "Pin this card" }, "Pinned, it stays on this card while you move elsewhere", S::Pin)
                        .header(Style::Button, if self.pinned { "Pinned" } else { "Pin" }, Some(egui_phosphor::regular::PUSH_PIN))
                        .group(Group::View)
                        .importance(MINOR),
                );
            }
            return out;
        }
        let mut out = self.moves();
        out.extend([
            f("spec.next_section", "Next section", "The card's next part: what it is, what it holds, its relations, its code", S::NextSection).group(Group::Navigate),
            f("spec.prev_section", "Previous section", "The card's part before", S::PrevSection).group(Group::Navigate).importance(MINOR),
        ]);
        if !self.floating {
            out.push(
                f("spec.pin", if self.pinned { "Unpin (follow what you're on)" } else { "Pin this component" }, "Pinned, it stays on this card while you move elsewhere", S::Pin)
                    .header(Style::Button, if self.pinned { "Pinned" } else { "Pin" }, Some(egui_phosphor::regular::PUSH_PIN))
                    .group(Group::View)
                    .importance(MINOR),
            );
        }
        // On a discussion, opening it leads (Enter); starting one is the shell's (`C`).
        let on_comment = self.comment_here().is_some();
        if self.source.plan().is_some() {
            out.push(
                f("spec.mark_reviewed", "Mark the plan reviewed (later changes are highlighted)", "From now on, what changes in the plan is marked as new", S::MarkReviewed)
                    .header(Style::Button, "Reviewed", Some(egui_phosphor::regular::CHECK_SQUARE))
                    .importance(MINOR),
            );
            // Always there: with no open points it says so.
            let points = if self.open_points().is_empty() { When::Unavailable } else { When::Always };
            out.push(f("spec.next_point", "Next discussion not settled", "The next question or change asked for on this card that isn't settled", S::Point { forward: true }).group(Group::Navigate).importance(if on_comment { SECONDARY } else { PRIMARY }).when(points).jump());
            out.push(f("spec.prev_point", "Previous discussion not settled", "The one before that isn't settled", S::Point { forward: false }).group(Group::Navigate).importance(MINOR).when(points).jump());
        }
        out.push(
            f("spec.section", "Go to a section", "Pick one of the card's parts", S::Section)
                .param(Param::Choice(Section::ALL.iter().enumerate().map(|(i, s)| Choice::new(i.to_string(), s.title()).current(*s == self.current)).collect()))
                .header(Style::Setting, "Section", None)
                .group(Group::Navigate)
                .importance(MINOR),
        );
        if self.out_of_date && self.detail.as_ref().is_some_and(|d| d.code.is_some()) {
            out.push(f("spec.reread", "Read this code again (it changed since)", "The index reads its file again, so the card is up to date", S::Reread).header(Style::Button, "Re-read", None).importance(MINOR));
        }
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        match action.command::<SpecCommand>() {
            Some(SpecCommand::MarkReviewed) => {
                if let Some(plan) = self.source.plan().cloned() {
                    cx.net.arch_mark_reviewed(&plan, &self.author, cx.ctx.clone(), self.posted());
                }
            }
            Some(SpecCommand::Point { .. }) if self.open_points().is_empty() => *cx.note = Some("No discussions not settled on this component.".into()),
            Some(SpecCommand::Point { forward }) => self.step_point(*forward),
            Some(SpecCommand::Reread) => self.asks.extend(self.code_location().map(|(file, _, _)| Ask::Reread(file))),
            Some(SpecCommand::NextSection) => self.step_section(1),
            Some(SpecCommand::PrevSection) => self.step_section(-1),
            Some(SpecCommand::Section) => {
                if let Some(section) = value.and_then(|v| v.parse::<usize>().ok()).and_then(|i| Section::ALL.get(i)) {
                    self.put_on(&Entry { section: *section, item: SpecItem::Header });
                }
            }
            Some(SpecCommand::Pin) => {
                self.pinned = !self.pinned;
                // Unpinned: on what the user is on now.
                if let Some(id) = self.followed.clone().filter(|_| !self.pinned) {
                    self.show(id);
                }
            }
            None => {}
        }
    }
}

impl SpecState {
    /// The shared moves where the cursor is, in the card's words: through
    /// its items and sections, into an item (a fold, a link, a reply), up to
    /// its section's title, folding sections.
    fn moves(&self) -> Vec<Action> {
        let mut out = crate::nav::list_moves();
        out.push(Move::Motion(Motion::NextSibling).offer("Next section"));
        out.push(Move::Motion(Motion::PrevSibling).offer("Previous section"));
        let entry = self.entry();
        let enter = entry.as_ref().and_then(|e| match &e.item {
            SpecItem::Header if self.folded.contains(&self.current) => Some("Unfold this section".to_owned()),
            SpecItem::Header => Some("Fold this section".to_owned()),
            SpecItem::Relation { other, .. } => Some(format!("Follow to {}", crate::nav::short_name(other))),
            SpecItem::Source | SpecItem::Placeholder(_) | SpecItem::Hunk(_) => Some("Open in the code".to_owned()),
            SpecItem::Comment(_) | SpecItem::Reply(..) => Some("Reply".to_owned()),
            SpecItem::Step(_) | SpecItem::Invariant(_) | SpecItem::Finding(_) => None,
        });
        out.push(Move::Enter.offer_if(enter.is_some(), enter.unwrap_or_else(|| "Open: fold a section, follow a relation, reply".into())));
        out.push(Move::Leave.offer_if(entry.as_ref().is_some_and(|e| e.item != SpecItem::Header), "Up to the section title"));
        out.extend(crate::nav::fold_here(Some((&format!("the {} section", self.current.title()), self.folded.contains(&self.current)))));
        out.extend(crate::nav::fold_all((!self.folded.is_empty(), "Unfold every section"), (self.folded.len() < Section::ALL.len(), "Fold every section")));
        out
    }
}

impl crate::nav::DrawsThings for SpecState {
    fn drawn_at(&self, subject: &harness_contracts::Subject) -> Option<egui::Rect> {
        self.drawn_rect(subject)
    }
}

impl Navigable for SpecState {
    /// Any component, but the one whose card it shows (a relation's other
    /// end is another); a part of a plan's card.
    fn can_show(&self, target: &Target) -> bool {
        use crate::nav::PlanPart;
        match target {
            Target::Plan { part, .. } => matches!(part, PlanPart::Component(_) | PlanPart::Section(..) | PlanPart::Relation { .. } | PlanPart::Item(_)),
            _ => target.in_component() && target.component() != self.target.as_ref(),
        }
    }

    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        if let Target::Plan { plan, part } = target {
            return self.show_plan_part(plan, part);
        }
        let id = target.component().ok_or("Not a component.")?;
        self.land_on(id.clone());
        Ok(())
    }

    fn things(&self, key: &str) -> crate::nav::Things {
        self.things_at(key)
    }

    fn draws_things(&self) -> Option<&dyn crate::nav::DrawsThings> {
        Some(self)
    }

    fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    /// The types, the user, the review, the source (unless pinned), and
    /// whether its code changed since it was read.
    fn share(&mut self, shared: &crate::shared::Shared) {
        if let Some(types) = shared.types.since(&mut self.seen.types) {
            self.types.clone_from(types);
        }
        if let Some(user) = shared.user.since(&mut self.seen.user) {
            self.author.clone_from(user);
        }
        if let Some(review) = shared.review_since(&mut self.seen.review) {
            self.discussions.clone_from(&review.discussions);
            self.diff.clone_from(&review.diff);
            if let Some(card) = self.item.as_mut() {
                card.read = match &review.plan {
                    Some(plan) if *plan == card.plan => review.read.clone(),
                    _ => Some(Err("it isn't the plan shown now: show it in the graph (its source) to read its items".to_owned())),
                };
            }
        }
        self.follow_source(shared.source.get());
        let desk = (shared.changes.comparison(), shared.changes.read(), shared.settings.get().changes.context);
        if desk != (&self.comparing.0, self.comparing.1, self.comparing.2) {
            self.comparing = (desk.0.clone(), desk.1, desk.2);
        }
        let file = self.detail.as_ref().and_then(|d| d.component.anchor.as_ref()).map(|a| a.file.to_string());
        self.out_of_date = self.detail.as_ref().is_some_and(|d| d.stale) || file.is_some_and(|f| shared.index.get().is_stale(&f));
    }

    /// Shows the component the user is on; pinned, remembers it (shown
    /// when it's unpinned).
    fn follow(&mut self, id: &ComponentId) {
        self.followed = Some(id.clone());
        if !self.pinned {
            self.show(id.clone());
        }
    }

    fn place(&self) -> Place {
        let plan_item = self.item.as_ref().map(|c| (c.plan.clone(), c.id.clone()));
        Place::of(&SpecPlace { component: self.target.clone(), item: self.entry().map(|e| e.key()), plan_item })
    }

    fn place_in_words(&self, place: &Place) -> String {
        place.read::<SpecPlace>().and_then(|p| p.component).map(|id| crate::nav::short_name(&id)).unwrap_or_default()
    }

    /// Back to the component and the item.
    fn restore(&mut self, place: &Place) {
        if let Some(SpecPlace { plan_item: Some((plan, id)), .. }) = place.read() {
            self.show_item(&plan, &id);
            return;
        }
        if let Some(SpecPlace { component, item, .. }) = place.read() {
            if let Some(id) = component {
                self.land_on(id);
            }
            let loaded = self.detail.as_ref().is_some_and(|d| Some(&d.component.id) == self.target.as_ref());
            match item.as_deref().and_then(|key| self.all_entries().into_iter().find(|e| e.key() == key)) {
                Some(entry) if loaded => self.put_on(&entry),
                // Not in yet: land there when it is.
                _ if !loaded => {
                    self.restore_item = item;
                    self.put_cursor(0);
                }
                _ => self.put_cursor(0),
            }
        }
    }

    fn target(&self) -> Option<Target> {
        if let Some(card) = &self.item {
            return Some(Target::Plan { plan: card.plan.clone(), part: crate::nav::PlanPart::Item(card.id.0.clone()) });
        }
        let component = self.target.clone()?;
        let entry = self.entry().unwrap_or(Entry { section: self.current, item: SpecItem::Header });
        // A change: its place in the file.
        if let (SpecItem::Hunk(i), Some((file, ..))) = (&entry.item, self.code_lines())
            && let Some(hunk) = self.changes.get(*i)
        {
            return Some(Target::Change { file, line: hunk.new_start.max(1) as usize });
        }
        // A relation: its other end; anything else: the card's component.
        Some(Target::Component(match entry.item {
            SpecItem::Relation { other, .. } => other,
            _ => component,
        }))
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let n = self.entries().len();
        if n == 0 {
            return;
        }
        match motion {
            Motion::Down => self.go(Go::Down(count)),
            Motion::Up => self.go(Go::Up(count)),
            Motion::HalfDown => self.go(Go::HalfDown(count)),
            Motion::HalfUp => self.go(Go::HalfUp(count)),
            Motion::First => self.go(Go::First),
            Motion::Last => self.go(Go::Last),
            Motion::Line(i) => self.put_cursor(i.saturating_sub(1)),
            Motion::NextSibling => self.step_section(count as i32),
            Motion::PrevSibling => self.step_section(-(count as i32)),
            Motion::Left | Motion::Right | Motion::HalfLeft | Motion::HalfRight | Motion::FewLeft | Motion::FewRight => {}
        }
    }

    /// Titles fold and unfold; a relation goes to the other component; the
    /// source opens in the code view; a comment takes a reply.
    fn enter(&mut self) -> Step {
        let Some(entry) = self.entry() else { return Step::Nothing("Nothing to show yet.".into()) };
        match entry.item {
            SpecItem::Header => {
                self.fold(Fold::Toggle);
                Step::Done
            }
            SpecItem::Relation { .. } => Step::GoTo(GoTo::Spec),
            SpecItem::Source | SpecItem::Hunk(_) => Step::GoTo(GoTo::Code),
            // A discussion: it opens (its own buffer, over the card).
            SpecItem::Comment(id) | SpecItem::Reply(id, _) => {
                self.point = Some(id.clone());
                self.asks.push(Ask::Show { target: Target::Discussion(id), in_: None });
                Step::Done
            }
            SpecItem::Placeholder(_) => Step::GoTo(GoTo::Code),
            SpecItem::Step(_) | SpecItem::Invariant(_) | SpecItem::Finding(_) => Step::Nothing("Start a discussion on it to ask about it, or for a change to it.".into()),
        }
    }

    /// Up to the section's title.
    fn leave(&mut self) -> Step {
        match self.entry() {
            Some(entry) if entry.item != SpecItem::Header => {
                self.put_on(&Entry { section: entry.section, item: SpecItem::Header });
                Step::Done
            }
            _ => Step::Nothing("Already on a section title.".into()),
        }
    }

    fn fold(&mut self, fold: Fold) {
        match fold {
            Fold::OpenAll => self.folded.clear(),
            Fold::CloseAll => self.folded = Section::ALL.into_iter().collect(),
            Fold::Open | Fold::Close | Fold::Toggle => {
                let section = self.current;
                let open = match fold {
                    Fold::Open => true,
                    Fold::Close => false,
                    _ => self.folded.contains(&section),
                };
                if open {
                    self.folded.remove(&section);
                } else {
                    self.folded.insert(section);
                }
            }
        }
        // The cursor stays in its section (on its title once folded).
        let section = self.current;
        if self.folded.contains(&section) {
            self.put_on_header_keeping_fold(section);
        } else if let Some(i) = self.entries().iter().position(|e| e.section == section && e.item == SpecItem::Header)
            && self.entry().is_none_or(|e| e.section != section) {
                self.put_cursor(i);
            }
    }

    fn items(&self) -> Vec<Item> {
        let detail = self.detail.as_ref();
        let spec = detail.and_then(|d| d.component.spec.as_ref());
        self.all_entries()
            .into_iter()
            .map(|e| {
                let (name, text) = match &e.item {
                    SpecItem::Header => (e.section.title().to_owned(), String::new()),
                    SpecItem::Step(i) => spec.and_then(|s| s.logic_steps.get(*i)).map_or_else(Default::default, |s| (s.name.clone(), s.description.clone())),
                    SpecItem::Invariant(i) => spec.and_then(|s| s.invariants.get(*i)).map_or_else(Default::default, |s| (s.name.clone(), s.description.clone())),
                    SpecItem::Relation { other, .. } => (other.as_str().rsplit([':', '.']).next().unwrap_or_default().to_owned(), other.to_string()),
                    SpecItem::Source => (String::new(), detail.and_then(|d| d.code.clone()).unwrap_or_default()),
                    SpecItem::Placeholder(_) => (String::new(), "placeholder".to_owned()),
                    // Deleted lines too: what was there.
                    SpecItem::Hunk(i) => (String::new(), self.changes.get(*i).map(|h| h.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()),
                    SpecItem::Finding(i) => (String::new(), self.findings.get(*i).map(|f| f.message.clone()).unwrap_or_default()),
                    SpecItem::Comment(id) => (String::new(), self.discussions.iter().find(|c| &c.id == id).map(|c| c.asking().to_owned()).unwrap_or_default()),
                    SpecItem::Reply(id, i) => (String::new(), self.discussions.iter().find(|c| &c.id == id).and_then(|c| c.said.get(*i + 1)).map(|r| r.text.clone()).unwrap_or_default()),
                };
                Item::new(e.key(), name, text)
            })
            .collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        if let Some(card) = &self.item {
            return Some(card.key());
        }
        self.entry().map(|e| e.key())
    }

    fn move_to(&mut self, key: &str) {
        if let Some(entry) = self.all_entries().into_iter().find(|e| e.key() == key) {
            self.put_on(&entry);
        }
    }
}

impl SpecState {
    /// The plan the card is read from, when its component is one of the
    /// plan's (anything but only observed in the code).
    fn plan_of_card(&self) -> Option<(&arch_graph::PlanId, &ComponentId)> {
        let detail = self.detail.as_ref()?;
        let id = self.target.as_ref().filter(|id| detail.component.id == **id)?;
        (detail.alignment != arch_graph::Alignment::Observed).then_some(())?;
        Some((self.source.plan()?, id))
    }

    /// The plan's relation an entry is (none: it isn't one, or the plan
    /// doesn't have it: the code does).
    fn plan_relation(&self, item: &SpecItem) -> Option<(RelationKind, ComponentId, ComponentId)> {
        let SpecItem::Relation { kind, other, outgoing } = item else { return None };
        let (id, detail) = (self.target.as_ref()?, self.detail.as_ref()?);
        let (source, target) = if *outgoing { (id.clone(), other.clone()) } else { (other.clone(), id.clone()) };
        let planned = detail.outgoing.iter().chain(&detail.incoming).any(|r| r.kind == *kind && r.source == source && r.target == target && r.provenance.planned);
        planned.then_some((*kind, source, target))
    }

    /// The things at entry `key`, narrowest first: from a plan, a relation
    /// (when the plan has it), the section it's in, the card, the plan; a
    /// component the plan doesn't change, the component.
    fn things_at(&self, key: &str) -> crate::nav::Things {
        use harness_contracts::Subject;
        let known = harness_contracts::Known::default();
        if let Some(card) = self.item.as_ref().filter(|c| c.key() == key) {
            let at = [Subject::PlanItem { plan: card.plan.clone(), item: card.id.0.clone() }, Subject::Plan { plan: card.plan.clone() }];
            return crate::nav::Things { at: at.into_iter().map(|s| crate::nav::Thing::named(s, &known)).collect(), unit: 0 };
        }
        let Some(entry) = self.all_entries().into_iter().find(|e| e.key() == key) else { return crate::nav::Things::default() };
        let Some((plan, id)) = self.plan_of_card() else {
            return match &self.target {
                Some(id) => crate::nav::Things::one(Subject::Component { id: id.clone(), was: None }, &known),
                None => crate::nav::Things::default(),
            };
        };
        let mut at = Vec::new();
        if let Some((relation, source, target)) = self.plan_relation(&entry.item) {
            at.push(Subject::PlanRelation { plan: plan.clone(), relation, source, target });
        }
        at.push(Subject::PlanSection { plan: plan.clone(), id: id.clone(), section: entry.section.key().to_owned() });
        at.push(Subject::PlanComponent { plan: plan.clone(), id: id.clone() });
        at.push(Subject::Plan { plan: plan.clone() });
        crate::nav::Things { at: at.into_iter().map(|s| crate::nav::Thing::named(s, &known)).collect(), unit: 0 }
    }

    /// Reads from `plan` (the desk too).
    fn read_plan(&mut self, plan: &arch_graph::PlanId) -> Result<(), String> {
        if self.pinned && self.source.plan() != Some(plan) {
            return Err("This card is pinned to another source: unpin it first.".into());
        }
        let source = Source::Plan(plan.clone());
        if self.source != source {
            self.asks.push(Ask::Source(source.clone()));
            self.follow_source(&source);
        }
        Ok(())
    }

    /// Goes to a plan's part: reads the card from that plan, lands on its
    /// component, the cursor on the section (or the relation) once it's in.
    fn show_plan_part(&mut self, plan: &arch_graph::PlanId, part: &crate::nav::PlanPart) -> Result<(), String> {
        use crate::nav::PlanPart;
        let (id, entry) = match part {
            PlanPart::Component(id) => (id.clone(), None),
            PlanPart::Section(id, section) => (id.clone(), Some(Entry { section: Section::from_key(section).ok_or_else(|| format!("A card has no {section} section."))?, item: SpecItem::Header })),
            PlanPart::Relation { kind, source, target } => (source.clone(), Some(Entry { section: Section::Relations, item: SpecItem::Relation { kind: *kind, other: target.clone(), outgoing: true } })),
            PlanPart::Item(item) => {
                self.read_plan(plan)?;
                self.show_item(plan, &arch_graph::ItemId(item.clone()));
                return Ok(());
            }
            PlanPart::Whole => return Err("Not a part of a card.".into()),
        };
        self.read_plan(plan)?;
        let loaded = self.detail.as_ref().is_some_and(|d| d.component.id == id) && self.target.as_ref() == Some(&id);
        self.land_on(id);
        match entry.map(|e| e.key()) {
            Some(key) if loaded => self.move_to(&key),
            key => self.restore_item = key,
        }
        Ok(())
    }

    /// Where a thing is drawn on the card: a section's rows, a relation's
    /// row, the whole card (its component, the plan's or the code's).
    fn drawn_rect(&self, subject: &harness_contracts::Subject) -> Option<egui::Rect> {
        use harness_contracts::Subject;
        if let (Some(card), Subject::PlanItem { plan, item }) = (&self.item, subject)
            && card.plan == *plan
            && card.id.0 == *item
        {
            return self.spots.iter().find(|s| s.key == card.key()).map(|s| s.rect);
        }
        let id = self.target.as_ref()?;
        let ours = |plan: &arch_graph::PlanId, of: &ComponentId| self.plan_of_card() == Some((plan, id)) && of == id;
        let keep: Box<dyn Fn(&Entry) -> bool> = match subject {
            Subject::PlanSection { plan, id: of, section } if ours(plan, of) => {
                let section = section.clone();
                Box::new(move |e: &Entry| e.section.key() == section)
            }
            Subject::PlanComponent { plan, id: of } if ours(plan, of) => Box::new(|_| true),
            Subject::Component { id: of, .. } if of == id && self.plan_of_card().is_none() => Box::new(|_| true),
            Subject::PlanRelation { plan, relation, source, target } if self.plan_of_card().is_some_and(|(p, _)| p == plan) => {
                let wanted = (*relation, source.clone(), target.clone());
                return self.all_entries().into_iter().filter(|e| self.plan_relation(&e.item).as_ref() == Some(&wanted)).filter_map(|e| self.spots.iter().find(|s| s.key == e.key()).map(|s| s.rect)).reduce(|a, b| a.union(b));
            }
            _ => return None,
        };
        let keys: Vec<String> = self.all_entries().into_iter().filter(|e| keep(e)).map(|e| e.key()).collect();
        self.spots.iter().filter(|s| keys.contains(&s.key)).map(|s| s.rect).reduce(|a, b| a.union(b))
    }

    /// The cursor onto a folded section's title (without unfolding it).
    fn put_on_header_keeping_fold(&mut self, section: Section) {
        if let Some(i) = self.entries().iter().position(|e| e.section == section && e.item == SpecItem::Header) {
            self.put_cursor(i);
        }
    }
}

/// Where a card is: its component, and the item (by its key: indexes
/// shift as the card loads).
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct SpecPlace {
    pub component: Option<ComponentId>,
    pub item: Option<String>,
    /// A plan's item's card, shown instead of a component's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_item: Option<(arch_graph::PlanId, arch_graph::ItemId)>,
}

impl crate::buffer::Buffer for SpecState {
    fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, _drawing: &mut crate::buffer::Drawing<'_>) {
        self.draw(ui, cx);
    }

    /// Fetches the target's detail (and its plan findings) when it changes.
    fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::Detail(id, result) if Some(&id) == self.target.as_ref() => match *result {
                    Ok(detail) => {
                        // Where its code is now: what shows it elsewhere may be out of date.
                        self.asks.push(Ask::Located { id: detail.component.id.clone(), anchor: detail.component.anchor.clone() });
                        self.detail = Some(detail);
                        self.error = None;
                        // A restored place whose item only exists once this is in.
                        if let Some(key) = self.restore_item.take() {
                            self.move_to(&key);
                        }
                    }
                    Err(e) => self.error = Some(e.into()),
                },
                Reply::Report(id, Ok(report)) if Some(&id) == self.target.as_ref() => {
                    self.findings = report.findings.into_iter().filter(|f| f.component() == Some(&id)).collect();
                }
                Reply::Posted(Ok(())) => {
                    self.note = None;
                    self.asks.push(Ask::Refresh(crate::nav::Refresh::Review));
                }
                Reply::Posted(Err(e)) => self.note = Some(e),
                _ => {}
            }
        }
        let Some(target) = self.target.clone() else {
            self.detail = None;
            return;
        };
        // Never another component's content under this one's name.
        if self.detail.as_ref().is_some_and(|d| d.component.id != target) {
            self.detail = None;
        }
        let key = (target.clone(), self.source.clone());
        if self.loaded.as_ref() == Some(&key) {
            return;
        }
        self.loaded = Some(key);
        self.findings.clear();
        let inbox = self.inbox.clone();
        let id = target.clone();
        net.arch_component(&target, &self.source, ctx.clone(), move |r| push(&inbox, Reply::Detail(id, Box::new(r))));
        if let Some(plan) = self.source.plan() {
            let inbox = self.inbox.clone();
            net.arch_validate(plan, ctx.clone(), move |r| push(&inbox, Reply::Report(target, r)));
        }
        self.follow_changes(net, ctx);
    }

    /// Changed code or plans: its component read again (an item's card
    /// has the desk's plan, read again).
    fn on_event(&mut self, event: &arch_graph::ArchEvent) {
        if matches!(event, arch_graph::ArchEvent::Changed { .. }) {
            crate::buffer::Buffer::invalidate(self);
        }
    }

    /// The architecture changed: load again.
    fn invalidate(&mut self) {
        self.loaded = None;
    }


    /// Whether it's pinned.
    fn keep(&self) -> Option<serde_json::Value> {
        self.pinned.then_some(serde_json::Value::Bool(true))
    }

    fn restore_kept(&mut self, kept: &serde_json::Value) {
        self.pinned = kept.as_bool().unwrap_or(false);
    }

    /// The component it shows: its name and where its code is.
    fn knows(&self, id: &ComponentId) -> Option<crate::buffer::Known> {
        Some(crate::buffer::Known::of(&self.detail.as_ref().filter(|d| d.component.id == *id)?.component))
    }

    /// A card over a panel (the quick look): held on its component; in a
    /// panel again, still held on it.
    fn as_card(&mut self, card: bool) {
        self.floating = card;
        self.pinned |= card;
    }

    fn describe(&self) -> String {
        if let Some(card) = &self.item {
            let title = card.read.as_ref().and_then(|r| r.as_ref().ok()).and_then(|p| p.item(&card.id)).map_or_else(|| card.id.to_string(), |i| format!("{} {}", card.id, i.title));
            return if self.pinned { format!("{title} (pinned)") } else { title };
        }
        let card = self.target.as_ref().map_or_else(|| "following what you're on".to_owned(), |id| match &self.detail {
            Some(d) if d.component.id == *id => d.component.name.clone(),
            _ => crate::nav::short_name(id),
        });
        if self.pinned { format!("{card} (pinned)") } else { card }
    }
}

impl crate::hosted::Hosted for SpecState {
    /// The component it shows (an item: its title).
    fn title(&self) -> Status {
        if let Some(card) = &self.item {
            let title = card.read.as_ref().and_then(|r| r.as_ref().ok()).and_then(|p| p.item(&card.id)).map_or_else(|| card.id.to_string(), |i| i.title.clone());
            return Status::new(title, Tone::Plain).detail(format!("{} of plan {}", card.id, card.plan));
        }
        match &self.detail {
            Some(d) => Status::new(d.component.name.clone(), Tone::Plain).detail(d.component.id.as_str()),
            None => Status::new("Spec", Tone::Muted),
        }
    }

    /// On its code: going to it in the code view.
    fn placed(&self) -> Vec<(String, crate::actions::Control)> {
        match self.entries().get(self.cursor).map(|e| &e.item) {
            Some(SpecItem::Source) => vec![("goto.code".to_owned(), crate::actions::Control::new(crate::actions::At::Item, Style::Button, "Open in the code", None))],
            _ => Vec::new(),
        }
    }

    /// What changed since the plan was reviewed (items that changed carry
    /// their own mark; this names the fields), and what went wrong.
    fn status(&self) -> Vec<Status> {
        let mut out = Vec::new();
        if let Some(id) = &self.target {
            let fields: Vec<String> = self.diff.fields(id).into_iter().filter(|f| f.rsplit('.').next().is_none_or(|last| last.parse::<usize>().is_err())).collect();
            if !fields.is_empty() {
                let since = self.diff.since.as_deref().map_or_else(|| "new in this plan (never reviewed)".to_owned(), |at| format!("changed since the review at {at}"));
                out.push(Status::new(format!("{} {since}: {}", egui_phosphor::regular::DOT, fields.join(", ")), Tone::Info));
            }
        }
        if let Some(note) = &self.note {
            out.push(Status::new(note.clone(), Tone::Error));
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use arch_graph::{Alignment, Component, ImplementationSpec, Invariant, LogicStep, PlanId, Relation, Stage, Status};

    pub(crate) fn detail(id: &ComponentId, other: &ComponentId) -> ComponentDetail {
        ComponentDetail {
            component: Component {
                id: id.clone(),
                name: "step".into(),
                type_name: "function".into(),
                description: "Does a step.".into(),
                status: Status::New,
                stage: Stage::PlanApproved,
                anchor: None,
                proposed: Some(arch_graph::ProposedLocation { language: "rust".into(), file: arch_graph::RepoPath::new("src/a.rs").unwrap(), parent: None, symbol: "a::step".into() }),
                signature: None,
                schemas: Default::default(),
                spec: Some(ImplementationSpec {
                    pattern: None,
                    logic_steps: vec![
                        LogicStep { sequence: 1, name: "read".into(), description: "Read the input.".into(), algorithm: None, complexity: None, check: None },
                        LogicStep { sequence: 2, name: "write".into(), description: "Write it out.".into(), algorithm: None, complexity: None, check: None },
                    ],
                    invariants: vec![Invariant { name: "pure".into(), kind: arch_graph::InvariantKind::PostCondition, description: "No I/O.".into() }],
                    ..Default::default()
                }),
                placeholders: vec![],
            },
            alignment: Alignment::Planned,
            outgoing: vec![Relation::new(RelationKind::Calls, id.clone(), other.clone(), arch_graph::Provenance::PLANNED)],
            incoming: vec![],
            code: None,
            stale: false,
            references: Vec::new(),
        }
    }

    fn frame(s: &mut SpecState, ctx: &egui::Context, mode: &mut crate::keymap::InputMode, slot: usize) {
        let net = NetClient::new("http://127.0.0.1:9");
        let mut note = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                s.draw(ui, &mut RunCx { net: &net, ctx, input_mode: mode, at: crate::keymap::Host::Panel(slot), note: &mut note, asked: &mut None });
            });
        });
    }

    /// A card's sections are named as the server names a plan's card's
    /// (one list): a note on a section names one the server knows.
    #[test]
    fn a_cards_sections_are_named_as_the_server_names_them() {
        assert_eq!(Section::ALL.map(Section::key), harness_contracts::CARD_SECTIONS);
    }

    #[test]
    fn spec_view_follows_unless_pinned_and_navigates_sections() {
        let mut s = SpecState::default();
        let a = ComponentId::new("rust:a::run");
        let b = ComponentId::new("rust:a::step");
        s.follow_source(&Source::Code);
        s.follow(&a);
        assert_eq!(s.target.as_ref(), Some(&a));
        s.pinned = true;
        s.follow_source(&Source::Code);
        s.follow(&b);
        assert_eq!(s.target.as_ref(), Some(&a), "pinned");
        s.pinned = false;
        s.follow_source(&Source::Plan(PlanId::new("p").unwrap()));
        s.follow(&b);
        assert_eq!((s.target.as_ref(), s.source.plan().is_some()), (Some(&b), true));

        s.step_section(1);
        s.step_section(1);
        assert_eq!(s.current, Section::Logic);
        s.fold(Fold::Toggle);
        assert!(s.folded.contains(&Section::Logic));
        s.fold(Fold::Toggle);
        assert!(!s.folded.contains(&Section::Logic));
        s.step_section(-10);
        assert_eq!(s.current, Section::Overview, "clamped");
        s.step_section(50);
        assert_eq!(s.current, Section::Findings);

        // Full content renders: every section, with steps, invariants and code.
        s.detail = Some(detail(&b, &a));
        let ctx = egui::Context::default();
        s.loaded = Some((b.clone(), s.source.clone()));
        let mut mode = crate::keymap::InputMode::Normal;
        for _ in 0..2 {
            frame(&mut s, &ctx, &mut mode, 0);
        }
        assert_eq!(s.code_location(), Some(("src/a.rs".into(), 1, 1)), "planned components open their proposed file");
        let actions = s.actions();
        for id in ["spec.next_section", "spec.prev_section", "spec.pin"] {
            assert!(actions.iter().any(|a| a.id == id), "{id}");
        }
    }

    #[test]
    fn the_cursor_walks_every_item_and_every_item_has_a_key() {
        use crate::review::tests::{discussion, on};
        let id = ComponentId::new("rust:a::step");
        let other = ComponentId::new("rust:a::save");
        let mut s = SpecState::default();
        s.follow_source(&Source::Plan(PlanId::new("p").unwrap()));
        s.follow(&id);
        s.detail = Some(detail(&id, &other));
        s.loaded = Some((id.clone(), s.source.clone()));
        let mut answered = discussion(1, on(&id, Some("logic")), DiscussionStatus::Answered);
        answered.said.push(harness_contracts::Said { by: "user".into(), at_ms: 2, kind: harness_contracts::SaidKind::Message, text: "and?".into(), changed: Vec::new() });
        s.discussions = vec![answered];
        let keys: Vec<String> = s.entries().iter().map(Entry::key).collect();
        let at = |k: &str| keys.iter().position(|x| x == k).unwrap_or_else(|| panic!("{k} in {keys:?}"));
        // j walks titles, steps, comments and replies in order.
        assert!(at("logic#Header") < at("logic#Step(0)") && at("logic#Step(1)") < at("logic#Comment(DiscussionId(\"d1\"))"));
        assert!(at("logic#Comment(DiscussionId(\"d1\"))") < at("logic#Reply(DiscussionId(\"d1\"), 0)"));
        s.motion(Motion::Line(at("logic#Step(1)") + 1), 1);
        assert_eq!(s.current, Section::Logic);
        s.motion(Motion::Down, 1);
        assert!(matches!(s.entry().unwrap().item, SpecItem::Comment(_)));
        // On a discussion: Enter opens it (its own buffer: it's replied to,
        // settled, reopened there).
        let ids: Vec<String> = s.actions().into_iter().map(|a| a.id).collect();
        for a in ["spec.next_point", "spec.mark_reviewed", "spec.pin"] {
            assert!(ids.contains(&a.to_owned()), "{a}: {ids:?}");
        }
        assert_eq!(s.enter(), Step::Done);
        assert_eq!(s.take_asks(), [Ask::Show { target: Target::Discussion(DiscussionId::new("d1")), in_: None }], "it opens");
        // Backspace goes up to the title; Enter on a title folds it.
        assert_eq!(s.leave(), Step::Done);
        assert_eq!(s.entry().unwrap().item, SpecItem::Header);
        assert_eq!(s.enter(), Step::Done);
        assert!(s.folded.contains(&Section::Logic));
        assert!(!s.entries().iter().any(|e| matches!(e.item, SpecItem::Step(_))), "folded items are skipped");
        s.fold(Fold::OpenAll);
        // Relations: Enter follows (to the other component's spec); the
        // relation itself is a thing (a discussion, a tag on it).
        let relation = Entry { section: Section::Relations, item: SpecItem::Relation { kind: RelationKind::Calls, other: other.clone(), outgoing: true } }.key();
        s.move_to(&relation);
        assert_eq!(s.target().unwrap().component(), Some(&other));
        assert_eq!(s.enter(), Step::GoTo(GoTo::Spec));
        assert_eq!(s.things(&relation).at.first().map(|t| t.subject.clone()), Some(Subject::PlanRelation { plan: PlanId::new("p").unwrap(), relation: RelationKind::Calls, source: id.clone(), target: other.clone() }));
        let ctx = egui::Context::default();
        // Search finds items; hints label what's drawn.
        let found = crate::nav::rank_matches(&s.items(), "write");
        assert_eq!(s.items()[found[0]].key, "logic#Step(1)");
        let mut mode = crate::keymap::InputMode::Normal;
        for _ in 0..2 {
            frame(&mut s, &ctx, &mut mode, 0);
        }
        let spots = s.spots();
        assert!(spots.iter().any(|sp| sp.key == "logic#Step(0)") && spots.iter().any(|sp| sp.key == "overview#Header"), "{spots:?}");
        // ] c lands on the open point; G and gg go to the ends.
        s.motion(Motion::First, 1);
        s.step_point(true);
        assert_eq!(s.entry().unwrap().item, SpecItem::Comment(DiscussionId::new("d1")));
        s.motion(Motion::Last, 1);
        assert_eq!(s.entry().unwrap().section, Section::Findings);
        s.motion(Motion::NextSibling, 1);
        assert_eq!(s.current, Section::Findings, "no section after the last");
        s.motion(Motion::PrevSibling, 2);
        assert_eq!(s.current, Section::Changes);
    }

    #[test]
    fn a_restored_item_is_found_once_the_component_loads() {
        let step = ComponentId::new("rust:a::step");
        let save = ComponentId::new("rust:a::save");
        let mut s = SpecState { pinned: true, ..Default::default() };
        s.show(save.clone());
        let relation = Entry { section: Section::Relations, item: SpecItem::Relation { kind: RelationKind::Calls, other: save.clone(), outgoing: true } }.key();
        s.restore(&Place::of(&SpecPlace { component: Some(step.clone()), item: Some(relation.clone()), plan_item: None }));
        assert_eq!(s.cursor, 0, "not loaded yet");
        s.loaded = Some((step.clone(), Source::Code));
        push(&s.inbox, Reply::Detail(step.clone(), Box::new(Ok(detail(&step, &save)))));
        let (ctx, net) = (egui::Context::default(), NetClient::new("http://127.0.0.1:9"));
        s.tick(&net, &ctx);
        assert_eq!(s.cursor_key(), Some(relation), "on the relation once it's in");
        assert_eq!(s.current, Section::Relations);
    }

    #[test]
    fn spec_view_threads_discussions_marks_changed_sections_and_composes_points() {
        use crate::review::tests::{discussion, on};
        use arch_graph::Change;
        let id = ComponentId::new("rust:a::step");
        let other = ComponentId::new("rust:a::run");
        let mut s = SpecState::default();
        s.follow_source(&Source::Plan(PlanId::new("p").unwrap()));
        s.follow(&id);
        s.author = "izo".into();
        // Diff fields land in the sections that show them.
        s.diff = PlanDiff {
            since: Some("2026-10-03T09:00:00Z".into()),
            changes: vec![
                Change::ComponentChanged { id: id.clone(), fields: vec!["description".into(), "spec.logic_steps".into(), "schemas.inputs".into()] },
                Change::RelationAdded { kind: RelationKind::Calls, source: other.clone(), target: id.clone() },
            ],
        };
        assert_eq!(s.changed_sections(), [Section::Overview, Section::Signature, Section::Logic, Section::Relations].into_iter().collect());
        // Threads land where their anchor points.
        let relation = Subject::PlanRelation { plan: PlanId::new("p").unwrap(), relation: RelationKind::Calls, source: other.clone(), target: id.clone() };
        s.discussions = vec![
            discussion(1, on(&id, None), DiscussionStatus::Open),
            discussion(2, on(&id, Some("invariants")), DiscussionStatus::Answered),
            discussion(3, relation, DiscussionStatus::Settled),
            discussion(4, on(&other, None), DiscussionStatus::Open),
        ];
        let ids = |v: Vec<&Discussion>| v.into_iter().map(|c| c.id.to_string()).collect::<Vec<_>>();
        assert_eq!(ids(s.comments_in(Some(Section::Overview))), vec!["d1"]);
        assert_eq!(ids(s.comments_in(Some(Section::Invariants))), vec!["d2"]);
        assert_eq!(ids(s.comments_in(Some(Section::Relations))), vec!["d3"]);
        assert_eq!(ids(s.open_points()), vec!["d1", "d2"], "settled and other components' points aren't open here");
        for section in Section::ALL {
            assert_eq!(Section::from_key(section.key()), Some(section));
        }

        // ] c walks the open points and brings their section up.
        s.step_point(true);
        assert_eq!((s.point.as_ref().map(|p| p.as_str()), s.current), (Some("d1"), Section::Overview));
        s.step_point(true);
        assert_eq!((s.point.as_ref().map(|p| p.as_str()), s.current), (Some("d2"), Section::Invariants));
        s.step_point(true);
        assert_eq!(s.point.as_ref().map(|p| p.as_str()), Some("d1"), "wraps around");

        // Review actions exist with a plan and a loaded component; it all renders.
        let mut d = detail(&id, &other);
        d.component.spec = None;
        d.outgoing.clear();
        s.detail = Some(d);
        s.motion(Motion::First, 1);
        let actions = s.actions();
        for a in ["spec.mark_reviewed", "spec.next_point"] {
            assert!(actions.iter().any(|x| x.id == a), "{a}");
        }
        let ctx = egui::Context::default();
        let mut mode = crate::keymap::InputMode::Normal;
        s.loaded = Some((id.clone(), s.source.clone()));
        for _ in 0..2 {
            frame(&mut s, &ctx, &mut mode, 1);
        }
    }

    /// Every text a frame draws, one per line.
    fn drawn(s: &mut SpecState, ctx: &egui::Context) -> String {
        let (net, mut mode, mut note) = (NetClient::new("http://127.0.0.1:9"), crate::keymap::InputMode::Normal, None);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                s.draw(ui, &mut RunCx { net: &net, ctx, input_mode: &mut mode, at: crate::keymap::Host::Panel(0), note: &mut note, asked: &mut None });
            });
        });
        fn collect(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push_str(t.galley.text());
                    out.push('\n');
                }
                egui::Shape::Vec(list) => list.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let mut text = String::new();
        out.shapes.iter().for_each(|c| collect(&c.shape, &mut text));
        text
    }

    #[test]
    fn the_spec_opens_with_a_header_says_whats_missing_and_marks_changed_items() {
        use crate::review::tests::{discussion, on};
        use arch_graph::{Change, Finding, FindingCode, Target};
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        crate::status::set(config.status.clone());
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        let word = |name: &str| crate::status::look(name).1;
        let (id, other) = (ComponentId::new("rust:a::step"), ComponentId::new("rust:a::run"));
        let plan = PlanId::new("p").unwrap();
        let mut s = SpecState::default();
        s.follow_source(&Source::Plan(plan.clone()));
        s.follow(&id);
        let mut d = detail(&id, &other);
        d.component.description.clear();
        s.detail = Some(d);
        s.loaded = Some((id.clone(), Source::Plan(plan)));
        // The type wants a description and a signature; this one has neither.
        s.types = Some(arch_graph::TypeSystem::from_toml("[types.function]\nrequired_fields = [\"description\", \"signature\", \"logic_steps\"]").unwrap());
        assert_eq!(s.missing_required(), [arch_graph::RequiredField::Description, arch_graph::RequiredField::Signature]);
        s.discussions = vec![discussion(1, on(&id, None), DiscussionStatus::Open)];
        // The second step and the call changed; the first step didn't.
        s.diff = PlanDiff {
            since: Some("2026-10-03T09:00:00Z".into()),
            changes: vec![
                Change::ComponentChanged { id: id.clone(), fields: vec!["spec.logic_steps".into(), "spec.logic_steps.1".into()] },
                Change::RelationAdded { kind: RelationKind::Calls, source: id.clone(), target: other.clone() },
            ],
        };
        s.findings = vec![
            Finding::info(FindingCode::MissingCallSite, Target::Component { id: id.clone() }, "a note"),
            Finding::error(FindingCode::MissingCallSite, Target::Component { id: id.clone() }, "a problem"),
        ];
        let text = drawn(&mut s, &ctx);
        let text = drawn(&mut s, &ctx).max(text);

        // V.2: the header names it, links its place, shows how far along it is and what needs looking at.
        for expected in ["step", "function", "rust:a::step", "src/a.rs (planned)", &word("planned"), &format!("1 {}", word("open_discussions")), &word("changed")] {
            assert!(text.contains(expected), "header shows {expected}: {text}");
        }
        for stage in arch_graph::Stage::ALL {
            assert!(text.contains(&word(crate::status::stage(stage))), "the track names {stage:?}");
        }
        // V.1: states are icon and word, not colour alone.
        let (glyph, _, _) = crate::status::look("planned");
        assert!(text.contains(&format!("{glyph} {}", word("planned"))), "icon with the word");
        assert!(!text.contains("PlanApproved") && !text.contains("Planned\n"), "no raw names: {text}");
        // V.3: sections say what the type needs and doesn't have; findings come worst first.
        assert!(text.contains(&format!("2 {}", word("required_missing"))), "the header counts it");
        let title = |name: &str| text.lines().find(|l| l.trim_start().contains(name) && l.contains("  ")).unwrap_or_default().to_owned();
        assert!(title("Overview").contains(&format!("{} 1 {}", crate::status::look("required_missing").0, word("required_missing"))), "{text}");
        assert!(title("Signature").contains(&format!("1 {}", word("required_missing"))), "{text}");
        let review: String = crate::hosted::Hosted::status(&s).into_iter().map(|st| st.text).collect();
        assert!(review.contains("spec.logic_steps") && !review.contains("spec.logic_steps.1"), "the review line names fields, not items: {review}");
        assert!(text.contains("description: none yet, required for a function.") && text.contains("signature: none yet, required for a function."));
        assert!(!text.contains("logic steps: none yet"), "it has logic steps");
        assert!(text.find("a problem") < text.find("a note") && text.contains(&word("severity_error")), "worst first, with its severity");
        let spots: Vec<String> = s.spots().into_iter().map(|sp| sp.key).collect();
        let at = |i: usize| spots.iter().position(|k| *k == Entry { section: Section::Findings, item: SpecItem::Finding(i) }.key());
        assert!(at(1) < at(0), "the error's item comes first: {spots:?}");
        // V.4: the changed step and the changed relation are marked, the unchanged step isn't.
        let changed = s.changed_items();
        assert!(changed.contains(&Entry { section: Section::Logic, item: SpecItem::Step(1) }.key()));
        assert!(changed.contains(&Entry { section: Section::Relations, item: SpecItem::Relation { kind: RelationKind::Calls, other: other.clone(), outgoing: true } }.key()));
        assert!(!changed.contains(&Entry { section: Section::Logic, item: SpecItem::Step(0) }.key()));
        assert_eq!(changed.len(), 2);
    }

    #[test]
    fn its_changes_section_shows_the_changes_on_its_lines_and_enter_opens_them() {
        use harness_contracts::changes::{DiffLine, LineKind};
        let (a, b) = (ComponentId::new("rust:a::step"), ComponentId::new("rust:a::run"));
        let mut s = SpecState::default();
        s.follow_source(&Source::Code);
        s.follow(&a);
        let mut d = detail(&a, &b);
        d.component.anchor = Some(arch_graph::CodeAnchor { language: "rust".into(), syntax_kind: "function_item".into(), file: arch_graph::RepoPath::new("src/a.rs").unwrap(), symbol: "a::step".into(), span: arch_graph::Span { start_line: 10, end_line: 20, start_byte: 0, end_byte: 0 }, content_hash: String::new() });
        s.detail = Some(d);
        s.loaded = Some((a.clone(), Source::Code));
        let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
        s.comparing = (Comparison::default(), 1, 3);
        s.follow_changes(&net, &ctx);
        assert!(net.sent.lock().unwrap().iter().any(|u| u.contains("/api/changes/file?from=rev%3AHEAD&to=files&path=src%2Fa.rs")), "its file, against the files");
        let hunk = |start: u32, text: &str| Hunk { old_start: start, old_lines: 1, new_start: start, new_lines: 1, lines: vec![DiffLine { kind: LineKind::Added, old: None, new: Some(start), text: text.into() }] };
        let file = FileChanges { comparison: Comparison::default().on_files(), path: "src/a.rs".into(), group: harness_contracts::changes::Group::Changes, context: 3, binary: false, hunks: vec![hunk(2, "above it"), hunk(12, "in it"), hunk(30, "below it")] };
        *s.changes_inbox.lock().unwrap() = Some((Comparison::default().on_files(), "src/a.rs".into(), Ok(file)));
        s.comparing = (Comparison::default(), 1, 3);
        s.follow_changes(&net, &ctx);
        assert_eq!(s.changes.iter().map(|h| h.new_start).collect::<Vec<_>>(), [12], "only what touches its lines");
        let at = s.entries().iter().position(|e| e.item == SpecItem::Hunk(0)).expect("a cursor stop");
        s.put_cursor(at);
        assert_eq!((s.target(), s.enter()), (Some(Target::Change { file: "src/a.rs".into(), line: 12 }), Step::GoTo(GoTo::Code)));
        assert!(s.items().iter().any(|i| i.text.contains("in it")), "searchable");
        let mut mode = crate::keymap::InputMode::Normal;
        frame(&mut s, &ctx, &mut mode, 0);
    }

    #[test]
    fn as_a_destination_it_shows_any_component_but_the_one_whose_card_is_shown() {
        let (a, b) = (ComponentId::new("rust:a::run"), ComponentId::new("rust:a::step"));
        let mut s = SpecState::default();
        s.follow_source(&Source::Code);
        s.follow(&a);
        assert!(!s.can_show(&Target::Component(a.clone())) && s.can_show(&Target::Component(b.clone())));
        assert_eq!(s.show_target(&Target::Component(b.clone())), Ok(()));
        assert_eq!(s.target.as_ref(), Some(&b));
    }

}
