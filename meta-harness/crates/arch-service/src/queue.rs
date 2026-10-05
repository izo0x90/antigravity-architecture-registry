//! The work queue over a plan: which job each component needs next, which
//! one runs now (dependency order, gates), what happened to the last
//! attempt, and the brief a session gets. The server drives the sessions;
//! this is the state machine and nothing else.

use std::collections::{BTreeMap, BTreeSet};

use arch_graph::{ComponentId, Finding, Graph, Job, JobKind, JobState, Plan, PlanId, QueueSnapshot, QueueStatus, Stage, TypeSystem};
use serde::{Deserialize, Serialize};

use crate::ServiceError;
use crate::checks::stage_gate;
use crate::progress::topological;

const DEFAULT_CONFIG: &str = include_str!("../config/queue.toml");
const PLACEHOLDERS: [&str; 11] = ["plan", "plan_title", "component", "name", "type", "stage", "goal", "attempt", "max_attempts", "contract", "findings"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueSettings {
    pub max_attempts: u32,
    pub skeleton_first: bool,
    pub job_timeout_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueConfig {
    pub queue: QueueSettings,
    /// Brief template per job kind (`design`, `plan`, `skeleton`, `implement`).
    pub briefs: BTreeMap<String, String>,
}

/// A project's partial override of the defaults.
#[derive(Clone, Debug, Default, Deserialize)]
struct Override {
    #[serde(default)]
    queue: Option<QueueSettings>,
    #[serde(default)]
    briefs: BTreeMap<String, String>,
}

impl QueueConfig {
    pub fn defaults() -> Result<Self, ServiceError> {
        let config: QueueConfig = toml::from_str(DEFAULT_CONFIG).map_err(|e| ServiceError::Corrupt(format!("built-in queue config: {e}")))?;
        config.check()?;
        Ok(config)
    }

    /// Replaces settings and any briefs `text` defines.
    pub fn extend_from_toml(&mut self, text: &str) -> Result<(), ServiceError> {
        let other: Override = toml::from_str(text).map_err(|e| ServiceError::InvalidOp(format!("queue config: {e}")))?;
        if let Some(q) = other.queue {
            self.queue = q;
        }
        self.briefs.extend(other.briefs);
        self.check()
    }

    fn check(&self) -> Result<(), ServiceError> {
        if self.queue.max_attempts == 0 {
            return Err(ServiceError::InvalidOp("queue config: max_attempts must be at least 1".into()));
        }
        for kind in JobKind::ALL {
            let template = self.briefs.get(kind.name()).ok_or_else(|| ServiceError::InvalidOp(format!("queue config: no brief for '{}' jobs", kind.name())))?;
            for name in placeholders(template) {
                if !PLACEHOLDERS.contains(&name.as_str()) {
                    return Err(ServiceError::InvalidOp(format!("queue config: the {} brief uses {{{name}}}, which isn't known (known: {})", kind.name(), PLACEHOLDERS.join(", "))));
                }
            }
        }
        Ok(())
    }
}

/// `{word}` names in a template.
fn placeholders(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if after[..end].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && end > 0 => {
                out.push(after[..end].to_owned());
                rest = &after[end + 1..];
            }
            _ => rest = after,
        }
    }
    out
}

/// What a brief is filled with.
pub struct BriefContext<'a> {
    pub plan: &'a Plan,
    pub component: &'a arch_graph::Component,
    pub kind: JobKind,
    pub attempt: u32,
    pub contract: &'a str,
    pub findings: &'a [Finding],
}

pub fn render_brief(config: &QueueConfig, cx: &BriefContext) -> String {
    let findings = if cx.findings.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> = cx.findings.iter().map(|f| format!("- [{:?}] {}", f.code, f.message)).collect();
        format!("\nThe last attempt didn't pass:\n{}\n", lines.join("\n"))
    };
    let values: BTreeMap<&str, String> = BTreeMap::from([
        ("plan", cx.plan.id.to_string()),
        ("plan_title", cx.plan.title.clone()),
        ("component", cx.component.id.to_string()),
        ("name", cx.component.name.clone()),
        ("type", cx.component.type_name.clone()),
        ("stage", format!("{:?}", cx.component.stage)),
        ("goal", format!("{:?}", cx.kind.goal())),
        ("attempt", cx.attempt.to_string()),
        ("max_attempts", config.queue.max_attempts.to_string()),
        ("contract", cx.contract.to_owned()),
        ("findings", findings),
    ]);
    let template = config.briefs.get(cx.kind.name()).map(String::as_str).unwrap_or_default();
    let mut out = String::with_capacity(template.len() + cx.contract.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}').and_then(|end| values.get(&after[..end]).map(|v| (end, v))) {
            Some((end, value)) => {
                out.push_str(value);
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// The job a component needs at `stage`.
pub fn job_for(stage: Stage, settings: &QueueSettings) -> Option<JobKind> {
    match stage {
        Stage::Declared => Some(JobKind::Design),
        Stage::ArchApproved => Some(JobKind::Plan),
        Stage::PlanApproved if settings.skeleton_first => Some(JobKind::Skeleton),
        Stage::PlanApproved | Stage::Skeleton => Some(JobKind::Implement),
        Stage::Implemented => None,
    }
}

/// How an attempt went.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Passed,
    Failed(Vec<Finding>),
}

/// The queue's memory between jobs; everything else is read from the plan.
#[derive(Clone, Debug)]
pub struct Queue {
    pub plan: PlanId,
    settings: QueueSettings,
    attempts: BTreeMap<(ComponentId, JobKind), u32>,
    findings: BTreeMap<ComponentId, Vec<Finding>>,
    /// Finished design/plan work, waiting for the user at this stage.
    awaiting: BTreeSet<(ComponentId, Stage)>,
    running: Option<(ComponentId, JobKind, Option<String>)>,
    stopping: bool,
    active: bool,
}

impl Queue {
    pub fn new(plan: PlanId, settings: QueueSettings) -> Self {
        Self { plan, settings, attempts: BTreeMap::new(), findings: BTreeMap::new(), awaiting: BTreeSet::new(), running: None, stopping: false, active: false }
    }

    pub fn settings(&self) -> &QueueSettings {
        &self.settings
    }

    fn attempts(&self, id: &ComponentId, kind: JobKind) -> u32 {
        self.attempts.get(&(id.clone(), kind)).copied().unwrap_or(0)
    }

    fn out_of_attempts(&self, id: &ComponentId, kind: JobKind) -> bool {
        self.attempts(id, kind) >= self.settings.max_attempts
    }

    /// The next job to run: the first component in dependency order whose
    /// job isn't waiting for the user, isn't out of attempts, and whose
    /// dependencies are far enough along for the job's goal.
    pub fn next(&self, plan: &Plan, graph: &Graph, types: &TypeSystem) -> Option<(ComponentId, JobKind)> {
        topological(plan, graph, types).into_iter().find_map(|id| {
            let c = plan.component(&id)?;
            let kind = job_for(c.stage, &self.settings)?;
            let ready = !self.awaiting.contains(&(id.clone(), c.stage))
                && !self.out_of_attempts(&id, kind)
                && (kind == JobKind::Design || stage_gate(plan, graph, types, &id, kind.goal()).is_empty());
            ready.then_some((id, kind))
        })
    }

    pub fn attempt_number(&self, id: &ComponentId, kind: JobKind) -> u32 {
        self.attempts(id, kind) + 1
    }

    pub fn last_findings(&self, id: &ComponentId) -> &[Finding] {
        self.findings.get(id).map(Vec::as_slice).unwrap_or_default()
    }

    /// The queue is being driven from now on.
    pub fn activate(&mut self) {
        self.active = true;
        self.stopping = false;
    }

    pub fn start(&mut self, id: &ComponentId, kind: JobKind) {
        self.active = true;
        self.running = Some((id.clone(), kind, None));
    }

    /// The session the running job got.
    pub fn attach(&mut self, thread: String) {
        if let Some((_, _, t)) = self.running.as_mut() {
            *t = Some(thread);
        }
    }

    /// Records how the job went; `stage` is the component's stage when it started.
    pub fn finish(&mut self, id: &ComponentId, kind: JobKind, stage: Stage, outcome: Outcome) {
        self.running = None;
        *self.attempts.entry((id.clone(), kind)).or_default() += 1;
        match outcome {
            Outcome::Passed => {
                self.findings.remove(id);
                self.attempts.remove(&(id.clone(), kind));
                if kind.needs_approval() {
                    self.awaiting.insert((id.clone(), stage));
                }
            }
            Outcome::Failed(findings) => {
                self.findings.insert(id.clone(), findings);
            }
        }
    }

    /// The user wants another go at a job that ran out of attempts.
    pub fn retry(&mut self, id: &ComponentId) {
        self.attempts.retain(|(c, _), _| c != id);
    }

    pub fn stop(&mut self) {
        self.stopping = true;
    }

    pub fn stopping(&self) -> bool {
        self.stopping
    }

    /// The queue isn't driving anything any more.
    pub fn halt(&mut self) {
        self.active = false;
        self.stopping = false;
        self.running = None;
    }

    pub fn snapshot(&self, plan: &Plan, graph: &Graph, types: &TypeSystem) -> QueueSnapshot {
        let next = self.next(plan, graph, types);
        let mut jobs = Vec::new();
        let mut implemented = true;
        for id in topological(plan, graph, types) {
            let Some(c) = plan.component(&id) else { continue };
            let Some(kind) = job_for(c.stage, &self.settings) else {
                jobs.push(Job { component: id, kind: JobKind::Implement, state: JobState::Done, attempts: 0, findings: Vec::new() });
                continue;
            };
            implemented = false;
            let state = match &self.running {
                Some((r, k, thread)) if *r == id && *k == kind => JobState::Running { thread: thread.clone() },
                _ if self.awaiting.contains(&(id.clone(), c.stage)) => JobState::AwaitingApproval,
                _ if self.out_of_attempts(&id, kind) => JobState::Failed,
                _ if self.findings.contains_key(&id) => JobState::Retrying,
                _ => JobState::Queued,
            };
            jobs.push(Job { component: id.clone(), kind, state, attempts: self.attempts(&id, kind), findings: self.last_findings(&id).to_vec() });
        }
        let status = if implemented {
            QueueStatus::Done
        } else if !self.active {
            QueueStatus::Idle
        } else if self.stopping {
            QueueStatus::Stopping
        } else if self.running.is_some() || next.is_some() {
            QueueStatus::Running
        } else {
            QueueStatus::WaitingForUser
        };
        QueueSnapshot { plan: self.plan.clone(), status, jobs }
    }
}
