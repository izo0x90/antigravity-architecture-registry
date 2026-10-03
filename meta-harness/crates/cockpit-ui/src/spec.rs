//! The spec view: everything about one component, in full. It follows the
//! graph's selection (or stays pinned), and is navigable by section.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use arch_graph::{ComponentDetail, ComponentId, Finding, PlanId, RelationKind, Report, Severity};
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

enum Reply {
    Detail(ComponentId, Box<NetResult<ComponentDetail>>),
    Report(ComponentId, NetResult<Report>),
}

#[derive(Default)]
pub struct SpecRequests {
    /// Select this component in the graph.
    pub select: Option<ComponentId>,
    /// Open this file at these lines.
    pub open_code: Option<(String, u32, u32)>,
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
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for section in Section::ALL {
                let open = !self.folded.contains(&section);
                let header = RichText::new(section.title()).strong().size(13.0).color(if self.current == section { colors().focus } else { colors().text_strong });
                let response = egui::CollapsingHeader::new(header).id_salt(("spec", section as u8)).open(Some(open)).show(ui, |ui| match section {
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
    }
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
        out
    }

    fn run(&mut self, action: &Action, _value: Option<String>, _cx: &mut RunCx<'_>) {
        match action.command::<SpecCommand>() {
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
}
