//! What needs the user, in the shell: the bell (what's waiting, and
//! whether system notifications are on) and the tray (updates), their
//! lists, the quick look on the inbox, what the inbox asks about them (the
//! next item on whichever server, putting it away).

use harness_contracts::{Level, Notification, NotificationId};

use super::*;
use crate::nav::Waiting;

/// The bell's list's first row: the system notifications switch.
const SWITCH: &str = "alerts";

impl CockpitApp {
    /// The item the quick look shows, while it shows the inbox.
    #[cfg(test)]
    pub(super) fn looked_at(&self) -> Option<NotificationId> {
        self.quick_look.as_ref().filter(|q| q.kind() == BufferKind::Inbox)?;
        self.inbox_item()
    }

    /// The quick look on the inbox, showing `id` (what it held is put back first).
    pub(super) fn open_attention(&mut self, id: NotificationId, ctx: &egui::Context) {
        if self.quick_look.as_ref().is_none_or(|q| q.kind() != BufferKind::Inbox) {
            self.look_at(BufferId::new(BufferKind::Inbox, 0), None, ctx);
        }
        self.inbox_show(id);
    }

    /// The quick look on server `server`'s item `id` (its desk shown first).
    pub(super) fn open_attention_at(&mut self, server: &str, id: NotificationId, ctx: &egui::Context) {
        self.switch_to(server, ctx);
        if self.desk.server == server {
            self.open_attention(id, ctx);
        }
    }

    /// Every server's open ones of `level` (both, given none), in the order
    /// they're gone through, each with its server.
    pub(super) fn all_open(&self, level: Option<Level>) -> Vec<(String, Notification)> {
        let mut all: Vec<(String, Notification)> = self.desks().flat_map(|d| crate::attention::list(d.shared.agents.get().store(), level).into_iter().map(|n| (d.server.clone(), n.clone())).collect::<Vec<_>>()).collect();
        all.sort_by(|a, b| harness_contracts::order(&a.1, &b.1));
        all
    }

    /// The item the inbox shows.
    fn inbox_item(&self) -> Option<NotificationId> {
        match self.desk.view(BufferKind::Inbox).target() {
            Some(Target::Notification(id)) => Some(id),
            _ => None,
        }
    }

    /// The inbox shows item `id`.
    fn inbox_show(&mut self, id: NotificationId) {
        let _ = self.desk.view_mut(BufferKind::Inbox).show_target(&Target::Notification(id));
    }

    /// The one `by` steps from the inbox's, across every server (none past
    /// either end; from one handled meanwhile, the list's start or end).
    fn step(&self, by: i32) -> Option<(String, NotificationId)> {
        let all = self.all_open(None);
        let from = self.inbox_item();
        let from = from.as_ref();
        let at = match all.iter().position(|(s, n)| *s == self.desk.server && Some(&n.id) == from) {
            Some(at) => at as i64 + i64::from(by),
            None if by > 0 => 0,
            None => all.len() as i64 - 1,
        };
        usize::try_from(at).ok().and_then(|at| all.get(at)).map(|(s, n)| (s.clone(), n.id.clone()))
    }

    /// Shows item `id` of `server` in the inbox: in the quick look when
    /// it's open, else in the inbox's panel (the desk switched first).
    fn inbox_to(&mut self, server: &str, id: NotificationId, ctx: &egui::Context) {
        if self.quick_look.as_ref().is_some_and(|q| q.kind() == BufferKind::Inbox) {
            let look = self.quick_look.take();
            self.switch_to(server, ctx);
            // Over the desk shown now (else it'd close, its panel gone).
            self.quick_look = look.map(|mut look| {
                (look.panel, look.over) = (self.desk.panels.focused(), self.desk.panels.focused_kind());
                look
            });
            self.inbox_show(id.clone());
        } else {
            self.switch_to(server, ctx);
            self.inbox_show(id.clone());
        }
    }

    /// The shell's notification actions: the bell's and the tray's lists,
    /// the next waiting, the system notifications switch.
    pub(super) fn attention_actions(&self) -> Vec<Action> {
        use ShellCommand as S;
        let now = crate::attention::now_ms();
        let many = self.servers.len() > 1;
        let rows = |level: Level| -> Vec<Choice> {
            self.all_open(Some(level))
                .iter()
                .map(|(server, n)| {
                    let agents = self.desks().find(|d| &d.server == server).expect("a desk per server listed").shared.agents.get();
                    let detail = crate::attention::detail(n, agents.store(), now);
                    Choice::new(format!("{server}/{}", n.id), &n.title).detail(Some(if many { format!("{server} · {detail}") } else { detail }))
                })
                .collect()
        };
        let (switch, why) = self.alerts_words();
        let mut waiting = vec![Choice::new(SWITCH, &switch).detail(why.clone())];
        waiting.extend(rows(Level::Waiting));
        let pick = |id: &str, label: &str, detail: &str, choices: Vec<Choice>| Action::new(id, label, detail, S::AttentionPick).param(Param::Choice(choices)).group(Group::Act).importance(SECONDARY);
        let mut out = vec![pick("attention.waiting", "What's waiting on you", "Approvals, questions and sign-ins the agents wait on, on every server: pick one to answer it", waiting)];
        let updates = rows(Level::Update);
        if !updates.is_empty() {
            out.push(pick("attention.updates", "Updates", "What happened that you'd want to know (a turn done, a report, a long command finished): pick one to see it", updates));
        }
        out.push(Action::new("attention.next_waiting", "Go to what's waiting next", "Opens the next thing an agent waits on you for, on whichever server it is", S::AttentionNextWaiting).group(Group::Act).importance(SECONDARY));
        out.push(Action::new("alerts.toggle", switch, why.unwrap_or_else(|| "Whether the browser tells you, outside the cockpit, when something new waits on you".into()), S::Alerts).group(Group::Act).importance(MINOR));
        out
    }

    /// The system notifications switch in words, and what's in the way.
    pub(super) fn alerts_words(&self) -> (String, Option<String>) {
        use crate::alerts::AlertState as A;
        match self.alerts.state() {
            A::Off => ("Notify me outside the cockpit: off".into(), Some("Turns them on: what newly waits on you, while the cockpit is out of sight".into())),
            A::On => ("Notify me outside the cockpit: on".into(), None),
            A::NeedsPermission => ("Notify me outside the cockpit: on, not allowed yet".into(), Some("Choosing it asks the browser to allow them".into())),
            A::Blocked => ("Notify me outside the cockpit: on, refused".into(), Some("Allow them in the browser's site settings (choosing it turns them off)".into())),
            A::Unavailable(why) => ("Notify me outside the cockpit: unavailable".into(), Some(why.into())),
        }
    }

    pub(super) fn run_attention(&mut self, command: ShellCommand, value: Option<String>, ctx: &egui::Context) {
        match command {
            ShellCommand::AttentionNextWaiting => match self.all_open(Some(Level::Waiting)).into_iter().next() {
                Some((server, n)) => self.open_attention_at(&server, n.id, ctx),
                None => self.note = Some("Nothing is waiting on you.".into()),
            },
            ShellCommand::AttentionPick => match value.as_deref() {
                Some(SWITCH) => self.run_attention(ShellCommand::Alerts, None, ctx),
                Some(v) => {
                    if let Some((server, id)) = v.split_once('/') {
                        self.open_attention_at(server, NotificationId::new(id), ctx);
                    }
                }
                None => {}
            },
            ShellCommand::Alerts => {
                self.alerts.toggle();
                if let crate::alerts::AlertState::Unavailable(why) = self.alerts.state() {
                    self.note = Some(format!("No system notifications here: {why}."));
                }
            }
            _ => {}
        }
    }

    /// Every buffer reads what they share, every server's open items (the
    /// inbox's place among them) with it.
    pub(super) fn share_with_buffers(&mut self) {
        let open = self.all_open(None).into_iter().map(|(s, n)| (s, n.id)).collect();
        self.desk.shared.open.set(open);
        self.desk.share();
    }

    /// Does what the inbox asked about what waits on the user.
    pub(super) fn do_waiting(&mut self, ask: Waiting, ctx: &egui::Context) {
        match ask {
            Waiting::Step(by) => match self.step(by) {
                Some((server, next)) => self.inbox_to(&server, next, ctx),
                None => self.note = Some(if by > 0 { "That was the last one." } else { "That was the first one." }.into()),
            },
            Waiting::Dismiss(id) => {
                let next = self.step(1);
                let later = self.notes_later.clone();
                self.desk.net.dismiss_notification(&id, ctx.clone(), move |r| {
                    if let (Err(e), Ok(mut notes)) = (r, later.lock()) {
                        notes.push(format!("It couldn't be put away: {}", e.message));
                    }
                });
                match next {
                    Some((server, next)) => self.inbox_to(&server, next, ctx),
                    None => self.close_quick_look(),
                }
            }
        }
    }

    /// A system notification clicked opens its item; notes that came
    /// later are said.
    pub(super) fn say_seen(&mut self, ctx: &egui::Context) {
        if let Some((server, id)) = self.alerts.take_clicked() {
            self.open_attention_at(&server, id, ctx);
        }
        if let Some(last) = self.notes_later.lock().ok().and_then(|mut n| std::mem::take(&mut *n).pop()) {
            self.note = Some(last);
        }
    }
}
