//! The inbox: what needs the user and what happened (the open
//! notifications), one at a time. A view like the others: the cursor moves
//! over its parts (what it says, each answer to its request, what the
//! agent said last), the shared keys scroll it and label its answers, and
//! Enter answers or opens. It shows in a column, or in the quick look over
//! one. What it asks for beyond itself (the next item on another server,
//! its chat, putting it away, an answer) it hands the shell as an ask.

use egui::{RichText, Vec2};
use egui_phosphor::regular as icon;
use harness_contracts::{Level, Notification, NotificationId, RequestId, RuntimeRequest, SlotId, ThreadId};

use crate::actions::{Action, ActionProvider, Group, MINOR, PRIMARY, RunCx, SECONDARY};
use crate::chat::ChatStreamState;
use crate::nav::{Fold, Item, Motion, Navigable, Place, Spot, Step, Target};
use crate::theme::colors;

/// What the inbox asks the shell to do.
#[derive(Clone, Debug, PartialEq)]
pub enum InboxAsk {
    /// The next (or previous) item, on whichever server it is.
    Step(i32),
    OpenChat(SlotId),
    Dismiss(NotificationId),
    Answer { thread: ThreadId, request: RequestId, option: String },
}

/// The item shown, as last read from the store.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub notification: Notification,
    /// Where it's from: `slot · kind · age · harness`.
    pub from: String,
    /// Its request, while the store has it.
    pub request: Option<RuntimeRequest>,
    /// What the agent said last.
    pub said: Option<String>,
}

/// A part of the item, where the cursor can be.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    Says,
    /// An answer to its request (its option).
    Answer { option: String, label: String },
    Said,
}

#[derive(Default)]
pub struct InboxView {
    /// The item shown; none: the first open one.
    pub item: Option<NotificationId>,
    pub shown: Option<Shown>,
    /// Where it is among every server's open ones (1-based), and how many.
    pub at: Option<(usize, usize)>,
    cursor: usize,
    /// Scroll the cursor's part into view next frame.
    scroll: bool,
    spots: Vec<Spot>,
    asks: Vec<InboxAsk>,
}

#[derive(Clone, Debug)]
enum Command {
    Step(i32),
    OpenChat,
    Dismiss,
    Answer(String),
}

impl InboxView {
    /// Shows item `id` from the top.
    pub fn show(&mut self, id: NotificationId) {
        if self.item.as_ref() != Some(&id) {
            self.cursor = 0;
            self.scroll = true;
        }
        self.item = Some(id);
    }

    /// Reads what it shows from the store (each frame: it's a view of it).
    /// `at` places an item among every server's open ones.
    pub fn sync(&mut self, chat: &ChatStreamState, at: impl Fn(&NotificationId) -> Option<(usize, usize)>, now_ms: u64) {
        let store = &chat.store;
        if self.item.is_none() {
            self.item = crate::attention::list(store, None).first().map(|n| n.id.clone());
        }
        self.shown = self.item.as_ref().and_then(|id| store.notifications.get(id)).map(|n| {
            let harness = n.slot.as_ref().and_then(|s| store.slots.get(s)).and_then(|s| s.instance_id.as_ref()).and_then(|i| store.instances.get(i)).map(|i| i.display_name.clone());
            let from = [Some(crate::attention::detail(n, store, now_ms)), harness].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            let request = n.thread.as_ref().zip(n.request.as_ref()).and_then(|(t, r)| store.threads.get(t)?.request(r).cloned());
            let said = n.thread.as_ref().and_then(|t| chat.last_said(t)).filter(|s| n.detail.as_deref() != Some(s.as_str()));
            Shown { notification: n.clone(), from, request, said }
        });
        self.at = self.item.as_ref().and_then(at);
        self.cursor = self.cursor.min(self.parts().len().saturating_sub(1));
    }

    /// What it asked for since last taken.
    pub fn take_asks(&mut self) -> Vec<InboxAsk> {
        std::mem::take(&mut self.asks)
    }

    fn parts(&self) -> Vec<Part> {
        let Some(shown) = &self.shown else { return Vec::new() };
        let answers = shown.request.iter().flat_map(crate::request_card::answers).map(|a| Part::Answer { option: a.option, label: a.label });
        std::iter::once(Part::Says).chain(answers).chain(shown.said.as_ref().map(|_| Part::Said)).collect()
    }

    fn put_cursor(&mut self, at: usize) {
        self.cursor = at.min(self.parts().len().saturating_sub(1));
        self.scroll = true;
    }

    fn answer(&mut self, option: String) -> Step {
        let Some(request) = self.shown.as_ref().and_then(|s| s.request.as_ref()) else { return Step::Nothing("Nothing to answer: it was answered already.".into()) };
        self.asks.push(InboxAsk::Answer { thread: request.thread_id.clone(), request: request.id.clone(), option });
        Step::Done
    }

    fn open_chat(&mut self) -> Step {
        match self.shown.as_ref().and_then(|s| s.notification.slot.clone()) {
            Some(slot) => {
                self.asks.push(InboxAsk::OpenChat(slot));
                Step::Done
            }
            None => Step::Nothing("It's from no agent's chat.".into()),
        }
    }

    pub fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, chat: &mut ChatStreamState) {
        let theme = crate::theme::look();
        ui.horizontal(|ui| {
            let (what, color) = match self.shown.as_ref().map(|s| s.notification.level) {
                Some(Level::Waiting) => (format!("{} Waiting on you", icon::BELL_RINGING), colors().warn),
                Some(Level::Update) => (format!("{} Update", icon::TRAY), colors().info),
                None if self.item.is_some() => (format!("{} Handled", icon::CHECK), colors().ok),
                None => (format!("{} Nothing needs you", icon::CHECK), colors().ok),
            };
            ui.label(RichText::new(what).strong().size(theme.text_label).color(color));
            if let Some((at, of)) = self.at {
                ui.label(RichText::new(format!("{at} of {of}")).size(theme.text_small).color(colors().muted));
            }
        });
        ui.separator();
        let Some(shown) = self.shown.clone() else {
            if self.item.is_some() {
                ui.label(RichText::new("Already handled: the next one is a key away.").size(theme.text_body).color(colors().muted));
            }
            self.spots.clear();
            return;
        };
        let parts = self.parts();
        let mut drawn = Vec::new();
        let (cursor, scroll) = (self.cursor, std::mem::take(&mut self.scroll));
        let cursor_at = cursor;
        let mark = |ui: &mut egui::Ui, rect: egui::Rect, index: usize, drawn: &mut Vec<Spot>| {
            drawn.push(Spot { key: index.to_string(), rect });
            if index == cursor {
                ui.painter().rect_stroke(rect.expand(2.0), theme.radius_m, egui::Stroke::new(theme.line_thin, colors().focus), egui::StrokeKind::Outside);
                if scroll {
                    ui.scroll_to_rect(rect, Some(egui::Align::Center));
                }
            }
        };
        egui::ScrollArea::vertical().id_salt(("inbox", cx.slot)).auto_shrink([false, true]).show(ui, |ui| {
            let says = ui
                .vertical(|ui| {
                    ui.label(RichText::new(&shown.from).size(theme.text_small).color(colors().muted));
                    ui.label(RichText::new(&shown.notification.title).strong().size(theme.text_title).color(colors().text_strong));
                    if let Some(detail) = &shown.notification.detail {
                        ui.add(egui::Label::new(RichText::new(detail).size(theme.text_body)).wrap());
                    }
                })
                .response
                .rect;
            mark(ui, says, 0, &mut drawn);
            if let (Some(thread), Some(request)) = (&shown.notification.thread, &shown.notification.request) {
                ui.add_space(theme.space_sm);
                // The card's own answers are the cursor's parts (and the hints').
                let cursor = match parts.get(cursor) {
                    Some(Part::Answer { option, .. }) => Some(option.as_str()),
                    _ => None,
                };
                match chat.render_request_of(ui, cx.net, thread, request, &crate::request_card::Shown { cursor, hints: cx.hints }) {
                    Some(card) => {
                        for (index, part) in parts.iter().enumerate() {
                            if let Part::Answer { option, .. } = part
                                && let Some((_, rect)) = card.rows.iter().find(|(o, _)| o == option)
                            {
                                drawn.push(Spot { key: index.to_string(), rect: *rect });
                                if index == cursor_at && scroll {
                                    ui.scroll_to_rect(*rect, Some(egui::Align::Center));
                                }
                            }
                        }
                    }
                    None => {
                        ui.label(RichText::new("Its request is gone (its session ended).").size(theme.text_body).color(colors().muted));
                    }
                }
            }
            if let Some(said) = &shown.said {
                ui.add_space(theme.space_sm);
                let rect = ui
                    .vertical(|ui| {
                        ui.label(RichText::new("It said last").size(theme.text_small).color(colors().muted));
                        let said: String = said.chars().take(1200).collect();
                        ui.add(egui::Label::new(RichText::new(said).size(theme.text_body)).wrap());
                    })
                    .response
                    .rect;
                mark(ui, rect, parts.len() - 1, &mut drawn);
            }
            ui.add_space(theme.space_sm);
            ui.horizontal(|ui| {
                if shown.notification.slot.is_some() && ui.button(format!("{} Open its chat", icon::CHAT_TEARDROP_TEXT)).clicked() {
                    let _ = self.open_chat();
                }
                if ui.button(format!("{} Put away", icon::ARCHIVE)).clicked() {
                    self.asks.push(InboxAsk::Dismiss(shown.notification.id.clone()));
                }
            });
            ui.allocate_space(Vec2::ZERO);
        });
        self.spots = drawn;
    }
}

impl Navigable for InboxView {
    fn place(&self) -> Place {
        Place::Inbox { item: self.item.as_ref().map(|i| i.to_string()) }
    }

    fn restore(&mut self, place: &Place) {
        if let Place::Inbox { item: Some(id) } = place {
            self.show(NotificationId::new(id.as_str()));
        }
    }

    fn target(&self) -> Option<Target> {
        None
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let n = self.parts().len();
        if n == 0 {
            return;
        }
        match motion {
            Motion::Down => self.put_cursor(self.cursor + count),
            Motion::Up => self.put_cursor(self.cursor.saturating_sub(count)),
            Motion::HalfDown => self.put_cursor(self.cursor + n.div_ceil(2) * count),
            Motion::HalfUp => self.put_cursor(self.cursor.saturating_sub(n.div_ceil(2) * count)),
            Motion::First => self.put_cursor(0),
            Motion::Last => self.put_cursor(n - 1),
            Motion::Line(i) => self.put_cursor(i.saturating_sub(1)),
            Motion::NextSibling | Motion::PrevSibling | Motion::Left | Motion::Right => {}
        }
    }

    /// An answer under the cursor answers; anything else opens its chat.
    fn enter(&mut self) -> Step {
        match self.parts().get(self.cursor).cloned() {
            Some(Part::Answer { option, .. }) => self.answer(option),
            Some(_) => self.open_chat(),
            None => Step::Nothing("Nothing needs you.".into()),
        }
    }

    fn can_enter(&self) -> bool {
        self.shown.is_some()
    }

    fn enter_label(&self) -> Option<String> {
        match self.parts().get(self.cursor)? {
            Part::Answer { label, .. } => Some(label.clone()),
            _ => self.shown.as_ref()?.notification.slot.as_ref().map(|_| "Open its chat".to_owned()),
        }
    }

    fn can_leave(&self) -> bool {
        false
    }

    fn leave(&mut self) -> Step {
        Step::Nothing("The inbox has nothing to go out of.".into())
    }

    fn fold(&mut self, _fold: Fold) {}

    fn items(&self) -> Vec<Item> {
        let Some(shown) = &self.shown else { return Vec::new() };
        self.parts()
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let text = match part {
                    Part::Says => format!("{} {}", shown.notification.title, shown.notification.detail.clone().unwrap_or_default()),
                    Part::Answer { label, .. } => label.clone(),
                    Part::Said => shown.said.clone().unwrap_or_default(),
                };
                Item::new(i.to_string(), "", text)
            })
            .collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.shown.as_ref().map(|_| self.cursor.to_string())
    }

    fn move_to(&mut self, key: &str) {
        if let Ok(at) = key.parse::<usize>() {
            self.put_cursor(at);
        }
    }
}

impl ActionProvider for InboxView {
    fn actions(&self) -> Vec<Action> {
        let act = |id: &str, label: &str, c: Command| Action::new(id, label, c).focused().group(Group::Act);
        let mut out = Vec::new();
        if self.at.is_some_and(|(_, of)| of > 1) || self.shown.is_none() && self.item.is_some() {
            out.push(act("inbox.next", "Next", Command::Step(1)).importance(PRIMARY));
            out.push(act("inbox.prev", "Previous", Command::Step(-1)).importance(SECONDARY));
        }
        let Some(shown) = &self.shown else { return out };
        for answer in shown.request.iter().flat_map(crate::request_card::answers) {
            let mut action = act(&answer.id, &answer.label, Command::Answer(answer.option)).importance(PRIMARY);
            if let Some(alias) = answer.alias {
                action = action.alias(alias);
            }
            if let Some(w) = answer.warning {
                action = action.detail(w);
            }
            out.push(action);
        }
        if shown.notification.slot.is_some() {
            out.push(act("inbox.open_chat", "Open its chat", Command::OpenChat).importance(SECONDARY));
        }
        out.push(act("inbox.dismiss", "Put it away", Command::Dismiss).importance(MINOR));
        out
    }

    fn run(&mut self, action: &Action, _value: Option<String>, cx: &mut RunCx<'_>) {
        let Some(command) = action.command::<Command>().cloned() else { return };
        let step = match command {
            Command::Step(by) => {
                self.asks.push(InboxAsk::Step(by));
                Step::Done
            }
            Command::OpenChat => self.open_chat(),
            Command::Dismiss => match &self.shown {
                Some(shown) => {
                    self.asks.push(InboxAsk::Dismiss(shown.notification.id.clone()));
                    Step::Done
                }
                None => Step::Nothing("Already handled.".into()),
            },
            Command::Answer(option) => self.answer(option),
        };
        if let Step::Nothing(why) = step {
            *cx.note = Some(why);
        }
    }
}
