//! What needs the user, in the cockpit: the counters' pickers, the quick
//! look on one notification (answered in place, the next a key away), the
//! chats seen, and the browser's notifications.

use harness_contracts::{ApprovalDecision, Level, NotificationId, RequestStatus, SlotId};

use super::*;
use crate::quick_look::Look;

/// The quick look's request answered with this option.
#[derive(Clone, Debug)]
pub(super) struct LookAnswer(pub String);

/// The quick look's own keys on a notification: offered only while it's
/// open, and winning over the keys of the view under it.
#[cfg(test)]
pub(crate) const LOOK_ONLY: &[&str] = &["attention.next", "attention.prev", "attention.open_slot", "attention.dismiss"];

/// How often a chat on screen says its slot was seen (seconds).
const SEEN_EVERY: f64 = 2.0;

impl CockpitApp {
    /// The notification the quick look shows.
    pub(super) fn looked_at(&self) -> Option<&NotificationId> {
        self.quick_look.as_ref().and_then(|q| q.attention())
    }

    /// The quick look on notification `id` (a card shown there is closed first).
    pub(super) fn open_attention(&mut self, id: NotificationId, ctx: &egui::Context) {
        if self.quick_look.as_ref().is_some_and(|q| q.look == Look::Card) {
            self.close_quick_look();
        }
        let now = ctx.input(|i| i.time);
        let (slot, over) = (self.columns.focused(), self.columns.focused_kind());
        self.quick_look = Some(QuickLook::open(Look::Attention(id), slot, over, self.used_pane(BufferKind::Spec).id, None, now));
    }

    /// The shell's notification actions: the pickers, the next waiting, the
    /// browser's notifications.
    pub(super) fn attention_actions(&self) -> Vec<Action> {
        use ShellCommand as S;
        let mut out = Vec::new();
        let now = crate::attention::now_ms();
        let store = &self.chat.store;
        let picker = |id: &str, label: &str, level: Level| {
            let list = crate::attention::list(store, Some(level));
            (!list.is_empty()).then(|| {
                let choices = list.iter().map(|n| Choice::new(n.id.as_str(), &n.title).detail(Some(crate::attention::detail(n, store, now)))).collect();
                Action::new(id, label, S::AttentionPick).param(Param::Choice(choices)).group(Group::Act).importance(SECONDARY)
            })
        };
        out.extend(picker("attention.waiting", "What's waiting on you", Level::Waiting));
        out.extend(picker("attention.updates", "Updates", Level::Update));
        out.push(Action::new("attention.next_waiting", "Go to what's waiting next", S::AttentionNextWaiting).group(Group::Act).importance(SECONDARY));
        out.push(
            Action::new("attention.browser", "Browser notifications while this tab is hidden", S::BrowserAlerts)
                .detail("For what newly waits on you; the browser asks to allow them")
                .param(Param::Toggle { on: self.alerts.on() })
                .group(Group::Act)
                .importance(MINOR),
        );
        out
    }

    /// The quick look's own actions on its notification (they win key
    /// conflicts while it's open): next and previous, its request's options,
    /// its slot's chat, put away.
    pub(super) fn looked_at_actions(&self) -> Vec<Action> {
        use ShellCommand as S;
        let Some(id) = self.looked_at() else { return Vec::new() };
        let act = |id: &str, label: &str, c: ShellCommand| Action::new(id, label, c).group(Group::Act).importance(PRIMARY);
        let mut out = vec![act("attention.next", "Next notification", S::AttentionStep(1)), act("attention.prev", "Previous notification", S::AttentionStep(-1))];
        let Some(n) = self.chat.store.notifications.get(id) else { return out };
        if let Some(request) = self.pending_request(id) {
            for option in &request.options {
                let action = Action::new(format!("attention.answer.{}", option.id), format!("{}: {}", option.label, request.title), LookAnswer(option.id.clone()))
                    .alias(format!("request.{}", crate::chat::actions::wire_name(&option.decision)))
                    .group(Group::Act)
                    .importance(if option.decision == ApprovalDecision::Accept { PRIMARY } else { SECONDARY });
                out.push(match &option.warning {
                    Some(w) => action.detail(w.clone()),
                    None => action,
                });
            }
        }
        if n.slot.is_some() {
            out.push(act("attention.open_slot", "Open its chat", S::AttentionOpenSlot));
        }
        out.push(act("attention.dismiss", "Put it away", S::AttentionDismiss).importance(SECONDARY));
        out
    }

    /// The request a notification waits on, while it's pending.
    fn pending_request(&self, id: &NotificationId) -> Option<&harness_contracts::RuntimeRequest> {
        let n = self.chat.store.notifications.get(id)?;
        let request = self.chat.store.threads.get(n.thread.as_ref()?)?.request(n.request.as_ref()?)?;
        (request.status == RequestStatus::Pending).then_some(request)
    }

    pub(super) fn run_attention(&mut self, command: ShellCommand, value: Option<String>, ctx: &egui::Context) {
        match command {
            ShellCommand::AttentionNextWaiting => match crate::attention::list(&self.chat.store, Some(Level::Waiting)).first() {
                Some(n) => self.open_attention(n.id.clone(), ctx),
                None => self.note = Some("Nothing is waiting on you.".into()),
            },
            ShellCommand::AttentionPick => {
                if let Some(id) = value {
                    self.open_attention(NotificationId::new(id), ctx);
                }
            }
            ShellCommand::AttentionStep(by) => {
                let Some(from) = self.looked_at().cloned() else { return };
                match crate::attention::step(&self.chat.store, &from, by) {
                    Some(next) => self.open_attention(next, ctx),
                    None => self.note = Some(if by > 0 { "That was the last one." } else { "That was the first one." }.into()),
                }
            }
            ShellCommand::AttentionOpenSlot => {
                let Some(slot) = self.looked_at().and_then(|id| self.chat.store.notifications.get(id)).and_then(|n| n.slot.clone()) else { return };
                self.close_quick_look();
                let from = self.jump_here();
                self.focus_kind(BufferKind::Chat);
                self.use_focused();
                self.chat.ui.slot = slot;
                self.chat.ui.cursor = None;
                self.after_jump(from);
            }
            ShellCommand::AttentionDismiss => {
                let Some(id) = self.looked_at().cloned() else { return };
                let next = crate::attention::step(&self.chat.store, &id, 1);
                let later = self.notes_later.clone();
                self.net.dismiss_notification(&id, ctx.clone(), move |r| {
                    if let (Err(e), Ok(mut notes)) = (r, later.lock()) {
                        notes.push(format!("It couldn't be put away: {}", e.message));
                    }
                });
                match next {
                    Some(next) => self.open_attention(next, ctx),
                    None => self.close_quick_look(),
                }
            }
            ShellCommand::BrowserAlerts => self.alerts.set(value.as_deref() == Some("true")),
            _ => {}
        }
    }

    /// The quick look's request answered with `option` (as its keys and
    /// buttons do).
    pub(super) fn answer_looked_at(&mut self, option: &str, ctx: &egui::Context) {
        let Some(id) = self.looked_at().cloned() else { return };
        let Some((thread, request)) = self.pending_request(&id).map(|r| (r.thread_id.clone(), r.id.clone())) else {
            self.note = Some("Already answered.".into());
            return;
        };
        self.chat.answer(&self.net, ctx, &thread, &request, option);
    }

    /// The quick look on a notification: where it's from, what it says, the
    /// request to answer (if it waits on one), what the agent said last.
    /// True when its close button was clicked.
    pub(super) fn render_attention(&mut self, ui: &mut egui::Ui, id: &NotificationId) -> bool {
        use egui_phosphor::regular as icon;
        let theme = crate::theme::look();
        let key = |action: &str| self.bound_actions().into_iter().find(|b| b.action.id == action).map(|b| b.key_hint()).unwrap_or_default();
        let (next, prev, back) = (key("attention.next"), key("attention.prev"), self.config.keymap.back.label());
        let (open_key, dismiss_key) = (key("attention.open_slot"), key("attention.dismiss"));
        let all = crate::attention::list(&self.chat.store, None);
        let at = all.iter().position(|n| &n.id == id);
        let shown = self.chat.store.notifications.get(id).cloned();
        let (mut close, mut open_slot, mut dismiss) = (false, false, false);
        ui.horizontal(|ui| {
            let (what, color) = match shown.as_ref().map(|n| n.level) {
                Some(Level::Waiting) => (format!("{} Waiting on you", icon::BELL_RINGING), colors().warn),
                Some(Level::Update) => (format!("{} Update", icon::BELL), colors().info),
                None => (format!("{} Handled", icon::CHECK), colors().ok),
            };
            ui.label(RichText::new(what).strong().size(theme.text_label).color(color));
            if let Some(at) = at {
                ui.label(RichText::new(format!("{} of {}", at + 1, all.len())).size(theme.text_small).color(colors().muted));
            }
            ui.label(RichText::new(format!("{next} next · {prev} previous · {back} closes")).size(theme.text_small).color(colors().muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close |= ui.small_button(icon::X).on_hover_text("Close").clicked();
            });
        });
        ui.separator();
        let Some(n) = shown else {
            ui.label(RichText::new(format!("Already handled. {next} for the next.")).size(theme.text_body).color(colors().muted));
            return close;
        };
        let now = crate::attention::now_ms();
        let harness = n.slot.as_ref().and_then(|s| self.chat.store.slots.get(s)).and_then(|s| s.instance_id.as_ref()).and_then(|i| self.chat.store.instances.get(i)).map(|i| i.display_name.clone());
        let from = [Some(crate::attention::detail(&n, &self.chat.store, now)), harness].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        ui.label(RichText::new(from).size(theme.text_small).color(colors().muted));
        ui.label(RichText::new(&n.title).strong().size(theme.text_title).color(colors().text_strong));
        egui::ScrollArea::vertical().id_salt("attention_body").auto_shrink([false, true]).max_height(ui.available_height() - 40.0).show(ui, |ui| {
            if let Some(detail) = &n.detail {
                ui.add(egui::Label::new(RichText::new(detail).size(theme.text_body)).wrap());
            }
            if let (Some(thread), Some(request)) = (&n.thread, &n.request) {
                ui.add_space(theme.space_sm);
                if !self.chat.render_request_of(ui, &self.net, thread, request) {
                    ui.label(RichText::new("Its request is gone (its session ended).").size(theme.text_body).color(colors().muted));
                }
            }
            if let Some(said) = n.thread.as_ref().and_then(|t| self.chat.last_said(t)).filter(|s| n.detail.as_deref() != Some(s.as_str())) {
                ui.add_space(theme.space_sm);
                ui.label(RichText::new("It said last").size(theme.text_small).color(colors().muted));
                let said: String = said.chars().take(1200).collect();
                ui.add(egui::Label::new(RichText::new(said).size(theme.text_body)).wrap());
            }
        });
        ui.horizontal(|ui| {
            if n.slot.is_some() {
                open_slot = ui.button(format!("{} Open its chat", icon::CHAT_TEARDROP_TEXT)).on_hover_text(open_key).clicked();
            }
            dismiss = ui.button(format!("{} Put away", icon::ARCHIVE)).on_hover_text(dismiss_key).clicked();
        });
        let ctx = ui.ctx().clone();
        if open_slot {
            self.run_attention(ShellCommand::AttentionOpenSlot, None, &ctx);
        } else if dismiss {
            self.run_attention(ShellCommand::AttentionDismiss, None, &ctx);
        }
        close
    }

    /// The top bar's counters: what waits on the user, and the updates
    /// (each hidden at none); clicked, its picker.
    pub(super) fn render_attention_counters(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as icon;
        let (waiting, updates) = crate::attention::counts(&self.chat.store);
        let theme = crate::theme::look();
        let mut pick = None;
        if waiting > 0 {
            let r = ui.add(egui::Button::new(RichText::new(format!("{} {waiting}", icon::BELL_RINGING)).strong().size(theme.text_label).color(colors().warn)).frame(false)).on_hover_text(format!("{waiting} waiting on you"));
            self.feedback.register_inspectable(ui, "top_bar:waiting", r.rect);
            if r.clicked() {
                pick = Some("attention.waiting");
            }
        }
        if updates > 0 {
            let r = ui.add(egui::Button::new(RichText::new(format!("{} {updates}", icon::BELL)).size(theme.text_label).color(colors().muted)).frame(false)).on_hover_text(format!("{updates} update(s)"));
            self.feedback.register_inspectable(ui, "top_bar:updates", r.rect);
            if r.clicked() {
                pick = Some("attention.updates");
            }
        }
        if let Some(id) = pick {
            let ctx = ui.ctx().clone();
            self.run_by_id(id, None, &ctx);
        }
    }

    /// Chats on screen say their slots were seen (now and then, and only
    /// with updates open for them); a browser notification clicked opens
    /// its item.
    pub(super) fn say_seen(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let drawn: Vec<SlotId> = self.chat.drawn.iter().cloned().collect();
        for slot in drawn {
            let has_updates = self.chat.store.notifications.values().any(|n| n.level == Level::Update && n.slot.as_ref() == Some(&slot));
            if has_updates && self.seen.due(&slot, now, SEEN_EVERY) {
                self.net.notifications_seen(&slot, ctx.clone(), |_| {});
            }
        }
        if let Some(id) = self.alerts.take_clicked() {
            self.open_attention(id, ctx);
        }
        if let Some(last) = self.notes_later.lock().ok().and_then(|mut n| std::mem::take(&mut *n).pop()) {
            self.note = Some(last);
        }
    }
}
