//! Runs the work queue over a plan: the next job (dependency order, gates)
//! goes to its own agent session with a brief from config; the result is
//! checked (validation for design work, verification for code); jobs that
//! need the user's approval wait for it; failed attempts run again with the
//! findings, up to the configured limit.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arch_graph::{ArchEvent, ComponentId, Finding, FindingCode, JobKind, PlanId, QueueSnapshot, QueueStatus, Severity, Target};
use arch_service::{ArchitectureService, BriefContext, Outcome, Queue, QueueConfig, ServiceError, missing_details, render_brief};
use async_trait::async_trait;
use futures::StreamExt;
use harness_contracts::{AdapterEvent, TurnInput, TurnStatus};
use harness_core::{OpenThread, SessionManager};
use tokio::sync::Notify;

use crate::arch::blocking;

/// One job handed to a session.
#[derive(Clone, Debug)]
pub struct Ticket {
    pub plan: PlanId,
    pub component: ComponentId,
    pub kind: JobKind,
    pub brief: String,
}

/// Runs one job to the end of its session's turn.
#[async_trait]
pub trait JobRunner: Send + Sync {
    /// `attach` is called with the session's thread id as soon as it exists.
    /// A job that runs past `timeout` is stopped (its session too) and fails.
    async fn run(&self, ticket: &Ticket, timeout: Duration, attach: &(dyn Fn(String) + Send + Sync)) -> Result<(), String>;
}

/// Opens a session per job on a configured harness instance and runs the
/// brief as one turn.
pub struct SessionRunner {
    pub sessions: Arc<SessionManager>,
    pub open: OpenThread,
}

#[async_trait]
impl JobRunner for SessionRunner {
    async fn run(&self, ticket: &Ticket, timeout: Duration, attach: &(dyn Fn(String) + Send + Sync)) -> Result<(), String> {
        // The whole job (opening the session included) runs against the clock.
        let opened: std::sync::Mutex<Option<harness_contracts::ThreadId>> = std::sync::Mutex::new(None);
        let job = async {
            let thread = self.sessions.open(self.open.clone()).await.map_err(|e| format!("couldn't open a session: {e}"))?;
            *opened.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(thread.clone());
            attach(thread.to_string());
            let session = self.sessions.get(&thread).map_err(|e| e.to_string())?;
            let mut events = session.events();
            let turn = session.start_turn(TurnInput { text: ticket.brief.clone(), attachments: Vec::new() }).await.map_err(|e| format!("couldn't start the turn: {e}"))?;
            while let Some(event) = events.next().await {
                if let AdapterEvent::TurnUpdated { turn: t } = event
                    && t.id == turn
                    && t.status.is_terminal()
                {
                    return match t.status {
                        TurnStatus::Completed => Ok(()),
                        other => {
                            let how = match other {
                                TurnStatus::Cancelled => "was cancelled",
                                TurnStatus::Interrupted => "was interrupted",
                                _ => "failed",
                            };
                            Err(format!("the session {how}{}", t.failure.map(|f| format!(": {}", f.message)).unwrap_or_default()))
                        }
                    };
                }
            }
            Err("the session closed before the turn ended".into())
        };
        match tokio::time::timeout(timeout, job).await {
            Ok(result) => result,
            Err(_) => {
                // Stop it for real: a retry must not race a session still editing the code.
                let thread = opened.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
                if let Some(thread) = thread {
                    if let Ok(session) = self.sessions.get(&thread) {
                        let _ = session.interrupt().await;
                    }
                    let _ = self.sessions.close(&thread).await;
                }
                Err(format!("the session didn't finish within {}s and was stopped", timeout.as_secs()))
            }
        }
    }
}

struct Running {
    queue: Mutex<Queue>,
    wake: Notify,
    /// What was announced last (patched when a session attaches).
    last: Mutex<Option<QueueSnapshot>>,
}

type Publish = Arc<dyn Fn(ArchEvent) + Send + Sync>;

/// The queues of every plan being worked on.
pub struct QueueRunner {
    arch: Arc<ArchitectureService>,
    config: QueueConfig,
    queues: Mutex<HashMap<PlanId, Arc<Running>>>,
    publish: Publish,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl QueueRunner {
    pub fn new(arch: Arc<ArchitectureService>, config: QueueConfig, publish: impl Fn(ArchEvent) + Send + Sync + 'static) -> Arc<Self> {
        let runner = Arc::new(Self { arch: arch.clone(), config, queues: Mutex::new(HashMap::new()), publish: Arc::new(publish) });
        // Approvals and plan edits can unblock a waiting queue.
        let weak = Arc::downgrade(&runner);
        arch.subscribe(move |event| {
            if let (ArchEvent::Changed { .. }, Some(runner)) = (event, weak.upgrade()) {
                for q in lock(&runner.queues).values() {
                    q.wake.notify_one();
                }
            }
        });
        runner
    }

    fn handle(&self, plan: &PlanId) -> Arc<Running> {
        lock(&self.queues)
            .entry(plan.clone())
            .or_insert_with(|| Arc::new(Running { queue: Mutex::new(Queue::new(plan.clone(), self.config.queue.clone())), wake: Notify::new(), last: Mutex::new(None) }))
            .clone()
    }

    pub async fn snapshot(&self, plan: &PlanId) -> Result<QueueSnapshot, ServiceError> {
        let handle = self.handle(plan);
        let id = plan.clone();
        let (current, graph) = blocking(&self.arch, move |a| Ok((a.plan(&id)?, a.graph(&arch_graph::Source::Plan(id.clone()))?))).await?;
        let snapshot = lock(&handle.queue).snapshot(&current, &graph, self.arch.types());
        Ok(snapshot)
    }

    async fn announce(&self, plan: &PlanId) -> Result<QueueSnapshot, ServiceError> {
        let snapshot = self.snapshot(plan).await?;
        *lock(&self.handle(plan).last) = Some(snapshot.clone());
        (self.publish)(ArchEvent::QueueUpdated { snapshot: Box::new(snapshot.clone()) });
        Ok(snapshot)
    }

    /// Starts working through `plan` with sessions from `runner`.
    pub async fn start(self: &Arc<Self>, plan: PlanId, runner: Arc<dyn JobRunner>) -> Result<QueueSnapshot, ServiceError> {
        let id = plan.clone();
        let found = blocking(&self.arch, move |a| a.plan(&id)).await?;
        arch_service::only_changes(&found, "built")?;
        let handle = self.handle(&plan);
        {
            let snapshot = self.snapshot(&plan).await?;
            if matches!(snapshot.status, QueueStatus::Running | QueueStatus::WaitingForUser | QueueStatus::Stopping) {
                return Err(ServiceError::InvalidOp(format!("the queue for {plan} is already running")));
            }
            lock(&handle.queue).activate();
        }
        let this = self.clone();
        let id = plan.clone();
        tokio::spawn(async move {
            if let Err(e) = this.drive(&id, &handle, runner).await {
                tracing::warn!("queue for {id} stopped: {e}");
                lock(&handle.queue).halt();
                let _ = this.announce(&id).await;
            }
        });
        self.announce(&plan).await
    }

    /// Finishes the running job, then stops.
    pub async fn stop(&self, plan: &PlanId) -> Result<QueueSnapshot, ServiceError> {
        let handle = self.handle(plan);
        lock(&handle.queue).stop();
        handle.wake.notify_one();
        self.announce(plan).await
    }

    /// The plan is gone: stop its queue and tell clients there's nothing left.
    pub fn forget(&self, plan: &PlanId) {
        if let Some(handle) = lock(&self.queues).remove(plan) {
            lock(&handle.queue).stop();
            handle.wake.notify_one();
        }
        (self.publish)(ArchEvent::QueueUpdated { snapshot: Box::new(QueueSnapshot { plan: plan.clone(), status: QueueStatus::Idle, jobs: Vec::new() }) });
    }

    /// Gives a job that ran out of attempts another round.
    pub async fn retry(&self, plan: &PlanId, id: &ComponentId) -> Result<QueueSnapshot, ServiceError> {
        let handle = self.handle(plan);
        lock(&handle.queue).retry(id);
        handle.wake.notify_one();
        self.announce(plan).await
    }

    async fn drive(&self, plan: &PlanId, handle: &Running, runner: Arc<dyn JobRunner>) -> Result<(), ServiceError> {
        loop {
            // Registered before looking, so a wake-up in between isn't lost.
            let woken = handle.wake.notified();
            if lock(&handle.queue).stopping() {
                lock(&handle.queue).halt();
                self.announce(plan).await?;
                return Ok(());
            }
            let id = plan.clone();
            let (current, graph) = blocking(&self.arch, move |a| Ok((a.plan(&id)?, a.graph(&arch_graph::Source::Plan(id.clone()))?))).await?;
            let next = lock(&handle.queue).next(&current, &graph, self.arch.types());
            let Some((component, kind)) = next else {
                let snapshot = self.announce(plan).await?;
                if snapshot.status == QueueStatus::Done {
                    lock(&handle.queue).halt();
                    self.announce(plan).await?;
                    return Ok(());
                }
                woken.await;
                continue;
            };
            let Some(planned) = current.component(&component).cloned() else { continue };
            let attempt = lock(&handle.queue).attempt_number(&component, kind);
            let findings = lock(&handle.queue).last_findings(&component).to_vec();
            let (p, c) = (plan.clone(), component.clone());
            let contract = blocking(&self.arch, move |a| Ok(a.contract(&p, &c)?.to_markdown())).await?;
            let brief = render_brief(&self.config, &BriefContext { plan: &current, component: &planned, kind, attempt, contract: &contract, findings: &findings });
            lock(&handle.queue).start(&component, kind);
            self.announce(plan).await?;
            let ticket = Ticket { plan: plan.clone(), component: component.clone(), kind, brief };
            let publish = self.publish.clone();
            let running = component.clone();
            let attach = move |thread: String| {
                lock(&handle.queue).attach(thread.clone());
                // Show the session right away: patch what was announced last.
                let mut last = lock(&handle.last);
                if let Some(snapshot) = last.as_mut() {
                    for job in snapshot.jobs.iter_mut().filter(|j| j.component == running) {
                        job.state = arch_graph::JobState::Running { thread: Some(thread.clone()) };
                    }
                    publish(ArchEvent::QueueUpdated { snapshot: Box::new(snapshot.clone()) });
                }
            };
            let timeout = Duration::from_secs(self.config.queue.job_timeout_secs);
            let ran = runner.run(&ticket, timeout, &attach).await;
            let outcome = match ran {
                Ok(()) => self.evaluate(plan, &component, kind).await?,
                Err(e) => Outcome::Failed(vec![Finding::error(FindingCode::SessionFailed, Target::Component { id: component.clone() }, e)]),
            };
            lock(&handle.queue).finish(&component, kind, planned.stage, outcome);
            self.announce(plan).await?;
        }
    }

    /// Whether the job did what it had to: design work must leave a complete,
    /// clean spec for the user to approve; code must verify.
    async fn evaluate(&self, plan: &PlanId, id: &ComponentId, kind: JobKind) -> Result<Outcome, ServiceError> {
        let (p, c) = (plan.clone(), id.clone());
        blocking(&self.arch, move |a| {
            if kind.needs_approval() {
                let report = a.validate(&p)?;
                let current = a.plan(&p)?;
                let graph = a.graph(&arch_graph::Source::Plan(p.clone()))?;
                let touches = |f: &Finding| match &f.target {
                    Target::Component { id } => id == &c,
                    Target::Relation { id } => graph.relation(id).is_some_and(|r| r.source == c || r.target == c),
                    Target::Plan { .. } => true,
                };
                let mut problems: Vec<Finding> = report.findings.into_iter().filter(|f| f.severity == Severity::Error && touches(f)).collect();
                if let (Some(component), Ok(ty)) = (graph.component(&c), a.types().resolve(&current.component(&c).map(|x| x.type_name.clone()).unwrap_or_default())) {
                    let missing = missing_details(component, &ty);
                    if !missing.is_empty() {
                        problems.push(Finding::error(FindingCode::SpecIncomplete, Target::Component { id: c.clone() }, format!("still missing: {}", missing.iter().map(|f| f.name()).collect::<Vec<_>>().join(", "))));
                    }
                }
                Ok(if problems.is_empty() { Outcome::Passed } else { Outcome::Failed(problems) })
            } else {
                let v = a.verify(&p, &c, kind.goal())?;
                Ok(if v.advanced_to == Some(kind.goal()) {
                    Outcome::Passed
                } else {
                    Outcome::Failed(v.report.findings.into_iter().filter(|f| f.severity == Severity::Error).collect())
                })
            }
        })
        .await
    }
}

// ---- REST ----

use axum::Json;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use harness_contracts::{InstanceId, ModelSelection, RuntimePolicy};
use serde::Deserialize;

use crate::arch::{ArchResult, plan_id};
use crate::cockpit::{Needs, can};
use crate::server::AppState;

pub fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/arch/plans/{plan}/queue", get(snapshot))
        .route("/api/arch/plans/{plan}/queue/start", post(start))
        .route("/api/arch/plans/{plan}/queue/stop", post(stop))
        .route("/api/arch/plans/{plan}/queue/retry", post(retry))
}

async fn snapshot(_: Needs<can::ViewRead>, State(state): State<AppState>, Path(plan): Path<String>) -> ArchResult<QueueSnapshot> {
    Ok(Json(state.queue.snapshot(&plan_id(&plan)?).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartBody {
    /// The harness instance whose sessions do the work.
    instance_id: InstanceId,
    #[serde(default)]
    policy: RuntimePolicy,
    #[serde(default)]
    model: Option<ModelSelection>,
}

/// The user starts the queue from the cockpit.
async fn start(_: Needs<can::PlansApprove>, State(state): State<AppState>, Path(plan): Path<String>, Json(body): Json<StartBody>) -> ArchResult<QueueSnapshot> {
    let plan = plan_id(&plan)?;
    let mut policy = body.policy;
    if policy.cwd.is_none() {
        policy.cwd = Some(state.arch.root().display().to_string());
    }
    let runner = Arc::new(SessionRunner {
        sessions: state.sessions.clone(),
        open: OpenThread { instance_id: body.instance_id, policy, model: body.model, resume: None, extra_mcp_servers: Vec::new() },
    });
    Ok(Json(state.queue.start(plan, runner).await?))
}

async fn stop(_: Needs<can::PlansApprove>, State(state): State<AppState>, Path(plan): Path<String>) -> ArchResult<QueueSnapshot> {
    Ok(Json(state.queue.stop(&plan_id(&plan)?).await?))
}

#[derive(Deserialize)]
struct RetryBody {
    component: ComponentId,
}

async fn retry(_: Needs<can::PlansApprove>, State(state): State<AppState>, Path(plan): Path<String>, Json(body): Json<RetryBody>) -> ArchResult<QueueSnapshot> {
    Ok(Json(state.queue.retry(&plan_id(&plan)?, &body.component).await?))
}
