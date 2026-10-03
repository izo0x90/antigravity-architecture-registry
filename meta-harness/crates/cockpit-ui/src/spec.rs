//! The spec view: everything about one component, in full. It follows the
//! graph's selection (or stays pinned), and is navigable by section.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use arch_graph::{Comment, CommentAnchor, CommentId, CommentKind, CommentStatus, ComponentDetail, ComponentId, Finding, PlanDiff, PlanId, RelationKind, Report, Severity};
use egui::RichText;

use crate::actions::{Action, ActionProvider, RunCx, ToolbarStyle, ToolbarUse, toolbar_control};
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

    /// Where a field changed since the last review shows (diff field paths).
    pub fn of_field(field: &str) -> Section {
        match field {
            "signature" => Self::Signature,
            f if f.starts_with("schemas") => Self::Signature,
            "spec.logic_steps" | "spec.tasks" => Self::Logic,
            "spec.invariants" => Self::Invariants,
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
}

pub struct SpecState {
    /// What it shows: the graph selection unless pinned.
    pub target: Option<ComponentId>,
    pub plan: Option<PlanId>,
    pub pinned: bool,
    pub detail: Option<ComponentDetail>,
    pub findings: Vec<Finding>,
    pub current: Section,
    pub folded: BTreeSet<Section>,
    scroll_to: Option<Section>,
    loaded: Option<(ComponentId, Option<PlanId>)>,
    error: Option<String>,
    pub requests: SpecRequests,
    /// The cockpit user (author of comments and reviews).
    pub author: String,
    /// The plan's comments (all of them) and its diff since the last review.
    pub comments: Vec<Comment>,
    pub diff: PlanDiff,
    pub composer: Option<Composer>,
    /// The review point under the cursor (`] c` cycles them).
    pub point: Option<CommentId>,
    note: Option<String>,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

impl Default for SpecState {
    fn default() -> Self {
        Self {
            target: None,
            plan: None,
            pinned: false,
            detail: None,
            findings: Vec::new(),
            current: Section::Overview,
            folded: BTreeSet::new(),
            scroll_to: None,
            loaded: None,
            error: None,
            requests: SpecRequests::default(),
            author: String::new(),
            comments: Vec::new(),
            diff: PlanDiff::default(),
            composer: None,
            point: None,
            note: None,
            inbox: Arc::default(),
        }
    }
}

impl SpecState {
    /// Follows the graph (unless pinned).
    pub fn follow(&mut self, selection: Option<&ComponentId>, plan: Option<&PlanId>) {
        if !self.pinned {
            self.target = selection.cloned();
            self.plan = plan.cloned();
        }
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
                    }
                    Err(e) => self.error = Some(e),
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
        let key = (target.clone(), self.plan.clone());
        if self.loaded.as_ref() == Some(&key) {
            return;
        }
        self.loaded = Some(key);
        self.findings.clear();
        let inbox = self.inbox.clone();
        let id = target.clone();
        net.arch_component(&target, self.plan.as_ref(), ctx.clone(), move |r| push(&inbox, Reply::Detail(id, Box::new(r))));
        if let Some(plan) = &self.plan {
            let inbox = self.inbox.clone();
            net.arch_validate(plan, ctx.clone(), move |r| push(&inbox, Reply::Report(target, r)));
        }
    }

    pub fn step_section(&mut self, delta: i32) {
        let at = Section::ALL.iter().position(|s| *s == self.current).unwrap_or(0) as i32;
        let next = (at + delta).clamp(0, Section::ALL.len() as i32 - 1) as usize;
        self.current = Section::ALL[next];
        self.scroll_to = Some(self.current);
    }

    pub fn toggle_fold(&mut self) {
        if !self.folded.remove(&self.current) {
            self.folded.insert(self.current);
        }
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

    /// Moves to the next unaddressed point (and its section).
    pub fn next_point(&mut self) {
        let points: Vec<(CommentId, Option<Section>)> = {
            let target = self.target.clone();
            self.open_points().into_iter().map(|c| (c.id.clone(), target.as_ref().and_then(|t| section_of(&c.anchor, t)))).collect()
        };
        if points.is_empty() {
            self.point = None;
            return;
        }
        let at = self.point.as_ref().and_then(|p| points.iter().position(|(id, _)| id == p)).map_or(0, |i| (i + 1) % points.len());
        let (id, section) = points[at].clone();
        self.point = Some(id);
        if let Some(section) = section {
            self.current = section;
            self.folded.remove(&section);
            self.scroll_to = Some(section);
        }
    }

    fn start_composing(&mut self, purpose: Compose, cx: &mut RunCx<'_>) {
        self.composer = Some(Composer { purpose, section: self.current, text: String::new() });
        *cx.input_mode = crate::keymap::InputMode::Insert { slot: cx.slot };
    }

    fn posted<T: Send + 'static>(&self) -> impl FnOnce(NetResult<T>) + Send + 'static {
        let inbox = self.inbox.clone();
        move |r: NetResult<T>| push(&inbox, Reply::Posted(r.map(|_| ())))
    }

    /// Sends what the composer holds.
    pub fn send_composer(&mut self, net: &NetClient, ctx: &egui::Context) {
        let (Some(composer), Some(plan), Some(target)) = (self.composer.clone(), self.plan.clone(), self.target.clone()) else { return };
        let text = composer.text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        match composer.purpose {
            Compose::New(kind) => {
                let anchor = match composer.section {
                    Section::Overview => CommentAnchor::Component { id: target },
                    section => CommentAnchor::Section { id: target, section: section.key().to_owned() },
                };
                let body = serde_json::json!({ "anchor": anchor, "kind": kind, "text": text, "by": self.author });
                net.arch_add_comment(&plan, body, ctx.clone(), self.posted());
            }
            Compose::Reply(id) => net.arch_comment_action(&plan, &id, "replies", serde_json::json!({ "text": text, "by": self.author }), ctx.clone(), self.posted()),
            Compose::Reopen(id) => net.arch_comment_action(&plan, &id, "reopen", serde_json::json!({ "text": text, "by": self.author }), ctx.clone(), self.posted()),
        }
        self.composer = None;
    }

    fn accept(&mut self, id: &CommentId, net: &NetClient, ctx: &egui::Context) {
        if let Some(plan) = &self.plan {
            net.arch_comment_action(plan, id, "accept", serde_json::json!({ "by": self.author }), ctx.clone(), self.posted());
        }
    }

    /// The answered point under the cursor, or else the first answered one
    /// (what accept and reopen act on).
    fn current_point(&self) -> Option<CommentId> {
        let answered: Vec<&Comment> = self.open_points().into_iter().filter(|c| c.status == CommentStatus::Answered).collect();
        self.point.clone().filter(|p| answered.iter().any(|c| &c.id == p)).or_else(|| answered.first().map(|c| c.id.clone()))
    }

    fn code_location(&self) -> Option<(String, u32, u32)> {
        let c = &self.detail.as_ref()?.component;
        match (&c.anchor, &c.proposed) {
            (Some(a), _) => Some((a.file.to_string(), a.span.start_line, a.span.end_line)),
            (None, Some(p)) => Some((p.file.to_string(), 1, 1)),
            (None, None) => None,
        }
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
                let label = RichText::new(section.title()).size(11.0);
                if ui.selectable_label(self.current == section, label).clicked() {
                    self.current = section;
                    self.scroll_to = Some(section);
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
            return;
        };
        let c = &detail.component;
        let changed = self.changed_sections();
        let mut clicked: Option<(CommentId, Option<Compose>)> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for section in Section::ALL {
                let open = !self.folded.contains(&section);
                let mut title = section.title().to_owned();
                if changed.contains(&section) {
                    title.push_str("  ● changed");
                }
                let open_here = self.comments_in(Some(section)).iter().filter(|c| !c.is_addressed()).count();
                if open_here > 0 {
                    title.push_str(&format!("  · {open_here} open"));
                }
                let header = RichText::new(title).strong().size(13.0).color(if self.current == section {
                    colors().focus
                } else if changed.contains(&section) {
                    colors().info
                } else {
                    colors().text_strong
                });
                let threads: Vec<Comment> = self.comments_in(Some(section)).into_iter().cloned().collect();
                let point = self.point.clone();
                let response = egui::CollapsingHeader::new(header).id_salt(("spec", section as u8)).open(Some(open)).show(ui, |ui| {
                    match section {
                        Section::Overview => {
                            ui.label(RichText::new(&c.name).size(16.0).strong());
                            ui.label(RichText::new(c.id.as_str()).monospace().size(11.0).color(colors().muted));
                            ui.label(format!("{} · {:?} · {:?} · {:?}", c.type_name, c.status, c.stage, detail.alignment));
                            match (&c.anchor, &c.proposed) {
                                (Some(a), _) => ui.label(RichText::new(format!("{} ({}:{}–{})", a.symbol, a.file, a.span.start_line, a.span.end_line)).monospace()),
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
                                ui.label(RichText::new(format!("→ {}", sig.returns.as_deref().unwrap_or("()"))).monospace());
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
                            for step in steps {
                                ui.label(RichText::new(format!("{}. {}", step.sequence, step.name)).strong());
                                if !step.description.is_empty() {
                                    ui.label(&step.description);
                                }
                                ui.add_space(4.0);
                            }
                        }
                        Section::Invariants => {
                            let invariants = c.spec.as_ref().map(|s| s.invariants.clone()).unwrap_or_default();
                            if invariants.is_empty() {
                                ui.label(RichText::new("No invariants declared.").color(colors().muted));
                            }
                            for inv in invariants {
                                ui.label(RichText::new(format!("{} ({})", inv.name, inv.kind)).strong());
                                if !inv.description.is_empty() {
                                    ui.label(&inv.description);
                                }
                            }
                        }
                        Section::Relations => {
                            for kind in RelationKind::ALL {
                                for (label, list, outgoing) in [(format!("{} →", kind.name()), &detail.outgoing, true), (format!("← {}", kind.name()), &detail.incoming, false)] {
                                    let items: Vec<&ComponentId> = list.iter().filter(|r| r.kind == kind).map(|r| if outgoing { &r.target } else { &r.source }).collect();
                                    if items.is_empty() {
                                        continue;
                                    }
                                    ui.label(RichText::new(label).size(11.0).color(colors().muted));
                                    for id in items {
                                        if ui.link(RichText::new(id.as_str()).monospace().size(11.0)).clicked() {
                                            self.requests.select = Some(id.clone());
                                        }
                                    }
                                }
                            }
                        }
                        Section::Code => match &detail.code {
                            Some(code) => {
                                egui::Frame::NONE.fill(colors().bg).inner_margin(6).corner_radius(4).show(ui, |ui| {
                                    ui.label(RichText::new(code).monospace().size(11.0));
                                });
                                if ui.button("Open in the code view").clicked() {
                                    self.requests.open_code = self.code_location();
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
                            for span in &c.placeholders {
                                ui.label(RichText::new(format!("placeholder body at line {}", span.start_line)).color(colors().warn));
                            }
                        }
                        Section::Findings => {
                            if self.plan.is_none() {
                                ui.label(RichText::new("Pick a plan in the graph to see its checks.").color(colors().muted));
                            } else if self.findings.is_empty() {
                                ui.label(RichText::new("No findings.").color(colors().ok));
                            }
                            for f in &self.findings {
                                let color = match f.severity {
                                    Severity::Error => colors().error,
                                    Severity::Warning => colors().warn,
                                    Severity::Info => colors().muted,
                                };
                                ui.label(RichText::new(&f.message).color(color));
                                if let Some(s) = &f.suggestion {
                                    ui.label(RichText::new(s).size(11.0).color(colors().muted));
                                }
                            }
                        }
                    }
                    if let Some(action) = render_threads(ui, &threads, point.as_ref()) {
                        clicked = Some(action);
                    }
                });
                if self.scroll_to == Some(section) {
                    response.header_response.scroll_to_me(Some(egui::Align::TOP));
                }
                if response.header_response.clicked() {
                    self.current = section;
                    self.toggle_fold();
                }
            }
        });
        self.scroll_to = None;
        match clicked {
            Some((id, None)) => self.accept(&id, cx.net, cx.ctx),
            Some((id, Some(purpose))) => {
                self.point = Some(id);
                self.start_composing(purpose, cx);
            }
            None => {}
        }
    }

    /// The review line under the toolbar: what changed since the review, and
    /// the composer when writing a comment, reply or reason to reopen.
    fn render_review_bar(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        if let Some(id) = &self.target {
            let fields = self.diff.fields(id);
            if !fields.is_empty() {
                let since = self.diff.since.as_deref().map_or_else(|| "new in this plan (never reviewed)".to_owned(), |at| format!("changed since the review at {at}"));
                ui.label(RichText::new(format!("● {since}: {}", fields.into_iter().collect::<Vec<_>>().join(", "))).size(11.0).color(colors().info));
            }
        }
        if let Some(note) = &self.note {
            ui.label(RichText::new(note).size(11.0).color(colors().error));
        }
        let Some(composer) = self.composer.as_mut() else { return };
        let what = match &composer.purpose {
            Compose::New(CommentKind::Comment) => format!("Comment on {}", composer.section.title()),
            Compose::New(CommentKind::ChangeRequest) => format!("Change request on {}", composer.section.title()),
            Compose::Reply(id) => format!("Reply to {id}"),
            Compose::Reopen(id) => format!("Why reopen {id}?"),
        };
        ui.label(RichText::new(what).size(11.0).color(colors().muted));
        let inserting = *cx.input_mode == crate::keymap::InputMode::Insert { slot: cx.slot };
        let output = ui.add(egui::TextEdit::multiline(&mut composer.text).id(egui::Id::new(("spec_composer", cx.slot))).desired_rows(2).desired_width(f32::INFINITY).hint_text("Enter sends, Shift+Enter for a new line"));
        if inserting {
            output.request_focus();
        }
        if output.clicked() || output.gained_focus() {
            *cx.input_mode = crate::keymap::InputMode::Insert { slot: cx.slot };
        }
        let enter = inserting && output.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        let (mut send, mut cancel) = (enter, false);
        ui.horizontal(|ui| {
            send |= ui.button("Send").clicked();
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

/// The comment threads in one section. Returns a click: accept (`None`) or
/// start writing (`Some(purpose)`) for a comment.
fn render_threads(ui: &mut egui::Ui, threads: &[Comment], point: Option<&CommentId>) -> Option<(CommentId, Option<Compose>)> {
    let mut clicked = None;
    for c in threads {
        let (label, color) = match c.status {
            CommentStatus::Open => ("OPEN", colors().warn),
            CommentStatus::Answered => ("ANSWERED", colors().info),
            CommentStatus::Resolved => ("RESOLVED", colors().ok),
        };
        let stroke = if point == Some(&c.id) { colors().focus } else { colors().border };
        egui::Frame::NONE.fill(colors().surface).stroke(egui::Stroke::new(1.0, stroke)).corner_radius(4).inner_margin(6).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(label).size(10.0).strong().color(color));
                if c.kind == CommentKind::ChangeRequest {
                    ui.label(RichText::new("change request").size(10.0).color(colors().special));
                }
                ui.label(RichText::new(format!("{} · {}", c.id, c.author)).size(10.0).color(colors().muted));
            });
            ui.label(&c.text);
            for r in &c.replies {
                ui.label(RichText::new(format!("{}: {}", r.author, r.text)).size(11.0).color(colors().text));
            }
            if let Some(a) = &c.answer {
                ui.label(RichText::new(format!("↳ {}: {}", a.author, a.text)).size(11.0).color(colors().info));
                for ch in &a.changes {
                    ui.label(RichText::new(format!("   {}", ch.describe())).size(10.0).monospace().color(colors().muted));
                }
            }
            if !c.is_addressed() {
                ui.horizontal(|ui| {
                    if c.status == CommentStatus::Answered {
                        if ui.small_button("Accept").clicked() {
                            clicked = Some((c.id.clone(), None));
                        }
                        if ui.small_button("Reopen").clicked() {
                            clicked = Some((c.id.clone(), Some(Compose::Reopen(c.id.clone()))));
                        }
                    }
                    if ui.small_button("Reply").clicked() {
                        clicked = Some((c.id.clone(), Some(Compose::Reply(c.id.clone()))));
                    }
                });
            }
        });
    }
    clicked
}

fn push(inbox: &Arc<Mutex<Vec<Reply>>>, reply: Reply) {
    if let Ok(mut i) = inbox.lock() {
        i.push(reply);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SpecCommand {
    NextSection,
    PrevSection,
    Fold,
    Pin,
    OpenCode,
    ShowInGraph,
    Comment,
    RequestChange,
    MarkReviewed,
    NextPoint,
    Accept,
    Reopen,
}

impl ActionProvider for SpecState {
    fn actions(&self) -> Vec<Action> {
        use SpecCommand as S;
        let f = |id: &str, label: &str, c: SpecCommand| Action::new(id, label, c).focused();
        let mut out = vec![
            f("spec.next_section", "Next section", S::NextSection),
            f("spec.prev_section", "Previous section", S::PrevSection),
            f("spec.fold", "Fold or unfold this section", S::Fold),
            f("spec.pin", if self.pinned { "Unpin (follow the graph)" } else { "Pin this component" }, S::Pin).toolbar(
                ToolbarStyle::Button,
                if self.pinned { "Pinned" } else { "Pin" },
                Some(egui_phosphor::regular::PUSH_PIN),
            ),
        ];
        if self.detail.is_some() {
            out.push(f("spec.open_code", "Open in code", S::OpenCode));
            out.push(f("spec.show_in_graph", "Show in the graph", S::ShowInGraph));
        }
        if self.plan.is_some() {
            out.push(f("spec.mark_reviewed", "Mark the plan reviewed (later changes are highlighted)", S::MarkReviewed).toolbar(
                ToolbarStyle::Button,
                "Reviewed",
                Some(egui_phosphor::regular::CHECK_SQUARE),
            ));
            if self.detail.is_some() {
                out.push(f("spec.comment", "Comment on this section", S::Comment));
                out.push(f("spec.request_change", "Ask for a change to this section", S::RequestChange));
            }
            if !self.open_points().is_empty() {
                out.push(f("spec.next_point", "Next open review point", S::NextPoint));
                out.push(f("spec.reopen", "Reopen the answered point", S::Reopen));
                out.push(f("spec.accept", "Accept the answer to this point", S::Accept));
            }
        }
        out
    }

    fn run(&mut self, action: &Action, _value: Option<String>, cx: &mut RunCx<'_>) {
        match action.command::<SpecCommand>() {
            Some(SpecCommand::Comment) => self.start_composing(Compose::New(CommentKind::Comment), cx),
            Some(SpecCommand::RequestChange) => self.start_composing(Compose::New(CommentKind::ChangeRequest), cx),
            Some(SpecCommand::MarkReviewed) => {
                if let Some(plan) = self.plan.clone() {
                    cx.net.arch_mark_reviewed(&plan, &self.author, cx.ctx.clone(), self.posted());
                }
            }
            Some(SpecCommand::NextPoint) => self.next_point(),
            Some(SpecCommand::Accept) => {
                if let Some(id) = self.current_point() {
                    self.accept(&id, cx.net, cx.ctx);
                }
            }
            Some(SpecCommand::Reopen) => {
                if let Some(id) = self.current_point() {
                    self.point = Some(id.clone());
                    self.start_composing(Compose::Reopen(id), cx);
                }
            }
            Some(SpecCommand::NextSection) => self.step_section(1),
            Some(SpecCommand::PrevSection) => self.step_section(-1),
            Some(SpecCommand::Fold) => self.toggle_fold(),
            Some(SpecCommand::Pin) => self.pinned = !self.pinned,
            Some(SpecCommand::OpenCode) => self.requests.open_code = self.code_location(),
            Some(SpecCommand::ShowInGraph) => self.requests.select = self.target.clone(),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arch_graph::{Alignment, Component, ImplementationSpec, Invariant, LogicStep, Stage, Status};

    // req: P9.16
    #[test]
    fn spec_view_follows_unless_pinned_and_navigates_sections() {
        let mut s = SpecState::default();
        let a = ComponentId::new("rust:a::run");
        let b = ComponentId::new("rust:a::step");
        s.follow(Some(&a), None);
        assert_eq!(s.target.as_ref(), Some(&a));
        s.pinned = true;
        s.follow(Some(&b), None);
        assert_eq!(s.target.as_ref(), Some(&a), "pinned");
        s.pinned = false;
        s.follow(Some(&b), Some(&PlanId::new("p").unwrap()));
        assert_eq!((s.target.as_ref(), s.plan.is_some()), (Some(&b), true));

        s.step_section(1);
        s.step_section(1);
        assert_eq!(s.current, Section::Logic);
        s.toggle_fold();
        assert!(s.folded.contains(&Section::Logic));
        s.toggle_fold();
        assert!(!s.folded.contains(&Section::Logic));
        s.step_section(-10);
        assert_eq!(s.current, Section::Overview, "clamped");
        s.step_section(50);
        assert_eq!(s.current, Section::Findings);

        // Full content renders: every section, with steps, invariants and code.
        s.detail = Some(ComponentDetail {
            component: Component {
                id: b.clone(),
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
                    logic_steps: vec![LogicStep { sequence: 1, name: "read".into(), description: "Read the input.".into(), algorithm: None, complexity: None, check: None }],
                    invariants: vec![Invariant { name: "pure".into(), kind: arch_graph::InvariantKind::PostCondition, description: "No I/O.".into() }],
                    ..Default::default()
                }),
                placeholders: vec![],
            },
            alignment: Alignment::Planned,
            outgoing: vec![],
            incoming: vec![],
            code: None,
        });
        let ctx = egui::Context::default();
        let net = NetClient::new("http://127.0.0.1:9");
        s.loaded = Some((b.clone(), s.plan.clone()));
        let mut mode = crate::keymap::InputMode::Normal;
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    s.render(ui, &mut RunCx { net: &net, ctx, input_mode: &mut mode, slot: 0, hint_chars: "asdf" });
                });
            });
        }
        assert_eq!(s.code_location(), Some(("src/a.rs".into(), 1, 1)), "planned components open their proposed file");
        let actions = s.actions();
        for id in ["spec.next_section", "spec.prev_section", "spec.fold", "spec.pin", "spec.open_code", "spec.show_in_graph"] {
            assert!(actions.iter().any(|a| a.id == id), "{id}");
        }
    }

    // req: R.1, R.2
    #[test]
    fn spec_view_threads_comments_marks_changed_sections_and_composes_points() {
        use crate::review::tests::comment;
        use arch_graph::{Change, CommentStatus};
        let id = ComponentId::new("rust:a::step");
        let other = ComponentId::new("rust:a::run");
        let mut s = SpecState::default();
        s.follow(Some(&id), Some(&PlanId::new("p").unwrap()));
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
        s.next_point();
        assert_eq!((s.point.as_ref().map(|p| p.0.as_str()), s.current), (Some("c1"), Section::Overview));
        s.next_point();
        assert_eq!((s.point.as_ref().map(|p| p.0.as_str()), s.current), (Some("c2"), Section::Invariants));
        s.next_point();
        assert_eq!(s.point.as_ref().map(|p| p.0.as_str()), Some("c1"), "wraps around");

        // Review actions exist with a plan and a loaded component; c starts a comment on the section in insert mode.
        s.detail = Some(ComponentDetail {
            component: Component {
                id: id.clone(),
                name: "step".into(),
                type_name: "function".into(),
                description: String::new(),
                status: Status::New,
                stage: Stage::Declared,
                anchor: None,
                proposed: None,
                signature: None,
                schemas: Default::default(),
                spec: None,
                placeholders: vec![],
            },
            alignment: Alignment::Planned,
            outgoing: vec![],
            incoming: vec![],
            code: None,
        });
        let actions = s.actions();
        for a in ["spec.comment", "spec.request_change", "spec.mark_reviewed", "spec.next_point", "spec.accept", "spec.reopen"] {
            assert!(actions.iter().any(|x| x.id == a), "{a}");
        }
        let ctx = egui::Context::default();
        let net = NetClient::new("http://127.0.0.1:9");
        let mut mode = crate::keymap::InputMode::Normal;
        s.current = Section::Logic;
        let request = actions.iter().find(|a| a.id == "spec.request_change").unwrap().clone();
        s.run(&request, None, &mut RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 1, hint_chars: "asdf" });
        assert_eq!(mode, crate::keymap::InputMode::Insert { slot: 1 });
        assert_eq!(s.composer, Some(Composer { purpose: Compose::New(CommentKind::ChangeRequest), section: Section::Logic, text: String::new() }));
        // It all renders: markers, threads with their buttons, the composer.
        s.loaded = Some((id.clone(), s.plan.clone()));
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    s.render(ui, &mut RunCx { net: &net, ctx, input_mode: &mut mode, slot: 1, hint_chars: "asdf" });
                });
            });
        }
        assert!(s.composer.is_some(), "an empty composer isn't sent");
        s.composer.as_mut().unwrap().text = "Steps 2 and 3 are one step.".into();
        s.send_composer(&net, &ctx);
        assert!(s.composer.is_none(), "sent");
    }
}
