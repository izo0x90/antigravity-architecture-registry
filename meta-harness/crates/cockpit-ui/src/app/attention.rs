//! What needs the user, in the shell: the bell (what's waiting, and
//! whether system notifications are on) and the tray (updates), their
//! lists, the quick look on the inbox, the inbox's asks (the next item on
//! whichever server, its chat, putting it away, an answer), chats seen.

use harness_contracts::{Level, Notification, NotificationId, SlotId};

use super::*;
use crate::inbox::InboxAsk;

/// How often a chat on screen says its slot was seen (seconds).
const SEEN_EVERY: f64 = 2.0;

/// The bell's list's first row: the system notifications switch.
const SWITCH: &str = "alerts";

impl CockpitApp {
    /// The item the quick look shows, while it shows the inbox.
    #[cfg(test)]
    pub(super) fn looked_at(&self) -> Option<&NotificationId> {
        self.quick_look.as_ref().filter(|q| q.kind == BufferKind::Inbox).and(self.desk.inbox.item.as_ref())
    }

    /// The quick look on the inbox, showing `id` (a card shown there is closed first).
    pub(super) fn open_attention(&mut self, id: NotificationId, ctx: &egui::Context) {
        if self.quick_look.as_ref().is_some_and(|q| q.kind == BufferKind::Spec) {
            self.close_quick_look();
        }
        self.desk.inbox.show(id);
        if self.quick_look.is_none() {
            let now = ctx.input(|i| i.time);
            let (slot, over) = (self.desk.columns.focused(), self.desk.columns.focused_kind());
            self.quick_look = Some(QuickLook::open(BufferKind::Inbox, slot, over, self.desk.used_pane(BufferKind::Spec).id, None, now));
        }
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
        let mut all: Vec<(String, Notification)> = self.desks().flat_map(|d| crate::attention::list(&d.chat.store, level).into_iter().map(|n| (d.server.clone(), n.clone()))).collect();
        all.sort_by(|a, b| harness_contracts::order(&a.1, &b.1));
        all
    }

    /// The one `by` steps from the inbox's, across every server (none past
    /// either end; from one handled meanwhile, the list's start or end).
    fn step(&self, by: i32) -> Option<(String, NotificationId)> {
        let all = self.all_open(None);
        let from = self.desk.inbox.item.as_ref();
        let at = match all.iter().position(|(s, n)| *s == self.desk.server && Some(&n.id) == from) {
            Some(at) => at as i64 + i64::from(by),
            None if by > 0 => 0,
            None => all.len() as i64 - 1,
        };
        usize::try_from(at).ok().and_then(|at| all.get(at)).map(|(s, n)| (s.clone(), n.id.clone()))
    }

    /// Shows item `id` of `server` in the inbox: in the quick look when
    /// it's open, else in the inbox's column (the desk switched first).
    fn inbox_to(&mut self, server: &str, id: NotificationId, ctx: &egui::Context) {
        if self.quick_look.as_ref().is_some_and(|q| q.kind == BufferKind::Inbox) {
            let look = self.quick_look.take();
            self.switch_to(server, ctx);
            // Over the desk shown now (else it'd close, its column gone).
            self.quick_look = look.map(|mut look| {
                (look.slot, look.over) = (self.desk.columns.focused(), self.desk.columns.focused_kind());
                look
            });
            self.desk.inbox.show(id);
        } else {
            self.switch_to(server, ctx);
            self.desk.inbox.show(id);
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
                    let store = &self.desks().find(|d| &d.server == server).expect("a desk per server listed").chat.store;
                    let detail = crate::attention::detail(n, store, now);
                    Choice::new(format!("{server}/{}", n.id), &n.title).detail(Some(if many { format!("{server} · {detail}") } else { detail }))
                })
                .collect()
        };
        let (switch, why) = self.alerts_words();
        let mut waiting = vec![Choice::new(SWITCH, &switch).detail(why.clone())];
        waiting.extend(rows(Level::Waiting));
        let pick = |id: &str, label: &str, choices: Vec<Choice>| Action::new(id, label, S::AttentionPick).param(Param::Choice(choices)).group(Group::Act).importance(SECONDARY);
        let mut out = vec![pick("attention.waiting", "What's waiting on you", waiting)];
        let updates = rows(Level::Update);
        if !updates.is_empty() {
            out.push(pick("attention.updates", "Updates", updates));
        }
        out.push(Action::new("attention.next_waiting", "Go to what's waiting next", S::AttentionNextWaiting).group(Group::Act).importance(SECONDARY));
        out.push(Action::new("alerts.toggle", switch, S::Alerts).detail(why.unwrap_or_default()).group(Group::Act).importance(MINOR));
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

    /// The inbox reads what it shows (each frame) and its asks are done.
    pub(super) fn inbox_tick(&mut self, ctx: &egui::Context) {
        self.sync_inbox();
        self.inbox_asks(ctx);
    }

    pub(super) fn sync_inbox(&mut self) {
        let all: Vec<(String, NotificationId)> = self.all_open(None).into_iter().map(|(s, n)| (s, n.id)).collect();
        let server = self.desk.server.clone();
        let at = |id: &NotificationId| all.iter().position(|(s, n)| *s == server && n == id).map(|i| (i + 1, all.len()));
        self.desk.inbox.sync(&self.desk.chat, at, crate::attention::now_ms());
    }

    /// Does what the inbox asked (after its keys, and each frame for its buttons).
    pub(super) fn inbox_asks(&mut self, ctx: &egui::Context) {
        for ask in self.desk.inbox.take_asks() {
            match ask {
                InboxAsk::Step(by) => match self.step(by) {
                    Some((server, next)) => self.inbox_to(&server, next, ctx),
                    None => self.note = Some(if by > 0 { "That was the last one." } else { "That was the first one." }.into()),
                },
                InboxAsk::OpenChat(slot) => {
                    self.close_quick_look();
                    let from = self.jump_here();
                    self.focus_kind(BufferKind::Chat);
                    self.use_focused();
                    self.desk.chat.ui.slot = slot;
                    self.desk.chat.ui.cursor = None;
                    self.after_jump(from);
                }
                InboxAsk::Dismiss(id) => {
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
                InboxAsk::Answer { thread, request, option } => self.desk.chat.answer(&self.desk.net, ctx, &thread, &request, &option),
            }
        }
    }

    /// Chats on screen say their slots were seen (now and then, and only
    /// with updates open for them); a system notification clicked opens
    /// its item.
    pub(super) fn say_seen(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let drawn: Vec<SlotId> = self.desk.chat.drawn.iter().cloned().collect();
        for slot in drawn {
            let has_updates = self.desk.chat.store.notifications.values().any(|n| n.level == Level::Update && n.slot.as_ref() == Some(&slot));
            if has_updates && self.seen.due(&slot, now, SEEN_EVERY) {
                self.desk.net.notifications_seen(&slot, ctx.clone(), |_| {});
            }
        }
        if let Some((server, id)) = self.alerts.take_clicked() {
            self.open_attention_at(&server, id, ctx);
        }
        if let Some(last) = self.notes_later.lock().ok().and_then(|mut n| std::mem::take(&mut *n).pop()) {
            self.note = Some(last);
        }
    }
}
