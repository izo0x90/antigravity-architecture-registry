//! The work queue view: every component's next job in dependency order, the
//! session running now, what waits for the user's approval, and why failed
//! attempts didn't pass. Starting, stopping, approving and retrying happen
//! here, from the cockpit (the server only takes them from its session).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use arch_graph::{ArchEvent, ComponentId, Job, JobState, PlanId, QueueSnapshot, QueueStatus};
use egui::RichText;
use harness_contracts::InstanceId;

use crate::actions::{Action, ActionProvider, RunCx, ToolbarStyle, ToolbarUse, toolbar_control};
use crate::net::{NetClient, NetResult};
use crate::theme::colors;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueCommand {
    Start,
    Stop,
    Approve,
    Retry,
    Next,
    Prev,
    Expand,
    ShowInGraph,
    OpenSession,
}

enum Reply {
    Snapshot(PlanId, NetResult<QueueSnapshot>),
    Done(NetResult<()>),
}

/// What the queue view asks of the other views.
#[derive(Default)]
pub struct QueueRequests {
    pub select: Option<ComponentId>,
    pub open_thread: Option<String>,
}

#[derive(Default)]
pub struct QueueState {
    pub plan: Option<PlanId>,
    pub snapshot: Option<QueueSnapshot>,
    /// Index into the snapshot's jobs.
    pub selected: usize,
    pub expanded: BTreeSet<ComponentId>,
    /// Who approves (the cockpit's user).
    pub author: String,
    /// The harness instance that runs the sessions (the chat's).
    pub instance: Option<InstanceId>,
    pub error: Option<String>,
    pub requests: QueueRequests,
    loaded: Option<Option<PlanId>>,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

fn push(inbox: &Arc<Mutex<Vec<Reply>>>, reply: Reply) {
    if let Ok(mut i) = inbox.lock() {
        i.push(reply);
    }
}

fn state_label(state: &JobState) -> (&'static str, egui::Color32) {
    match state {
        JobState::Queued => ("queued", colors().muted),
        JobState::Running { .. } => ("running", colors().info),
        JobState::AwaitingApproval => ("needs your approval", colors().warn),
        JobState::Retrying => ("retrying", colors().warn),
        JobState::Failed => ("failed", colors().error),
        JobState::Done => ("done", colors().ok),
    }
}

impl QueueState {
    pub fn invalidate(&mut self) {
        self.loaded = None;
    }

    pub fn on_event(&mut self, event: &ArchEvent) {
        match event {
            ArchEvent::QueueUpdated { snapshot } if Some(&snapshot.plan) == self.plan.as_ref() => self.set(snapshot.as_ref().clone()),
            ArchEvent::Changed { .. } => self.invalidate(),
            _ => {}
        }
    }

    fn set(&mut self, snapshot: QueueSnapshot) {
        self.selected = self.selected.min(snapshot.jobs.len().saturating_sub(1));
        self.snapshot = Some(snapshot);
    }

    /// Applies replies; fetches when the plan changes or after `invalidate`.
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context, plan: Option<&PlanId>) {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::Snapshot(p, Ok(s)) if Some(&p) == self.plan.as_ref() => self.set(s),
                Reply::Snapshot(_, Err(e)) | Reply::Done(Err(e)) => self.error = Some(e),
                Reply::Done(Ok(())) => {
                    self.error = None;
                    self.invalidate();
                }
                Reply::Snapshot(..) => {}
            }
        }
        let wanted = plan.cloned();
        if self.loaded.as_ref() == Some(&wanted) {
            return;
        }
        self.loaded = Some(wanted.clone());
        if self.plan != wanted {
            self.snapshot = None;
            self.selected = 0;
        }
        self.plan = wanted.clone();
        let Some(plan) = wanted else { return };
        let inbox = self.inbox.clone();
        let p = plan.clone();
        net.arch_queue(&plan, ctx.clone(), move |r| push(&inbox, Reply::Snapshot(p, r)));
    }

    pub fn job(&self) -> Option<&Job> {
        self.snapshot.as_ref().and_then(|s| s.jobs.get(self.selected))
    }

    fn done<T: Send + 'static>(&self) -> impl FnOnce(NetResult<T>) + Send + 'static {
        let inbox = self.inbox.clone();
        move |r: NetResult<T>| push(&inbox, Reply::Done(r.map(|_| ())))
    }

    fn status(&self) -> Option<QueueStatus> {
        self.snapshot.as_ref().map(|s| s.status)
    }

    fn step(&mut self, by: isize) {
        let n = self.snapshot.as_ref().map_or(0, |s| s.jobs.len());
        if n > 0 {
            self.selected = (self.selected as isize + by).rem_euclid(n as isize) as usize;
        }
    }

    pub fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        let actions = self.actions();
        let mut used = None;
        ui.horizontal_wrapped(|ui| {
            let title = match (&self.plan, self.status()) {
                (Some(p), Some(s)) => format!("Queue for {p}: {s:?}"),
                (Some(p), None) => format!("Queue for {p}"),
                (None, _) => "No plan selected (pick one in the graph)".to_owned(),
            };
            ui.label(RichText::new(title).strong());
            for action in actions.iter().filter(|a| a.toolbar.is_some()) {
                if let Some(u) = toolbar_control(ui, action) {
                    used = Some((action.clone(), u));
                }
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(colors().error, e);
        }
        ui.separator();
        let mut clicked: Option<(usize, Option<QueueCommand>)> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let Some(snapshot) = &self.snapshot else {
                ui.label(RichText::new("Nothing to show yet.").color(colors().muted));
                return;
            };
            for (i, job) in snapshot.jobs.iter().enumerate() {
                let selected = i == self.selected;
                let (label, color) = state_label(&job.state);
                let frame = egui::Frame::new().inner_margin(4.0).stroke(egui::Stroke::new(1.0, if selected { colors().focus } else { colors().border }));
                frame.show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let name = job.component.as_str().split_once(':').map_or(job.component.as_str(), |(_, s)| s);
                        if ui.selectable_label(selected, RichText::new(name).strong()).clicked() {
                            clicked = Some((i, None));
                        }
                        ui.label(RichText::new(job.kind.name()).color(colors().muted));
                        ui.colored_label(color, label);
                        if job.attempts > 0 {
                            ui.label(RichText::new(format!("attempt {}", job.attempts + 1)).color(colors().muted));
                        }
                        match &job.state {
                            JobState::AwaitingApproval if ui.button(format!("Approve {:?}", job.kind.goal())).clicked() => clicked = Some((i, Some(QueueCommand::Approve))),
                            JobState::Failed if ui.button("Retry").clicked() => clicked = Some((i, Some(QueueCommand::Retry))),
                            JobState::Running { thread: Some(_) } if ui.button("Session").clicked() => clicked = Some((i, Some(QueueCommand::OpenSession))),
                            _ => {}
                        }
                        if !job.findings.is_empty() && ui.small_button(format!("{} findings", job.findings.len())).clicked() {
                            clicked = Some((i, Some(QueueCommand::Expand)));
                        }
                    });
                    if self.expanded.contains(&job.component) {
                        for f in &job.findings {
                            ui.label(RichText::new(format!("• [{:?}] {}", f.code, f.message)).color(colors().warn).size(11.0));
                        }
                    }
                });
            }
        });
        if let Some((i, command)) = clicked {
            self.selected = i;
            if let Some(c) = command {
                self.apply(c, cx);
            }
        }
        if let Some((action, ToolbarUse::Run(value))) = used {
            self.run(&action, value, cx);
        }
    }

    fn apply(&mut self, command: QueueCommand, cx: &mut RunCx<'_>) {
        let Some(plan) = self.plan.clone() else { return };
        match command {
            QueueCommand::Start => match &self.instance {
                Some(instance) => cx.net.arch_queue_start(&plan, instance, cx.ctx.clone(), self.done()),
                None => self.error = Some("connect a harness in the chat first: its instance runs the sessions".into()),
            },
            QueueCommand::Stop => cx.net.arch_queue_stop(&plan, cx.ctx.clone(), self.done()),
            QueueCommand::Approve => {
                if let Some(job) = self.job().filter(|j| j.state == JobState::AwaitingApproval).cloned() {
                    cx.net.arch_approve(&plan, &job.component, job.kind.goal(), &self.author, cx.ctx.clone(), self.done());
                }
            }
            QueueCommand::Retry => {
                if let Some(job) = self.job().filter(|j| j.state == JobState::Failed).cloned() {
                    cx.net.arch_queue_retry(&plan, &job.component, cx.ctx.clone(), self.done());
                }
            }
            QueueCommand::Next => self.step(1),
            QueueCommand::Prev => self.step(-1),
            QueueCommand::Expand => {
                if let Some(id) = self.job().map(|j| j.component.clone())
                    && !self.expanded.remove(&id)
                {
                    self.expanded.insert(id);
                }
            }
            QueueCommand::ShowInGraph => self.requests.select = self.job().map(|j| j.component.clone()),
            QueueCommand::OpenSession => {
                if let Some(JobState::Running { thread: Some(t) }) = self.job().map(|j| &j.state) {
                    self.requests.open_thread = Some(t.clone());
                }
            }
        }
    }
}

impl ActionProvider for QueueState {
    fn actions(&self) -> Vec<Action> {
        use QueueCommand as Q;
        let f = |id: &str, label: &str, c: QueueCommand| Action::new(id, label, c).focused();
        let mut out = vec![f("queue.next", "Next job", Q::Next), f("queue.prev", "Previous job", Q::Prev)];
        if self.plan.is_none() {
            return out;
        }
        let running = matches!(self.status(), Some(QueueStatus::Running | QueueStatus::WaitingForUser | QueueStatus::Stopping));
        if running {
            out.push(f("queue.stop", "Stop the queue (after the running job)", Q::Stop).toolbar(ToolbarStyle::Button, "Stop", Some(egui_phosphor::regular::STOP)));
        } else if self.status() != Some(QueueStatus::Done) {
            out.push(f("queue.start", "Start working through the plan", Q::Start).toolbar(ToolbarStyle::Button, "Start", Some(egui_phosphor::regular::PLAY)));
        }
        if let Some(job) = self.job() {
            out.push(f("queue.show_in_graph", "Show this component in the graph", Q::ShowInGraph));
            if !job.findings.is_empty() {
                out.push(f("queue.expand", "Show or hide why the last attempt failed", Q::Expand));
            }
            match job.state {
                JobState::AwaitingApproval => out.push(f("queue.approve", "Approve this component's design", Q::Approve)),
                JobState::Failed => out.push(f("queue.retry", "Give this job another round", Q::Retry)),
                JobState::Running { thread: Some(_) } => out.push(f("queue.open_session", "Open the session working on it", Q::OpenSession)),
                _ => {}
            }
        }
        out
    }

    fn run(&mut self, action: &Action, _value: Option<String>, cx: &mut RunCx<'_>) {
        if let Some(c) = action.command::<QueueCommand>().copied() {
            self.apply(c, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arch_graph::{Finding, FindingCode, JobKind, Target};

    fn id(s: &str) -> ComponentId {
        ComponentId::new(format!("rust:{s}"))
    }

    fn snapshot(status: QueueStatus) -> QueueSnapshot {
        let job = |s: &str, kind, state, findings: Vec<Finding>| Job { component: id(s), kind, state, attempts: findings.len() as u32, findings };
        QueueSnapshot {
            plan: PlanId::new("p").unwrap(),
            status,
            jobs: vec![
                job("b", JobKind::Implement, JobState::Running { thread: Some("t1".into()) }, vec![]),
                job("a", JobKind::Design, JobState::AwaitingApproval, vec![]),
                job("c", JobKind::Implement, JobState::Failed, vec![Finding::error(FindingCode::MissingCallSite, Target::Component { id: id("c") }, "doesn't call b")]),
            ],
        }
    }

    fn ids(q: &QueueState) -> Vec<String> {
        q.actions().into_iter().map(|a| a.id).collect()
    }

    // req: P9.12
    #[test]
    fn queue_sessions_approvals_and_findings_are_visible_and_actionable() {
        let ctx = egui::Context::default();
        let net = NetClient::new("http://localhost:0");
        let mut q = QueueState { author: "izo".into(), ..QueueState::default() };
        q.tick(&net, &ctx, Some(&PlanId::new("p").unwrap()));
        q.on_event(&ArchEvent::QueueUpdated { snapshot: Box::new(snapshot(QueueStatus::Running)) });
        assert_eq!(q.snapshot.as_ref().unwrap().jobs.len(), 3, "the pushed snapshot is shown");
        let mut mode = crate::keymap::InputMode::Normal;
        let mut cx = RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, hint_chars: "asdf" };
        // The running job links to its session.
        assert!(ids(&q).contains(&"queue.open_session".to_owned()) && ids(&q).contains(&"queue.stop".to_owned()));
        q.apply(QueueCommand::OpenSession, &mut cx);
        assert_eq!(q.requests.open_thread.take(), Some("t1".into()));
        // Approval waits on the user: the action is offered on that job only.
        q.apply(QueueCommand::Next, &mut cx);
        assert!(ids(&q).contains(&"queue.approve".to_owned()) && !ids(&q).contains(&"queue.retry".to_owned()));
        q.apply(QueueCommand::ShowInGraph, &mut cx);
        assert_eq!(q.requests.select.take(), Some(id("a")));
        // Failed work shows why and can be retried.
        q.apply(QueueCommand::Next, &mut cx);
        assert!(ids(&q).contains(&"queue.retry".to_owned()) && ids(&q).contains(&"queue.expand".to_owned()));
        q.apply(QueueCommand::Expand, &mut cx);
        assert!(q.expanded.contains(&id("c")));
        q.apply(QueueCommand::Next, &mut cx);
        assert_eq!(q.selected, 0, "wraps around");
        // Starting needs a harness to run sessions on.
        q.on_event(&ArchEvent::QueueUpdated { snapshot: Box::new(snapshot(QueueStatus::Idle)) });
        assert!(ids(&q).contains(&"queue.start".to_owned()));
        q.apply(QueueCommand::Start, &mut cx);
        assert!(q.error.as_deref().unwrap_or_default().contains("connect a harness"));
        // Other plans' updates don't leak in.
        let mut other = snapshot(QueueStatus::Done);
        other.plan = PlanId::new("other").unwrap();
        q.on_event(&ArchEvent::QueueUpdated { snapshot: Box::new(other) });
        assert_eq!(q.status(), Some(QueueStatus::Idle));
        // It renders headless.
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut mode = crate::keymap::InputMode::Normal;
                let mut cx = RunCx { net: &net, ctx, input_mode: &mut mode, slot: 0, hint_chars: "asdf" };
                q.render(ui, &mut cx);
            });
        });
    }
}
