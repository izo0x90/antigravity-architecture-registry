//! The spec view: everything about one component, in full. It follows the
//! graph's selection (or stays pinned). A cursor walks every item in it
//! (section titles, relations, steps, invariants, findings, comments and
//! replies), so every control has a key.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use arch_graph::{Comment, CommentAnchor, CommentId, CommentKind, CommentStatus, ComponentDetail, ComponentId, Finding, PlanDiff, RelationKind, Report, Severity, Source};
use egui::{Rect, RichText};
use harness_contracts::changes::{Comparison, FileChanges, Hunk};

use crate::actions::{Action, ActionProvider, Choice, Group, MINOR, PRIMARY, Param, RunCx, SECONDARY, Style, When};
use crate::hosted::{Status, Tone};
use crate::nav::{Ask, Fold, GoTo, Item, Motion, Move, Navigable, Place, SpecItem, Spot, Step, Target};
use crate::net::{NetClient, NetResult};
use crate::theme::colors;
use crate::viewport::{Go, Measured, Show};

/// A file's changes, by the comparison and file they were asked for.
type ChangesReply = (Comparison, String, NetResult<FileChanges>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Overview,
    Signature,
    Logic,
    Invariants,
    Relations,
    Code,
    Changes,
    Placeholders,
    Findings,
}

impl Section {
    pub const ALL: [Section; 9] =
        [Section::Overview, Section::Signature, Section::Logic, Section::Invariants, Section::Relations, Section::Code, Section::Changes, Section::Placeholders, Section::Findings];

    /// The stable name comments use for it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Signature => "signature",
            Self::Logic => "logic",
            Self::Invariants => "invariants",
            Self::Relations => "relations",
            Self::Code => "code",
            Self::Changes => "changes",
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
            Self::Changes => "Changes",
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
    /// Where the comment box is, while it's typed in.
    typing: Option<Rect>,
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
            comments: Vec::new(),
            diff: PlanDiff::default(),
            composer: None,
            point: None,
            note: None,
            out_of_date: false,
            types: None,
            changes: Vec::new(),
            changes_asked: None,
            comparing: Default::default(),
            changes_inbox: Arc::default(),
            restore_item: None,
            spots: Vec::new(),
            typing: None,
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

    /// Moving to another component: the cursor, the review point and an
    /// unsent composer belonged to the old one.
    fn leave_component(&mut self) {
        self.changes.clear();
        self.changes_asked = None;
        self.cursor = 0;
        self.current = Section::Overview;
        self.point = None;
        self.composer = None;
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
        self.target = Some(id);
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
        self.asks.push(Ask::Type);
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
        crate::buffer::code_of(&self.detail.as_ref()?.component)
    }

    pub(crate) fn draw(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        self.render_composer(ui, cx);
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
                                    crate::changes::draw_hunk(ui, hunk, false);
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
                    for comment in self.comments_in(Some(section)) {
                        render_thread(ui, comment, self.point.as_ref(), section, &mut item);
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

    /// The review line under the toolbar: what changed since the review, and
    /// the composer when writing a comment, reply or reason to reopen.
    fn render_composer(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        self.typing = None;
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
        let inserting = *cx.input_mode == crate::keymap::InputMode::Insert { panel: cx.panel };
        let keys = crate::keymap::local();
        let output = ui.add(keys.text_box(&mut composer.text).id(egui::Id::new(("spec_composer", cx.panel))).desired_rows(2).desired_width(f32::INFINITY).hint_text("Write it here…"));
        if inserting {
            output.request_focus();
        }
        if output.clicked() || output.gained_focus() {
            *cx.input_mode = crate::keymap::InputMode::Insert { panel: cx.panel };
        }
        self.typing = inserting.then_some(output.rect);
        if inserting && output.has_focus() && ui.input(|i| crate::keymap::local().input.sent(i).is_some()) {
            self.send_composer(cx.net, cx.ctx);
            *cx.input_mode = crate::keymap::InputMode::Normal;
        }
    }
}

/// Draws one cursor stop (`Entry`), its contents drawn by the closure given.
type ItemDrawer<'a> = dyn FnMut(&mut egui::Ui, Entry, &mut dyn FnMut(&mut egui::Ui)) -> egui::Response + 'a;

/// One comment thread: the comment and its replies are cursor stops; its
/// buttons name their keys.
fn render_thread(ui: &mut egui::Ui, c: &Comment, point: Option<&CommentId>, section: Section, item: &mut ItemDrawer) {
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
    Send,
    CancelCompose,
    /// Read the component's code again (it changed since it was read).
    Reread,
    NextSection,
    PrevSection,
    /// Go to the section (its index in `Section::ALL`).
    Section,
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
        let mut out = self.moves();
        if self.composer.is_some() {
            out.push(f("spec.send", "Send what's written", S::Send).importance(PRIMARY).on_typing(Style::Button, "Send", Some(egui_phosphor::regular::PAPER_PLANE_RIGHT)));
            out.push(f("spec.cancel", "Throw away what's being written", S::CancelCompose).importance(PRIMARY).on_typing(Style::Button, "Cancel", None));
        }
        out.extend([
            f("spec.next_section", "Next section", S::NextSection).group(Group::Navigate),
            f("spec.prev_section", "Previous section", S::PrevSection).group(Group::Navigate).importance(MINOR),
        ]);
        if !self.floating {
            out.push(
                f("spec.pin", if self.pinned { "Unpin (follow what you're on)" } else { "Pin this component" }, S::Pin)
                    .header(Style::Button, if self.pinned { "Pinned" } else { "Pin" }, Some(egui_phosphor::regular::PUSH_PIN))
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
                    .header(Style::Button, "Reviewed", Some(egui_phosphor::regular::CHECK_SQUARE))
                    .importance(MINOR),
            );
            // Always there: with no open points it says so.
            let points = if self.open_points().is_empty() { When::Unavailable } else { When::Always };
            out.push(f("spec.next_point", "Next open review point", S::Point { forward: true }).group(Group::Navigate).importance(if on_comment { SECONDARY } else { PRIMARY }).when(points).jump());
            out.push(f("spec.prev_point", "Previous open review point", S::Point { forward: false }).group(Group::Navigate).importance(MINOR).when(points).jump());
            // On an open point, its controls are under it.
            let on_open = self.comment_here().is_some_and(|c| !c.is_addressed());
            let under = |a: Action, caption: &str| if on_open { a.on_item(Style::Button, caption, None) } else { a };
            if self.reply_point().is_some() {
                out.push(under(f("spec.reply", "Reply to this point", S::Reply).importance(if on_comment { PRIMARY } else { SECONDARY }), "Reply"));
            }
            if self.answered_point().is_some() {
                out.push(under(f("spec.accept", "Accept the answer to this point", S::Accept).importance(PRIMARY), "Accept"));
                out.push(under(f("spec.reopen", "Reopen this point (say why)", S::Reopen).importance(SECONDARY), "Reopen"));
            }
        }
        out.push(
            f("spec.section", "Go to a section", S::Section)
                .param(Param::Choice(Section::ALL.iter().enumerate().map(|(i, s)| Choice::new(i.to_string(), s.title()).current(*s == self.current)).collect()))
                .header(Style::Setting, "Section", None)
                .group(Group::Navigate)
                .importance(MINOR),
        );
        if self.out_of_date && self.detail.as_ref().is_some_and(|d| d.code.is_some()) {
            out.push(f("spec.reread", "Read this code again (it changed since)", S::Reread).header(Style::Button, "Re-read", None).importance(MINOR));
        }
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
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
            Some(SpecCommand::CancelCompose) => {
                self.composer = None;
                *cx.input_mode = crate::keymap::InputMode::Normal;
            }
            Some(SpecCommand::Send) => {
                self.send_composer(cx.net, cx.ctx);
                *cx.input_mode = crate::keymap::InputMode::Normal;
            }
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

impl Navigable for SpecState {
    /// Any component, but the one whose card it shows (a relation's other
    /// end is another).
    fn can_show(&self, target: &Target) -> bool {
        target.in_component() && target.component() != self.target.as_ref()
    }

    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        let id = target.component().ok_or("Not a component.")?;
        self.land_on(id.clone());
        Ok(())
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
            self.comments.clone_from(&review.comments);
            self.diff.clone_from(&review.diff);
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
        Place::Spec { component: self.target.clone(), item: self.entry().map(|e| e.key()) }
    }

    /// Back to the component and the item.
    fn restore(&mut self, place: &Place) {
        if let Place::Spec { component, item } = place {
            if let Some(id) = component {
                self.land_on(id.clone());
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
        // A change: its place in the file.
        if let (SpecItem::Hunk(i), Some((file, ..))) = (&entry.item, self.code_lines())
            && let Some(hunk) = self.changes.get(*i)
        {
            return Some(Target::Change { file, line: hunk.new_start.max(1) as usize });
        }
        Some(Target::Spec { component, section: entry.section, item: entry.item })
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
            SpecItem::Comment(id) | SpecItem::Reply(id, _) => {
                self.point = Some(id.clone());
                self.start_composing(Compose::Reply(id));
                Step::Done
            }
            SpecItem::Placeholder(_) => Step::GoTo(GoTo::Code),
            SpecItem::Step(_) | SpecItem::Invariant(_) | SpecItem::Finding(_) => Step::Nothing("c comments on it, C asks for a change.".into()),
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

    /// Changed code or plans: its component read again.
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
        let card = self.target.as_ref().map_or_else(|| "following what you're on".to_owned(), |id| match &self.detail {
            Some(d) if d.component.id == *id => d.component.name.clone(),
            _ => crate::nav::short_name(id),
        });
        if self.pinned { format!("{card} (pinned)") } else { card }
    }
}

impl crate::hosted::Hosted for SpecState {
    /// The component it shows.
    fn title(&self) -> Status {
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

    fn typing_rect(&self) -> Option<Rect> {
        self.typing
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
                s.draw(ui, &mut RunCx { net: &net, ctx, input_mode: mode, panel: slot, note: &mut note, asked: &mut None });
            });
        });
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
        s.follow_source(&Source::Plan(PlanId::new("p").unwrap()));
        s.follow(&id);
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
        assert_eq!(s.take_asks(), [Ask::Type], "the composer takes the keyboard");
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
        s.run(&comment_action, None, &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, panel: 0, note: &mut note, asked: &mut None });
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
        assert_eq!(s.current, Section::Changes);
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
        s.run(&request, None, &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, panel: 1, note: &mut note, asked: &mut None });
        assert_eq!(s.take_asks(), [Ask::Type]);
        assert_eq!(
            s.composer,
            Some(Composer { purpose: Compose::New(CommentKind::ChangeRequest), anchor: Some(CommentAnchor::Section { id: id.clone(), section: "logic".into() }), section: Section::Logic, text: String::new() })
        );
        // It all renders: markers, threads with their buttons, the composer.
        s.loaded = Some((id.clone(), s.source.clone()));
        mode = crate::keymap::InputMode::Insert { panel: 1 };
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
                s.draw(ui, &mut RunCx { net: &net, ctx, input_mode: &mut mode, panel: 0, note: &mut note, asked: &mut None });
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
        s.follow_source(&Source::Plan(plan.clone()));
        s.follow(&id);
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
        let relation = Target::Spec { component: a.clone(), section: Section::Relations, item: SpecItem::Relation { kind: RelationKind::Calls, other: b.clone(), outgoing: true } };
        assert!(s.can_show(&relation), "a relation's other end is another card");
        assert_eq!(s.show_target(&Target::Component(b.clone())), Ok(()));
        assert_eq!(s.target.as_ref(), Some(&b));
    }

}
