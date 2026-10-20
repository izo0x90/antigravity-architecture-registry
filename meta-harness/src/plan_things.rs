//! A plan's parts as things notes are on: the plan, a component's card, a
//! section of it, a relation, an item. Named as `Subject::name` names them
//! (`plan quick-look component rust:app::open section logic`); what each
//! says is read from the plan as it is now (plans last, as files in
//! `.nah/plans`: nothing of them is logged); one is there while its plan
//! and its part are.

use std::sync::Arc;

use arch_graph::{Component, ComponentId, ItemId, Plan, PlanId, RelationKind, Source};
use arch_service::ArchitectureService;
use harness_contracts::{Known, Subject};

pub struct PlanParts {
    arch: Arc<ArchitectureService>,
}

impl PlanParts {
    pub fn new(arch: Arc<ArchitectureService>) -> Arc<Self> {
        Arc::new(Self { arch })
    }

    fn plan(&self, id: &PlanId) -> Option<Plan> {
        self.arch.plan(id).ok()
    }

    /// The component's code, where the code has it now (read as the
    /// project's files are, `code_tags::project_lines`).
    fn code(&self, id: &ComponentId) -> Option<String> {
        let graph = self.arch.graph(&Source::Code).ok()?;
        let anchor = graph.component(id)?.anchor.clone()?;
        crate::code_tags::project_lines(&self.arch, anchor.file.as_str(), anchor.span.start_line, anchor.span.end_line)
    }

    /// Section `section` of `c`'s card in `plan`, in words (none: there's
    /// no such section).
    fn section(&self, plan: &Plan, c: &Component, section: &str) -> Option<String> {
        Some(match section {
            "overview" => overview(c),
            "signature" => signature(c),
            "logic" => logic(c),
            "invariants" => invariants(c),
            "relations" => plan.relations.iter().filter(|r| r.source == c.id || r.target == c.id).map(|r| format!("{} {} {}", r.source, r.kind.name(), r.target)).collect::<Vec<_>>().join("\n"),
            "code" => self.code(&c.id).unwrap_or_default(),
            "changes" => arch_graph::review::diff(plan.last_review.as_ref(), plan).fields(&c.id).into_iter().collect::<Vec<_>>().join("\n"),
            "placeholders" => c.placeholders.iter().map(|s| format!("lines {}-{}", s.start_line, s.end_line)).collect::<Vec<_>>().join("\n"),
            "findings" => {
                let report = self.arch.validate(&plan.id).ok()?;
                report.findings.iter().filter(|f| matches!(&f.target, arch_graph::Target::Component { id } if *id == c.id)).map(|f| format!("{:?}: {}", f.severity, f.message)).collect::<Vec<_>>().join("\n")
            }
            _ => return None,
        })
    }
}

/// What a component is, and its description.
fn overview(c: &Component) -> String {
    let file = c.file().map(|f| format!(", in {f}")).unwrap_or_default();
    format!("{} {} ({:?}, {:?}{file})\n{}", c.type_name, c.name, c.status, c.stage, c.description).trim_end().to_owned()
}

/// How it's called, and what goes in and out.
fn signature(c: &Component) -> String {
    let mut out = Vec::new();
    if let Some(s) = &c.signature {
        let params: Vec<String> = s.params.iter().map(|p| match &p.ty { Some(ty) => format!("{}: {ty}", p.name), None => p.name.clone() }).collect();
        let returns = s.returns.as_ref().map(|r| format!(" -> {r}")).unwrap_or_default();
        out.push(format!("{}{}({}){returns}", s.qualifiers.iter().map(|q| format!("{q} ")).collect::<String>(), c.name, params.join(", ")));
    }
    for (what, schema) in [("inputs", &c.schemas.inputs), ("outputs", &c.schemas.outputs), ("properties", &c.schemas.properties)] {
        if let Some(schema) = schema {
            out.push(format!("{what}: {}", serde_json::to_string(schema).unwrap_or_default()));
        }
    }
    out.extend(c.schemas.side_effects.iter().map(|e| format!("side effect: {}", serde_json::to_string(e).unwrap_or_default())));
    out.join("\n")
}

/// Its logic: the pattern, then each step.
fn logic(c: &Component) -> String {
    let spec = c.spec.as_ref();
    let mut out: Vec<String> = spec.and_then(|s| s.pattern.clone()).map(|p| format!("pattern: {p}")).into_iter().collect();
    out.extend(spec.into_iter().flat_map(|s| &s.logic_steps).map(|s| if s.description.is_empty() { format!("{}. {}", s.sequence, s.name) } else { format!("{}. {}: {}", s.sequence, s.name, s.description) }));
    out.join("\n")
}

/// Its invariants, each.
fn invariants(c: &Component) -> String {
    c.spec.iter().flat_map(|s| &s.invariants).map(|i| if i.description.is_empty() { format!("- {}", i.name) } else { format!("- {}: {}", i.name, i.description) }).collect::<Vec<_>>().join("\n")
}

/// The card: what the component is, how it's called, its logic and
/// invariants (each as its section says it).
fn card(c: &Component) -> String {
    let mut out = vec![overview(c)];
    for (title, text) in [("signature", signature(c)), ("logic", logic(c)), ("invariants", invariants(c))] {
        if !text.is_empty() {
            out.push(format!("{title}:\n{text}"));
        }
    }
    out.join("\n")
}

use harness_contracts::CARD_SECTIONS as SECTIONS;

impl nah_things::Resolve for PlanParts {
    fn owns(&self, subject: &Subject) -> bool {
        matches!(subject, Subject::Plan { .. } | Subject::PlanComponent { .. } | Subject::PlanSection { .. } | Subject::PlanRelation { .. } | Subject::PlanItem { .. })
    }

    fn name(&self, subject: &Subject) -> String {
        subject.name(&Known::default())
    }

    fn now(&self, subject: &Subject) -> Option<String> {
        match subject {
            Subject::Plan { plan } => {
                let p = self.plan(plan)?;
                Some(format!("{} ({} components)\n{}", p.title, p.components.len(), p.description).trim_end().to_owned())
            }
            Subject::PlanComponent { plan, id } => self.plan(plan)?.components.iter().find(|c| c.id == *id).map(card),
            Subject::PlanSection { plan, id, section } => {
                let p = self.plan(plan)?;
                let c = p.components.iter().find(|c| c.id == *id)?;
                self.section(&p, c, section)
            }
            Subject::PlanRelation { plan, relation, source, target } => {
                let p = self.plan(plan)?;
                p.relations.iter().any(|r| r.kind == *relation && r.source == *source && r.target == *target).then(|| format!("{source} {} {target}", relation.name()))
            }
            Subject::PlanItem { plan, item } => {
                let p = self.plan(plan)?;
                let item = p.item(&ItemId(item.clone()))?;
                Some(format!("{} ({})\n{}", item.title, item.state.name(), item.text).trim_end().to_owned())
            }
            _ => None,
        }
    }

    /// `plan <id>`, `… component <id>`, `… component <id> section <name>`,
    /// `… relation <source> <kind> <target>`, `… item <id>`.
    fn named(&self, words: &str) -> Option<Result<Subject, String>> {
        let rest = words.trim().strip_prefix("plan ")?;
        let (plan, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        Some((|| {
            let plan = PlanId::new(plan).map_err(|e| e.to_string())?;
            let p = self.plan(&plan).ok_or_else(|| format!("there's no plan {plan}"))?;
            let component = |id: &str| {
                let id = ComponentId::new(id);
                if p.components.iter().any(|c| c.id == id) { Ok(id) } else { Err(format!("plan {plan} has no component {id}")) }
            };
            let words: Vec<&str> = rest.split_whitespace().collect();
            Ok(match words.as_slice() {
                [] => Subject::Plan { plan },
                ["component", id] => Subject::PlanComponent { id: component(id)?, plan },
                ["component", id, "section", section] => {
                    if !SECTIONS.contains(section) {
                        return Err(format!("a card's sections are {}", SECTIONS.join(", ")));
                    }
                    Subject::PlanSection { id: component(id)?, plan, section: (*section).to_owned() }
                }
                ["item", item] => {
                    if p.item(&ItemId((*item).to_owned())).is_none() {
                        return Err(format!("plan {plan} has no item {item}"));
                    }
                    Subject::PlanItem { plan, item: (*item).to_owned() }
                }
                ["relation", source, kind, target] => {
                    let relation = RelationKind::ALL.into_iter().find(|k| k.name() == *kind).ok_or_else(|| format!("{kind:?} isn't a relation's kind"))?;
                    let (source, target) = (ComponentId::new(*source), ComponentId::new(*target));
                    if !p.relations.iter().any(|r| r.kind == relation && r.source == source && r.target == target) {
                        return Err(format!("plan {plan} has no relation {source} {kind} {target}"));
                    }
                    Subject::PlanRelation { plan, relation, source, target }
                }
                _ => return Err(format!("a plan's `component <id>`, `component <id> section <name>`, `relation <source> <kind> <target>` or `item <id>`, not {rest:?}")),
            })
        })())
    }

    /// Its plan is, and its part is in it (no section checks run).
    fn there(&self, subject: &Subject) -> bool {
        let has = |plan: &PlanId, id: &ComponentId| self.plan(plan).is_some_and(|p| p.components.iter().any(|c| c.id == *id));
        match subject {
            Subject::Plan { plan } => self.plan(plan).is_some(),
            Subject::PlanComponent { plan, id } => has(plan, id),
            Subject::PlanSection { plan, id, section } => has(plan, id) && SECTIONS.contains(&section.as_str()),
            Subject::PlanRelation { .. } => self.now(subject).is_some(),
            Subject::PlanItem { plan, item } => self.plan(plan).is_some_and(|p| p.item(&ItemId(item.clone())).is_some()),
            _ => false,
        }
    }
}
