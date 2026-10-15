//! The spec view: everything about one component, in full. It follows the
//! graph's selection (or stays pinned). A cursor walks every item in it
//! (section titles, relations, steps, invariants, findings, comments and
//! replies), so every control has a key.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use arch_graph::{Comment, CommentAnchor, CommentId, CommentKind, CommentStatus, ComponentDetail, ComponentId, Finding, PlanDiff, RelationKind, Report, Severity, Source};
use egui::{Rect, RichText};

use crate::actions::{Action, ActionProvider, Group, MINOR, PRIMARY, RunCx, SECONDARY, ToolbarStyle, ToolbarUse, When, toolbar_control};
use crate::nav::{Fold, GoTo, Item, Motion, Navigable, Place, SpecItem, Spot, Step, Target};
use crate::net::{NetClient, NetResult};
use crate::theme::colors;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Overview,
    Signature,
    Logic,
    Invariants,
    Relations,
    Code,
    Placeholders,
    Findings,
}

impl Section {
    pub const ALL: [Section; 8] =
        [Section::Overview, Section::Signature, Section::Logic, Section::Invariants, Section::Relations, Section::Code, Section::Placeholders, Section::Findings];

    /// The stable name comments use for it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Signature => "signature",
            Self::Logic => "logic",
            Self::Invariants => "invariants",
            Self::Relations => "relations",
            Self::Code => "code",
            Self::Placeholders => "placeholders",
            Self::Findings => "findings",
        }
    }

    pub fn from_key(key: &str) -> Option<Section> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }

    /// Where a required detail is written.
    pub fn of_required(field: arch_graph::RequiredField) -> Section {
        use arch_graph::RequiredField as F;
        match field {
            F::Description => Self::Overview,
            F::Signature | F::Inputs | F::Outputs | F::Properties | F::SideEffects => Self::Signature,
            F::Spec | F::Pattern | F::LogicSteps | F::Validation => Self::Logic,
            F::Invariants => Self::Invariants,
        }
    }

    /// Where a field changed since the last review shows (diff field paths).
    pub fn of_field(field: &str) -> Section {
        match field {
            "signature" => Self::Signature,
            f if f.starts_with("schemas") => Self::Signature,
            f if f.starts_with("spec.logic_steps") || f.starts_with("spec.tasks") => Self::Logic,
            f if f.starts_with("spec.invariants") => Self::Invariants,
            "relations" => Self::Relations,
            "placeholders" => Self::Placeholders,
            _ => Self::Overview,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Signature => "Signature",
            Self::Logic => "Logic steps",
            Self::Invariants => "Invariants",
            Self::Relations => "Relations",
            Self::Code => "Code",
            Self::Placeholders => "Placeholders",
            Self::Findings => "Findings",
        }
    }
}

/// The section a comment shows in, when it is about `id`: node comments in
/// the overview, relation comments in relations, section comments in theirs.
pub fn section_of(anchor: &CommentAnchor, id: &ComponentId) -> Option<Section> {
    match anchor {
        CommentAnchor::Component { id: on } if on == id => Some(Section::Overview),
        CommentAnchor::Section { id: on, section } if on == id => Some(Section::from_key(section).unwrap_or(Section::Overview)),
        CommentAnchor::Relation { source, target, .. } if source == id || target == id => Some(Section::Relations),
        _ => None,
    }
}

/// What the composer is writing.
#[derive(Clone, Debug, PartialEq)]
pub enum Compose {
    New(CommentKind),
    Reply(CommentId),
    Reopen(CommentId),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Composer {
    pub purpose: Compose,
    /// What a new comment is about.
    pub anchor: Option<CommentAnchor>,
    pub section: Section,
    pub text: String,
}

enum Reply {
    Detail(ComponentId, Box<NetResult<ComponentDetail>>),
    Report(ComponentId, NetResult<Report>),
    /// A review write finished.
    Posted(Result<(), String>),
}

#[derive(Default)]
pub struct SpecRequests {
    /// Select this component in the graph.
    pub select: Option<ComponentId>,
    /// Open this file at these lines.
    pub open_code: Option<(String, u32, u32)>,
    /// A review write went through: refresh the plan's review state.
    pub review_changed: bool,
    /// The composer opened: the keyboard goes to it.
    pub insert: bool,
    /// Read this file again (its code changed since it was indexed).
    pub reread: Option<String>,
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

pub struct SpecState {
    /// What it shows: the graph selection unless pinned.
    pub target: Option<ComponentId>,
    /// What it's read from: the code, or the code with a plan (where its
    /// comments and checks live).
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
    /// Scroll the cursor's item into view next frame.
    scroll: bool,
    pub(crate) loaded: Option<(ComponentId, Source)>,
    error: Option<String>,
    pub requests: SpecRequests,
    /// The cockpit user (author of comments and reviews).
    pub author: String,
    /// The plan's comments (all of them) and its diff since the last review.
    pub comments: Vec<Comment>,
    pub diff: PlanDiff,
    pub composer: Option<Composer>,
    /// The review point `] c` is on.
    pub point: Option<CommentId>,
    note: Option<String>,
    /// The component's code changed since it was indexed (set by the app).
    pub out_of_date: bool,
    /// The component types (what each requires), from the server.
    pub types: Option<arch_graph::TypeSystem>,
    /// The item a restored place is on, until the component's detail is in.
    restore_item: Option<String>,
    /// Items drawn last frame, and where; and the viewport.
    spots: Vec<Spot>,
    view: Rect,
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
            scroll: false,
            loaded: None,
            error: None,
            requests: SpecRequests::default(),
            author: String::new(),
            comments: Vec::new(),
            diff: PlanDiff::default(),
            composer: None,
            point: None,
            note: None,
            out_of_date: false,
            types: None,
            restore_item: None,
            spots: Vec::new(),
            view: Rect::NOTHING,
            inbox: Arc::default(),
        }
    }
}

impl SpecState {
    /// Follows the graph (unless pinned).
    pub fn follow(&mut self, selection: Option<&ComponentId>, source: &Source) {
        if !self.pinned {
            if self.target.as_ref() != selection {
                self.leave_component();
            }
            self.target = selection.cloned();
            self.source = source.clone();
        }
    }

    /// Moving to another component: the cursor, the review point and an
    /// unsent composer belonged to the old one.
    fn leave_component(&mut self) {
        self.cursor = 0;
        self.current = Section::Overview;
        self.point = None;
        self.composer = None;
    }

    /// Shows `id` even while pinned (a go-to asked for it).
    pub fn show(&mut self, id: ComponentId) {
        if self.target.as_ref() != Some(&id) {
            self.leave_component();
        }
        self.target = Some(id);
    }

    /// The architecture changed: load again.
    pub fn invalidate(&mut self) {
        self.loaded = None;
    }

    /// Fetches the target's detail (and its plan findings) when it changes.
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::Detail(id, result) if Some(&id) == self.target.as_ref() => match *result {
                    Ok(detail) => {
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
                    self.requests.review_changed = true;
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
            Section::Placeholders => out.extend((0..self.detail.as_ref().map_or(0, |d| d.component.placeholders.len())).map(SpecItem::Placeholder)),
            Section::Findings => out.extend((0..self.findings.len()).map(SpecItem::Finding)),
            _ => {}
        }
        for c in self.comments_in(Some(section)) {
            out.push(SpecItem::Comment(c.id.clone()));
            out.extend((0..c.replies.len()).map(|i| SpecItem::Reply(c.id.clone(), i)));
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
        self.scroll = true;
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

    /// The comments on the target, in a section (or all of them), oldest first.
    pub fn comments_in(&self, section: Option<Section>) -> Vec<&Comment> {
        let Some(target) = &self.target else { return Vec::new() };
        let mut out: Vec<&Comment> = self.comments.iter().filter(|c| section_of(&c.anchor, target).is_some_and(|s| section.is_none_or(|want| want == s))).collect();
        out.sort_by_key(|c| c.id.0.trim_start_matches('c').parse::<u32>().unwrap_or(u32::MAX));
        out
    }

    /// Unaddressed points on the target.
    pub fn open_points(&self) -> Vec<&Comment> {
        self.comments_in(None).into_iter().filter(|c| !c.is_addressed()).collect()
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
    fn render_header(&self, ui: &mut egui::Ui, detail: &ComponentDetail) -> Option<Click> {
        let c = &detail.component;
        let look = crate::theme::look();
        let mut clicked = None;
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
                    if ui.link(RichText::new(format!("{} {text}", egui_phosphor::regular::FILE_CODE)).monospace().size(look.text_small)).on_hover_text("Open in the code view").clicked() {
                        clicked = Some(Click::OpenCode);
                    }
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
                    crate::status::chip(ui, "open_comments", Some(&open.to_string()));
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
        clicked
    }

    /// Moves to the next (or previous) unaddressed point, cursor and all.
    pub fn step_point(&mut self, forward: bool) {
        let points: Vec<(CommentId, Option<Section>)> = {
            let target = self.target.clone();
            self.open_points().into_iter().map(|c| (c.id.clone(), target.as_ref().and_then(|t| section_of(&c.anchor, t)))).collect()
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

    /// The comment the cursor is on (or in a reply to).
    fn comment_here(&self) -> Option<&Comment> {
        let id = match self.entry()?.item {
            SpecItem::Comment(id) | SpecItem::Reply(id, _) => id,
            _ => return None,
        };
        self.comments.iter().find(|c| c.id == id)
    }

    fn start_composing(&mut self, purpose: Compose) {
        let entry = self.entry();
        let section = entry.as_ref().map_or(self.current, |e| e.section);
        let anchor = match (&purpose, &self.target, entry.map(|e| e.item)) {
            (Compose::New(_), Some(target), Some(SpecItem::Relation { kind, other, outgoing })) => {
                let (source, rel_target) = if outgoing { (target.clone(), other) } else { (other, target.clone()) };
                Some(CommentAnchor::Relation { kind, source, target: rel_target })
            }
            (Compose::New(_), Some(target), _) if section == Section::Overview => Some(CommentAnchor::Component { id: target.clone() }),
            (Compose::New(_), Some(target), _) => Some(CommentAnchor::Section { id: target.clone(), section: section.key().to_owned() }),
            _ => None,
        };
        self.composer = Some(Composer { purpose, anchor, section, text: String::new() });
        self.requests.insert = true;
    }

    fn posted<T: Send + 'static>(&self) -> impl FnOnce(NetResult<T>) + Send + 'static {
        let inbox = self.inbox.clone();
        move |r: NetResult<T>| push(&inbox, Reply::Posted(r.map(|_| ()).map_err(String::from)))
    }

    /// Sends what the composer holds.
    pub fn send_composer(&mut self, net: &NetClient, ctx: &egui::Context) {
        let (Some(composer), Some(plan)) = (self.composer.clone(), self.source.plan().cloned()) else { return };
        let text = composer.text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        match composer.purpose {
            Compose::New(kind) => {
                let Some(anchor) = composer.anchor else { return };
                let body = serde_json::json!({ "anchor": anchor, "kind": kind, "text": text, "by": self.author });
                net.arch_add_comment(&plan, body, ctx.clone(), self.posted());
            }
            Compose::Reply(id) => net.arch_comment_action(&plan, &id, "replies", serde_json::json!({ "text": text, "by": self.author }), ctx.clone(), self.posted()),
            Compose::Reopen(id) => net.arch_comment_action(&plan, &id, "reopen", serde_json::json!({ "text": text, "by": self.author }), ctx.clone(), self.posted()),
        }
        self.composer = None;
    }

    fn accept(&mut self, id: &CommentId, net: &NetClient, ctx: &egui::Context) {
        if let Some(plan) = self.source.plan() {
            net.arch_comment_action(plan, id, "accept", serde_json::json!({ "by": self.author }), ctx.clone(), self.posted());
        }
    }

    /// The answered point accept and reopen act on: the one under the
    /// cursor, else the one `] c` is on, else the first answered one.
    fn answered_point(&self) -> Option<CommentId> {
        if let Some(c) = self.comment_here() {
            return (c.status == CommentStatus::Answered).then(|| c.id.clone());
        }
        let answered: Vec<&Comment> = self.open_points().into_iter().filter(|c| c.status == CommentStatus::Answered).collect();
        self.point.clone().filter(|p| answered.iter().any(|c| &c.id == p)).or_else(|| answered.first().map(|c| c.id.clone()))
    }

    /// The comment a reply goes to: the one under the cursor, else the one
    /// `] c` is on.
    fn reply_point(&self) -> Option<CommentId> {
        let here = self.comment_here().map(|c| c.id.clone());
        here.or_else(|| self.point.clone().filter(|p| self.comments_in(None).iter().any(|c| &c.id == p)))
    }

    fn code_location(&self) -> Option<(String, u32, u32)> {
        let c = &self.detail.as_ref()?.component;
        match (&c.anchor, &c.proposed) {
            (Some(a), _) if crate::status::is_folder(a) => None,
            (Some(a), _) => Some((a.file.to_string(), a.span.start_line, a.span.end_line)),
            (None, Some(p)) => Some((p.file.to_string(), 1, 1)),
            (None, None) => None,
        }
    }

    fn half_page_items(&self) -> usize {
        let heights: Vec<f32> = self.spots.iter().map(|s| s.rect.height()).collect();
        let avg = if heights.is_empty() { 20.0 } else { heights.iter().sum::<f32>() / heights.len() as f32 };
        if self.view.height() > 0.0 { ((self.view.height() / avg.max(8.0)) / 2.0).floor().max(1.0) as usize } else { 8 }
    }

    pub fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        self.tick(cx.net, cx.ctx);
        let actions = self.actions();
        let mut used = None;
        ui.horizontal_wrapped(|ui| {
            for action in actions.iter().filter(|a| a.toolbar.is_some()) {
                if let Some(u) = toolbar_control(ui, action) {
                    used = Some((action.clone(), u));
                }
            }
            for section in Section::ALL {
                let label = RichText::new(section.title()).size(crate::theme::look().text_body);
                if ui.selectable_label(self.current == section, label).on_hover_text("J / K").clicked() {
                    self.put_on(&Entry { section, item: SpecItem::Header });
                }
            }
        });
        if let Some((action, ToolbarUse::Run(value))) = used {
            self.run(&action, value, cx);
        }
        self.render_review_bar(ui, cx);
        ui.separator();
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
        let mut clicked: Option<Click> = None;
        let scroll = std::mem::take(&mut self.scroll);
        let relations = self.relations();
        let changed_items = self.changed_items();
        let missing = self.missing_required();
        let type_name = c.type_name.clone();
        let output = egui::ScrollArea::vertical().id_salt("spec_items").auto_shrink([false, false]).show(ui, |ui| {
            // One item: highlighted under the cursor, scrolled to when it moved there.
            let mut item = |ui: &mut egui::Ui, entry: Entry, add: &mut dyn FnMut(&mut egui::Ui)| -> egui::Response {
                let on = cursor_key.as_deref() == Some(entry.key().as_str());
                let fill = if on { colors().selection } else { egui::Color32::TRANSPARENT };
                let response = egui::Frame::NONE.fill(fill).corner_radius(crate::theme::look().radius_s).inner_margin(crate::theme::margin2(crate::theme::look().space_s, crate::theme::look().space_hair)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    add(ui);
                }).response;
                if on && scroll {
                    response.scroll_to_me(None);
                }
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
            if let Some(click) = self.render_header(ui, &detail) {
                clicked = Some(click);
            }
            for section in Section::ALL {
                let open = !self.folded.contains(&section);
                let mut title = section.title().to_owned();
                if changed.contains(&section) {
                    title.push_str(&format!("  {} changed", egui_phosphor::regular::DOT));
                }
                let open_here = self.comments_in(Some(section)).iter().filter(|c| !c.is_addressed()).count();
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
                let header = item(ui, Entry { section, item: SpecItem::Header }, &mut |ui| {
                    ui.add(egui::Label::new(RichText::new(format!("{caret} {title}")).strong().size(crate::theme::look().text_title).color(color)).sense(egui::Sense::click()));
                });
                if header.interact(egui::Sense::click()).clicked() {
                    clicked = Some(Click::Header(section));
                }
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
                                let mut followed = false;
                                item(ui, entry, &mut |ui| {
                                    followed |= ui.link(RichText::new(&text).monospace().size(crate::theme::look().text_body)).on_hover_text(format!("{}{how}", other.as_str())).clicked();
                                });
                                if followed {
                                    clicked = Some(Click::Follow(other.clone()));
                                }
                            }
                        }
                        Section::Code => match &detail.code {
                            Some(code) => {
                                if self.out_of_date {
                                    ui.horizontal_wrapped(|ui| {
                                        crate::status::chip(ui, "stale", None);
                                        ui.label(RichText::new("This code changed since it was indexed; the excerpt may be old.").size(crate::theme::look().text_body).color(colors().muted));
                                        if ui.small_button("Re-read it").clicked() {
                                            clicked = Some(Click::Reread);
                                        }
                                    });
                                }
                                let mut open_code = false;
                                item(ui, Entry { section, item: SpecItem::Source }, &mut |ui| {
                                    egui::Frame::NONE.fill(colors().bg).inner_margin(crate::theme::margin(crate::theme::look().space_sm)).corner_radius(crate::theme::look().radius_m).show(ui, |ui| {
                                        ui.label(RichText::new(code).monospace().size(crate::theme::look().text_body));
                                    });
                                    open_code |= ui.button("Open in the code view").on_hover_text("g d").clicked();
                                });
                                if open_code {
                                    clicked = Some(Click::OpenCode);
                                }
                            }
                            None => {
                                ui.label(RichText::new("Not in the code yet.").color(colors().muted));
                            }
                        },
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
                    for comment in self.comments_in(Some(section)) {
                        if let Some(click) = render_thread(ui, comment, self.point.as_ref(), section, &mut item) {
                            clicked = Some(click);
                        }
                    }
                });
            }
        });
        self.view = output.inner_rect;
        self.spots = spots.into_iter().filter(|s| s.rect.intersects(output.inner_rect)).collect();
        match clicked {
            Some(Click::Header(section)) => {
                self.put_on(&Entry { section, item: SpecItem::Header });
                self.fold(Fold::Toggle);
            }
            Some(Click::Follow(id)) => self.requests.select = Some(id),
            Some(Click::Reread) => self.requests.reread = self.code_location().map(|(file, _, _)| file),
            Some(Click::OpenCode) => self.requests.open_code = self.code_location(),
            Some(Click::Accept(id)) => self.accept(&id, cx.net, cx.ctx),
            Some(Click::Compose(id, purpose)) => {
                self.point = Some(id);
                self.start_composing(purpose);
            }
            None => {}
        }
    }

    /// The review line under the toolbar: what changed since the review, and
    /// the composer when writing a comment, reply or reason to reopen.
    fn render_review_bar(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        if let Some(id) = &self.target {
            // Items that changed carry their own mark; the line names the fields.
            let fields: Vec<String> = self.diff.fields(id).into_iter().filter(|f| f.rsplit('.').next().is_none_or(|last| last.parse::<usize>().is_err())).collect();
            if !fields.is_empty() {
                let since = self.diff.since.as_deref().map_or_else(|| "new in this plan (never reviewed)".to_owned(), |at| format!("changed since the review at {at}"));
                ui.label(RichText::new(format!("{} {since}: {}", egui_phosphor::regular::DOT, fields.into_iter().collect::<Vec<_>>().join(", "))).size(crate::theme::look().text_body).color(colors().info));
            }
        }
        if let Some(note) = &self.note {
            ui.label(RichText::new(note).size(crate::theme::look().text_body).color(colors().error));
        }
        let Some(composer) = self.composer.as_mut() else { return };
        let what = match (&composer.purpose, &composer.anchor) {
            (Compose::New(kind), Some(CommentAnchor::Relation { kind: rel, source, target })) => {
                let name = |id: &ComponentId| id.as_str().rsplit([':', '.']).next().unwrap_or(id.as_str()).to_owned();
                let what = if *kind == CommentKind::ChangeRequest { "Change request" } else { "Comment" };
                format!("{what} on {} {} {}", name(source), rel.name(), name(target))
            }
            (Compose::New(CommentKind::ChangeRequest), _) => format!("Change request on {}", composer.section.title()),
            (Compose::New(_), _) => format!("Comment on {}", composer.section.title()),
            (Compose::Reply(id), _) => format!("Reply to {id}"),
            (Compose::Reopen(id), _) => format!("Why reopen {id}?"),
        };
        ui.label(RichText::new(what).size(crate::theme::look().text_body).color(colors().muted));
        let inserting = *cx.input_mode == crate::keymap::InputMode::Insert { slot: cx.slot };
        let output = ui.add(egui::TextEdit::multiline(&mut composer.text).id(egui::Id::new(("spec_composer", cx.slot))).desired_rows(2).desired_width(f32::INFINITY).hint_text("Enter sends, Shift+Enter for a new line, Esc leaves"));
        if inserting {
            output.request_focus();
        }
        if output.clicked() || output.gained_focus() {
            *cx.input_mode = crate::keymap::InputMode::Insert { slot: cx.slot };
        }
        let enter = inserting && output.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        let (mut send, mut cancel) = (enter, false);
        ui.horizontal(|ui| {
            send |= ui.button("Send").on_hover_text("Enter").clicked();
            cancel = ui.button("Cancel").clicked();
        });
        if cancel {
            self.composer = None;
        } else if send {
            self.send_composer(cx.net, cx.ctx);
        }
        if (cancel || send) && inserting {
            *cx.input_mode = crate::keymap::InputMode::Normal;
        }
    }
}

/// What a click in the spec asks for.
enum Click {
    Reread,
    Header(Section),
    Follow(ComponentId),
    OpenCode,
    Accept(CommentId),
    Compose(CommentId, Compose),
}

/// Draws one cursor stop (`Entry`), its contents drawn by the closure given.
type ItemDrawer<'a> = dyn FnMut(&mut egui::Ui, Entry, &mut dyn FnMut(&mut egui::Ui)) -> egui::Response + 'a;

/// One comment thread: the comment and its replies are cursor stops; its
/// buttons name their keys.
fn render_thread(ui: &mut egui::Ui, c: &Comment, point: Option<&CommentId>, section: Section, item: &mut ItemDrawer) -> Option<Click> {
    let mut clicked = None;
    let (label, color) = match c.status {
        CommentStatus::Open => ("OPEN", colors().warn),
        CommentStatus::Answered => ("ANSWERED", colors().info),
        CommentStatus::Resolved => ("RESOLVED", colors().ok),
    };
    let stroke = if point == Some(&c.id) { colors().focus } else { colors().border };
    egui::Frame::NONE.fill(colors().surface).stroke(egui::Stroke::new(crate::theme::look().line_thin, stroke)).corner_radius(crate::theme::look().radius_m).inner_margin(crate::theme::margin(crate::theme::look().space_sm)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        item(ui, Entry { section, item: SpecItem::Comment(c.id.clone()) }, &mut |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(label).size(crate::theme::look().text_small).strong().color(color));
                if c.kind == CommentKind::ChangeRequest {
                    ui.label(RichText::new("change request").size(crate::theme::look().text_small).color(colors().special));
                }
                ui.label(RichText::new(format!("{} · {}", c.id, c.author)).size(crate::theme::look().text_small).color(colors().muted));
            });
            ui.label(&c.text);
        });
        for (i, r) in c.replies.iter().enumerate() {
            item(ui, Entry { section, item: SpecItem::Reply(c.id.clone(), i) }, &mut |ui| {
                ui.label(RichText::new(format!("{}: {}", r.author, r.text)).size(crate::theme::look().text_body).color(colors().text));
            });
        }
        if let Some(a) = &c.answer {
            ui.label(RichText::new(format!("{} {}: {}", egui_phosphor::regular::ARROW_ELBOW_DOWN_RIGHT, a.author, a.text)).size(crate::theme::look().text_body).color(colors().info));
            for ch in &a.changes {
                ui.label(RichText::new(format!("   {}", ch.describe())).size(crate::theme::look().text_small).monospace().color(colors().muted));
            }
        }
        if !c.is_addressed() {
            ui.horizontal(|ui| {
                if c.status == CommentStatus::Answered {
                    if ui.small_button("Accept").on_hover_text("a").clicked() {
                        clicked = Some(Click::Accept(c.id.clone()));
                    }
                    if ui.small_button("Reopen").on_hover_text("x").clicked() {
                        clicked = Some(Click::Compose(c.id.clone(), Compose::Reopen(c.id.clone())));
                    }
                }
                if ui.small_button("Reply").on_hover_text("r").clicked() {
                    clicked = Some(Click::Compose(c.id.clone(), Compose::Reply(c.id.clone())));
                }
            });
        }
    });
    clicked
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
fn capitalized(text: &str) -> String {
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
    CancelCompose,
    NextSection,
    PrevSection,
    Pin,
    Comment,
    RequestChange,
    MarkReviewed,
    Point { forward: bool },
    Reply,
    Accept,
    Reopen,
}

impl ActionProvider for SpecState {
    fn actions(&self) -> Vec<Action> {
        use SpecCommand as S;
        let f = |id: &str, label: &str, c: SpecCommand| Action::new(id, label, c).focused();
        let mut out = vec![];
        if self.composer.is_some() {
            out.push(f("spec.cancel", "Throw away what's being written", S::CancelCompose).importance(PRIMARY));
        }
        out.extend([
            f("spec.next_section", "Next section", S::NextSection).group(Group::Navigate),
            f("spec.prev_section", "Previous section", S::PrevSection).group(Group::Navigate).importance(MINOR),
        ]);
        if !self.floating {
            out.push(
                f("spec.pin", if self.pinned { "Unpin (follow the graph)" } else { "Pin this component" }, S::Pin)
                    .toolbar(ToolbarStyle::Button, if self.pinned { "Pinned" } else { "Pin" }, Some(egui_phosphor::regular::PUSH_PIN))
                    .group(Group::View)
                    .importance(MINOR),
            );
        }
        // On a comment, its own verbs lead; without a plan, commenting only says why not.
        let on_comment = self.comment_here().is_some();
        let comment_rank = if self.source.plan().is_none() { MINOR } else if on_comment { SECONDARY } else { PRIMARY };
        if self.detail.is_some() {
            out.push(f("spec.comment", "Comment on this item", S::Comment).importance(comment_rank));
            out.push(f("spec.request_change", "Ask for a change to this item", S::RequestChange).importance(comment_rank));
        }
        if self.source.plan().is_some() {
            out.push(
                f("spec.mark_reviewed", "Mark the plan reviewed (later changes are highlighted)", S::MarkReviewed)
                    .toolbar(ToolbarStyle::Button, "Reviewed", Some(egui_phosphor::regular::CHECK_SQUARE))
                    .importance(MINOR),
            );
            // Always there: with no open points it says so.
            let points = if self.open_points().is_empty() { When::Unavailable } else { When::Always };
            out.push(f("spec.next_point", "Next open review point", S::Point { forward: true }).group(Group::Navigate).importance(if on_comment { SECONDARY } else { PRIMARY }).when(points).jump());
            out.push(f("spec.prev_point", "Previous open review point", S::Point { forward: false }).group(Group::Navigate).importance(MINOR).when(points).jump());
            if self.reply_point().is_some() {
                out.push(f("spec.reply", "Reply to this point", S::Reply).importance(if on_comment { PRIMARY } else { SECONDARY }));
            }
            if self.answered_point().is_some() {
                out.push(f("spec.accept", "Accept the answer to this point", S::Accept).importance(PRIMARY));
                out.push(f("spec.reopen", "Reopen this point (say why)", S::Reopen).importance(SECONDARY));
            }
        }
        out
    }

    fn run(&mut self, action: &Action, _value: Option<String>, cx: &mut RunCx<'_>) {
        match action.command::<SpecCommand>() {
            // Comments live in a plan.
            Some(SpecCommand::Comment | SpecCommand::RequestChange) if self.source.plan().is_none() => *cx.note = Some("Comments go in a plan: pick one in the graph first.".into()),
            Some(SpecCommand::Comment) => self.start_composing(Compose::New(CommentKind::Comment)),
            Some(SpecCommand::RequestChange) => self.start_composing(Compose::New(CommentKind::ChangeRequest)),
            Some(SpecCommand::MarkReviewed) => {
                if let Some(plan) = self.source.plan().cloned() {
                    cx.net.arch_mark_reviewed(&plan, &self.author, cx.ctx.clone(), self.posted());
                }
            }
            Some(SpecCommand::Point { .. }) if self.open_points().is_empty() => *cx.note = Some("No open review points on this component.".into()),
            Some(SpecCommand::Point { forward }) => self.step_point(*forward),
            Some(SpecCommand::Reply) => match self.reply_point() {
                Some(id) => {
                    self.point = Some(id.clone());
                    self.start_composing(Compose::Reply(id));
                }
                None => *cx.note = Some("Put the cursor on a comment to reply to it.".into()),
            },
            Some(SpecCommand::Accept) => match self.answered_point() {
                Some(id) => self.accept(&id, cx.net, cx.ctx),
                None => *cx.note = Some("Only an answered point can be accepted.".into()),
            },
            Some(SpecCommand::Reopen) => match self.answered_point() {
                Some(id) => {
                    self.point = Some(id.clone());
                    self.start_composing(Compose::Reopen(id));
                }
                None => *cx.note = Some("Only an answered point can be reopened.".into()),
            },
            Some(SpecCommand::CancelCompose) => self.composer = None,
            Some(SpecCommand::NextSection) => self.step_section(1),
            Some(SpecCommand::PrevSection) => self.step_section(-1),
            Some(SpecCommand::Pin) => self.pinned = !self.pinned,
            None => {}
        }
    }
}

impl Navigable for SpecState {
    fn place(&self) -> Place {
        Place::Spec { component: self.target.clone(), item: self.entry().map(|e| e.key()) }
    }

    /// Back to the component (pinned there, so the graph doesn't take it
    /// away) and the item.
    fn restore(&mut self, place: &Place) {
        if let Place::Spec { component, item } = place {
            if let Some(id) = component {
                self.show(id.clone());
            }
            let loaded = self.detail.as_ref().is_some_and(|d| Some(&d.component.id) == self.target.as_ref());
            match item.as_deref().and_then(|key| self.all_entries().into_iter().find(|e| e.key() == key)) {
                Some(entry) if loaded => self.put_on(&entry),
                // Not in yet: land there when it is.
                _ if !loaded => {
                    self.restore_item = item.clone();
                    self.put_cursor(0);
                }
                _ => self.put_cursor(0),
            }
        }
    }

    fn target(&self) -> Option<Target> {
        let component = self.target.clone()?;
        let entry = self.entry().unwrap_or(Entry { section: self.current, item: SpecItem::Header });
        Some(Target::Spec { component, section: entry.section, item: entry.item })
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let n = self.entries().len();
        if n == 0 {
            return;
        }
        match motion {
            Motion::Down => self.put_cursor(self.cursor + count),
            Motion::Up => self.put_cursor(self.cursor.saturating_sub(count)),
            Motion::HalfDown => self.put_cursor(self.cursor + self.half_page_items() * count),
            Motion::HalfUp => self.put_cursor(self.cursor.saturating_sub(self.half_page_items() * count)),
            Motion::First => self.put_cursor(0),
            Motion::Last => self.put_cursor(n - 1),
            Motion::Line(i) => self.put_cursor(i.saturating_sub(1)),
            Motion::NextSibling => self.step_section(count as i32),
            Motion::PrevSibling => self.step_section(-(count as i32)),
            Motion::Left | Motion::Right => {}
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
            SpecItem::Source => Step::GoTo(GoTo::Code),
            SpecItem::Comment(id) | SpecItem::Reply(id, _) => {
                self.point = Some(id.clone());
                self.start_composing(Compose::Reply(id));
                Step::Done
            }
            SpecItem::Placeholder(_) => Step::GoTo(GoTo::Code),
            SpecItem::Step(_) | SpecItem::Invariant(_) | SpecItem::Finding(_) => Step::Nothing("c comments on it, C asks for a change.".into()),
        }
    }

    fn can_enter(&self) -> bool {
        self.entry().is_some_and(|e| !matches!(e.item, SpecItem::Step(_) | SpecItem::Invariant(_) | SpecItem::Finding(_)))
    }

    fn can_leave(&self) -> bool {
        self.entry().is_some_and(|e| e.item != SpecItem::Header)
    }

    fn enter_label(&self) -> Option<String> {
        let name = |id: &ComponentId| id.as_str().rsplit([':', '.']).next().unwrap_or(id.as_str()).to_owned();
        Some(match self.entry()?.item {
            SpecItem::Header if self.folded.contains(&self.current) => "Unfold this section".to_owned(),
            SpecItem::Header => "Fold this section".to_owned(),
            SpecItem::Relation { other, .. } => format!("Follow to {}", name(&other)),
            SpecItem::Source | SpecItem::Placeholder(_) => "Open in the code".to_owned(),
            SpecItem::Comment(_) | SpecItem::Reply(..) => "Reply".to_owned(),
            SpecItem::Step(_) | SpecItem::Invariant(_) | SpecItem::Finding(_) => return None,
        })
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
                    SpecItem::Finding(i) => (String::new(), self.findings.get(*i).map(|f| f.message.clone()).unwrap_or_default()),
                    SpecItem::Comment(id) => (String::new(), self.comments.iter().find(|c| &c.id == id).map(|c| c.text.clone()).unwrap_or_default()),
                    SpecItem::Reply(id, i) => (String::new(), self.comments.iter().find(|c| &c.id == id).and_then(|c| c.replies.get(*i)).map(|r| r.text.clone()).unwrap_or_default()),
                };
                Item::new(e.key(), name, text)
            })
            .collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.entry().map(|e| e.key())
    }

    fn move_to(&mut self, key: &str) {
        if let Some(entry) = self.all_entries().into_iter().find(|e| e.key() == key) {
            self.put_on(&entry);
        }
    }
}

impl SpecState {
    /// The cursor onto a folded section's title (without unfolding it).
    fn put_on_header_keeping_fold(&mut self, section: Section) {
        if let Some(i) = self.entries().iter().position(|e| e.section == section && e.item == SpecItem::Header) {
            self.put_cursor(i);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
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
                s.render(ui, &mut RunCx { net: &net, ctx, input_mode: mode, slot, note: &mut note, shell: &[], hints: &[] });
            });
        });
    }

    #[test]
    fn spec_view_follows_unless_pinned_and_navigates_sections() {
        let mut s = SpecState::default();
        let a = ComponentId::new("rust:a::run");
        let b = ComponentId::new("rust:a::step");
        s.follow(Some(&a), &Source::Code);
        assert_eq!(s.target.as_ref(), Some(&a));
        s.pinned = true;
        s.follow(Some(&b), &Source::Code);
        assert_eq!(s.target.as_ref(), Some(&a), "pinned");
        s.pinned = false;
        s.follow(Some(&b), &Source::Plan(PlanId::new("p").unwrap()));
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
        for id in ["spec.next_section", "spec.prev_section", "spec.pin", "spec.comment"] {
            assert!(actions.iter().any(|a| a.id == id), "{id}");
        }
    }

    #[test]
    fn the_cursor_walks_every_item_and_every_item_has_a_key() {
        use crate::review::tests::comment;
        let id = ComponentId::new("rust:a::step");
        let other = ComponentId::new("rust:a::save");
        let mut s = SpecState::default();
        s.follow(Some(&id), &Source::Plan(PlanId::new("p").unwrap()));
        s.detail = Some(detail(&id, &other));
        s.loaded = Some((id.clone(), s.source.clone()));
        let mut answered = comment(1, CommentAnchor::Section { id: id.clone(), section: "logic".into() }, CommentStatus::Answered);
        answered.replies.push(arch_graph::Reply { author: arch_graph::Author::User { name: "izo".into() }, text: "and?".into(), at: String::new() });
        s.comments = vec![answered];
        let keys: Vec<String> = s.entries().iter().map(Entry::key).collect();
        let at = |k: &str| keys.iter().position(|x| x == k).unwrap_or_else(|| panic!("{k} in {keys:?}"));
        // j walks titles, steps, comments and replies in order.
        assert!(at("logic#Header") < at("logic#Step(0)") && at("logic#Step(1)") < at("logic#Comment(CommentId(\"c1\"))"));
        assert!(at("logic#Comment(CommentId(\"c1\"))") < at("logic#Reply(CommentId(\"c1\"), 0)"));
        s.motion(Motion::Line(at("logic#Step(1)") + 1), 1);
        assert_eq!(s.current, Section::Logic);
        s.motion(Motion::Down, 1);
        assert!(matches!(s.entry().unwrap().item, SpecItem::Comment(_)));
        // On a comment: a accepts it, x reopens it, r (or Enter) replies.
        let ids: Vec<String> = s.actions().into_iter().map(|a| a.id).collect();
        for a in ["spec.reply", "spec.accept", "spec.reopen", "spec.comment", "spec.request_change", "spec.next_point", "spec.mark_reviewed", "spec.pin"] {
            assert!(ids.contains(&a.to_owned()), "{a}: {ids:?}");
        }
        assert_eq!(s.enter(), Step::Done);
        assert_eq!(s.composer.as_ref().map(|c| c.purpose.clone()), Some(Compose::Reply(CommentId("c1".into()))));
        assert!(std::mem::take(&mut s.requests.insert), "the composer takes the keyboard");
        s.composer = None;
        // Backspace goes up to the title; Enter on a title folds it.
        assert_eq!(s.leave(), Step::Done);
        assert_eq!(s.entry().unwrap().item, SpecItem::Header);
        assert_eq!(s.enter(), Step::Done);
        assert!(s.folded.contains(&Section::Logic));
        assert!(!s.entries().iter().any(|e| matches!(e.item, SpecItem::Step(_))), "folded items are skipped");
        s.fold(Fold::OpenAll);
        // Relations: Enter follows (to the other component's spec); c comments on the relation itself.
        s.move_to(&Entry { section: Section::Relations, item: SpecItem::Relation { kind: RelationKind::Calls, other: other.clone(), outgoing: true } }.key());
        assert_eq!(s.target().unwrap().component(), Some(&other));
        assert_eq!(s.enter(), Step::GoTo(GoTo::Spec));
        let comment_action = s.actions().into_iter().find(|a| a.id == "spec.comment").unwrap();
        let (ctx, net, mut mode, mut note) = (egui::Context::default(), NetClient::new("http://127.0.0.1:9"), crate::keymap::InputMode::Normal, None);
        s.run(&comment_action, None, &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[], hints: &[] });
        assert_eq!(s.composer.as_ref().unwrap().anchor, Some(CommentAnchor::Relation { kind: RelationKind::Calls, source: id.clone(), target: other.clone() }));
        s.composer = None;
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
        assert_eq!(s.entry().unwrap().item, SpecItem::Comment(CommentId("c1".into())));
        s.motion(Motion::Last, 1);
        assert_eq!(s.entry().unwrap().section, Section::Findings);
        s.motion(Motion::NextSibling, 1);
        assert_eq!(s.current, Section::Findings, "no section after the last");
        s.motion(Motion::PrevSibling, 2);
        assert_eq!(s.current, Section::Code);
    }

    #[test]
    fn a_restored_item_is_found_once_the_component_loads() {
        let step = ComponentId::new("rust:a::step");
        let save = ComponentId::new("rust:a::save");
        let mut s = SpecState { pinned: true, ..Default::default() };
        s.show(save.clone());
        let relation = Entry { section: Section::Relations, item: SpecItem::Relation { kind: RelationKind::Calls, other: save.clone(), outgoing: true } }.key();
        s.restore(&Place::Spec { component: Some(step.clone()), item: Some(relation.clone()) });
        assert_eq!(s.cursor, 0, "not loaded yet");
        s.loaded = Some((step.clone(), Source::Code));
        push(&s.inbox, Reply::Detail(step.clone(), Box::new(Ok(detail(&step, &save)))));
        let (ctx, net) = (egui::Context::default(), NetClient::new("http://127.0.0.1:9"));
        s.tick(&net, &ctx);
        assert_eq!(s.cursor_key(), Some(relation), "on the relation once it's in");
        assert_eq!(s.current, Section::Relations);
    }

    #[test]
    fn spec_view_threads_comments_marks_changed_sections_and_composes_points() {
        use crate::review::tests::comment;
        use arch_graph::{Change, CommentStatus};
        let id = ComponentId::new("rust:a::step");
        let other = ComponentId::new("rust:a::run");
        let mut s = SpecState::default();
        s.follow(Some(&id), &Source::Plan(PlanId::new("p").unwrap()));
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
        s.comments = vec![
            comment(1, CommentAnchor::Component { id: id.clone() }, CommentStatus::Open),
            comment(2, CommentAnchor::Section { id: id.clone(), section: "invariants".into() }, CommentStatus::Answered),
            comment(3, CommentAnchor::Relation { kind: RelationKind::Calls, source: other.clone(), target: id.clone() }, CommentStatus::Resolved),
            comment(4, CommentAnchor::Component { id: other.clone() }, CommentStatus::Open),
        ];
        let ids = |v: Vec<&Comment>| v.into_iter().map(|c| c.id.0.clone()).collect::<Vec<_>>();
        assert_eq!(ids(s.comments_in(Some(Section::Overview))), vec!["c1"]);
        assert_eq!(ids(s.comments_in(Some(Section::Invariants))), vec!["c2"]);
        assert_eq!(ids(s.comments_in(Some(Section::Relations))), vec!["c3"]);
        assert_eq!(ids(s.open_points()), vec!["c1", "c2"], "resolved and other components' points aren't open here");
        for section in Section::ALL {
            assert_eq!(Section::from_key(section.key()), Some(section));
        }

        // ] c walks the open points and brings their section up.
        s.step_point(true);
        assert_eq!((s.point.as_ref().map(|p| p.0.as_str()), s.current), (Some("c1"), Section::Overview));
        s.step_point(true);
        assert_eq!((s.point.as_ref().map(|p| p.0.as_str()), s.current), (Some("c2"), Section::Invariants));
        s.step_point(true);
        assert_eq!(s.point.as_ref().map(|p| p.0.as_str()), Some("c1"), "wraps around");

        // Review actions exist with a plan and a loaded component; C on a section starts a change request in insert mode.
        let mut d = detail(&id, &other);
        d.component.spec = None;
        d.outgoing.clear();
        s.detail = Some(d);
        // Off the comments, accept and reopen act on the answered point.
        s.motion(Motion::First, 1);
        let actions = s.actions();
        for a in ["spec.comment", "spec.request_change", "spec.mark_reviewed", "spec.next_point", "spec.accept", "spec.reopen"] {
            assert!(actions.iter().any(|x| x.id == a), "{a}");
        }
        let ctx = egui::Context::default();
        let net = NetClient::new("http://127.0.0.1:9");
        let mut mode = crate::keymap::InputMode::Normal;
        s.step_section(2);
        assert_eq!(s.current, Section::Logic);
        let request = actions.iter().find(|a| a.id == "spec.request_change").unwrap().clone();
        let mut note = None;
        s.run(&request, None, &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 1, note: &mut note, shell: &[], hints: &[] });
        assert!(s.requests.insert);
        assert_eq!(
            s.composer,
            Some(Composer { purpose: Compose::New(CommentKind::ChangeRequest), anchor: Some(CommentAnchor::Section { id: id.clone(), section: "logic".into() }), section: Section::Logic, text: String::new() })
        );
        // It all renders: markers, threads with their buttons, the composer.
        s.loaded = Some((id.clone(), s.source.clone()));
        mode = crate::keymap::InputMode::Insert { slot: 1 };
        for _ in 0..2 {
            frame(&mut s, &ctx, &mut mode, 1);
        }
        assert!(s.composer.is_some(), "an empty composer isn't sent");
        s.composer.as_mut().unwrap().text = "Steps 2 and 3 are one step.".into();
        s.send_composer(&net, &ctx);
        assert!(s.composer.is_none(), "sent");
    }

    /// Every text a frame draws, one per line.
    fn drawn(s: &mut SpecState, ctx: &egui::Context) -> String {
        let (net, mut mode, mut note) = (NetClient::new("http://127.0.0.1:9"), crate::keymap::InputMode::Normal, None);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                s.render(ui, &mut RunCx { net: &net, ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[], hints: &[] });
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
        use crate::review::tests::comment;
        use arch_graph::{Change, CommentStatus, Finding, FindingCode, Target};
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        crate::status::set(config.status.clone());
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        let word = |name: &str| crate::status::look(name).1;
        let (id, other) = (ComponentId::new("rust:a::step"), ComponentId::new("rust:a::run"));
        let plan = PlanId::new("p").unwrap();
        let mut s = SpecState::default();
        s.follow(Some(&id), &Source::Plan(plan.clone()));
        let mut d = detail(&id, &other);
        d.component.description.clear();
        s.detail = Some(d);
        s.loaded = Some((id.clone(), Source::Plan(plan)));
        // The type wants a description and a signature; this one has neither.
        s.types = Some(arch_graph::TypeSystem::from_toml("[types.function]\nrequired_fields = [\"description\", \"signature\", \"logic_steps\"]").unwrap());
        assert_eq!(s.missing_required(), [arch_graph::RequiredField::Description, arch_graph::RequiredField::Signature]);
        s.comments = vec![comment(1, CommentAnchor::Component { id: id.clone() }, CommentStatus::Open)];
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
        for expected in ["step", "function", "rust:a::step", "src/a.rs (planned)", &word("planned"), &format!("1 {}", word("open_comments")), &word("changed")] {
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
        assert!(text.contains("spec.logic_steps") && !text.contains("spec.logic_steps.1"), "the review line names fields, not items");
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
}
