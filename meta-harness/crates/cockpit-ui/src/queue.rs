//! The work queue view: every component's next job in dependency order, the
//! session running now, what waits for the user's approval, and why failed
//! attempts didn't pass. Starting, stopping, approving and retrying happen
//! here, from the cockpit (the server only takes them from its session).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use arch_graph::{ArchEvent, ComponentId, Job, JobState, PlanId, QueueSnapshot, QueueStatus};
use egui::RichText;
use harness_contracts::InstanceId;

use crate::actions::{Action, ActionProvider, Group, MINOR, PRIMARY, RunCx, ToolbarStyle, ToolbarUse, When, toolbar_control};
use crate::nav::{Fold, Item, Motion, Navigable, Place, Spot, Step, Target};
use crate::net::{NetClient, NetResult};
use crate::theme::colors;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueCommand {
    Start,
    Stop,
    Approve,
    Retry,
    Expand,
    OpenSession,
}

enum Reply {
    Snapshot(PlanId, NetResult<QueueSnapshot>),
    Done(NetResult<()>),
}

/// What the queue view asks of the other views.
#[derive(Default)]
pub struct QueueRequests {
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
    /// Jobs drawn last frame, and where; and the viewport.
    spots: Vec<Spot>,
    view_h: f32,
    /// Scroll the selected job into view next frame.
    scroll: bool,
}

fn push(inbox: &Arc<Mutex<Vec<Reply>>>, reply: Reply) {
    if let Ok(mut i) = inbox.lock() {
        i.push(reply);
    }
}

/// The four steps of a component's work, as the stage track shows them.
const STEPS: [(arch_graph::JobKind, &str); 4] = [
    (arch_graph::JobKind::Design, "stage_arch_approved"),
    (arch_graph::JobKind::Plan, "stage_plan_approved"),
    (arch_graph::JobKind::Skeleton, "stage_skeleton"),
    (arch_graph::JobKind::Implement, "stage_implemented"),
];

/// A job's track: the four steps (by status name and word) and the one it's on.
fn track_of(job: &arch_graph::Job) -> (Vec<(&'static str, &'static str)>, Option<usize>) {
    let steps = STEPS.iter().map(|(kind, name)| (*name, kind.name())).collect();
    (steps, STEPS.iter().position(|(kind, _)| *kind == job.kind))
}

/// What a job waits for, in words (none when it's moving).
fn waiting(state: &JobState) -> Option<&'static str> {
    match state {
        JobState::Queued => Some("waiting its turn (and for what it depends on)"),
        JobState::AwaitingApproval => Some("waiting for your approval"),
        JobState::Retrying => Some("waiting to try again with what failed"),
        JobState::Running { .. } | JobState::Failed | JobState::Done => None,
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
                Reply::Snapshot(_, Err(e)) | Reply::Done(Err(e)) => self.error = Some(e.into()),
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

    fn jobs(&self) -> usize {
        self.snapshot.as_ref().map_or(0, |s| s.jobs.len())
    }

    fn put_cursor(&mut self, index: usize) {
        let n = self.jobs();
        if n > 0 {
            self.selected = index.min(n - 1);
            self.scroll = true;
        }
    }

    pub fn has_job(&self, id: &ComponentId) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.jobs.iter().any(|j| &j.component == id))
    }

    /// Selects the job for `id`, if the queue has one.
    pub fn select_component(&mut self, id: &ComponentId) -> bool {
        let at = self.snapshot.as_ref().and_then(|s| s.jobs.iter().position(|j| &j.component == id));
        if let Some(i) = at {
            self.put_cursor(i);
        }
        at.is_some()
    }

    fn half_page_jobs(&self) -> usize {
        let avg = if self.spots.is_empty() { 30.0 } else { self.spots.iter().map(|s| s.rect.height()).sum::<f32>() / self.spots.len() as f32 };
        if self.view_h > 0.0 { ((self.view_h / avg.max(8.0)) / 2.0).floor().max(1.0) as usize } else { 5 }
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
        let mut spots = Vec::new();
        let scroll = std::mem::take(&mut self.scroll);
        let output = egui::ScrollArea::vertical().id_salt("queue_jobs").auto_shrink([false, false]).show(ui, |ui| {
            let Some(snapshot) = &self.snapshot else {
                ui.label(RichText::new("Nothing to show yet.").color(colors().muted));
                return;
            };
            for (i, job) in snapshot.jobs.iter().enumerate() {
                let selected = i == self.selected;
                let mut frame = egui::Frame::new().inner_margin(crate::theme::margin(crate::theme::look().space_s)).stroke(egui::Stroke::new(crate::theme::look().line_thin, if selected { colors().focus } else { colors().border }));
                if selected {
                    frame = frame.fill(colors().selection);
                }
                let shown = frame.show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let name = job.component.as_str().split_once(':').map_or(job.component.as_str(), |(_, s)| s);
                        if ui.selectable_label(selected, RichText::new(name).strong()).clicked() {
                            clicked = Some((i, None));
                        }
                        crate::status::chip(ui, crate::status::job(&job.state), None);
                        if job.attempts > 0 {
                            ui.label(RichText::new(format!("attempt {}", job.attempts + 1)).color(colors().muted));
                        }
                        match &job.state {
                            JobState::AwaitingApproval if ui.button(format!("Approve {:?}", job.kind.goal())).on_hover_text("a").clicked() => clicked = Some((i, Some(QueueCommand::Approve))),
                            JobState::Failed if ui.button("Retry").on_hover_text("r").clicked() => clicked = Some((i, Some(QueueCommand::Retry))),
                            JobState::Running { thread: Some(_) } if ui.button("Session").on_hover_text("o").clicked() => clicked = Some((i, Some(QueueCommand::OpenSession))),
                            _ => {}
                        }
                        if !job.findings.is_empty() && ui.small_button(format!("{} findings", job.findings.len())).on_hover_text("Enter").clicked() {
                            clicked = Some((i, Some(QueueCommand::Expand)));
                        }
                    });
                    // Where it is in design, plan, skeleton, implement; and what it waits for.
                    ui.horizontal_wrapped(|ui| {
                        let (steps, current) = track_of(job);
                        crate::status::track(ui, &steps, current);
                        if let Some(why) = waiting(&job.state) {
                            ui.label(RichText::new(why).size(crate::theme::look().text_small).color(colors().muted));
                        }
                    });
                    if self.expanded.contains(&job.component) {
                        for f in &job.findings {
                            ui.horizontal_wrapped(|ui| {
                                crate::status::chip(ui, crate::status::severity(f.severity), None);
                                ui.label(RichText::new(format!("[{:?}] {}", f.code, f.message)).color(colors().text).size(crate::theme::look().text_body));
                            });
                        }
                    }
                });
                if selected && scroll {
                    shown.response.scroll_to_me(None);
                }
                spots.push(Spot { key: i.to_string(), rect: shown.response.rect });
            }
        });
        self.view_h = output.inner_rect.height();
        self.spots = spots.into_iter().filter(|s| s.rect.intersects(output.inner_rect)).collect();
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
            QueueCommand::Approve => match self.job().filter(|j| j.state == JobState::AwaitingApproval).cloned() {
                Some(job) => cx.net.arch_approve(&plan, &job.component, job.kind.goal(), &self.author, cx.ctx.clone(), self.done()),
                None => *cx.note = Some("Only a job waiting for your approval can be approved.".into()),
            },
            QueueCommand::Retry => match self.job().filter(|j| j.state == JobState::Failed).cloned() {
                Some(job) => cx.net.arch_queue_retry(&plan, &job.component, cx.ctx.clone(), self.done()),
                None => *cx.note = Some("Only a failed job can be retried.".into()),
            },
            QueueCommand::Expand => {
                if let Some(id) = self.job().map(|j| j.component.clone())
                    && !self.expanded.remove(&id)
                {
                    self.expanded.insert(id);
                }
            }
            QueueCommand::OpenSession => match self.job().map(|j| &j.state) {
                Some(JobState::Running { thread: Some(t) }) => self.requests.open_thread = Some(t.clone()),
                _ => *cx.note = Some("Only a running job has a session.".into()),
            },
        }
    }
}

impl ActionProvider for QueueState {
    fn actions(&self) -> Vec<Action> {
        use QueueCommand as Q;
        let f = |id: &str, label: &str, c: QueueCommand| Action::new(id, label, c).focused();
        let mut out = Vec::new();
        if self.plan.is_none() {
            return out;
        }
        let running = matches!(self.status(), Some(QueueStatus::Running | QueueStatus::WaitingForUser | QueueStatus::Stopping));
        if running {
            out.push(f("queue.stop", "Stop the queue (after the running job)", Q::Stop).toolbar(ToolbarStyle::Button, "Stop", Some(egui_phosphor::regular::STOP)).importance(MINOR));
        } else if self.status() != Some(QueueStatus::Done) {
            out.push(f("queue.start", "Start working through the plan", Q::Start).toolbar(ToolbarStyle::Button, "Start", Some(egui_phosphor::regular::PLAY)));
        }
        if let Some(job) = self.job() {
            if !job.findings.is_empty() {
                out.push(f("queue.expand", "Show or hide why the last attempt failed", Q::Expand).group(Group::View));
            }
            // Always listed (the key sheet shows them); they apply to some jobs.
            let applies = |yes: bool| if yes { When::Always } else { When::Unavailable };
            out.push(f("queue.approve", "Approve this component's design", Q::Approve).importance(PRIMARY).when(applies(job.state == JobState::AwaitingApproval)));
            out.push(f("queue.retry", "Give this job another round", Q::Retry).importance(PRIMARY).when(applies(job.state == JobState::Failed)));
            out.push(f("queue.open_session", "Open the session working on it", Q::OpenSession).group(Group::GoTo).importance(PRIMARY).when(applies(matches!(job.state, JobState::Running { thread: Some(_) }))));
        }
        out
    }

    fn run(&mut self, action: &Action, _value: Option<String>, cx: &mut RunCx<'_>) {
        if let Some(c) = action.command::<QueueCommand>().copied() {
            self.apply(c, cx);
        }
    }
}

impl Navigable for QueueState {
    fn place(&self) -> Place {
        Place::Queue { component: self.job().map(|j| j.component.clone()) }
    }

    fn restore(&mut self, place: &Place) {
        if let Place::Queue { component: Some(id) } = place {
            self.select_component(id);
        }
    }

    fn target(&self) -> Option<Target> {
        self.job().map(|j| Target::Job { component: j.component.clone() })
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let n = self.jobs();
        if n == 0 {
            return;
        }
        match motion {
            Motion::Down => self.put_cursor(self.selected + count),
            Motion::Up => self.put_cursor(self.selected.saturating_sub(count)),
            Motion::HalfDown => self.put_cursor(self.selected + self.half_page_jobs() * count),
            Motion::HalfUp => self.put_cursor(self.selected.saturating_sub(self.half_page_jobs() * count)),
            Motion::First => self.put_cursor(0),
            Motion::Last => self.put_cursor(n - 1),
            Motion::Line(i) => self.put_cursor(i.saturating_sub(1)),
            Motion::NextSibling | Motion::PrevSibling | Motion::Left | Motion::Right => {}
        }
    }

    /// Shows or hides why the job's last attempt failed.
    fn enter(&mut self) -> Step {
        match self.job() {
            Some(job) if !job.findings.is_empty() => {
                let id = job.component.clone();
                if !self.expanded.remove(&id) {
                    self.expanded.insert(id);
                }
                Step::Done
            }
            Some(_) => Step::Nothing("No findings on this job.".into()),
            None => Step::Nothing("The queue is empty.".into()),
        }
    }

    fn can_enter(&self) -> bool {
        self.job().is_some_and(|j| !j.findings.is_empty())
    }

    fn can_leave(&self) -> bool {
        self.job().is_some_and(|j| self.expanded.contains(&j.component))
    }

    fn leave(&mut self) -> Step {
        match self.job().map(|j| j.component.clone()) {
            Some(id) if self.expanded.remove(&id) => Step::Done,
            _ => Step::Nothing("Nothing open here.".into()),
        }
    }

    fn fold(&mut self, fold: Fold) {
        let with_findings: Vec<ComponentId> = self.snapshot.as_ref().map(|s| s.jobs.iter().filter(|j| !j.findings.is_empty()).map(|j| j.component.clone()).collect()).unwrap_or_default();
        let here = self.job().map(|j| j.component.clone());
        match (fold, here) {
            (Fold::OpenAll, _) => self.expanded = with_findings.into_iter().collect(),
            (Fold::CloseAll, _) => self.expanded.clear(),
            (Fold::Open, Some(id)) if with_findings.contains(&id) => {
                self.expanded.insert(id);
            }
            (Fold::Close, Some(id)) => {
                self.expanded.remove(&id);
            }
            (Fold::Toggle, Some(_)) => {
                self.enter();
            }
            _ => {}
        }
    }

    fn items(&self) -> Vec<Item> {
        let Some(snapshot) = &self.snapshot else { return Vec::new() };
        snapshot
            .jobs
            .iter()
            .enumerate()
            .map(|(i, j)| {
                let symbol = j.component.as_str().split_once(':').map_or(j.component.as_str(), |(_, s)| s);
                let name = symbol.rsplit([':', '.']).next().unwrap_or(symbol);
                Item::new(i.to_string(), name, format!("{symbol} {} {}", j.kind.name(), crate::status::look(crate::status::job(&j.state)).1))
            })
            .collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        (self.jobs() > 0).then(|| self.selected.to_string())
    }

    fn move_to(&mut self, key: &str) {
        if let Ok(i) = key.parse() {
            self.put_cursor(i);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use arch_graph::{Finding, FindingCode, JobKind, Target};

    fn id(s: &str) -> ComponentId {
        ComponentId::new(format!("rust:{s}"))
    }

    pub(crate) fn snapshot(status: QueueStatus) -> QueueSnapshot {
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

    /// The actions that apply to the selected job now.
    fn ids(q: &QueueState) -> Vec<String> {
        q.actions().into_iter().filter(|a| a.when != When::Unavailable).map(|a| a.id).collect()
    }

    #[test]
    fn queue_sessions_approvals_and_findings_are_visible_and_actionable() {
        let ctx = egui::Context::default();
        let net = NetClient::new("http://localhost:0");
        let mut q = QueueState { author: "izo".into(), ..QueueState::default() };
        q.tick(&net, &ctx, Some(&PlanId::new("p").unwrap()));
        q.on_event(&ArchEvent::QueueUpdated { snapshot: Box::new(snapshot(QueueStatus::Running)) });
        assert_eq!(q.snapshot.as_ref().unwrap().jobs.len(), 3, "the pushed snapshot is shown");
        let mut mode = crate::keymap::InputMode::Normal;
        let mut note = None;
        let mut cx = RunCx { net: &net, ctx: &ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[] };
        // The running job links to its session.
        assert!(ids(&q).contains(&"queue.open_session".to_owned()) && ids(&q).contains(&"queue.stop".to_owned()));
        q.apply(QueueCommand::OpenSession, &mut cx);
        assert_eq!(q.requests.open_thread.take(), Some("t1".into()));
        // Approval waits on the user: the action is offered on that job only.
        q.motion(Motion::Down, 1);
        assert!(ids(&q).contains(&"queue.approve".to_owned()) && !ids(&q).contains(&"queue.retry".to_owned()));
        assert_eq!(q.target(), Some(crate::nav::Target::Job { component: id("a") }), "the job's component is the cursor's target");
        // Failed work shows why and can be retried.
        q.motion(Motion::Down, 1);
        assert!(ids(&q).contains(&"queue.retry".to_owned()) && ids(&q).contains(&"queue.expand".to_owned()));
        assert_eq!(q.enter(), Step::Done, "Enter shows the findings");
        assert!(q.expanded.contains(&id("c")));
        assert_eq!(q.leave(), Step::Done, "Backspace hides them");
        q.apply(QueueCommand::Expand, &mut cx);
        assert!(q.expanded.contains(&id("c")));
        q.motion(Motion::Down, 1);
        assert_eq!(q.selected, 2, "stops at the last job");
        q.motion(Motion::First, 1);
        assert_eq!(q.selected, 0);
        q.motion(Motion::Last, 1);
        assert_eq!(q.selected, 2);
        assert!(q.select_component(&id("a")) && q.selected == 1, "a go-to finds a component's job");
        assert!(!q.select_component(&id("zzz")));
        let found = crate::nav::rank_matches(&q.items(), "c");
        assert_eq!(q.items()[found[0]].key, "2", "search finds jobs by name first");
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
                let mut note = None;
                let mut cx = RunCx { net: &net, ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[] };
                q.render(ui, &mut cx);
            });
        });
    }

    #[test]
    fn each_job_shows_its_four_steps_and_what_it_waits_for() {
        let config = crate::config::UiConfig::with_overrides("").unwrap();
        crate::status::set(config.status.clone());
        let ctx = egui::Context::default();
        crate::theme::set(config.theme.clone(), &ctx);
        let net = NetClient::new("http://localhost:0");
        let mut q = QueueState::default();
        q.tick(&net, &ctx, Some(&PlanId::new("p").unwrap()));
        q.on_event(&ArchEvent::QueueUpdated { snapshot: Box::new(snapshot(QueueStatus::Running)) });
        let jobs = q.snapshot.clone().unwrap().jobs;
        // The design job is on the first step, the implement jobs on the last.
        let (steps, current) = track_of(&jobs[1]);
        assert_eq!(steps.iter().map(|(_, w)| *w).collect::<Vec<_>>(), ["design", "plan", "skeleton", "implement"]);
        assert_eq!((current, track_of(&jobs[0]).1), (Some(0), Some(3)));
        q.expanded.insert(id("c"));
        let out = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (mut mode, mut note) = (crate::keymap::InputMode::Normal, None);
                q.render(ui, &mut RunCx { net: &net, ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[] });
            });
        });
        fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
                egui::Shape::Vec(list) => list.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        out.shapes.iter().for_each(|c| collect(&c.shape, &mut texts));
        for step in ["design", "plan", "skeleton", "implement"] {
            assert_eq!(texts.iter().filter(|t| *t == step).count(), 3, "every job's track names {step}: {texts:?}");
        }
        assert!(texts.iter().any(|t| t == "waiting for your approval"), "the waiting job says what for");
        assert_eq!(texts.iter().filter(|t| t.starts_with("waiting")).count(), 1, "moving and failed jobs don't wait");
        // States are icon and word.
        for state in ["job_running", "job_awaiting_approval", "job_failed", "severity_error"] {
            let (glyph, word, _) = crate::status::look(state);
            assert!(texts.iter().any(|t| *t == format!("{glyph} {word}")), "{state} as a chip: {texts:?}");
        }
    }
}
