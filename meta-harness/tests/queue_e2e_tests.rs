//! The work queue end to end on a copy of the Rust fixture: a fake agent
//! (writing the spec and code a real session would) works through a plan in
//! dependency order, one job at a time, while the test plays the user who
//! approves in the cockpit.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use arch_graph::{ComponentId, JobKind, JobState, Param, PlanId, QueueStatus, RelationKind, Signature, Stage};
use arch_service::{Actor, ArchitectureService, PlanOp, QueueConfig};
use async_trait::async_trait;
use common::{fixture_architecture, http, http_anonymous, spawn_app_with};
use meta_harness::queue::{JobRunner, QueueRunner, Ticket};

fn id(s: &str) -> ComponentId {
    ComponentId::new(format!("rust:{s}"))
}

/// Does what an agent would for each job, straight on the repo and plan.
/// The first attempt at implementing `b` gets the signature wrong.
struct FakeAgent {
    arch: Arc<ArchitectureService>,
    log: Mutex<Vec<Ticket>>,
}

impl FakeAgent {
    fn write(&self, name: &str, body: &str, input: &str) {
        let lib = self.arch.root().join("src/lib.rs");
        let text = std::fs::read_to_string(&lib).unwrap();
        let header = format!("\npub fn {name}(");
        let kept = match text.find(&header) {
            Some(at) => {
                let end = text[at + 1..].find("\n}\n").map(|e| at + 1 + e + 3).unwrap();
                format!("{}{}", &text[..at], &text[end..])
            }
            None => text,
        };
        std::fs::write(&lib, format!("{kept}\npub fn {name}(input: {input}) -> bool {{\n    {body}\n}}\n")).unwrap();
    }
}

#[async_trait]
impl JobRunner for FakeAgent {
    async fn run(&self, ticket: &Ticket, _timeout: Duration, attach: &(dyn Fn(String) + Send + Sync)) -> Result<(), String> {
        attach(format!("thread-{}-{}", ticket.component, ticket.kind.name()));
        let attempts = self.log.lock().unwrap().iter().filter(|t| t.component == ticket.component && t.kind == ticket.kind).count();
        self.log.lock().unwrap().push(ticket.clone());
        let name = ticket.component.as_str().rsplit("::").next().unwrap().to_owned();
        let calls_b = name == "a";
        match ticket.kind {
            JobKind::Design => {
                let sig = Signature { params: vec![Param { name: "input".into(), ty: Some("&str".into()), optional: false, variadic: false }], returns: Some("bool".into()), qualifiers: vec![] };
                let op = PlanOp::Modify { id: ticket.component.clone(), description: Some(format!("{name} checks the input.")), signature: Some(sig), schemas: None, spec: None };
                self.arch.apply(&ticket.plan, op).map_err(|e| e.to_string())?;
            }
            JobKind::Plan => {}
            JobKind::Skeleton => self.write(&name, if calls_b { "let _ = b(input);\n    todo!()" } else { "todo!()" }, "&str"),
            JobKind::Implement => {
                let wrong = name == "b" && attempts == 0;
                self.write(&name, if calls_b { "b(input)" } else { "!input.is_empty()" }, if wrong { "u32" } else { "&str" });
            }
        }
        Ok(())
    }
}

async fn wait_for(queue: &QueueRunner, plan: &PlanId, status: QueueStatus) -> arch_graph::QueueSnapshot {
    for _ in 0..400 {
        let snapshot = queue.snapshot(plan).await.unwrap();
        if snapshot.status == status {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("queue never reached {status:?}: {:?}", queue.snapshot(plan).await.unwrap());
}

fn approve(arch: &ArchitectureService, plan: &PlanId, symbol: &str, to: Stage) {
    arch.approve(plan, &id(symbol), to, &Actor::User { name: "izo".into() }).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_queue_runs_components_in_dependency_order_pauses_for_approvals_and_requeues_failures() {
    let arch = fixture_architecture();
    let plan = PlanId::new("q").unwrap();
    arch.create_plan("q", "Queue", "", arch_graph::PlanKind::Change).unwrap();
    arch.apply(&plan, PlanOp::SetGreenfield { greenfield: true }).unwrap();
    // Added as bare names: the design jobs fill them in. a calls b.
    for name in ["a", "b"] {
        arch.apply(&plan, PlanOp::AddComponent { name: name.into(), type_name: "function".into(), description: String::new(), parent: Some(id("sample")), file: None, signature: None, schemas: None, spec: None }).unwrap();
    }
    arch.apply(&plan, PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::a"), target: id("sample::b"), order: None, ports: None, note: None, expects: None }).unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let queue = QueueRunner::new(arch.clone(), QueueConfig::defaults().unwrap(), move |e| sink.lock().unwrap().push(e));
    let agent = Arc::new(FakeAgent { arch: arch.clone(), log: Mutex::new(Vec::new()) });
    assert_eq!(queue.snapshot(&plan).await.unwrap().status, QueueStatus::Idle);
    queue.start(plan.clone(), agent.clone()).await.unwrap();
    assert!(queue.start(plan.clone(), agent.clone()).await.is_err(), "one runner per plan");

    // Designs done, both wait for the user.
    let waiting = wait_for(&queue, &plan, QueueStatus::WaitingForUser).await;
    assert_eq!(waiting.jobs.iter().filter(|j| j.state == JobState::AwaitingApproval).count(), 2, "{waiting:?}");
    approve(&arch, &plan, "sample::b", Stage::ArchApproved);
    approve(&arch, &plan, "sample::a", Stage::ArchApproved);
    // Plans done, waiting again.
    for _ in 0..400 {
        let s = queue.snapshot(&plan).await.unwrap();
        if s.status == QueueStatus::WaitingForUser && s.jobs.iter().filter(|j| j.state == JobState::AwaitingApproval).count() == 2 && s.jobs.iter().all(|j| j.kind == JobKind::Plan) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    approve(&arch, &plan, "sample::b", Stage::PlanApproved);
    approve(&arch, &plan, "sample::a", Stage::PlanApproved);
    let done = wait_for(&queue, &plan, QueueStatus::Done).await;
    assert!(done.jobs.iter().all(|j| j.state == JobState::Done));
    let current = arch.plan(&plan).unwrap();
    for name in ["a", "b"] {
        assert_eq!(current.component(&id(&format!("sample::{name}"))).unwrap().stage, Stage::Implemented);
    }

    // One session per job, dependencies first, the failed attempt re-queued.
    let log = agent.log.lock().unwrap().clone();
    let order: Vec<(String, &str)> = log.iter().map(|t| (t.component.as_str().rsplit("::").next().unwrap().to_owned(), t.kind.name())).collect();
    let expected: Vec<(String, &str)> = [("b", "design"), ("a", "design"), ("b", "plan"), ("a", "plan"), ("b", "skeleton"), ("b", "implement"), ("b", "implement"), ("a", "skeleton"), ("a", "implement")]
        .into_iter()
        .map(|(n, k)| (n.to_owned(), k))
        .collect();
    assert_eq!(order, expected);

    // Briefs come from the config templates, per job kind, with what failed last time.
    // b's code is finished before a's starts: what a builds on is done first.
    let brief = |n: usize| log[n].brief.as_str();
    assert!(brief(0).starts_with("You are designing one component of plan q (Queue): rust:sample::b"), "{}", brief(0));
    assert!(brief(2).starts_with("The user approved the architecture of rust:sample::b"), "{}", brief(2));
    assert!(brief(4).starts_with("Lay the skeleton of rust:sample::b"), "{}", brief(4));
    assert!(brief(5).contains("Attempt 1 of 3") && !brief(5).contains("didn't pass"));
    assert!(brief(6).contains("Attempt 2 of 3") && brief(6).contains("The last attempt didn't pass") && brief(6).contains("SignatureMismatch"), "{}", brief(6));
    assert!(brief(8).contains("## Signature\n`(input: &str) -> bool`"), "the brief carries the contract: {}", brief(8));
    // Progress was announced, including the session each job ran in.
    let announced = events.lock().unwrap();
    assert!(announced.iter().any(|e| matches!(e, arch_graph::ArchEvent::QueueUpdated { snapshot } if snapshot.jobs.iter().any(|j| matches!(&j.state, JobState::Running { thread: Some(t) } if t.starts_with("thread-"))))));
}

#[test]
fn briefs_are_config_templates_checked_for_known_placeholders() {
    let mut config = QueueConfig::defaults().unwrap();
    config.extend_from_toml("[briefs]\nimplement = \"Build {component} ({type}), try {attempt}.\"").unwrap();
    assert_eq!(config.briefs["implement"], "Build {component} ({type}), try {attempt}.");
    assert!(config.briefs["design"].contains("{contract}"), "others keep their defaults");
    let err = config.clone().extend_from_toml("[briefs]\nplan = \"Do {mystery}\"").unwrap_err().to_string();
    assert!(err.contains("{mystery}"), "{err}");
    assert!(config.clone().extend_from_toml("[queue]\nmax_attempts = 0\nskeleton_first = true\njob_timeout_secs = 1").is_err());
    let mut plan = arch_graph::Plan::new("p", "P");
    let c = arch_graph::Component {
        id: id("x::f"),
        name: "f".into(),
        type_name: "function".into(),
        description: String::new(),
        status: arch_graph::Status::New,
        stage: Stage::PlanApproved,
        anchor: None,
        proposed: None,
        signature: None,
        schemas: Default::default(),
        spec: None,
        placeholders: vec![],
    };
    plan.components.push(c.clone());
    let text = arch_service::render_brief(&config, &arch_service::BriefContext { plan: &plan, component: &c, kind: JobKind::Implement, attempt: 2, contract: "", findings: &[] });
    assert_eq!(text, "Build rust:x::f (function), try 2.");
}

#[tokio::test]
async fn queue_control_is_for_the_user_in_the_cockpit() {
    let arch = fixture_architecture();
    arch.create_plan("q", "Queue", "", arch_graph::PlanKind::Change).unwrap();
    let (addr, _) = spawn_app_with(arch).await;
    let (status, body) = http(addr, "GET", "/api/arch/plans/q/queue", None).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"status\":\"done\"") || body.contains("\"status\":\"idle\""), "{body}");
    for path in ["/api/arch/plans/q/queue/start", "/api/arch/plans/q/queue/stop", "/api/arch/plans/q/queue/retry"] {
        let (status, _) = http_anonymous(addr, "POST", path, Some(r#"{"instanceId":"fake","component":"rust:x"}"#)).await;
        assert_eq!(status, 403, "{path}");
    }
    let (status, body) = http(addr, "POST", "/api/arch/plans/q/queue/stop", None).await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ingest_plan_validate_skeleton_implement_verify_break_and_detect_on_a_fixture_copy() {
    let arch = fixture_architecture();
    // Ingest: the fixture's code is the starting point.
    let ingested = arch.ingest(arch.root()).unwrap();
    assert!(ingested.components > 0);
    let plan = PlanId::new("e2e").unwrap();
    arch.create_plan("e2e", "End to end", "", arch_graph::PlanKind::Change).unwrap();
    arch.apply(&plan, PlanOp::SetGreenfield { greenfield: true }).unwrap();
    for name in ["a", "b"] {
        arch.apply(&plan, PlanOp::AddComponent { name: name.into(), type_name: "function".into(), description: String::new(), parent: Some(id("sample")), file: None, signature: None, schemas: None, spec: None }).unwrap();
    }
    arch.apply(&plan, PlanOp::Relate { kind: RelationKind::Calls, source: id("sample::a"), target: id("sample::b"), order: None, ports: None, note: None, expects: None }).unwrap();
    // Validate: the bare components aren't ready for approval yet.
    let report = arch.validate(&plan).unwrap();
    assert!(report.findings.iter().any(|f| f.code == arch_graph::FindingCode::SpecIncomplete), "{report:?}");

    // The queue designs, lays the skeleton, implements and verifies; the test approves as the user.
    let queue = QueueRunner::new(arch.clone(), QueueConfig::defaults().unwrap(), |_| {});
    let agent = Arc::new(FakeAgent { arch: arch.clone(), log: Mutex::new(Vec::new()) });
    queue.start(plan.clone(), agent.clone()).await.unwrap();
    for to in [Stage::ArchApproved, Stage::PlanApproved] {
        for _ in 0..400 {
            let s = queue.snapshot(&plan).await.unwrap();
            if s.status == QueueStatus::WaitingForUser && s.jobs.iter().filter(|j| j.state == JobState::AwaitingApproval).count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        approve(&arch, &plan, "sample::b", to);
        approve(&arch, &plan, "sample::a", to);
    }
    wait_for(&queue, &plan, QueueStatus::Done).await;
    let kinds: Vec<&str> = agent.log.lock().unwrap().iter().map(|t| t.kind.name()).collect();
    assert!(kinds.contains(&"skeleton") && kinds.contains(&"implement"));
    let stage = |s: &str| arch.plan(&plan).unwrap().component(&id(s)).unwrap().stage;
    assert_eq!((stage("sample::a"), stage("sample::b")), (Stage::Implemented, Stage::Implemented));

    // Break: a stops calling b. Detect: the change is re-verified and a drops back.
    let lib = arch.root().join("src/lib.rs");
    std::fs::write(&lib, std::fs::read_to_string(&lib).unwrap().replace("    b(input)\n", "    !input.is_empty()\n")).unwrap();
    let refresh = arch.refresh(std::slice::from_ref(&lib)).unwrap();
    let a = refresh.reverified.iter().find(|r| r.component == id("sample::a")).expect("a is re-verified");
    assert!(a.report.findings.iter().any(|f| f.code == arch_graph::FindingCode::MissingCallSite), "{:?}", a.report);
    assert_ne!(stage("sample::a"), Stage::Implemented, "broken code isn't implemented any more");
    assert_eq!(stage("sample::b"), Stage::Implemented, "b is untouched");
}

#[tokio::test]
async fn a_session_past_its_time_is_stopped_before_the_job_is_retried() {
    let (_, state) = common::spawn_app().await;
    let runner = meta_harness::queue::SessionRunner {
        slots: state.slots.clone(),
        sessions: state.sessions.clone(),
        settings: harness_core::SlotSettings { instance: Some(harness_contracts::InstanceId::new("fake")), ..Default::default() },
    };
    let ticket = Ticket { plan: PlanId::new("p").unwrap(), component: id("x::f"), kind: JobKind::Implement, brief: "[hang] take forever".into() };
    let thread = Arc::new(Mutex::new(None));
    let seen = thread.clone();
    let result = runner.run(&ticket, Duration::from_millis(200), &move |t| *seen.lock().unwrap() = Some(t)).await;
    assert!(result.unwrap_err().contains("didn't finish"), "times out");
    let thread = thread.lock().unwrap().clone().expect("a session was opened");
    assert!(state.sessions.get(&harness_contracts::ThreadId::new(thread)).is_err(), "and its session is closed, so a retry can't race it");
    assert!(state.slots.list().iter().all(|s| s.role != "worker"), "and its worker slot is gone");
    // A turn that completes is fine.
    let ok = Ticket { brief: "do it".into(), ..ticket.clone() };
    assert!(runner.run(&ok, Duration::from_secs(5), &|_| {}).await.is_ok());
    // A harness that never finishes opening the session is cut off too.
    let stuck = meta_harness::queue::SessionRunner {
        slots: state.slots.clone(),
        sessions: state.sessions.clone(),
        settings: harness_core::SlotSettings {
            instance: Some(harness_contracts::InstanceId::new("fake")),
            policy: harness_contracts::RuntimePolicy { cwd: Some("[hang-open]".into()), ..Default::default() },
            model: None,
        },
    };
    let started = std::time::Instant::now();
    let result = stuck.run(&ticket, Duration::from_millis(200), &|_| {}).await;
    assert!(result.unwrap_err().contains("didn't finish") && started.elapsed() < Duration::from_secs(3));
    assert!(state.slots.list().iter().all(|s| s.role != "worker"), "a worker that never got its session goes too");
}

/// A session that never gets the work done.
struct BrokenAgent;

#[async_trait]
impl JobRunner for BrokenAgent {
    async fn run(&self, _ticket: &Ticket, _timeout: Duration, _attach: &(dyn Fn(String) + Send + Sync)) -> Result<(), String> {
        Err("the session was cancelled".into())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_session_is_reported_as_such() {
    let arch = fixture_architecture();
    let plan = PlanId::new("broken").unwrap();
    arch.create_plan("broken", "Broken", "", arch_graph::PlanKind::Change).unwrap();
    arch.apply(&plan, PlanOp::SetGreenfield { greenfield: true }).unwrap();
    arch.apply(&plan, PlanOp::AddComponent { name: "a".into(), type_name: "function".into(), description: String::new(), parent: Some(id("sample")), file: None, signature: None, schemas: None, spec: None }).unwrap();
    let queue = QueueRunner::new(arch.clone(), QueueConfig::defaults().unwrap(), |_| {});
    queue.start(plan.clone(), Arc::new(BrokenAgent)).await.unwrap();
    let snapshot = wait_for(&queue, &plan, QueueStatus::WaitingForUser).await;
    let job = snapshot.jobs.iter().find(|j| j.component == id("sample::a")).unwrap();
    assert_eq!(job.state, JobState::Failed, "out of attempts");
    assert_eq!(job.findings[0].code, arch_graph::FindingCode::SessionFailed);
    assert_eq!(job.findings[0].message, "the session was cancelled");
}
